//! Library manifest encoding, decoding and validation, and library exports collected from checked exports.

use super::*;

impl LibraryManifest {
    /// Encode already admitted facts for a private generation-bound cache, preserving the public JSON contract.
    pub(crate) fn encode_retained(&self) -> Result<Vec<u8>, String> {
        super::retained::encode(self)
    }

    /// Decode a private cache after its caller verifies the exact compiler and source-file generations.
    pub(crate) fn decode_retained(bytes: &[u8]) -> Result<Self, String> {
        super::retained::decode(bytes)
    }
    /// Create a new manifest seeded with the current compiler version and format version.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            incan_version: incan_lang::version::INCAN_VERSION.to_string(),
            manifest_format: LIBRARY_MANIFEST_FORMAT,
            exports: LibraryExports::default(),
            made_private_types: Vec::new(),
            vocab: None,
            soft_keywords: SoftKeywordExports::default(),
            contract_metadata: LibraryContractMetadata::default(),
            rust_abi: None,
        }
    }

    /// Build a semantic manifest directly from checked frontend exports.
    ///
    /// This is used by library packaging paths that already hold semantically checked declarations and want a
    /// deterministic manifest surface without going through a raw transport payload.
    pub fn from_checked_exports(
        name: impl Into<String>,
        version: impl Into<String>,
        checked_exports: &[CheckedNamedExport],
    ) -> Self {
        let name = name.into();
        let mut manifest = Self::new(name.clone(), version);
        manifest.exports = LibraryExports::from_checked_exports(&name, checked_exports);
        manifest.contract_metadata.identity_graph = LibraryIdentityGraph::from_checked_exports(&name, checked_exports);
        manifest
            .exports
            .rewrite_newtype_underlying_names_to_public_exports(checked_exports);
        for enum_export in &mut manifest.exports.enums {
            if enum_export.value_type.is_some() && enum_export.ordinal_type_identity.is_none() {
                enum_export.ordinal_type_identity = Some(format!("{name}.{}", enum_export.name));
            }
        }
        manifest
    }

    /// Validate and encode this manifest before a caller atomically publishes it beside immutable sidecars.
    pub fn to_json_string(&self) -> Result<String, LibraryManifestError> {
        let raw = RawLibraryManifest::from_semantic(self);
        validate_raw_manifest(&raw)?;
        let content =
            serde_json::to_string_pretty(&raw).map_err(|error| LibraryManifestError::Serialize(error.to_string()))?;
        Ok(format!("{content}\n"))
    }

    /// Read, decode, validate, and convert a manifest from disk.
    pub fn read_from_path(path: &Path) -> Result<Self, LibraryManifestError> {
        let content = fs::read_to_string(path).map_err(|source| LibraryManifestError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_json_str(&content)
    }

    /// Decode, validate, and convert a manifest from JSON text.
    ///
    /// The format gate runs before the full decode, and the order is the point. `RawLibraryManifest` deserializes
    /// every typed field, so a manifest written by a newer compiler fails inside serde on a variant this build has
    /// never heard of, and it fails there *before* `validate_manifest_version` can say which format it is. The
    /// reader then reports an opaque parse error for what is really a version mismatch, which is the one case the
    /// format number exists to make legible. Reading just that number first keeps the designed refusal reachable
    /// however far the shape has moved. See #1339.
    pub fn from_json_str(content: &str) -> Result<Self, LibraryManifestError> {
        Self::require_supported_manifest_format(content)?;
        let raw: RawLibraryManifest = {
            let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_frontend::decode_manifest_json", bytes = content.len()).entered();
            serde_json::from_str(content).map_err(|err| LibraryManifestError::Parse(err.to_string()))?
        };
        {
            let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_frontend::validate_manifest", loaf = %raw.name).entered();
            validate_raw_manifest(&raw)?;
        }
        let _invocation =
            tracing::trace_span!(target: "incan::invocation", "incan_frontend::convert_manifest", loaf = %raw.name)
                .entered();
        raw.into_semantic()
    }

    /// Refuse a manifest whose declared format this build does not implement, before decoding its body.
    ///
    /// Only `manifest_format` is read. Serde ignores the unknown fields around it, so this succeeds on any shape a
    /// future compiler writes, which is exactly what lets the refusal name the format instead of the first field
    /// that failed to parse. A manifest with no `manifest_format` at all is left to the full decode to reject, so
    /// this adds a refusal and never replaces one.
    fn require_supported_manifest_format(content: &str) -> Result<(), LibraryManifestError> {
        #[derive(serde::Deserialize)]
        struct DeclaredFormat {
            manifest_format: u32,
        }
        let Ok(declared) = serde_json::from_str::<DeclaredFormat>(content) else {
            return Ok(());
        };
        if !matches!(
            declared.manifest_format,
            LIBRARY_MANIFEST_FORMAT | super::OWNERSHIP_MANIFEST_FORMAT
        ) {
            return Err(LibraryManifestError::Invalid(format!(
                "unsupported manifest_format {} (expected {})",
                declared.manifest_format, LIBRARY_MANIFEST_FORMAT
            )));
        }
        Ok(())
    }
}

impl LibraryExports {
    /// Build manifest exports from checked frontend exports.
    fn from_checked_exports(package_name: &str, exports: &[CheckedNamedExport]) -> Self {
        let mut model = Self::default();

        for export in exports {
            match &export.kind {
                CheckedExportKind::Function(function_export) => {
                    model.functions.push(super::with_checked_type_origins(
                        function_export_from_checked(package_name, function_export),
                        &export.identity.type_origins,
                    ));
                }
                CheckedExportKind::Partial(partial_export) => {
                    model.partials.push(super::with_checked_type_origins(
                        partial_export_from_checked(partial_export),
                        &export.identity.type_origins,
                    ));
                }
                CheckedExportKind::Alias(alias_export) => {
                    model.aliases.push(super::with_checked_type_origins(
                        alias_export_from_checked(package_name, alias_export),
                        &export.identity.type_origins,
                    ));
                }
                CheckedExportKind::TypeAlias(type_alias_export) => {
                    model.type_aliases.push(super::with_checked_type_origins(
                        type_alias_export_from_checked(type_alias_export),
                        &export.identity.type_origins,
                    ));
                }
                CheckedExportKind::Model(model_export) => {
                    model.models.push(super::with_checked_type_origins(
                        model_export_from_checked(package_name, model_export),
                        &export.identity.type_origins,
                    ));
                }
                CheckedExportKind::Class(class_export) => {
                    model.classes.push(super::with_checked_type_origins(
                        class_export_from_checked(package_name, class_export),
                        &export.identity.type_origins,
                    ));
                }
                CheckedExportKind::Trait(trait_export) => {
                    model.traits.push(super::with_checked_type_origins(
                        trait_export_from_checked(package_name, trait_export),
                        &export.identity.type_origins,
                    ));
                }
                CheckedExportKind::Enum(enum_export) => {
                    model.enums.push(super::with_checked_type_origins(
                        enum_export_from_checked(package_name, enum_export),
                        &export.identity.type_origins,
                    ));
                }
                CheckedExportKind::Newtype(newtype_export) => {
                    model.newtypes.push(super::with_checked_type_origins(
                        newtype_export_from_checked(package_name, newtype_export),
                        &export.identity.type_origins,
                    ));
                }
                CheckedExportKind::Const(const_export) => {
                    model.consts.push(super::with_checked_type_origins(
                        const_export_from_checked(const_export),
                        &export.identity.type_origins,
                    ));
                }
                CheckedExportKind::Static(static_export) => {
                    model.statics.push(super::with_checked_type_origins(
                        static_export_from_checked(static_export),
                        &export.identity.type_origins,
                    ));
                }
            }
        }

        model.sort_deterministically();
        model
    }

    /// Sort every export group by stable public name.
    fn sort_deterministically(&mut self) {
        self.models.sort_by(|left, right| left.name.cmp(&right.name));
        self.aliases.sort_by(|left, right| left.name.cmp(&right.name));
        self.partials.sort_by(|left, right| left.name.cmp(&right.name));
        self.classes.sort_by(|left, right| left.name.cmp(&right.name));
        self.functions.sort_by(|left, right| left.name.cmp(&right.name));
        self.traits.sort_by(|left, right| left.name.cmp(&right.name));
        self.enums.sort_by(|left, right| left.name.cmp(&right.name));
        self.type_aliases.sort_by(|left, right| left.name.cmp(&right.name));
        self.newtypes.sort_by(|left, right| left.name.cmp(&right.name));
        self.consts.sort_by(|left, right| left.name.cmp(&right.name));
        self.statics.sort_by(|left, right| left.name.cmp(&right.name));
    }

    /// Rewrite newtype composition references through the public identity graph selected for this package.
    fn rewrite_newtype_underlying_names_to_public_exports(&mut self, exports: &[CheckedNamedExport]) {
        let mut public_names_by_source: std::collections::HashMap<Vec<String>, Vec<String>> =
            std::collections::HashMap::new();
        for export in exports {
            if !matches!(
                &export.kind,
                CheckedExportKind::Model(_)
                    | CheckedExportKind::Class(_)
                    | CheckedExportKind::Enum(_)
                    | CheckedExportKind::Newtype(_)
            ) {
                continue;
            }
            let source_path = match &export.identity.projection {
                CheckedExportProjection::Direct => &export.identity.source_path,
                CheckedExportProjection::Alias { target_path } | CheckedExportProjection::Reexport { target_path } => {
                    target_path
                }
                CheckedExportProjection::Partial { .. } => continue,
            };
            public_names_by_source
                .entry(source_path.clone())
                .or_default()
                .push(export.name.clone());
        }
        let public_names = public_names_by_source
            .into_iter()
            .filter_map(|(source_path, mut names)| {
                let source_name = source_path.last()?;
                names.sort();
                names.dedup();
                let preferred = names
                    .iter()
                    .find(|name| *name == source_name)
                    .cloned()
                    .unwrap_or_else(|| names[0].clone());
                Some((source_path, preferred))
            })
            .collect::<std::collections::HashMap<_, _>>();
        let mut source_paths_by_leaf: std::collections::HashMap<String, Vec<Vec<String>>> =
            std::collections::HashMap::new();
        for source_path in public_names.keys() {
            if let Some(source_name) = source_path.last() {
                source_paths_by_leaf
                    .entry(source_name.clone())
                    .or_default()
                    .push(source_path.clone());
            }
        }
        for newtype in &mut self.newtypes {
            let owner_module_path = exports
                .iter()
                .find(|export| export.name == newtype.name && matches!(&export.kind, CheckedExportKind::Newtype(_)))
                .map(|export| match &export.identity.projection {
                    CheckedExportProjection::Direct => export.identity.source_path.as_slice(),
                    CheckedExportProjection::Alias { target_path }
                    | CheckedExportProjection::Reexport { target_path } => target_path.as_slice(),
                    CheckedExportProjection::Partial { .. } => export.identity.source_path.as_slice(),
                })
                .and_then(|path| path.split_last().map(|(_, module_path)| module_path.to_vec()))
                .unwrap_or_default();
            rewrite_type_ref_names(
                &mut newtype.underlying,
                &owner_module_path,
                &public_names,
                &source_paths_by_leaf,
            );
        }
    }
}

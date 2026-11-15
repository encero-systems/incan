//! The library identity graph, export identities, canonical identity exports, and the Rust ABI record.

use super::*;

/// Serialized schema version for the public export identity graph.
pub const LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION: u32 = 2;

/// Identity-graph schema emitted before canonical symbol identities crossed compiled-library boundaries.
pub const LEGACY_LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION: u32 = 1;

/// Serializable semantic identity graph for exported library declarations.
///
/// The graph separates a public export's spelling from the declaration identity and projection it represents. Consumers
/// should consult this graph before falling back to short-name manifest lookup whenever aliases, reexports, partial
/// presets, decorators, generated helpers, or public package boundaries can observe the symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryIdentityGraph {
    /// Version of the serialized identity graph payload.
    #[serde(default = "default_identity_graph_schema_version")]
    pub schema_version: u32,
    /// Public export entries keyed by public spelling, semantic source path, and projection metadata.
    #[serde(default)]
    pub exports: Vec<ExportIdentity>,
}

impl Default for LibraryIdentityGraph {
    fn default() -> Self {
        Self {
            schema_version: LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION,
            exports: Vec::new(),
        }
    }
}

impl LibraryIdentityGraph {
    /// Build the serialized identity graph from checked public exports while preserving distinct overload identities.
    pub fn from_checked_exports(package_name: &str, exports: &[CheckedNamedExport]) -> Self {
        let mut graph = Self {
            schema_version: LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION,
            exports: exports
                .iter()
                .map(|export| export_identity_from_checked(package_name, export))
                .collect(),
        };
        graph.sort_and_deduplicate();
        graph
    }

    /// Extend the graph with declarations exposed through checked public module namespaces.
    ///
    /// `api.public_namespaces` owns the consumer-visible path, while `checked_modules` owns the compiler-selected
    /// declaration identities. Joining them by the namespace's exact source-module path avoids inferring identity from
    /// the public spelling and preserves overload, alias, and reexport targets independently.
    pub fn extend_checked_api_exports(
        &mut self,
        package_name: &str,
        api: &CheckedApiMetadataPackage,
        checked_modules: &[(Vec<String>, Vec<CheckedNamedExport>)],
    ) -> Result<(), LibraryManifestError> {
        for namespace in &api.public_namespaces {
            for member in &namespace.members {
                let (source_name, source_module_path) = member.source_path.split_last().ok_or_else(|| {
                    LibraryManifestError::Invalid(format!(
                        "checked public namespace member `{}` has an empty source path",
                        member.name
                    ))
                })?;
                let module_exports = checked_modules
                    .iter()
                    .find(|(module_path, _)| module_path == source_module_path)
                    .map(|(_, exports)| exports)
                    .ok_or_else(|| {
                        LibraryManifestError::Invalid(format!(
                            "checked public namespace member `{}` refers to unknown source module `{}`",
                            member.name,
                            source_module_path.join(".")
                        ))
                    })?;
                let mut matched = false;
                for export in module_exports.iter().filter(|export| export.name == *source_name) {
                    matched = true;
                    let mut entry = export_identity_from_checked(package_name, export);
                    entry.public_name = member.name.clone();
                    entry.public_path = std::iter::once(package_name.to_string())
                        .chain(namespace.path.iter().cloned())
                        .chain(std::iter::once(member.name.clone()))
                        .collect();
                    self.exports.push(entry);
                }
                if !matched {
                    return Err(LibraryManifestError::Invalid(format!(
                        "checked public namespace member `{}` has no checked export in source module `{}`",
                        member.name,
                        source_module_path.join(".")
                    )));
                }
            }
        }
        self.sort_and_deduplicate();
        Ok(())
    }

    /// Order exported identities deterministically and collapse entries publishing one declaration at one path.
    ///
    /// Two entries are the same export when they agree on where they are published and on which declaration they
    /// name. Being spelled differently does not make them distinct: a declaration reachable by more than one route --
    /// directly and through a facade, or republished into a parent namespace from two sibling modules -- yields one
    /// record per route, each carrying the path of the route it came from, and every one of them carries the same
    /// canonical identity. Keying deduplication on those spellings kept each route as its own entry, and the graph
    /// then failed its own duplicate-export check on a program that was perfectly valid.
    ///
    /// `source_path` still participates in the ordering so the surviving entry is chosen deterministically rather
    /// than by input order.
    fn sort_and_deduplicate(&mut self) {
        self.exports.sort_by(|left, right| {
            left.public_path
                .cmp(&right.public_path)
                .then(left.canonical.cmp(&right.canonical))
                .then(left.source_path.cmp(&right.source_path))
        });
        self.exports
            .dedup_by(|left, right| left.public_path == right.public_path && left.canonical == right.canonical);
    }

    /// Return whether the graph has no public export identities to serialize.
    pub fn is_empty(&self) -> bool {
        self.exports.is_empty()
    }

    /// Return the first identity entry for a public export name, which represents the shared projection for overload
    /// sets.
    pub fn entry_for_public_name(&self, name: &str) -> Option<&ExportIdentity> {
        self.exports
            .iter()
            .find(|entry| entry.public_path.len() == 2 && entry.public_name == name)
    }

    /// Return every canonical overload identity published for one public function spelling, in declaration order.
    pub fn function_identities_for_public_name(&self, name: &str) -> Vec<Option<CanonicalSymbolId>> {
        self.exports
            .iter()
            .filter(|entry| {
                entry.public_path.len() == 2 && entry.public_name == name && entry.kind == ExportIdentityKind::Function
            })
            .map(|entry| entry.canonical.as_ref().and_then(CanonicalIdentityExport::hydrate))
            .collect()
    }

    /// Return the one canonical identity published for a non-overloaded public spelling.
    ///
    /// Multiple distinct identities are ambiguous and therefore produce no aggregate identity. Overload consumers
    /// use [`Self::function_identities_for_public_name`] instead.
    pub fn canonical_for_public_name(&self, name: &str) -> Option<CanonicalSymbolId> {
        unique_canonical_identity(
            self.exports
                .iter()
                .filter(|entry| entry.public_path.len() == 2 && entry.public_name == name),
        )
    }

    /// Return the one canonical identity published at one exact package-visible path.
    pub fn canonical_for_public_path(&self, public_path: &[String]) -> Option<CanonicalSymbolId> {
        unique_canonical_identity(self.exports.iter().filter(|entry| entry.public_path == public_path))
    }

    /// Retain identities from the declaration source the checked namespace selected. A module's own member can
    /// precede a same-named child projection at one public path; its signature and identity must select the same
    /// source. Reexports without an exact source record retain the public path's existing ambiguity.
    pub fn identities_for_public_source_path(
        &self,
        public_path: &[String],
        source_path: &[String],
    ) -> Vec<Option<CanonicalSymbolId>> {
        let exact_source = self
            .exports
            .iter()
            .any(|entry| entry.public_path == public_path && entry.source_path == source_path);
        self.exports
            .iter()
            .filter(|entry| entry.public_path == public_path && (!exact_source || entry.source_path == source_path))
            .map(|entry| entry.canonical.as_ref().and_then(CanonicalIdentityExport::hydrate))
            .collect()
    }

    /// Every distinct function identity published from one declaring source path, in graph order, whichever public
    /// paths expose it: the identities a consumer's call to that function can select.
    pub fn function_identities_for_source_path(&self, source_path: &[String]) -> Vec<CanonicalSymbolId> {
        let mut identities = Vec::new();
        for identity in self
            .exports
            .iter()
            .filter(|entry| entry.source_path == source_path && entry.kind == ExportIdentityKind::Function)
            .filter_map(|entry| entry.canonical.as_ref()?.hydrate())
        {
            if !identities.contains(&identity) {
                identities.push(identity);
            }
        }
        identities
    }

    /// Return every canonical identity published at one exact package-visible path, in graph order.
    pub fn canonicals_at_public_path(&self, public_path: &[String]) -> Vec<CanonicalSymbolId> {
        let mut canonicals = Vec::new();
        for canonical in self
            .exports
            .iter()
            .filter(|entry| entry.public_path == public_path)
            .filter_map(|entry| entry.canonical.as_ref()?.hydrate())
        {
            if !canonicals.contains(&canonical) {
                canonicals.push(canonical);
            }
        }
        canonicals
    }

    /// Return the source path of the declaration `canonical` identifies, as the declaration's direct entry publishes
    /// it.
    ///
    /// A declaration published at several paths -- its declaring module's namespace, a facade that re-exports it, the
    /// package root -- carries the same canonical identity at each of them, while each re-export entry's
    /// `source_path` names the hop it forwards through. Only a direct entry's `source_path` is the declaration's own.
    pub fn declaration_source_path(&self, canonical: &CanonicalSymbolId) -> Option<&[String]> {
        self.exports
            .iter()
            .find(|entry| {
                entry.projection == ExportIdentityProjection::Direct
                    && entry
                        .canonical
                        .as_ref()
                        .is_some_and(|candidate| candidate.represents(canonical))
            })
            .map(|entry| entry.source_path.as_slice())
    }
}

/// Return one unambiguous canonical export while hydrating only the identity that escapes the graph.
fn unique_canonical_identity<'a>(entries: impl Iterator<Item = &'a ExportIdentity>) -> Option<CanonicalSymbolId> {
    let mut candidates = entries
        .filter_map(|entry| entry.canonical.as_ref())
        .filter(|identity| identity.can_hydrate());
    let first = candidates.next()?;
    candidates
        .all(|candidate| candidate == first)
        .then(|| first.hydrate())
        .flatten()
}

/// Return the legacy identity graph schema version when deserializing manifests that predate the field.
pub(super) fn default_identity_graph_schema_version() -> u32 {
    LEGACY_LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION
}

/// Decode an omitted identity graph as the legacy schema that actually omitted the field.
pub(super) fn legacy_library_identity_graph() -> LibraryIdentityGraph {
    LibraryIdentityGraph {
        schema_version: LEGACY_LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION,
        exports: Vec::new(),
    }
}

/// Decode a manifest that omitted the whole contract-metadata envelope without claiming current identity facts.
pub(in crate::library_manifest) fn legacy_library_contract_metadata() -> LibraryContractMetadata {
    LibraryContractMetadata {
        identity_graph: legacy_library_identity_graph(),
        ..LibraryContractMetadata::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportIdentity {
    /// Name that consumers import from the public package surface.
    pub public_name: String,
    /// Package-qualified public path exposed by the `.incnlib` manifest.
    pub public_path: Vec<String>,
    /// Provider-local declaration path that owns the semantic identity.
    pub source_path: Vec<String>,
    /// Coarse export category used by consumers before reconstructing a full checked export surface.
    pub kind: ExportIdentityKind,
    /// Projection layered on top of `source_path`, such as a direct export, alias, reexport, or partial preset.
    pub projection: ExportIdentityProjection,
    /// Stable compiled-artifact representation of the declaration this public spelling resolves to.
    ///
    /// Schema-v1 manifests omitted this field. A v2 producer must provide it; a legacy consumer sees `None` and stays
    /// explicitly unproven rather than reconstructing identity from `source_path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical: Option<CanonicalIdentityExport>,
}

/// Stable `.incnlib` representation of one compiler-owned canonical symbol identity.
///
/// This is deliberately not a serialized [`crate::symbols::SymbolId`], whose integer value is local to one
/// compiler process. Producer-local module origins are rebased to a package origin at publication, making equal
/// module paths from two compiled packages remain distinct in a consumer compilation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CanonicalIdentityExport {
    pub namespace: CanonicalIdentityNamespaceExport,
    pub origin: CanonicalIdentityOriginExport,
    pub declaration_name: String,
    pub kind: String,
    pub declaration_span: CanonicalIdentitySpanExport,
}

impl CanonicalIdentityExport {
    /// Project a frontend identity into the stable compiled-package identity domain.
    pub fn from_canonical(package_name: &str, identity: &CanonicalSymbolId) -> Option<Self> {
        if identity.scope_discriminant.is_some() || matches!(identity.kind, SemanticSourceTargetKind::Other(_)) {
            return None;
        }
        let origin = match &identity.origin {
            SymbolOrigin::Module(module_path) => CanonicalIdentityOriginExport::Package {
                library: package_name.to_string(),
                module_path: module_path.clone(),
            },
            SymbolOrigin::Package { library, module_path } => CanonicalIdentityOriginExport::Package {
                library: library.clone(),
                module_path: module_path.clone(),
            },
            SymbolOrigin::RustCrate(path) => CanonicalIdentityOriginExport::RustCrate { path: path.clone() },
            SymbolOrigin::Builtin => CanonicalIdentityOriginExport::Builtin,
        };
        Some(Self {
            namespace: identity.namespace.into(),
            origin,
            declaration_name: identity.declaration_name.clone(),
            kind: identity.kind.as_str().to_string(),
            declaration_span: CanonicalIdentitySpanExport {
                start: u64::try_from(identity.declaration_span.start).ok()?,
                end: u64::try_from(identity.declaration_span.end).ok()?,
                expansion: identity.declaration_span.expansion,
            },
        })
    }

    /// Hydrate the compiler-owned identity represented by a validated manifest record.
    pub fn hydrate(&self) -> Option<CanonicalSymbolId> {
        let kind = SemanticSourceTargetKind::from_kind_str(&self.kind);
        if matches!(kind, SemanticSourceTargetKind::Other(_)) {
            return None;
        }
        let origin = match &self.origin {
            CanonicalIdentityOriginExport::Package { library, module_path } => SymbolOrigin::Package {
                library: library.clone(),
                module_path: module_path.clone(),
            },
            CanonicalIdentityOriginExport::RustCrate { path } => SymbolOrigin::RustCrate(path.clone()),
            CanonicalIdentityOriginExport::Builtin => SymbolOrigin::Builtin,
        };
        Some(CanonicalSymbolId {
            namespace: self.namespace.into(),
            origin,
            declaration_name: self.declaration_name.clone(),
            kind,
            scope_discriminant: None,
            declaration_span: HirSourceSpan::new(
                usize::try_from(self.declaration_span.start).ok()?,
                usize::try_from(self.declaration_span.end).ok()?,
            )
            .with_expansion(self.declaration_span.expansion),
        })
    }

    /// Return whether this wire identity can reconstruct a compiler identity on the current target.
    fn can_hydrate(&self) -> bool {
        !matches!(
            SemanticSourceTargetKind::from_kind_str(&self.kind),
            SemanticSourceTargetKind::Other(_)
        ) && usize::try_from(self.declaration_span.start).is_ok()
            && usize::try_from(self.declaration_span.end).is_ok()
    }

    /// Compare with a hydrated compiler identity without cloning the wire identity's strings or paths.
    pub(crate) fn represents(&self, identity: &CanonicalSymbolId) -> bool {
        let Ok(start) = u64::try_from(identity.declaration_span.start) else {
            return false;
        };
        let Ok(end) = u64::try_from(identity.declaration_span.end) else {
            return false;
        };
        let origin_matches = match (&self.origin, &identity.origin) {
            (
                CanonicalIdentityOriginExport::Package { library, module_path },
                SymbolOrigin::Package {
                    library: candidate_library,
                    module_path: candidate_path,
                },
            ) => library == candidate_library && module_path == candidate_path,
            (CanonicalIdentityOriginExport::RustCrate { path }, SymbolOrigin::RustCrate(candidate)) => {
                path == candidate
            }
            (CanonicalIdentityOriginExport::Builtin, SymbolOrigin::Builtin) => true,
            _ => false,
        };
        SymbolNamespace::from(self.namespace) == identity.namespace
            && origin_matches
            && self.declaration_name == identity.declaration_name
            && self.kind == identity.kind.as_str()
            && self.declaration_span.start == start
            && self.declaration_span.end == end
            && self.declaration_span.expansion == identity.declaration_span.expansion
            && identity.scope_discriminant.is_none()
    }
}

/// Stable namespace vocabulary for compiled canonical identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalIdentityNamespaceExport {
    OrdinaryLexical,
    Member,
    ModulePath,
}

impl From<SymbolNamespace> for CanonicalIdentityNamespaceExport {
    fn from(namespace: SymbolNamespace) -> Self {
        match namespace {
            SymbolNamespace::OrdinaryLexical => Self::OrdinaryLexical,
            SymbolNamespace::Member => Self::Member,
            SymbolNamespace::ModulePath => Self::ModulePath,
        }
    }
}

impl From<CanonicalIdentityNamespaceExport> for SymbolNamespace {
    fn from(namespace: CanonicalIdentityNamespaceExport) -> Self {
        match namespace {
            CanonicalIdentityNamespaceExport::OrdinaryLexical => Self::OrdinaryLexical,
            CanonicalIdentityNamespaceExport::Member => Self::Member,
            CanonicalIdentityNamespaceExport::ModulePath => Self::ModulePath,
        }
    }
}

/// Stable origin vocabulary for identities that survive a compiled-package boundary.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CanonicalIdentityOriginExport {
    Package { library: String, module_path: Vec<String> },
    RustCrate { path: Vec<String> },
    Builtin,
}

/// Stable byte-offset provenance for one declaration in the published source projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CanonicalIdentitySpanExport {
    /// Compiler-owned context of a generated declaration, absent for authored declarations.
    pub expansion: Option<[u8; 32]>,
    pub start: u64,
    pub end: u64,
}

impl ExportIdentity {
    /// Return the projected target path when this public export is an alias, reexport, or partial preset.
    pub fn target_path(&self) -> Option<&[String]> {
        match &self.projection {
            ExportIdentityProjection::Direct => None,
            ExportIdentityProjection::Alias { target_path }
            | ExportIdentityProjection::Reexport { target_path }
            | ExportIdentityProjection::Partial { target_path, .. } => Some(target_path.as_slice()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportIdentityKind {
    /// Public function or overload set.
    Function,
    /// Public partial preset.
    Partial,
    /// Public alias declaration.
    Alias,
    /// Public type alias declaration.
    TypeAlias,
    /// Public model declaration.
    Model,
    /// Public class declaration.
    Class,
    /// Public trait declaration.
    Trait,
    /// Public enum declaration.
    Enum,
    /// Public newtype declaration.
    Newtype,
    /// Public const declaration.
    Const,
    /// Public static declaration.
    Static,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExportIdentityProjection {
    /// The public export exposes its own declaration directly.
    Direct,
    /// The public export is a source-level alias over another declaration or overload set.
    Alias {
        /// Provider-local declaration path that the alias projects.
        target_path: Vec<String>,
    },
    /// The public export forwards another declaration through a facade.
    Reexport {
        /// Provider-local declaration path that the reexport projects.
        target_path: Vec<String>,
    },
    /// The public export is a partial preset over a callable or constructor target.
    Partial {
        /// Provider-local declaration path that the partial preset projects.
        target_path: Vec<String>,
        /// Kind of target being projected, used to rebuild the consumer callable surface.
        target_kind: PartialTargetKindExport,
    },
}

/// Versioned Rust ABI payload persisted into `.incnlib`.
///
/// The payload stores the backend-neutral metadata shape extracted from rustc, but ships it with the
/// library artifact so consumers can resolve Rust-backed imports without loading the producer workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryRustAbi {
    /// Serialized ABI schema version.
    #[serde(default = "default_rust_abi_schema_version")]
    pub schema_version: u32,
    /// Canonical Rust item metadata keyed by `RustItemMetadata::canonical_path`.
    #[serde(default)]
    pub items: Vec<RustItemMetadata>,
    /// Digest of the indexed `<name>.rustabi` asset containing dependency-owned native facts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_digest: Option<String>,
}

/// Default Rust ABI schema version for manifest payloads that predate explicit serde fields.
pub(super) fn default_rust_abi_schema_version() -> u32 {
    1
}

impl LibraryRustAbi {
    /// Build a deterministic ABI payload from extracted Rust metadata.
    pub fn from_items(mut items: Vec<RustItemMetadata>) -> Option<Self> {
        items.sort_by(|left, right| left.canonical_path.cmp(&right.canonical_path));
        items.dedup_by(|left, right| left.canonical_path == right.canonical_path);
        if items.is_empty() {
            return None;
        }
        Some(Self {
            schema_version: RUST_ABI_SCHEMA_VERSION,
            items,
            cache_digest: None,
        })
    }

    /// Return metadata for the exact queried Rust surface. Definition identity does not transfer surface-specific trait
    /// proofs.
    pub fn get(&self, canonical_path: &str) -> Option<&RustItemMetadata> {
        self.items.iter().find(|item| item.canonical_path == canonical_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Graph lookups compare retained identities without changing ambiguity or direct-source selection.
    #[test]
    fn graph_queries_preserve_canonical_identity_contracts() -> Result<(), &'static str> {
        let identity = CanonicalSymbolId::module_declaration(
            vec!["records".to_string()],
            "load",
            SemanticSourceTargetKind::Function,
            HirSourceSpan::new(4, 12),
        );
        let canonical =
            CanonicalIdentityExport::from_canonical("pkg", &identity).ok_or("canonical identity missing")?;
        let public_path = vec!["pkg".to_string(), "load".to_string()];
        let direct = ExportIdentity {
            public_name: "load".to_string(),
            public_path: public_path.clone(),
            source_path: vec!["records".to_string(), "load".to_string()],
            kind: ExportIdentityKind::Function,
            projection: ExportIdentityProjection::Direct,
            canonical: Some(canonical.clone()),
        };
        let graph = LibraryIdentityGraph {
            schema_version: LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION,
            exports: vec![direct],
        };
        let hydrated = canonical.hydrate().ok_or("canonical identity did not hydrate")?;

        assert!(canonical.represents(&hydrated));
        assert_eq!(graph.canonical_for_public_path(&public_path), Some(hydrated.clone()));
        assert_eq!(
            graph.declaration_source_path(&hydrated),
            Some(["records".to_string(), "load".to_string()].as_slice())
        );

        let mut scoped = hydrated;
        scoped.scope_discriminant = Some(incan_semantics_core::ScopeDiscriminant(1));
        assert!(!canonical.represents(&scoped));
        Ok(())
    }
}

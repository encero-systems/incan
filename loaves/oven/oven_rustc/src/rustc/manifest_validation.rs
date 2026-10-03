//! Validating direct-rustc artifact manifests and their source-search closure.

use super::{
    BTreeMap, BTreeSet, OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
    OVEN_RUSTC_LEGACY_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenBuildIntent, OvenRegistryLeafAuthority,
    OvenRustcArtifactManifest, OvenRustcArtifactPlan, OvenRustcError, OvenRustcLegacySearchProjection,
    OvenRustcRegistryLeaf, OvenRustcRegistrySourcePackage, OvenRustcSourceSearchClosure, OvenRustcSupportingArtifact,
    Path, compiler_runtime_artifacts_by_name, compiler_runtime_execution_support, digest_bytes, expected_artifacts,
    normalized_relative_path, validate_rust_identifier, validate_rust_target, validated_compile_environment,
};

/// Complete semantic key for one sealed registry inspection source.
type RegistrySourceIdentity<'a> = (&'a str, &'a str, &'a str, &'a str);

/// Registry inspection sources indexed by their complete semantic identity.
type RegistrySourceIndex<'a> = BTreeMap<RegistrySourceIdentity<'a>, &'a OvenRustcRegistrySourcePackage>;

impl OvenRustcArtifactManifest {
    /// Return the exact compiler-runtime crate names sealed by this release artifact manifest.
    ///
    /// The `incan_` prefix alone does not establish compiler ownership. Consumers must derive runtime ownership from
    /// the selected release Loaf so caller crates with similar names remain caller-owned.
    pub fn compiler_runtime_crate_names(&self) -> Result<BTreeSet<String>, OvenRustcError> {
        self.validate_shape(&self.intent)?;
        Ok(compiler_runtime_artifacts_by_name(self)?.into_keys().collect())
    }

    /// Return the complete artifact file set declared by this immutable plan without reading artifact bytes.
    pub fn declared_artifact_paths(&self) -> Result<BTreeSet<String>, OvenRustcError> {
        self.validate_shape(&self.intent)?;
        Ok(expected_artifacts(self)?.into_keys().collect())
    }

    /// Return the same admitted artifact catalog with its producer-established byte identities.
    ///
    /// Native-compilation projection uses this table to label the already selected native artifact plan. It must not
    /// scan a search directory or derive a second dependency graph merely to recover bytes the publisher already
    /// recorded.
    pub fn declared_artifact_digests(&self) -> Result<BTreeMap<String, String>, OvenRustcError> {
        self.validate_shape(&self.intent)?;
        expected_artifacts(self)
    }

    /// Return the physical release artifacts required by its main, native, and vocabulary search closures.
    pub fn release_execution_artifacts(&self) -> Result<Vec<OvenRustcSupportingArtifact>, OvenRustcError> {
        compiler_runtime_execution_support(self)
    }

    /// Capture selected directory membership from this publisher's existing exact artifact declarations.
    pub fn capture_source_search_closure(
        &self,
        paths: &[String],
    ) -> Result<OvenRustcSourceSearchClosure, OvenRustcError> {
        Ok(OvenRustcSourceSearchClosure::publisher_selected(
            paths.to_vec(),
            &expected_artifacts(self)?,
        ))
    }

    /// Preserve one admitted contributor's source projection before any other closure is merged into it.
    ///
    /// Callers retain the original contributor's existing store lease. The digest records provenance only; the
    /// enclosing publication and materialization still verify all artifact ownership and bytes.
    pub fn source_search_closure(&self, key: &str) -> Result<OvenRustcSourceSearchClosure, OvenRustcError> {
        self.validate_shape(&self.intent)?;
        if self.schema_version == OVEN_RUSTC_LEGACY_ARTIFACT_MANIFEST_SCHEMA_VERSION {
            let projected = self.for_source_evidence(key)?;
            return Ok(OvenRustcSourceSearchClosure {
                publisher_paths: Vec::new(),
                legacy_projections: vec![OvenRustcLegacySearchProjection {
                    source_schema_version: self.schema_version,
                    source_manifest_digest: digest_bytes(&serde_json::to_vec(self).map_err(|error| {
                        OvenRustcError::InvalidInput {
                            field: "legacy search projection",
                            message: format!("cannot encode admitted manifest: {error}"),
                        }
                    })?),
                    source_evidence_key: key.to_string(),
                    dependency_search_paths: self
                        .capture_source_search_closure(&projected.dependency_search_paths)?
                        .publisher_paths,
                }],
            });
        }
        if self.entrypoint_externs.is_empty() {
            // An unscoped current publisher has no helper-expanded source roles. Capture its original normal
            // closure before composition introduces any named roles or paths from another contributor.
            return self.capture_source_search_closure(&self.dependency_search_paths);
        }
        self.entrypoint_dependency_search_paths
            .get(key)
            .cloned()
            .ok_or_else(|| OvenRustcError::InvalidInput {
                field: "artifact manifest source search closure",
                message: format!("source evidence `{key}` has no recorded search closure"),
            })
    }

    /// Validate receipt-independent manifest shape before a stored plan may be selected or republished.
    ///
    /// This deliberately avoids a full byte-by-byte artifact traversal. Publication and execution perform that
    /// stronger validation; selection uses this inexpensive gate so a legacy malformed payload cannot make a newly
    /// corrected plan ambiguous forever.
    pub fn validate_shape(&self, expected_intent: &OvenBuildIntent) -> Result<(), OvenRustcError> {
        self.validate_manifest_identity_and_externs(expected_intent)?;
        let declared_artifacts = self.declared_artifacts_for_validation()?;
        let registry_source_identities = self.validate_registry_sources(&declared_artifacts)?;
        self.validate_registry_leaves(&declared_artifacts, &registry_source_identities)?;
        self.validate_vocabulary_auxiliary_targets()?;
        Ok(())
    }

    /// Validate schema, intent, compile environment, source roles, and public extern aliases.
    fn validate_manifest_identity_and_externs(&self, expected_intent: &OvenBuildIntent) -> Result<(), OvenRustcError> {
        if !matches!(
            self.schema_version,
            OVEN_RUSTC_LEGACY_ARTIFACT_MANIFEST_SCHEMA_VERSION | OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION
        ) {
            return Err(OvenRustcError::UnsupportedSchema {
                found: self.schema_version,
                expected: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            });
        }
        if &self.intent != expected_intent {
            return Err(OvenRustcError::IntentMismatch);
        }
        let _ = validated_compile_environment(&self.compile_environment)?;
        self.validate_source_search_roles()?;
        let mut names = BTreeSet::new();
        for artifact in &self.externs {
            validate_rust_identifier(&artifact.crate_name)?;
            if !names.insert(artifact.crate_name.as_str()) {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest",
                    message: format!("declares duplicate extern crate `{}`", artifact.crate_name),
                });
            }
        }
        for (source_evidence_key, crate_names) in &self.entrypoint_externs {
            if source_evidence_key.trim().is_empty() {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest entrypoint externs",
                    message: "must not contain an empty source-evidence key".to_string(),
                });
            }
            let mut seen = BTreeSet::new();
            for crate_name in crate_names {
                validate_rust_identifier(crate_name)?;
                if !names.contains(crate_name.as_str()) {
                    return Err(OvenRustcError::InvalidInput {
                        field: "artifact manifest entrypoint externs",
                        message: format!(
                            "source evidence `{source_evidence_key}` names undeclared extern `{crate_name}`"
                        ),
                    });
                }
                if !seen.insert(crate_name.as_str()) {
                    return Err(OvenRustcError::InvalidInput {
                        field: "artifact manifest entrypoint externs",
                        message: format!(
                            "source evidence `{source_evidence_key}` names extern `{crate_name}` more than once"
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    /// Index every declared artifact while rejecting duplicate portable paths.
    fn declared_artifacts_for_validation(&self) -> Result<BTreeMap<&str, &str>, OvenRustcError> {
        let mut declared_artifacts = BTreeMap::new();
        for artifact in self
            .externs
            .iter()
            .map(|artifact| (&artifact.relative_path, &artifact.digest))
            .chain(
                self.supporting_artifacts
                    .iter()
                    .map(|artifact| (&artifact.relative_path, &artifact.digest)),
            )
        {
            if declared_artifacts
                .insert(artifact.0.as_str(), artifact.1.as_str())
                .is_some()
            {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry catalog",
                    message: format!("declares artifact `{}` more than once", artifact.0),
                });
            }
        }
        Ok(declared_artifacts)
    }

    /// Validate sealed registry sources and index their complete semantic identities.
    fn validate_registry_sources<'a>(
        &'a self,
        declared_artifacts: &BTreeMap<&str, &str>,
    ) -> Result<RegistrySourceIndex<'a>, OvenRustcError> {
        let mut registry_source_identities = BTreeMap::new();
        let mut registry_package_sources = BTreeMap::new();
        for package in &self.registry_sources {
            if package.package.trim().is_empty() || package.version.trim().is_empty() {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry sources",
                    message: "registry source package and version must not be empty".to_string(),
                });
            }
            let mut features = BTreeSet::new();
            for feature in &package.features {
                if feature.trim().is_empty() || !features.insert(feature.as_str()) {
                    return Err(OvenRustcError::InvalidInput {
                        field: "artifact manifest registry sources",
                        message: format!(
                            "registry source `{}` `{}` declares an empty or duplicate feature",
                            package.package, package.version
                        ),
                    });
                }
            }
            if !package.source.registry.starts_with("registry+")
                || package.source.checksum.trim().is_empty()
                || package.source.digest.trim().is_empty()
            {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry sources",
                    message: format!(
                        "registry source `{}` `{}` has incomplete source identity",
                        package.package, package.version
                    ),
                });
            }
            let source_root = Path::new(&package.source.relative_root);
            if source_root.is_absolute()
                || source_root.as_os_str().is_empty()
                || source_root.components().any(|component| {
                    matches!(
                        component,
                        std::path::Component::ParentDir
                            | std::path::Component::RootDir
                            | std::path::Component::Prefix(_)
                    )
                })
            {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry sources",
                    message: format!(
                        "registry source `{}` `{}` has an unsafe source root",
                        package.package, package.version
                    ),
                });
            }
            let source_manifest = source_root.join("Cargo.toml").to_string_lossy().replace('\\', "/");
            if !declared_artifacts.contains_key(source_manifest.as_str()) {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry sources",
                    message: format!(
                        "registry source `{}` `{}` is not declared by the immutable plan",
                        package.package, package.version
                    ),
                });
            }
            // A package version from one registry has one source record. Compare the complete source before naming
            // the refusal: the same source declared again is a repeat, while a differing checksum, staged root or
            // tree digest is a second source identity.
            let package_key = (
                package.package.as_str(),
                package.version.as_str(),
                package.source.registry.as_str(),
            );
            if let Some(declared) = registry_package_sources.insert(package_key, &package.source) {
                let message = if *declared == package.source {
                    format!(
                        "declares registry source `{}` version `{}` more than once",
                        package.package, package.version
                    )
                } else {
                    format!(
                        "declares more than one source identity for registry package `{}` version `{}`",
                        package.package, package.version
                    )
                };
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry sources",
                    message,
                });
            }
            registry_source_identities.insert(
                (
                    package.package.as_str(),
                    package.version.as_str(),
                    package.source.registry.as_str(),
                    package.source.checksum.as_str(),
                ),
                package,
            );
        }
        Ok(registry_source_identities)
    }

    /// Validate registry-leaf uniqueness and each leaf's artifact and source binding.
    fn validate_registry_leaves(
        &self,
        declared_artifacts: &BTreeMap<&str, &str>,
        registry_source_identities: &RegistrySourceIndex<'_>,
    ) -> Result<(), OvenRustcError> {
        let mut package_versions = BTreeSet::new();
        for leaf in &self.registry_leaves {
            if leaf.package.trim().is_empty() || leaf.version.trim().is_empty() {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry catalog",
                    message: "registry leaf package and version must not be empty".to_string(),
                });
            }
            validate_rust_identifier(&leaf.crate_name)?;
            if leaf.crate_name != leaf.artifact.crate_name {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry catalog",
                    message: format!(
                        "registry leaf `{}` `{}` has inconsistent crate identity",
                        leaf.package, leaf.version
                    ),
                });
            }
            // One package version compiles once per domain and kind: a target library, and a host library or
            // procedural macro when a macro depends on it. Each is a distinct selected unit with its own leaf.
            if !package_versions.insert((
                leaf.package.as_str(),
                leaf.version.as_str(),
                leaf.domain,
                leaf.crate_kind,
            )) {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry catalog",
                    message: format!(
                        "declares package `{}` version `{}` more than once for one domain and kind",
                        leaf.package, leaf.version
                    ),
                });
            }
            self.validate_registry_leaf(leaf, declared_artifacts, registry_source_identities)?;
        }
        Ok(())
    }

    /// Validate one registry leaf against its declared artifact and sealed inspection source.
    fn validate_registry_leaf(
        &self,
        leaf: &OvenRustcRegistryLeaf,
        declared_artifacts: &BTreeMap<&str, &str>,
        registry_source_identities: &RegistrySourceIndex<'_>,
    ) -> Result<(), OvenRustcError> {
        let mut features = BTreeSet::new();
        for feature in &leaf.features {
            if feature.trim().is_empty() || !features.insert(feature.as_str()) {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry catalog",
                    message: format!(
                        "registry leaf `{}` `{}` declares an empty or duplicate feature",
                        leaf.package, leaf.version
                    ),
                });
            }
        }
        if !leaf.crate_kind.admits_artifact_extension(
            Path::new(&leaf.artifact.relative_path)
                .extension()
                .and_then(|extension| extension.to_str()),
        ) {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest registry catalog",
                message: format!(
                    "registry leaf `{}` `{}` must reference an artifact of its declared kind",
                    leaf.package, leaf.version
                ),
            });
        }
        match declared_artifacts.get(leaf.artifact.relative_path.as_str()) {
            Some(digest) if *digest == leaf.artifact.digest.as_str() => {}
            Some(_) => {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry catalog",
                    message: format!(
                        "registry leaf `{}` `{}` has a digest that disagrees with its sealed artifact",
                        leaf.package, leaf.version
                    ),
                });
            }
            None => {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry catalog",
                    message: format!(
                        "registry leaf `{}` `{}` references undeclared artifact `{}`",
                        leaf.package, leaf.version, leaf.artifact.relative_path
                    ),
                });
            }
        }
        let source_key = (
            leaf.package.as_str(),
            leaf.version.as_str(),
            leaf.source.registry.as_str(),
            leaf.source.checksum.as_str(),
        );
        let Some(source) = registry_source_identities.get(&source_key) else {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest registry catalog",
                message: format!(
                    "registry leaf `{}` `{}` has no matching sealed inspection source",
                    leaf.package, leaf.version
                ),
            });
        };
        let source_features = source.features.iter().map(String::as_str).collect::<BTreeSet<_>>();
        if source.source != leaf.source
            || leaf
                .features
                .iter()
                .any(|feature| !source_features.contains(feature.as_str()))
        {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest registry catalog",
                message: format!(
                    "registry leaf `{}` `{}` disagrees with or exceeds its sealed inspection source",
                    leaf.package, leaf.version
                ),
            });
        }
        Ok(())
    }

    /// Validate uniqueness of target-specific vocabulary auxiliary extern sets.
    fn validate_vocabulary_auxiliary_targets(&self) -> Result<(), OvenRustcError> {
        let mut auxiliary_targets = BTreeSet::new();
        for auxiliary in &self.vocab_auxiliary_targets {
            validate_rust_target(&auxiliary.target)?;
            if !auxiliary_targets.insert(auxiliary.target.as_str()) {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest vocabulary auxiliary targets",
                    message: format!("declares target `{}` more than once", auxiliary.target),
                });
            }
            let mut auxiliary_names = BTreeSet::new();
            for artifact in &auxiliary.externs {
                validate_rust_identifier(&artifact.crate_name)?;
                if !auxiliary_names.insert(artifact.crate_name.as_str()) {
                    return Err(OvenRustcError::InvalidInput {
                        field: "artifact manifest vocabulary auxiliary targets",
                        message: format!(
                            "target `{}` declares duplicate extern crate `{}`",
                            auxiliary.target, artifact.crate_name
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    /// Expose only this plan's copied, digest-verified registry catalog to a direct-rustc consumer.
    ///
    /// A normal command must not aggregate leaves from a different compiler Loaf: Rust metadata can bind one direct
    /// crate to a particular feature-unified dependency graph even when the package/version names look identical.
    pub fn registry_leaf_authority(
        &self,
        artifact_root: &Path,
        plan: &OvenRustcArtifactPlan,
    ) -> Option<OvenRegistryLeafAuthority> {
        (!self.registry_leaves.is_empty()).then(|| {
            OvenRegistryLeafAuthority::new_with_trusted_dependency_search_paths(
                artifact_root.to_path_buf(),
                self.registry_leaves.clone(),
                plan.dependency_search_paths.clone(),
            )
        })
    }
}

/// Normalize a Cargo package name for root-registry comparison; Cargo exposes `-` and `_` interchangeably.
pub(super) fn normalized_package_name(name: &str) -> String {
    name.replace('-', "_")
}

/// Recover the crate name from a Rust library filename such as `libincan_std_core-9c218ea857854fce.rlib`.
///
/// Returns `None` for anything that is not a `lib<name>-<hash>.{rlib,rmeta}`, so a source file, a native archive, or
/// an unhashed artifact never looks like an explicitly externed crate.
pub(super) fn rust_library_crate_name(relative_path: &str) -> Option<&str> {
    let file_name = relative_path.rsplit('/').next()?;
    let stem = file_name
        .strip_suffix(".rlib")
        .or_else(|| file_name.strip_suffix(".rmeta"))?;
    let named = stem.strip_prefix("lib")?;
    let (crate_name, hash) = named.rsplit_once('-')?;
    (!crate_name.is_empty() && !hash.is_empty() && hash.chars().all(|character| character.is_ascii_hexdigit()))
        .then_some(crate_name)
}

/// Whether a declared artifact lives somewhere below one declared search directory.
pub(super) fn artifact_is_below_search_path(relative_path: &str, search_path: &str) -> bool {
    Path::new(relative_path)
        .strip_prefix(Path::new(search_path))
        .is_ok_and(|suffix| !suffix.as_os_str().is_empty())
}

/// Validate a publisher's declared search-path shape before atomic copying.
///
/// Cargo target directories contain object and dep-info files that Oven intentionally does not retain. The eventual
/// store entry is still checked for exact directory completeness by `materialize`; this pre-copy check only confirms
/// that each declared directory is safe, unique, and owns at least one manifest-recorded artifact.
pub(super) fn validate_publisher_search_paths(
    paths: &[String],
    kind: &'static str,
    expected: &BTreeMap<String, String>,
) -> Result<(), OvenRustcError> {
    let mut seen = BTreeSet::new();
    for relative in paths {
        let normalized = normalized_relative_path(relative, kind)?;
        if !seen.insert(normalized.clone()) {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest",
                message: format!("declares duplicate {kind} path `{relative}`"),
            });
        }
        let prefix = format!("{normalized}/");
        if !expected.keys().any(|artifact| artifact.starts_with(&prefix)) {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest",
                message: format!("declares {kind} path `{relative}` without a recorded artifact"),
            });
        }
    }
    Ok(())
}

//! Validating project inspection authority and release-base bindings.

use super::{
    BTreeMap, BTreeSet, OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION,
    OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION, OvenArtifactKind, OvenBuildIntent, OvenProjectExtensionPayload,
    OvenProjectInspectionAuthorityPayload, OvenProjectInspectionConstituent, OvenProjectInspectionRootDependency,
    OvenProjectInspectionSourceOwner, OvenProjectInspectionTestDependencyRoot, OvenProjectRegistrySourceDependency,
    OvenRustcArtifactManifest, OvenRustcArtifactPartition, OvenRustcError, OvenRustcRegistrySourcePackage, Version,
    normalized_relative_path, validate_rust_identifier,
};

/// Validate one stored project extension against the exact installed release Loaf it names.
///
/// Both Rust-inspection source selection and final execution call this boundary. A payload may not become source
/// authority merely because its complete plan has a valid shape: the publisher plan, selected base, partition, and
/// retained extension paths must all describe the same immutable closure.
pub fn validate_project_extension_payload_against_base(
    payload: &OvenProjectExtensionPayload,
    base_loaf_identity: &str,
    base_build_unit_identity: &str,
    base: &OvenRustcArtifactManifest,
) -> Result<OvenRustcArtifactPartition, OvenRustcError> {
    validate_project_extension_payload_shape(payload, &base.intent)?;
    if payload.base_loaf_identity != base_loaf_identity || payload.base_build_unit_identity != base_build_unit_identity
    {
        return Err(OvenRustcError::InvalidInput {
            field: "project extension base",
            message: "does not name the exact installed release Loaf and build unit".to_string(),
        });
    }
    // Recomposition must reproduce the sealed complete plan exactly, so it consumes the payload's own record of
    // which registry packages the generated root declares; see `with_release_cohort_from_base`.
    let root_registry_packages = payload
        .registry_source_dependencies
        .iter()
        .chain(&payload.dev_registry_source_dependencies)
        .map(|dependency| dependency.package.clone())
        .collect::<BTreeSet<_>>();
    let recomposed = payload
        .publisher_plan
        .with_release_cohort_from_base(base, &root_registry_packages)?;
    if recomposed != payload.complete_plan {
        return Err(OvenRustcError::InvalidInput {
            field: "project extension release cohort",
            message: "effective plan does not match its sealed publisher plan and exact release cohort".to_string(),
        });
    }
    let partition = payload.complete_plan.partition_against_base(base)?;
    if payload.extension_paths != partition.extension_paths.iter().cloned().collect::<Vec<_>>() {
        return Err(OvenRustcError::InvalidInput {
            field: "project extension fragment",
            message: "does not retain the exact delta derived from its selected base".to_string(),
        });
    }
    if partition.base_paths.is_empty() || partition.extension_paths.is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "project extension fragment",
            message: "must contain both a base fragment and a project-specific fragment".to_string(),
        });
    }
    Ok(partition)
}

/// Validate one stored project extension without reading or hashing its materialized artifact bytes.
///
/// Exact base recomposition remains a later validation step once the compiler-owned release Loaf is resolved. This
/// boundary rejects malformed stored constituents immediately after their exact identity and lease are acquired.
pub(super) fn validate_project_extension_payload_shape(
    payload: &OvenProjectExtensionPayload,
    intent: &OvenBuildIntent,
) -> Result<(), OvenRustcError> {
    if payload.schema_version != OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION {
        return Err(OvenRustcError::InvalidInput {
            field: "project extension payload",
            message: format!(
                "schema {} is incompatible with current schema {OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION}",
                payload.schema_version
            ),
        });
    }
    if payload.base_loaf_identity.trim().is_empty() || payload.base_build_unit_identity.trim().is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "project extension base",
            message: "must name one exact release Loaf and build unit".to_string(),
        });
    }
    payload.publisher_plan.validate_shape(intent)?;
    payload.complete_plan.validate_shape(intent)?;
    validate_project_registry_source_dependencies(
        &payload.registry_source_dependencies,
        payload.complete_plan.registry_sources.as_slice(),
    )?;
    validate_project_registry_source_dependencies(
        &payload.dev_registry_source_dependencies,
        payload.complete_plan.registry_sources.as_slice(),
    )?;
    let declared = payload.complete_plan.declared_artifact_paths()?;
    let mut prior = None::<String>;
    for path in &payload.extension_paths {
        let normalized = normalized_relative_path(path, "project extension artifact")?;
        if prior.as_deref().is_some_and(|prior| prior >= normalized.as_str()) || !declared.contains(&normalized) {
            return Err(OvenRustcError::InvalidInput {
                field: "project extension fragment",
                message: "must be strictly sorted and contain only artifacts declared by the complete plan".to_string(),
            });
        }
        prior = Some(normalized);
    }
    Ok(())
}

/// Validate the explicit baker's alias-to-source authority against the complete immutable source catalog.
pub(super) fn validate_project_registry_source_dependencies(
    dependencies: &[OvenProjectRegistrySourceDependency],
    sources: &[OvenRustcRegistrySourcePackage],
) -> Result<(), OvenRustcError> {
    let mut previous_alias = None;
    for dependency in dependencies {
        validate_rust_identifier(&dependency.alias)?;
        if dependency.package.trim().is_empty()
            || dependency.version.trim().is_empty()
            || Version::parse(&dependency.version).is_err()
            || !dependency.registry.starts_with("registry+")
            || dependency.checksum.trim().is_empty()
        {
            return Err(OvenRustcError::InvalidInput {
                field: "project extension registry dependencies",
                message: format!(
                    "dependency alias `{}` has an incomplete or invalid locked source identity",
                    dependency.alias
                ),
            });
        }
        if previous_alias.is_some_and(|previous| previous >= dependency.alias.as_str()) {
            return Err(OvenRustcError::InvalidInput {
                field: "project extension registry dependencies",
                message: "must be strictly sorted by unique dependency alias".to_string(),
            });
        }
        previous_alias = Some(dependency.alias.as_str());
        let matches = sources
            .iter()
            .filter(|source| {
                source.package == dependency.package
                    && source.version == dependency.version
                    && source.source.registry == dependency.registry
                    && source.source.checksum == dependency.checksum
            })
            .count();
        if matches != 1 {
            return Err(OvenRustcError::InvalidInput {
                field: "project extension registry dependencies",
                message: format!(
                    "dependency alias `{}` has {matches} exact records in the sealed registry source catalog",
                    dependency.alias
                ),
            });
        }
    }
    Ok(())
}

/// Validate one singular project inspection authority before publication or selection.
pub fn validate_project_inspection_authority_payload(
    payload: &OvenProjectInspectionAuthorityPayload,
) -> Result<(), OvenRustcError> {
    validate_project_inspection_authority_identity(payload)?;
    validate_project_inspection_constituents(payload)?;
    validate_project_inspection_test_envelope(payload)?;
    validate_project_inspection_source_catalog(payload)
}

/// Validate the authority schema and required top-level identities.
fn validate_project_inspection_authority_identity(
    payload: &OvenProjectInspectionAuthorityPayload,
) -> Result<(), OvenRustcError> {
    if payload.schema_version != OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION {
        return Err(OvenRustcError::InvalidInput {
            field: "project inspection authority",
            message: format!(
                "schema {} is incompatible with current schema {OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION}",
                payload.schema_version
            ),
        });
    }
    if payload.project_identity.trim().is_empty()
        || payload.source_authority_digest.trim().is_empty()
        || payload.compiler_version.trim().is_empty()
        || payload.registry_lock_digest.trim().is_empty()
    {
        return Err(OvenRustcError::InvalidInput {
            field: "project inspection authority",
            message: "must bind project, source, compiler, and canonical registry-lock identities".to_string(),
        });
    }
    Ok(())
}

/// Validate constituent ordering, identity uniqueness, receipts, kinds, and base bindings.
fn validate_project_inspection_constituents(
    payload: &OvenProjectInspectionAuthorityPayload,
) -> Result<(), OvenRustcError> {
    let mut seen_constituents = Vec::new();
    let mut seen_constituent_identities = BTreeSet::new();
    let mut saw_stored_constituent = false;
    let mut release_identities = BTreeSet::new();
    for constituent in &payload.constituents {
        match constituent {
            OvenProjectInspectionConstituent::ReleaseLoaf {
                loaf_identity,
                build_unit_identity,
                ..
            } if loaf_identity.trim().is_empty() || build_unit_identity.trim().is_empty() => {
                return Err(OvenRustcError::InvalidInput {
                    field: "project inspection authority constituent",
                    message: "release Loaf identity and build-unit identity must be non-empty".to_string(),
                });
            }
            OvenProjectInspectionConstituent::ReleaseLoaf {
                loaf_identity, receipt, ..
            } => {
                if saw_stored_constituent
                    || !release_identities.insert(loaf_identity.as_str())
                    || !seen_constituent_identities.insert(loaf_identity.as_str())
                {
                    return Err(OvenRustcError::InvalidInput {
                        field: "project inspection authority constituents",
                        message: "release Loafs must be unique and precede every store-owned constituent".to_string(),
                    });
                }
                receipt
                    .verify_identity()
                    .map_err(|error| OvenRustcError::InvalidInput {
                        field: "project inspection authority release receipt",
                        message: error.to_string(),
                    })?;
            }
            OvenProjectInspectionConstituent::Stored {
                identity,
                artifact_kind,
                receipt,
                base_loaf_identity,
            } => {
                saw_stored_constituent = true;
                receipt
                    .verify_identity()
                    .map_err(|error| OvenRustcError::InvalidInput {
                        field: "project inspection authority constituent receipt",
                        message: error.to_string(),
                    })?;
                if identity.trim().is_empty()
                    || !seen_constituent_identities.insert(identity.as_str())
                    || !matches!(
                        artifact_kind,
                        OvenArtifactKind::DirectRustcPlan | OvenArtifactKind::ProjectPayload
                    )
                    || matches!(artifact_kind, OvenArtifactKind::DirectRustcPlan) && base_loaf_identity.is_some()
                    || matches!(artifact_kind, OvenArtifactKind::ProjectPayload) && base_loaf_identity.is_none()
                {
                    return Err(OvenRustcError::InvalidInput {
                        field: "project inspection authority constituent",
                        message: "stored constituent has inconsistent identity, kind, or base evidence".to_string(),
                    });
                }
                if let Some(base_loaf_identity) = base_loaf_identity
                    && !release_identities.contains(base_loaf_identity.as_str())
                {
                    return Err(OvenRustcError::InvalidInput {
                        field: "project inspection authority constituent",
                        message: "project extension must follow and name one exact release-Loaf constituent"
                            .to_string(),
                    });
                }
            }
        }
        if seen_constituents.contains(constituent) {
            return Err(OvenRustcError::InvalidInput {
                field: "project inspection authority constituents",
                message: "must not repeat an immutable constituent".to_string(),
            });
        }
        seen_constituents.push(constituent.clone());
    }
    Ok(())
}

/// Validate the optional debug dependency envelope and every role-bearing root.
fn validate_project_inspection_test_envelope(
    payload: &OvenProjectInspectionAuthorityPayload,
) -> Result<(), OvenRustcError> {
    if let Some(envelope) = &payload.test_dependency_envelope {
        if envelope.dependency_surface_digest.trim().is_empty() {
            return Err(OvenRustcError::InvalidInput {
                field: "project inspection test dependency envelope",
                message: "must bind a non-empty canonical dependency-surface digest".to_string(),
            });
        }
        let Some(constituent) = payload.constituents.get(envelope.constituent_index) else {
            return Err(OvenRustcError::InvalidInput {
                field: "project inspection test dependency envelope",
                message: "must name one exact immutable constituent".to_string(),
            });
        };
        let receipt = match constituent {
            OvenProjectInspectionConstituent::ReleaseLoaf { receipt, .. } => receipt,
            OvenProjectInspectionConstituent::Stored {
                artifact_kind: OvenArtifactKind::DirectRustcPlan,
                receipt,
                base_loaf_identity: None,
                ..
            } => receipt,
            OvenProjectInspectionConstituent::Stored {
                artifact_kind: OvenArtifactKind::ProjectPayload,
                receipt,
                base_loaf_identity: Some(_),
                ..
            } => receipt,
            _ => {
                return Err(OvenRustcError::InvalidInput {
                    field: "project inspection test dependency envelope",
                    message: "must name the exact release Loaf, one self-contained direct plan, or one base-partitioned project constituent"
                        .to_string(),
                });
            }
        };
        if receipt.intent.profile != "debug" {
            return Err(OvenRustcError::InvalidInput {
                field: "project inspection test dependency envelope",
                message: "must name a debug-profile dependency constituent".to_string(),
            });
        }
        let mut role_indices = vec![envelope.constituent_index];
        let mut provider_keys = BTreeSet::new();
        for provider in &envelope.provider_constituents {
            if provider.dependency_key.trim().is_empty() || !provider_keys.insert(provider.dependency_key.as_str()) {
                return Err(OvenRustcError::InvalidInput {
                    field: "project inspection test dependency envelope",
                    message: "provider constituent keys must be non-empty and unique".to_string(),
                });
            }
            if role_indices.contains(&provider.constituent_index) {
                return Err(OvenRustcError::InvalidInput {
                    field: "project inspection test dependency envelope",
                    message: "must not repeat a role-bearing constituent".to_string(),
                });
            }
            let Some(OvenProjectInspectionConstituent::Stored {
                artifact_kind,
                receipt: provider_receipt,
                base_loaf_identity,
                ..
            }) = payload.constituents.get(provider.constituent_index)
            else {
                return Err(OvenRustcError::InvalidInput {
                    field: "project inspection test dependency envelope",
                    message: "provider role must name one exact stored direct-Rustc constituent".to_string(),
                });
            };
            let valid_shape = matches!(
                (artifact_kind, base_loaf_identity),
                (OvenArtifactKind::DirectRustcPlan, None) | (OvenArtifactKind::ProjectPayload, Some(_))
            );
            if !valid_shape || provider_receipt.intent != receipt.intent {
                return Err(OvenRustcError::InvalidInput {
                    field: "project inspection test dependency envelope",
                    message: "provider role has a different kind, base, or build intent".to_string(),
                });
            }
            role_indices.push(provider.constituent_index);
        }
        for (alias, root) in &envelope.dependency_roots {
            validate_rust_identifier(alias)?;
            let (dependency_digest, locked) = match root {
                OvenProjectInspectionTestDependencyRoot::Registry {
                    dependency_digest,
                    locked,
                } => (dependency_digest, Some(locked)),
                OvenProjectInspectionTestDependencyRoot::Path { dependency_digest }
                | OvenProjectInspectionTestDependencyRoot::Git { dependency_digest } => (dependency_digest, None),
            };
            if dependency_digest.trim().is_empty() {
                return Err(OvenRustcError::InvalidInput {
                    field: "project inspection test dependency envelope root",
                    message: format!("dependency alias `{alias}` has no portable root digest"),
                });
            }
            if let Some(locked) = locked
                && (locked.alias != *alias
                    || !payload
                        .registry_source_dependencies
                        .iter()
                        .chain(&payload.dev_registry_source_dependencies)
                        .any(|candidate| candidate == locked))
            {
                return Err(OvenRustcError::InvalidInput {
                    field: "project inspection test dependency envelope root",
                    message: format!(
                        "registry dependency alias `{alias}` does not name one exact normal/dev publisher root"
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Validate source ordering, ownership, root edges, and normal/dev alias agreement.
fn validate_project_inspection_source_catalog(
    payload: &OvenProjectInspectionAuthorityPayload,
) -> Result<(), OvenRustcError> {
    let mut catalog = Vec::with_capacity(payload.registry_sources.len());
    let mut prior_key: Option<(String, String, String, String)> = None;
    for source in &payload.registry_sources {
        let package = &source.package;
        let key = (
            package.package.clone(),
            package.version.clone(),
            package.source.registry.clone(),
            package.source.checksum.clone(),
        );
        if prior_key.as_ref().is_some_and(|prior| prior >= &key)
            || package.package.trim().is_empty()
            || Version::parse(&package.version).is_err()
            || !package.source.registry.starts_with("registry+")
            || package.source.checksum.trim().is_empty()
            || package.source.digest.trim().is_empty()
            || normalized_relative_path(&package.source.relative_root, "registry source root").is_err()
        {
            return Err(OvenRustcError::InvalidInput {
                field: "project inspection authority source catalog",
                message: "must be strictly sorted and contain complete portable source identities".to_string(),
            });
        }
        if let OvenProjectInspectionSourceOwner::Constituent { index } = source.owner
            && index >= payload.constituents.len()
        {
            return Err(OvenRustcError::InvalidInput {
                field: "project inspection authority source owner",
                message: format!("references missing constituent index {index}"),
            });
        }
        prior_key = Some(key);
        catalog.push(package.clone());
    }
    validate_project_inspection_root_dependencies(&payload.registry_source_dependencies, &catalog)?;
    validate_project_inspection_root_dependencies(&payload.dev_registry_source_dependencies, &catalog)?;
    let normal_by_alias = payload
        .registry_source_dependencies
        .iter()
        .map(|dependency| (dependency.alias.as_str(), dependency))
        .collect::<BTreeMap<_, _>>();
    for dependency in &payload.dev_registry_source_dependencies {
        if normal_by_alias
            .get(dependency.alias.as_str())
            .is_some_and(|normal| *normal != dependency)
        {
            return Err(OvenRustcError::InvalidInput {
                field: "project inspection authority dependencies",
                message: format!(
                    "normal and dev dependency alias `{}` resolve to conflicting exact root edges",
                    dependency.alias
                ),
            });
        }
    }
    Ok(())
}

/// Validate feature-bound root edges against the singular authority's exact source catalog.
pub(super) fn validate_project_inspection_root_dependencies(
    dependencies: &[OvenProjectInspectionRootDependency],
    sources: &[OvenRustcRegistrySourcePackage],
) -> Result<(), OvenRustcError> {
    let mut previous_alias = None;
    for dependency in dependencies {
        validate_rust_identifier(&dependency.alias)?;
        let mut features = dependency.requested_features.clone();
        features.sort();
        features.dedup();
        if dependency.package.trim().is_empty()
            || Version::parse(&dependency.version).is_err()
            || !dependency.registry.starts_with("registry+")
            || dependency.checksum.trim().is_empty()
            || features != dependency.requested_features
            || previous_alias.is_some_and(|previous| previous >= dependency.alias.as_str())
        {
            return Err(OvenRustcError::InvalidInput {
                field: "project inspection authority dependencies",
                message: "must be strictly alias-sorted and contain complete locked source and feature evidence"
                    .to_string(),
            });
        }
        previous_alias = Some(dependency.alias.as_str());
        let matches = sources
            .iter()
            .filter(|source| {
                source.package == dependency.package
                    && source.version == dependency.version
                    && source.source.registry == dependency.registry
                    && source.source.checksum == dependency.checksum
                    && dependency
                        .requested_features
                        .iter()
                        .all(|feature| source.features.contains(feature))
            })
            .count();
        if matches != 1 {
            return Err(OvenRustcError::InvalidInput {
                field: "project inspection authority dependencies",
                message: format!(
                    "dependency alias `{}` has {matches} exact feature-compatible records in the sealed source catalog",
                    dependency.alias
                ),
            });
        }
    }
    Ok(())
}

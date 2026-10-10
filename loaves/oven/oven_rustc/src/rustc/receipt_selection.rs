//! Selecting receipt-bound direct-rustc plans and project inspection authority.

use super::{
    BTreeSet, DependencySource, DependencySpec, Deserialize, OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION,
    OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH, OvenArtifactKind, OvenArtifactManifest, OvenCallerOwnedRustcLibrary,
    OvenDirectRustcBake, OvenDirectRustcOutputKind, OvenDirectRustcTestBake, OvenLoadedProjectInspectionAuthority,
    OvenProjectExtensionPayload, OvenProjectInspectionAuthorityPayload, OvenProjectInspectionAuthorityRef,
    OvenProjectInspectionConstituent, OvenProjectInspectionTestDependencyRoot, OvenReceipt, OvenRustcArtifactManifest,
    OvenRustcError, OvenRustcRegistrySourcePackage, OvenStore, OvenStoreExecutionPayload, OvenStoreLease,
    OvenStoredDirectRustcLibraryRequest, OvenStoredDirectRustcRunRequest, OvenStoredDirectRustcTestRequest, PathBuf,
    Version, VersionReq, attach_caller_owned_rustc_libraries, bake_direct_rustc, digest_bytes, fs,
    validate_project_extension_payload_shape, validate_project_inspection_authority_payload,
};

/// Select a receipt-bound direct-rustc closure and retain its lease until the caller finishes execution.
pub fn bake_stored_direct_rustc_test(
    request: &OvenStoredDirectRustcTestRequest<'_>,
) -> Result<OvenDirectRustcTestBake, OvenRustcError> {
    bake_stored_direct_rustc_test_with_libraries(request, &[])
}

/// Select a receipt-bound direct-rustc closure and compile a native test while linking caller-owned direct Rust
/// libraries.
///
/// The supplemental libraries are generated only by the path-only Oven materializer. They are caller output rather
/// than store artifacts, but their bytes enter the direct output reuse identity through the same checked attachment
/// mechanism used for materialized Incan library dependencies.
pub fn bake_stored_direct_rustc_test_with_libraries(
    request: &OvenStoredDirectRustcTestRequest<'_>,
    caller_owned_libraries: &[OvenCallerOwnedRustcLibrary],
) -> Result<OvenDirectRustcTestBake, OvenRustcError> {
    let (artifacts, artifact_root, lease) =
        select_stored_direct_rustc_plan(request.store, &request.plan_identity, &request.receipt)?;
    let mut artifact_plan = artifacts.materialize_trusted_store(&artifact_root, &request.receipt.intent)?;
    attach_caller_owned_rustc_libraries(&mut artifact_plan, caller_owned_libraries)?;
    let mut bake = bake_direct_rustc(
        &request.receipt,
        &artifacts,
        &artifact_root,
        &request.rustc,
        &request.source,
        &request.output,
        &request.crate_name,
        &request.edition,
        &request.source_evidence_key,
        &request.source_evidence_key,
        true,
        OvenDirectRustcOutputKind::Binary,
        true,
        Some(&artifact_plan),
        false,
        &request.receipt.intent.features,
    )?;
    bake.lease = Some(lease);
    Ok(bake)
}

/// Select a receipt-bound direct-rustc closure and retain its lease until the caller finishes running the binary.
pub fn bake_stored_direct_rustc_run(
    request: &OvenStoredDirectRustcRunRequest<'_>,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_stored_direct_rustc_run_with_libraries(request, &[])
}

/// Select a receipt-bound direct-rustc closure and compile a binary while linking explicit caller-owned libraries.
///
/// The additional libraries are not stored artifacts and never participate in native-plan selection. They are the
/// caller-output bridge for already materialized Incan `pub::` dependencies; their exact bytes are recorded in the
/// consumer output sidecar before reuse is allowed.
pub fn bake_stored_direct_rustc_run_with_libraries(
    request: &OvenStoredDirectRustcRunRequest<'_>,
    caller_owned_libraries: &[OvenCallerOwnedRustcLibrary],
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    let (artifacts, artifact_root, lease) =
        select_stored_direct_rustc_plan(request.store, &request.plan_identity, &request.receipt)?;
    let mut artifact_plan = artifacts.materialize_trusted_store(&artifact_root, &request.receipt.intent)?;
    attach_caller_owned_rustc_libraries(&mut artifact_plan, caller_owned_libraries)?;
    let mut bake = bake_direct_rustc(
        &request.receipt,
        &artifacts,
        &artifact_root,
        &request.rustc,
        &request.source,
        &request.output,
        &request.crate_name,
        &request.edition,
        &request.source_evidence_key,
        &request.source_evidence_key,
        false,
        OvenDirectRustcOutputKind::Binary,
        true,
        Some(&artifact_plan),
        false,
        &request.receipt.intent.features,
    )?;
    bake.lease = Some(lease);
    Ok(bake)
}

/// Select a receipt-bound direct-rustc closure and compile one regular caller-owned Rust library without a Cargo
/// consumer process.
pub fn bake_stored_direct_rustc_library(
    request: &OvenStoredDirectRustcLibraryRequest<'_>,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_stored_direct_rustc_library_with_libraries(request, &[])
}

/// Select a receipt-bound direct-rustc closure and compile a library while linking explicit caller-owned libraries.
pub fn bake_stored_direct_rustc_library_with_libraries(
    request: &OvenStoredDirectRustcLibraryRequest<'_>,
    caller_owned_libraries: &[OvenCallerOwnedRustcLibrary],
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    let (artifacts, artifact_root, lease) =
        select_stored_direct_rustc_plan(request.store, &request.plan_identity, &request.receipt)?;
    let mut artifact_plan = artifacts.materialize_trusted_store(&artifact_root, &request.receipt.intent)?;
    attach_caller_owned_rustc_libraries(&mut artifact_plan, caller_owned_libraries)?;
    let mut bake = bake_direct_rustc(
        &request.receipt,
        &artifacts,
        &artifact_root,
        &request.rustc,
        &request.source,
        &request.output,
        &request.crate_name,
        &request.edition,
        &request.source_evidence_key,
        &request.source_evidence_key,
        false,
        OvenDirectRustcOutputKind::Library,
        true,
        Some(&artifact_plan),
        false,
        &request.receipt.intent.features,
    )?;
    bake.lease = Some(lease);
    Ok(bake)
}

/// Select the unique stored direct-rustc plan authorized by a generated-project receipt and retain its lease.
///
/// Normal Oven commands select through the receipt's reusable build-unit identity rather than accepting a
/// caller-provided cache location or artifact identity. Generated source remains verified independently at execution,
/// so compatible clean worktrees can reuse one native closure without sharing source or final-output directories.
/// Distinct plans remain an explicit publisher error: silently choosing a "latest" plan would make normal command
/// execution non-deterministic. Byte-identical closures from compatible receipts are equivalent reusable entries;
/// those are collapsed by stable identity while future publication deduplicates them at admission. Matching,
/// integrity verification, and lease acquisition occur under one store-manager lock so policy pruning cannot reclaim
/// a compatible candidate after header selection but before execution begins.
pub fn select_direct_rustc_plan_for_execution(
    store: &OvenStore,
    receipt: &OvenReceipt,
) -> Result<Option<OvenStoreExecutionPayload>, OvenRustcError> {
    receipt
        .verify_identity()
        .map_err(|error| OvenRustcError::InvalidInput {
            field: "receipt",
            message: error.to_string(),
        })?;
    let mut matches = Vec::new();
    for selected in store.select_payloads_matching_for_execution(|manifest| {
        manifest.kind == OvenArtifactKind::DirectRustcPlan
            && manifest.build_unit_identity == receipt.build_unit_identity
            && manifest.intent == receipt.intent
    })? {
        let Ok(plan) = serde_json::from_slice::<OvenRustcArtifactManifest>(&selected.payload) else {
            continue;
        };
        // A prior Alpha publisher may have retained a payload whose `--extern` identifiers no longer satisfy the
        // stricter direct-rustc contract. Ignore it for selection so a corrected explicit publication can coexist
        // until ordinary policy-driven pruning reclaims the inactive entry.
        if plan.validate_shape(&receipt.intent).is_ok() {
            matches.push(selected);
        }
    }
    match matches.len() {
        1 => Ok(matches.pop()),
        0 => Ok(None),
        _ if matches
            .iter()
            .skip(1)
            .all(|candidate| reusable_direct_rustc_entry(&matches[0], candidate)) =>
        {
            matches.sort_by(|left, right| left.manifest.identity.cmp(&right.manifest.identity));
            Ok(Some(matches.remove(0)))
        }
        _ => Err(OvenRustcError::PlanSelection {
            receipt_identity: receipt.identity.clone(),
            message: format!(
                "multiple compatible stored direct-rustc plans are available: {}",
                matches
                    .iter()
                    .map(|entry| entry.manifest.identity.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }),
    }
}

/// Return whether two receipt-compatible selections retain precisely the same reusable immutable closure.
pub(super) fn reusable_direct_rustc_entry(left: &OvenStoreExecutionPayload, right: &OvenStoreExecutionPayload) -> bool {
    left.manifest.domain == right.manifest.domain
        && left.manifest.payload == right.manifest.payload
        && left.manifest.materialized_files == right.manifest.materialized_files
}

/// Select the unique stored direct-rustc plan identity authorized by a generated-project receipt.
///
/// This compatibility helper drops the returned execution lease with the identity. Normal command execution must
/// use [`select_direct_rustc_plan_for_execution`] so a concurrent bounded-policy publication cannot prune a chosen
/// plan before the caller starts using it.
pub fn select_direct_rustc_plan_identity(store: &OvenStore, receipt: &OvenReceipt) -> Result<String, OvenRustcError> {
    select_direct_rustc_plan_for_execution(store, receipt)?
        .map(|selected| selected.manifest.identity)
        .ok_or_else(|| OvenRustcError::PlanSelection {
            receipt_identity: receipt.identity.clone(),
            message: "no compatible stored direct-rustc plan is available".to_string(),
        })
}

/// Minimal project-inspection header decoded before any version-specific authority fields.
#[derive(Deserialize)]
struct OvenProjectInspectionAuthorityHeader {
    schema_version: u32,
}

/// Reject an unknown project-inspection wire version before decoding its version-specific body.
pub(super) fn preflight_project_inspection_authority_schema(
    identity: &str,
    bytes: &[u8],
) -> Result<(), OvenRustcError> {
    let header = serde_json::from_slice::<OvenProjectInspectionAuthorityHeader>(bytes).map_err(|error| {
        OvenRustcError::InvalidStoredPlan {
            identity: identity.to_string(),
            message: format!("payload has no valid project inspection authority header: {error}"),
        }
    })?;
    if header.schema_version != OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION {
        return Err(OvenRustcError::UnsupportedProjectInspectionAuthoritySchema {
            found: header.schema_version,
            expected: OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION,
        });
    }
    Ok(())
}

/// Decode one current project inspection authority after its schema and physical owner have been validated.
pub(super) fn decode_project_inspection_authority(
    identity: &str,
    bytes: &[u8],
) -> Result<OvenProjectInspectionAuthorityPayload, OvenRustcError> {
    serde_json::from_slice::<OvenProjectInspectionAuthorityPayload>(bytes).map_err(|error| {
        OvenRustcError::InvalidStoredPlan {
            identity: identity.to_string(),
            message: format!("payload is not a project inspection authority: {error}"),
        }
    })
}

/// Load the one project inspection authority named by a source-current completed output.
///
/// Selection never searches by dependency compatibility. The authority entry is exact, and all of its store-owned
/// constituents are acquired in one batch before any source is projected. Release-Loaf constituents are resolved
/// separately by the caller against the active immutable toolchain generation.
pub fn load_project_inspection_authority(
    store: &OvenStore,
    authority_ref: &OvenProjectInspectionAuthorityRef,
    project_identity: &str,
    source_authority_digest: &str,
    compiler_version: &str,
) -> Result<OvenLoadedProjectInspectionAuthority, OvenRustcError> {
    let (source_owner, payload) = select_project_inspection_source(
        store,
        authority_ref,
        project_identity,
        source_authority_digest,
        compiler_version,
    )?;
    let stored_constituents = select_project_inspection_constituents(store, &payload)?;
    validate_selected_project_inspection_constituents(&payload, &stored_constituents)?;
    Ok(OvenLoadedProjectInspectionAuthority::new(
        source_owner,
        payload,
        stored_constituents,
    ))
}

/// Select and validate the exact authority owner, payload identity, and registry lock.
fn select_project_inspection_source(
    store: &OvenStore,
    authority_ref: &OvenProjectInspectionAuthorityRef,
    project_identity: &str,
    source_authority_digest: &str,
    compiler_version: &str,
) -> Result<(OvenStoreExecutionPayload, OvenProjectInspectionAuthorityPayload), OvenRustcError> {
    let mut selected = store
        .select_payloads_for_execution(std::slice::from_ref(&authority_ref.identity))
        .map_err(|error| OvenRustcError::PlanSelection {
            receipt_identity: authority_ref.receipt_identity.clone(),
            message: format!(
                "source-current project output cannot select exact project inspection authority `{}`: {error}",
                authority_ref.identity
            ),
        })?;
    let source_owner = selected.pop().ok_or_else(|| OvenRustcError::PlanSelection {
        receipt_identity: authority_ref.receipt_identity.clone(),
        message: format!(
            "source-current project output could not retain exact project inspection authority `{}`",
            authority_ref.identity
        ),
    })?;
    let manifest = &source_owner.manifest;
    if manifest.identity != authority_ref.identity
        || manifest.kind != OvenArtifactKind::ProjectInspectionAuthority
        || manifest.receipt_identity != authority_ref.receipt_identity
        || manifest.build_unit_identity != authority_ref.build_unit_identity
    {
        return Err(OvenRustcError::InvalidStoredPlan {
            identity: manifest.identity.clone(),
            message: "project inspection authority differs from its exact kind, receipt, or build-unit reference"
                .to_string(),
        });
    }
    preflight_project_inspection_authority_schema(&manifest.identity, &source_owner.payload)?;
    source_owner.verify_materialized_files()?;
    let payload = decode_project_inspection_authority(&manifest.identity, &source_owner.payload)?;
    validate_project_inspection_authority_payload(&payload)?;
    if payload.project_identity != project_identity
        || payload.source_authority_digest != source_authority_digest
        || payload.compiler_version != compiler_version
    {
        return Err(OvenRustcError::InvalidStoredPlan {
            identity: manifest.identity.clone(),
            message: "project inspection authority does not match the selected output's project, source, or compiler evidence"
                .to_string(),
        });
    }
    if !payload.registry_sources.is_empty() {
        let lock = source_owner.artifact_root.join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH);
        let lock_bytes = fs::read(&lock).map_err(|source| OvenRustcError::Io {
            path: lock.clone(),
            source,
        })?;
        let actual = digest_bytes(&lock_bytes);
        if actual != payload.registry_lock_digest {
            return Err(OvenRustcError::ArtifactDigestMismatch {
                path: lock,
                expected: payload.registry_lock_digest.clone(),
                actual,
            });
        }
    }
    Ok((source_owner, payload))
}

/// Acquire every stored authority constituent in one lease-retaining batch.
fn select_project_inspection_constituents(
    store: &OvenStore,
    payload: &OvenProjectInspectionAuthorityPayload,
) -> Result<Vec<OvenStoreExecutionPayload>, OvenRustcError> {
    let stored_refs = payload
        .constituents
        .iter()
        .filter_map(|constituent| match constituent {
            OvenProjectInspectionConstituent::Stored { identity, .. } => Some(identity.clone()),
            OvenProjectInspectionConstituent::ReleaseLoaf { .. } => None,
        })
        .collect::<Vec<_>>();
    let stored_constituents = if stored_refs.is_empty() {
        Vec::new()
    } else {
        store.select_payloads_for_execution(&stored_refs)?
    };
    Ok(stored_constituents)
}

/// Revalidate every selected constituent against its sealed receipt, kind, and release base.
pub(super) fn validate_selected_project_inspection_constituents(
    payload: &OvenProjectInspectionAuthorityPayload,
    stored_constituents: &[OvenStoreExecutionPayload],
) -> Result<(), OvenRustcError> {
    let mut selected_index = 0;
    for constituent in &payload.constituents {
        let OvenProjectInspectionConstituent::Stored {
            identity,
            artifact_kind,
            receipt,
            base_loaf_identity,
        } = constituent
        else {
            continue;
        };
        let selected = stored_constituents
            .get(selected_index)
            .ok_or_else(|| OvenRustcError::PlanSelection {
                receipt_identity: receipt.identity.clone(),
                message: format!("project inspection authority lost constituent `{identity}` during batch selection"),
            })?;
        selected_index += 1;
        if selected.manifest.identity != *identity
            || selected.manifest.kind != *artifact_kind
            || !project_inspection_constituent_matches_receipt(&selected.manifest, *artifact_kind, receipt)
        {
            return Err(OvenRustcError::InvalidStoredPlan {
                identity: identity.clone(),
                message: "project inspection constituent differs from its sealed identity, receipt, kind, or intent"
                    .to_string(),
            });
        }
        match artifact_kind {
            OvenArtifactKind::DirectRustcPlan => {
                let plan = serde_json::from_slice::<OvenRustcArtifactManifest>(&selected.payload).map_err(|error| {
                    OvenRustcError::InvalidStoredPlan {
                        identity: identity.clone(),
                        message: format!("direct-plan constituent payload is invalid: {error}"),
                    }
                })?;
                plan.validate_shape(&receipt.intent)?;
            }
            OvenArtifactKind::ProjectPayload => {
                let extension =
                    serde_json::from_slice::<OvenProjectExtensionPayload>(&selected.payload).map_err(|error| {
                        OvenRustcError::InvalidStoredPlan {
                            identity: identity.clone(),
                            message: format!("project-extension constituent payload is invalid: {error}"),
                        }
                    })?;
                validate_project_extension_payload_shape(&extension, &receipt.intent)?;
                let base_build_unit_matches = payload.constituents.iter().any(|candidate| {
                    matches!(
                        candidate,
                        OvenProjectInspectionConstituent::ReleaseLoaf {
                            loaf_identity,
                            build_unit_identity,
                            ..
                        } if loaf_identity == &extension.base_loaf_identity
                            && build_unit_identity == &extension.base_build_unit_identity
                    )
                });
                if base_loaf_identity.as_deref() != Some(extension.base_loaf_identity.as_str())
                    || !base_build_unit_matches
                {
                    return Err(OvenRustcError::InvalidStoredPlan {
                        identity: identity.clone(),
                        message: "project-extension constituent has different release-Loaf or build-unit evidence"
                            .to_string(),
                    });
                }
            }
            unsupported => {
                return Err(OvenRustcError::InvalidStoredPlan {
                    identity: identity.clone(),
                    message: format!("unsupported project inspection constituent kind {unsupported:?}"),
                });
            }
        }
    }
    Ok(())
}

/// Return whether one stored inspection constituent is authorized by its receiving project receipt.
///
/// Direct-Rustc plans are shared immutable closures, so their original publisher receipt may differ from the
/// receiving project receipt while the reusable build unit and build intent remain exact. Project extensions remain
/// receipt-specific because they contain caller-owned project material.
pub fn project_inspection_constituent_matches_receipt(
    manifest: &OvenArtifactManifest,
    artifact_kind: OvenArtifactKind,
    receipt: &OvenReceipt,
) -> bool {
    manifest.build_unit_identity == receipt.build_unit_identity
        && manifest.intent == receipt.intent
        && (artifact_kind == OvenArtifactKind::DirectRustcPlan || manifest.receipt_identity == receipt.identity)
}

/// Check a generated inspection batch against the exact normal/dev roots sealed by one project authority.
pub fn project_inspection_authority_supports_dependencies(
    payload: &OvenProjectInspectionAuthorityPayload,
    dependencies: &[DependencySpec],
) -> bool {
    if validate_project_inspection_authority_payload(payload).is_err() {
        return false;
    }
    let catalog = payload
        .registry_sources
        .iter()
        .map(|source| &source.package)
        .collect::<Vec<_>>();
    let mut aliases = BTreeSet::new();
    dependencies
        .iter()
        .filter(|dependency| matches!(dependency.source, DependencySource::Registry))
        .all(|dependency| {
            let alias = dependency.crate_name.replace('-', "_");
            if !aliases.insert(alias.clone()) {
                return false;
            }
            let Some(requirement) = dependency
                .version
                .as_deref()
                .and_then(|version| VersionReq::parse(version).ok())
            else {
                return false;
            };
            let package = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
            let requested_features = {
                let mut features = dependency.features.clone();
                features.sort();
                features.dedup();
                features
            };
            let matches = payload
                .registry_source_dependencies
                .iter()
                .chain(&payload.dev_registry_source_dependencies)
                .filter(|record| {
                    record.alias == alias
                        && record.package == package
                        && Version::parse(&record.version).is_ok_and(|version| requirement.matches(&version))
                        && record.requested_features == requested_features
                        && record.default_features == dependency.default_features
                        && catalog.iter().any(|source| {
                            source.package == record.package
                                && source.version == record.version
                                && source.source.registry == record.registry
                                && source.source.checksum == record.checksum
                        })
                })
                .collect::<Vec<_>>();
            matches.len() == 1 || matches.len() == 2 && matches[0] == matches[1]
        })
}

/// Check one generated native-test batch against the exact per-root dependency evidence in its singular authority.
///
/// The caller canonicalizes normal/dev duplicates first. This function then admits only a true subset: every named
/// alias must retain the same package/source/version/features/defaults, and path roots must still hash to the sealed
/// source identity. It never searches another Loaf when one root is absent or stale.
pub fn project_inspection_test_dependency_envelope_supports_dependencies(
    payload: &OvenProjectInspectionAuthorityPayload,
    dependencies: &[DependencySpec],
    provider_hooks: &dyn oven_store::OvenProviderHooks,
) -> Result<bool, OvenRustcError> {
    project_inspection_test_dependency_envelope_mismatch(payload, dependencies, provider_hooks)
        .map(|mismatch| mismatch.is_none())
}

/// Name the first requested test dependency the sealed test envelope does not support, and why, or `None` when the
/// envelope supports every one of them.
///
/// This is the refusal detail behind [`project_inspection_test_dependency_envelope_supports_dependencies`]: an alias
/// requested twice, an alias the envelope has no root for, a root of another source kind, or a root whose sealed
/// dependency digest differs from the requested dependency's current digest, with both digests.
pub fn project_inspection_test_dependency_envelope_mismatch(
    payload: &OvenProjectInspectionAuthorityPayload,
    dependencies: &[DependencySpec],
    provider_hooks: &dyn oven_store::OvenProviderHooks,
) -> Result<Option<String>, OvenRustcError> {
    validate_project_inspection_authority_payload(payload)?;
    let Some(envelope) = payload.test_dependency_envelope.as_ref() else {
        return Ok(Some("the authority has no test dependency envelope".to_string()));
    };
    let mut aliases = BTreeSet::new();
    for dependency in dependencies {
        let alias = dependency.crate_name.replace('-', "_");
        if !aliases.insert(alias.clone()) {
            return Ok(Some(format!("`{alias}` is requested more than once")));
        }
        let Some(root) = envelope.dependency_roots.get(&alias) else {
            return Ok(Some(format!("`{alias}` has no sealed root")));
        };
        let actual =
            oven_store::digest_dependency_specs(std::slice::from_ref(dependency), provider_hooks).map_err(|error| {
                OvenRustcError::InvalidInput {
                    field: "project inspection test dependency root",
                    message: error.to_string(),
                }
            })?;
        let (expected, source_matches) = match root {
            OvenProjectInspectionTestDependencyRoot::Registry { dependency_digest, .. } => (
                dependency_digest,
                matches!(dependency.source, DependencySource::Registry),
            ),
            OvenProjectInspectionTestDependencyRoot::Path { dependency_digest } => (
                dependency_digest,
                matches!(dependency.source, DependencySource::Path { .. }),
            ),
            OvenProjectInspectionTestDependencyRoot::Git { dependency_digest } => (
                dependency_digest,
                matches!(dependency.source, DependencySource::Git { .. }),
            ),
        };
        if !source_matches {
            return Ok(Some(format!(
                "`{alias}` is sealed from another source kind than the requested {}",
                dependency_source_kind(&dependency.source)
            )));
        }
        if actual != *expected {
            return Ok(Some(format!(
                "`{alias}` was sealed with dependency digest {expected}, but the requested dependency now digests to {actual}"
            )));
        }
    }
    Ok(None)
}

/// Name a dependency's source kind for a refusal detail.
pub(super) fn dependency_source_kind(source: &DependencySource) -> &'static str {
    match source {
        DependencySource::Registry => "registry dependency",
        DependencySource::Path { .. } => "path dependency",
        DependencySource::Git { .. } => "git dependency",
    }
}

/// Check whether one sealed registry-source catalog covers every selected direct registry dependency.
///
/// This is shared by compiler-shipped and project-owned Loaf selection. It intentionally validates sources only;
/// callers still need a separately selected receipt-bound plan before linking any artifact.
pub fn registry_source_dependencies_supported_by_catalog(
    sources: &[OvenRustcRegistrySourcePackage],
    dependencies: &[&DependencySpec],
) -> bool {
    dependencies.iter().all(|dependency| {
        let Some(requirement) = dependency
            .version
            .as_deref()
            .and_then(|version| VersionReq::parse(version).ok())
        else {
            return false;
        };
        let package = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
        let required_features = dependency.features.iter().map(String::as_str).collect::<BTreeSet<_>>();
        let matching = sources
            .iter()
            .filter(|source| {
                source.package == package
                    && Version::parse(&source.version).is_ok_and(|version| requirement.matches(&version))
                    && required_features
                        .iter()
                        .all(|feature| source.features.iter().any(|selected| selected == *feature))
            })
            .count();
        matching == 1
    })
}

/// Select a stored plan only when it matches the requested reusable build unit and return its closure with a live
/// lease.
pub(super) fn select_stored_direct_rustc_plan(
    store: &OvenStore,
    plan_identity: &str,
    receipt: &OvenReceipt,
) -> Result<(OvenRustcArtifactManifest, PathBuf, OvenStoreLease), OvenRustcError> {
    receipt
        .verify_identity()
        .map_err(|error| OvenRustcError::InvalidInput {
            field: "receipt",
            message: error.to_string(),
        })?;
    let (manifest, artifact_root, payload, lease) = store.select_payload_for_execution(plan_identity)?;
    if manifest.kind != OvenArtifactKind::DirectRustcPlan {
        return Err(OvenRustcError::InvalidStoredPlan {
            identity: manifest.identity,
            message: "selected artifact kind is not direct_rustc_plan".to_string(),
        });
    }
    if manifest.build_unit_identity != receipt.build_unit_identity || manifest.intent != receipt.intent {
        return Err(OvenRustcError::InvalidStoredPlan {
            identity: manifest.identity,
            message: "selected artifact is not authorized by the requested Oven build unit".to_string(),
        });
    }
    let artifacts = serde_json::from_slice::<OvenRustcArtifactManifest>(&payload).map_err(|error| {
        OvenRustcError::InvalidStoredPlan {
            identity: plan_identity.to_string(),
            message: format!("stored payload is not an Oven Rust artifact manifest: {error}"),
        }
    })?;
    Ok((artifacts, artifact_root, lease))
}

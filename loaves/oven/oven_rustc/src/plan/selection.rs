//! Lease exactly one receipt-compatible direct-Rustc closure from the store.
//!
//! Every selector here validates the stored payload against the receipt it was published under and keeps the
//! execution lease alive inside the returned plan, so policy pruning cannot reclaim a selected entry between
//! selection and the bake that consumes it. Selection never falls back to a compatibility match the receipt did not
//! name.

use std::collections::BTreeSet;

use crate::legacy_cargo::{OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION, OvenProjectExtensionPayload};
use crate::loaf::resolve_compiler_owned_loaf_by_identity;
use crate::rustc::{
    OvenRegistryLeafAuthority, OvenRustcArtifactManifest, OvenTrustedRustcArtifactRoot,
    project_inspection_constituent_matches_receipt, select_direct_rustc_plan_for_execution,
    validate_project_extension_payload_against_base,
};
use oven_store::closure_proof::OvenClosureProof;
use oven_store::store::{OvenArtifactKind, OvenStore, OvenStoreExecutionPayload};

use super::PackagedProviderCandidate;
use super::{
    OvenDirectRustcPlanSelection, OvenPackagedLibraryLoafEntry, OvenPlanError, OvenPlanResult,
    OvenProjectExtensionExecutionPlan, OvenStoredDirectRustcExecutionPlan, OvenStoredProjectExtensionExecutionPlan,
};
use oven_store::OvenReceipt;

/// Resolve a receipt-compatible direct-Rustc payload while retaining the execution lease acquired during matching.
///
/// A normal Oven consumer never converts an unleased manifest header into a later identity lookup: policy pruning may
/// legitimately reclaim that inactive entry between those steps. The store's matching selector verifies the payload
/// and acquires its active lease atomically, while integrity and authorization failures still fail closed.
pub fn select_receipt_direct_rustc_execution_plan(
    store: &OvenStore,
    receipt: &OvenReceipt,
) -> OvenPlanResult<Option<OvenStoredDirectRustcExecutionPlan>> {
    let Some(selected) = select_direct_rustc_plan_for_execution(store, receipt)? else {
        return Ok(None);
    };
    let (stored_manifest, artifact_root, payload, lease) = selected.into_parts();
    let plan_identity = &stored_manifest.identity;
    if stored_manifest.kind != OvenArtifactKind::DirectRustcPlan
        || stored_manifest.build_unit_identity != receipt.build_unit_identity
        || stored_manifest.intent != receipt.intent
    {
        return Err(OvenPlanError::selection(
            "selected Oven store entry is not the receipt-bound direct-Rustc plan".to_string(),
        ));
    }
    let artifacts = serde_json::from_slice::<OvenRustcArtifactManifest>(&payload).map_err(|error| {
        OvenPlanError::selection(format!(
            "selected Oven direct-Rustc plan has an invalid payload: {error}"
        ))
    })?;
    // The entry is content-addressed and leased, so its closure proof lives beside the store under its identity
    // (#1546): the first process walks every file, the rest read one small record.
    let artifact_plan = artifacts.materialize_proven_store(
        &artifact_root,
        &receipt.intent,
        plan_identity,
        &OvenClosureProof::path(store.root(), plan_identity),
    )?;
    Ok(Some(OvenStoredDirectRustcExecutionPlan {
        identity: plan_identity.clone(),
        artifacts,
        artifact_root,
        artifact_plan,
        _lease: lease,
    }))
}

/// Select one exact direct-plan entry transported by a public package.
///
/// Unlike ordinary receipt selection, package composition carries an immutable entry identity. Requiring that identity
/// prevents a compatible-but-different direct plan from replacing the provider's sealed Rust ABI closure after its
/// package crossed the explicit bake boundary.
pub fn select_packaged_direct_rustc_execution_plan(
    store: &OvenStore,
    receipt: &OvenReceipt,
    required_identity: &str,
) -> OvenPlanResult<Option<OvenStoredDirectRustcExecutionPlan>> {
    receipt
        .verify_identity()
        .map_err(|error| OvenPlanError::selection(format!("invalid packaged Oven receipt: {error}")))?;
    let mut selected = store
        .select_payloads_matching_for_execution(|manifest| {
            manifest.identity == required_identity
                && manifest.kind == OvenArtifactKind::DirectRustcPlan
                && manifest.receipt_identity == receipt.identity
                && manifest.build_unit_identity == receipt.build_unit_identity
                && manifest.intent == receipt.intent
        })
        .map_err(|error| {
            OvenPlanError::selection(format!("failed to select packaged Oven direct-plan Loaf: {error}"))
        })?;
    if selected.len() > 1 {
        return Err(OvenPlanError::selection(format!(
            "Oven Alpha found multiple imported direct-plan Loafs for sealed package entry `{required_identity}`"
        )));
    }
    let Some(selected) = selected.pop() else {
        return Ok(None);
    };
    let (manifest, artifact_root, payload, lease) = selected.into_parts();
    let artifacts = serde_json::from_slice::<OvenRustcArtifactManifest>(&payload).map_err(|error| {
        OvenPlanError::selection(format!(
            "selected Oven direct-plan Loaf `{}` has an invalid payload: {error}",
            manifest.identity
        ))
    })?;
    let artifact_plan = artifacts.materialize_trusted_store(&artifact_root, &receipt.intent)?;
    Ok(Some(OvenStoredDirectRustcExecutionPlan {
        identity: manifest.identity,
        artifacts,
        artifact_root,
        artifact_plan,
        _lease: lease,
    }))
}

/// Drop project extensions whose base Loaf the active toolchain does not ship, when at least one candidate's base
/// is available.
///
/// When no candidate's base is available nothing is dropped: the caller then reports the ambiguity or the missing
/// base exactly as before, rather than silently selecting nothing.
fn retain_project_extensions_with_available_base<T>(
    selected: &mut Vec<(T, OvenProjectExtensionPayload)>,
    available_bases: &BTreeSet<String>,
) {
    if selected
        .iter()
        .any(|(_, payload)| available_bases.contains(&payload.base_loaf_identity))
    {
        selected.retain(|(_, payload)| available_bases.contains(&payload.base_loaf_identity));
    }
}

/// Select one receipt-bound project extension and reconstitute its exact base-plus-extension execution set.
///
/// The extension payload names the content address of the standard-library Loaf it was partitioned against.  A
/// compatible substitute is deliberately not accepted: Rust metadata and a `-L` path are paired artifacts, so a
/// newer Loaf with matching crate names could still be semantically different.  Both roots remain leased/locked in
/// the returned selection and are composed before any direct-Rustc command starts.
pub fn select_receipt_project_extension_execution_plan(
    store: &OvenStore,
    receipt: &OvenReceipt,
    required_identity: Option<&str>,
) -> OvenPlanResult<Option<OvenProjectExtensionExecutionPlan>> {
    let selected = store
        .select_payloads_matching_for_execution(|manifest| {
            manifest.kind == OvenArtifactKind::ProjectPayload
                && manifest.receipt_identity == receipt.identity
                && manifest.build_unit_identity == receipt.build_unit_identity
                && manifest.intent == receipt.intent
                && required_identity.is_none_or(|identity| manifest.identity == identity)
        })
        .map_err(|error| OvenPlanError::selection(error.to_string()))?;
    let mut selected = selected
        .into_iter()
        .filter_map(|candidate| {
            let payload = serde_json::from_slice::<OvenProjectExtensionPayload>(&candidate.payload).map_err(|error| {
                OvenPlanError::selection(format!(
                    "selected Oven project extension Loaf `{}` has an invalid payload: {error}",
                    candidate.manifest.identity
                ))
            });
            match payload {
                Ok(payload) if payload.schema_version == OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION => {
                    Some(Ok((candidate, payload)))
                }
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            }
        })
        .collect::<OvenPlanResult<Vec<_>>>()?;
    if selected.is_empty() {
        tracing::debug!(
            "no stored project extension matches receipt {} (build unit {})",
            receipt.identity,
            receipt.build_unit_identity
        );
        return Ok(None);
    }
    if selected.len() != 1 {
        // A receipt does not name the standard-library Loaf family it was baked against, so a store that retains
        // extensions from two installed families (a toolchain switch, #1444) offers two distinct candidates for one
        // receipt. Only the one whose base the active toolchain still ships can execute; the other is stale, not a
        // rival.
        let base_identities = selected
            .iter()
            .map(|(_, payload)| payload.base_loaf_identity.clone())
            .collect::<BTreeSet<_>>();
        let mut available_bases = BTreeSet::new();
        for base in base_identities {
            if resolve_compiler_owned_loaf_by_identity(receipt, &base)
                .map_err(|error| OvenPlanError::selection(error.to_string()))?
                .is_some()
            {
                available_bases.insert(base);
            }
        }
        retain_project_extensions_with_available_base(&mut selected, &available_bases);
    }
    if selected.len() != 1 {
        let identities = selected
            .iter()
            .map(|(candidate, _)| candidate.manifest.identity.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let first_payload = &selected[0].1;
        if selected.iter().all(|(_, payload)| payload == first_payload) {
            // A project extension contains only the receipt-bound dependency delta, never the caller's generated root.
            // Independent projects can therefore publish byte-identical extensions concurrently. They are
            // interchangeable; choose the stable content address rather than rejecting a correct shared closure as
            // ambiguous.
            selected.sort_by(|(left, _), (right, _)| left.manifest.identity.cmp(&right.manifest.identity));
        } else {
            return Err(OvenPlanError::selection(format!(
                "multiple distinct receipt-compatible Oven project extension Loafs are available: {identities}"
            )));
        }
    }
    let (selected, extension_payload) = selected.remove(0);
    project_extension_execution_plan_from_selected(selected, extension_payload, receipt).map(Some)
}

/// Reconstitute an exact base-plus-extension plan from one already leased store constituent.
///
/// Project inspection authorities batch-select every named constituent once. Native test setup passes that exact
/// leased value here instead of scanning the store again or accepting another compatibility match.
pub fn project_extension_execution_plan_from_selected(
    selected: OvenStoreExecutionPayload,
    extension_payload: OvenProjectExtensionPayload,
    receipt: &OvenReceipt,
) -> OvenPlanResult<OvenProjectExtensionExecutionPlan> {
    let (manifest, artifact_root, _payload, lease) = selected.into_parts();
    let base = resolve_compiler_owned_loaf_by_identity(receipt, &extension_payload.base_loaf_identity)
        .map_err(|error| OvenPlanError::selection(error.to_string()))?
        .ok_or_else(|| {
            OvenPlanError::selection(format!(
                "selected Oven project extension Loaf requires base `{}`, but that exact installed standard-library Loaf is unavailable; rebake the project for this Incan release",
                extension_payload.base_loaf_identity
            ))
        })?;
    let partition = validate_project_extension_payload_against_base(
        &extension_payload,
        &base.loaf_identity,
        &base.loaf_build_unit_identity,
        &base.artifacts,
    )?;
    let base_fragment = extension_payload
        .complete_plan
        .artifact_fragment(&partition.base_paths)?;
    let extension_fragment = extension_payload
        .complete_plan
        .artifact_fragment(&partition.extension_paths)?;
    let base_artifacts = base_fragment.composition_artifacts()?;
    let extension_artifacts = extension_fragment.composition_artifacts()?;
    let base_inventory = base.artifacts.composition_artifacts()?;
    let roots = [
        OvenTrustedRustcArtifactRoot {
            artifact_root: &base.artifact_root,
            dependency_search_paths: &base_fragment.dependency_search_paths,
            native_search_paths: &base_fragment.native_search_paths,
            supporting_artifacts: &base_artifacts,
            root_inventory: Some(&base_inventory),
        },
        OvenTrustedRustcArtifactRoot {
            artifact_root: &artifact_root,
            dependency_search_paths: &extension_fragment.dependency_search_paths,
            native_search_paths: &extension_fragment.native_search_paths,
            supporting_artifacts: &extension_artifacts,
            root_inventory: Some(&extension_artifacts),
        },
    ];
    let artifact_plan = extension_payload
        .complete_plan
        .materialize_trusted_store_composed(&roots, &receipt.intent)?;
    let registry_leaf_entries = extension_payload
        .complete_plan
        .registry_leaves
        .iter()
        .map(|leaf| {
            let artifact_root = if partition.base_paths.contains(&leaf.artifact.relative_path) {
                base.artifact_root.clone()
            } else if partition.extension_paths.contains(&leaf.artifact.relative_path) {
                artifact_root.clone()
            } else {
                return Err(OvenPlanError::selection(format!(
                    "selected Oven project extension Loaf has a registry leaf outside its declared base and extension fragments: {}",
                    leaf.artifact.relative_path
                )));
            };
            Ok((artifact_root, leaf.clone()))
        })
        .collect::<OvenPlanResult<Vec<_>>>()?;
    let registry_leaf_authority = OvenRegistryLeafAuthority::from_composed_plan(registry_leaf_entries, &artifact_plan);
    let vocab_paths = extension_payload
        .complete_plan
        .vocab_auxiliary_targets
        .iter()
        .flat_map(|target| target.externs.iter().map(|artifact| artifact.relative_path.as_str()))
        .collect::<BTreeSet<_>>();
    let vocab_artifact_root =
        if vocab_paths.is_empty() || vocab_paths.iter().all(|path| partition.base_paths.contains(*path)) {
            Some(base.artifact_root.clone())
        } else if vocab_paths.iter().all(|path| partition.extension_paths.contains(*path)) {
            Some(artifact_root.clone())
        } else {
            None
        };
    Ok(OvenProjectExtensionExecutionPlan {
        base,
        extension: OvenStoredProjectExtensionExecutionPlan {
            identity: manifest.identity,
            artifact_root,
            receipt: receipt.clone(),
            _lease: lease,
        },
        artifacts: extension_payload.complete_plan.clone(),
        artifact_plan,
        registry_leaf_authority,
        vocab_artifact_root,
        source_payload: extension_payload,
    })
}

/// Build the role-bearing test dependency plan from one exact constituent selected with its authority.
pub fn project_test_dependency_plan_from_constituent(
    selected: OvenStoreExecutionPayload,
    receipt: &OvenReceipt,
) -> OvenPlanResult<OvenDirectRustcPlanSelection> {
    if !project_inspection_constituent_matches_receipt(&selected.manifest, selected.manifest.kind, receipt) {
        return Err(OvenPlanError::selection(
            "project inspection test dependency constituent changed kind, receipt, build unit, or intent",
        ));
    }
    match selected.manifest.kind {
        OvenArtifactKind::DirectRustcPlan => {
            let (manifest, artifact_root, payload, lease) = selected.into_parts();
            let artifacts = serde_json::from_slice::<OvenRustcArtifactManifest>(&payload).map_err(|error| {
                OvenPlanError::selection(format!(
                    "project inspection test dependency constituent `{}` has an invalid direct-plan payload: {error}",
                    manifest.identity
                ))
            })?;
            let artifact_plan = artifacts.materialize_trusted_store(&artifact_root, &receipt.intent)?;
            Ok(OvenDirectRustcPlanSelection::Stored(Box::new(
                OvenStoredDirectRustcExecutionPlan {
                    identity: manifest.identity,
                    artifacts,
                    artifact_root,
                    artifact_plan,
                    _lease: lease,
                },
            )))
        }
        OvenArtifactKind::ProjectPayload => {
            let payload = serde_json::from_slice::<OvenProjectExtensionPayload>(&selected.payload).map_err(|error| {
                OvenPlanError::selection(format!(
                    "project inspection test dependency constituent `{}` has an invalid project-extension payload: {error}",
                    selected.manifest.identity
                ))
            })?;
            if payload.schema_version != OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION {
                return Err(OvenPlanError::selection(
                    "project inspection test dependency constituent uses an incompatible project-extension schema",
                ));
            }
            project_extension_execution_plan_from_selected(selected, payload, receipt)
                .map(|plan| OvenDirectRustcPlanSelection::ProjectExtension(Box::new(plan)))
        }
        _ => Err(OvenPlanError::selection(
            "project inspection test dependency constituent changed kind, receipt, build unit, or intent",
        )),
    }
}

/// Select an imported public-provider Loaf as the consumer's complete Rust ABI foundation.
///
/// A provider such as IncQL was compiled against its own sealed `incan_stdlib`, DataFusion, and transitive Rust
/// artifacts. Attaching only its top-level rlib to an unrelated consumer plan would permit Rust to discover two ABI
/// closures. This selector instead lets the consumer compile against the exact package closures after the explicit
/// consumer bake imported them into the consumer's bounded store. It is deliberately a closure compositor rather
/// than a first-provider shortcut: independent packages may contribute one ABI-compatible collection of Loafs.
/// Already selected package inputs shared by requirement inspection and normal composition.
#[derive(Default)]
pub struct SelectedPackagedProviderPlans {
    /// Leased compiler-base extensions, each with the dependency key and store entry it was selected for.
    pub extensions: Vec<(String, OvenPackagedLibraryLoafEntry, OvenProjectExtensionExecutionPlan)>,
    /// Leased self-contained direct plans, each with the dependency key and store entry it was selected for.
    pub direct: Vec<(String, OvenPackagedLibraryLoafEntry, OvenStoredDirectRustcExecutionPlan)>,
}

/// Select each checked package entry once and retain its execution lease through preparation.
pub fn select_packaged_provider_plans(
    store: &OvenStore,
    candidates: &[PackagedProviderCandidate<'_>],
) -> OvenPlanResult<SelectedPackagedProviderPlans> {
    // A provider that uses only the compiler-shipped base can safely use the consumer's ordinary base selection.
    // There is no package delta to compose for it.
    if candidates.iter().all(|candidate| candidate.entries.is_empty()) {
        return Ok(SelectedPackagedProviderPlans::default());
    }
    let mut extension_selected = Vec::new();
    let mut direct_selected = Vec::new();
    for candidate in candidates {
        let dependency_key = candidate.dependency_key;
        for entry in candidate.entries {
            match entry.kind {
                OvenArtifactKind::ProjectPayload => {
                    let plan = select_receipt_project_extension_execution_plan(
                        store,
                        &entry.receipt,
                        Some(&entry.identity),
                    )?
                    .ok_or_else(|| {
                        OvenPlanError::selection(format!(
                            "Oven Alpha has no imported package Loaf `{}` for pub::{dependency_key}; run `incan oven bake --project .` in this consumer to import the already baked provider closure",
                            entry.identity
                        ))
                    })?;
                    if let Some(expected_base) = entry.base_loaf_identity.as_deref()
                        && plan.base.loaf_identity != expected_base
                    {
                        return Err(OvenPlanError::selection(format!(
                            "Oven Alpha refuses pub::{dependency_key} package Loaf: selected base `{}` differs from sealed package base `{expected_base}`",
                            plan.base.loaf_identity
                        )));
                    }
                    extension_selected.push((dependency_key.to_string(), entry.clone(), plan));
                }
                OvenArtifactKind::DirectRustcPlan => {
                    if entry.base_loaf_identity.is_some() {
                        return Err(OvenPlanError::selection(format!(
                            "Oven Alpha refuses pub::{dependency_key} package Loaf `{}` because a self-contained direct plan cannot name a compiler base",
                            entry.identity
                        )));
                    }
                    let plan = select_packaged_direct_rustc_execution_plan(
                        store,
                        &entry.receipt,
                        &entry.identity,
                    )?
                    .ok_or_else(|| {
                        OvenPlanError::selection(format!(
                            "Oven Alpha has no imported package Loaf `{}` for pub::{dependency_key}; run `incan oven bake --project .` in this consumer to import the already baked provider closure",
                            entry.identity
                        ))
                    })?;
                    direct_selected.push((dependency_key.to_string(), entry.clone(), plan));
                }
                _ => {
                    return Err(OvenPlanError::selection(format!(
                        "Oven Alpha cannot compose pub::{dependency_key} package Loaf `{}` because its stored role is not a direct Rust closure",
                        entry.identity
                    )));
                }
            }
        }
    }
    Ok(SelectedPackagedProviderPlans {
        extensions: extension_selected,
        direct: direct_selected,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    /// Two extensions from different compiler families are not an ambiguity when only one family is still
    /// installed; when neither ships, both are kept so the caller reports the ambiguity rather than guessing.
    #[test]
    fn a_retained_extension_from_a_family_the_toolchain_no_longer_ships_is_not_a_rival_issue1444() {
        let plan = OvenRustcArtifactManifest {
            schema_version: crate::rustc::OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: oven_store::OvenBuildIntent {
                target: "fixture-target".to_string(),
                toolchain: "rustc fixture".to_string(),
                profile: "debug".to_string(),
                features: Vec::new(),
            },
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            entrypoint_dependency_search_paths: Default::default(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        };
        let extension = |base: &str| OvenProjectExtensionPayload {
            schema_version: OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION,
            base_loaf_identity: base.to_string(),
            base_build_unit_identity: "sha256:unit".to_string(),
            publisher_plan: plan.clone(),
            complete_plan: plan.clone(),
            registry_source_dependencies: Vec::new(),
            dev_registry_source_dependencies: Vec::new(),
            extension_paths: Vec::new(),
        };
        // Family A is still installed; family B was the previous toolchain's.
        let mut selected = vec![("a", extension("sha256:family-a")), ("b", extension("sha256:family-b"))];
        let available = BTreeSet::from(["sha256:family-a".to_string()]);
        retain_project_extensions_with_available_base(&mut selected, &available);
        assert_eq!(selected.iter().map(|(name, _)| *name).collect::<Vec<_>>(), vec!["a"]);

        // Neither base ships: nothing is dropped, so the caller still reports the ambiguity honestly.
        let mut none_available = vec![("a", extension("sha256:family-a")), ("b", extension("sha256:family-b"))];
        retain_project_extensions_with_available_base(&mut none_available, &BTreeSet::new());
        assert_eq!(none_available.len(), 2);
    }
}

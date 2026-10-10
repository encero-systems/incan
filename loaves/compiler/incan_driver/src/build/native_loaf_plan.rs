//! Direct native consumer plans selected from ordinary declaration-rooted Loaf records.
//!
//! Physical records carry no checked language or macro authority. This boundary projects their verified native
//! members and retains original execution owners without admitting a separate SDK or copying dependency bytes.

use std::collections::BTreeMap;
use std::path::Path;

use oven_rustc::native_loaf::NativeLoafClosure;
use oven_rustc::plan::OvenDirectRustcPlanSelection;
use oven_rustc::plan::selection::select_receipt_direct_rustc_execution_plan_with_native_owners_for_domain;
use oven_rustc::plan::shared::OvenSharedNativePlan;
use oven_rustc::rustc::{
    OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenRustcArtifactExtern, OvenRustcArtifactManifest,
    OvenRustcSourceSearchClosure, OvenRustcSupportingArtifact,
};
use oven_store::store::{OvenArtifactKind, OvenArtifactPublishRequest, OvenStore};
use oven_store::{OvenReceipt, receipt_with_build_unit_input};

use crate::error::{CliError, CliResult};

const DOMAIN: &str = "ordinary-native-consumer-plan";

/// Select a compiler-tooling plan from its current ordinary declaration and explicit producer inputs.
///
/// This adapter keeps the Incan bootstrap on the same current-input and rooted-admission boundary as native tests.
/// It returns measured native preparation facts alongside the existing plan, whose original execution owners stay
/// leased through compilation. Dependencies and the consumer plan use the same supplied Store; the output directory
/// carries only staging and mutable hints. The runtime engine remains separate supplemental source authority.
#[allow(clippy::too_many_arguments)]
pub fn select_declared_native_loaf_plan(
    store: &OvenStore,
    source_receipt: &OvenReceipt,
    declaration: &Path,
    rustc: &Path,
    graph: &Path,
    index: &Path,
    blobs: &Path,
    output: &Path,
) -> CliResult<(OvenReceipt, OvenDirectRustcPlanSelection, String)> {
    let declaration = declaration.canonicalize().map_err(failure)?;
    let owner = declaration
        .parent()
        .ok_or_else(|| CliError::failure("ordinary native declaration has no owner"))?;
    let manifest = oven_model::manifest::ProjectManifest::load(&declaration).map_err(failure)?;
    let dependencies = manifest.rust_dependency_values();
    let target = oven_rustc::rustc::rustc_host_target(rustc).map_err(failure)?;
    let prepared = oven_rustc::native_loaf::prepare_declared_native_loafs_in_store(
        &oven_rustc::native_loaf::NativeLoafConsumerRequest {
            graph,
            index,
            blobs,
            output,
            rustc,
            target: &target,
            profile: "debug",
            dependencies: &dependencies,
            declaration_owner: owner,
            domain: "target",
        },
        store,
    )
    .map_err(failure)?;
    let (receipt, plan) = select_native_loaf_plan(store, source_receipt, prepared.closure())?;
    let report = serde_json::to_string(prepared.report()).map_err(failure)?;
    Ok((receipt, plan, report))
}

/// Bind current declaration-selected physical roots into a consumer receipt and retain its exact native inputs.
///
/// The caller supplies a closure selected by current source, lock and intent authority. Root record identities seal
/// their complete forward physical edges. Only authored aliases become direct imports; transitive units contribute
/// search paths. An explicitly empty closure publishes an ordinary empty plan without a shared-owner wrapper.
pub fn select_native_loaf_plan(
    store: &OvenStore,
    source_receipt: &OvenReceipt,
    closure: &NativeLoafClosure,
) -> CliResult<(OvenReceipt, OvenDirectRustcPlanSelection)> {
    let roots_digest = super::native_runtime_inputs::ordinary_roots_digest(closure.roots())?;
    if source_receipt
        .sources
        .build_unit_inputs
        .get("ordinary-native-roots")
        .is_some_and(|original| original != &roots_digest)
    {
        return Err(CliError::failure(
            "ordinary native plan roots differ from the original runtime projection",
        ));
    }
    let receipt =
        receipt_with_build_unit_input(source_receipt, "ordinary-native-roots", roots_digest).map_err(failure)?;
    let owners = closure.shared_owners().map_err(failure)?;
    if let Some(plan) =
        select_receipt_direct_rustc_execution_plan_with_native_owners_for_domain(store, &receipt, &owners, DOMAIN)
            .map_err(failure)?
    {
        return Ok((receipt, OvenDirectRustcPlanSelection::Stored(Box::new(plan))));
    }
    let mut artifacts = empty_manifest(&receipt);
    let mut shared_roots = Vec::new();
    for (index, (identity, unit)) in closure.graph().units().iter().enumerate() {
        unit.verify().map_err(failure)?;
        let record = unit.record();
        if record.recipe.intent.toolchain != receipt.intent.toolchain
            || (record.source.domain == "target" && record.recipe.intent.target != receipt.intent.target)
            || record.recipe.intent.profile != receipt.intent.profile
        {
            return Err(CliError::failure(format!(
                "ordinary native Loaf `{}` differs from the consumer compiler, target or profile",
                record.source.loaf
            )));
        }
        let prefix = format!("units/{index}");
        artifacts.dependency_search_paths.push(prefix.clone());
        shared_roots.push(closure.shared_root(identity, &prefix).map_err(failure)?);
        for (alias, root_identity) in closure.roots() {
            if root_identity == identity {
                artifacts.externs.push(OvenRustcArtifactExtern {
                    crate_name: alias.clone(),
                    relative_path: format!("{prefix}/{}", record.native.relative_path),
                    digest: record.native.digest.clone(),
                });
            }
        }
        for file in unit.materialized_files().map_err(failure)? {
            if Path::new(&file.relative_path)
                .file_name()
                .is_some_and(|name| name == "Cargo.toml" || name == "Cargo.lock")
            {
                return Err(CliError::failure("ordinary native payload contains Cargo metadata"));
            }
            let relative_path = format!("{prefix}/{}", file.relative_path);
            if file.relative_path.ends_with(".a") {
                let parent = Path::new(&relative_path)
                    .parent()
                    .ok_or_else(|| CliError::failure("ordinary native archive has no parent"))?;
                artifacts
                    .native_search_paths
                    .push(parent.to_string_lossy().into_owned());
            }
            if !artifacts
                .externs
                .iter()
                .any(|external| external.relative_path == relative_path)
            {
                artifacts.supporting_artifacts.push(OvenRustcSupportingArtifact {
                    relative_path,
                    digest: file.digest.clone(),
                });
            }
        }
    }
    artifacts.native_search_paths.sort();
    artifacts.native_search_paths.dedup();
    seal_source_roles(&receipt, &mut artifacts);
    artifacts.validate_shape(&receipt.intent).map_err(failure)?;
    let payload = if shared_roots.is_empty() {
        serde_json::to_vec(&artifacts)
    } else {
        serde_json::to_vec(&OvenSharedNativePlan {
            artifacts,
            shared_native_roots: shared_roots,
        })
    }
    .map_err(failure)?;
    store
        .publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: DOMAIN.to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload,
            materialized_files: Vec::new(),
            materialized_directories: Vec::new(),
        })
        .map_err(failure)?;
    let plan =
        select_receipt_direct_rustc_execution_plan_with_native_owners_for_domain(store, &receipt, &owners, DOMAIN)
            .map_err(failure)?
            .ok_or_else(|| CliError::failure("published ordinary native consumer plan is unavailable"))?;
    Ok((receipt, OvenDirectRustcPlanSelection::Stored(Box::new(plan))))
}

/// Construct the existing direct-Rustc manifest without adding ambient dependency or source authority.
fn empty_manifest(receipt: &OvenReceipt) -> OvenRustcArtifactManifest {
    OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: receipt.intent.clone(),
        dependency_search_paths: Vec::new(),
        native_search_paths: Vec::new(),
        externs: Vec::new(),
        entrypoint_externs: BTreeMap::new(),
        entrypoint_dependency_search_paths: BTreeMap::new(),
        registry_leaves: Vec::new(),
        registry_sources: Vec::new(),
        compile_environment: BTreeMap::new(),
        vocab_auxiliary_targets: Vec::new(),
        supporting_artifacts: Vec::new(),
    }
}

/// Bind each declared source role to exact native members admitted for this consumer.
fn seal_source_roles(receipt: &OvenReceipt, artifacts: &mut OvenRustcArtifactManifest) {
    let mut members = artifacts
        .supporting_artifacts
        .iter()
        .map(|file| (file.relative_path.clone(), file.digest.clone()))
        .collect::<BTreeMap<_, _>>();
    for external in &artifacts.externs {
        members.insert(external.relative_path.clone(), external.digest.clone());
    }
    let closure = OvenRustcSourceSearchClosure::publisher_selected(artifacts.dependency_search_paths.clone(), &members);
    let names = artifacts
        .externs
        .iter()
        .map(|file| file.crate_name.clone())
        .collect::<Vec<_>>();
    for role in receipt.sources.supplemental_digests.keys() {
        artifacts.entrypoint_externs.insert(role.clone(), names.clone());
        artifacts
            .entrypoint_dependency_search_paths
            .insert(role.clone(), closure.clone());
    }
}

/// Preserve underlying authority and I/O diagnostics at the compiler's CLI boundary.
fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}

#[cfg(test)]
mod tests;

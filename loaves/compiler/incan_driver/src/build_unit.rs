//! The identity of one Oven build unit for a project: its native provider records, its dependency-spec digest,
//! and the promoted test dependencies the lock and the test runner agree on.
//!
//! Every command that bakes or resolves a project computes these the same way; they are here so `build`, `lock`
//! and `test` cannot drift.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::{CliError, CliResult};
use incan_provider::ProviderPlan;
use incan_provider::dependency_resolver::ResolvedDependencies;
use incan_provider::lock_semantics::{CheckedProviderSemanticIdentities, provider_semantic_identities};
use incan_provider::requirements::{ProjectRequirements, semantic_sdk_path_dependencies};
use oven_model::manifest::DependencySpec;
use oven_rustc::loaf::runtime_build_unit_inputs;
use oven_store::digest_dependency_specs;

/// Prepare an executable for Oven Alpha without launching Cargo, inspecting a Cargo target, or auto-publishing SDK
/// providers.
///
/// The generated Rust remains caller-owned so it can be inspected and regenerated normally. Reusable native inputs
/// are not derived from that directory: normal execution selects one matching direct-rustc provider/dependency plan
/// from the bounded Oven store. Incan-generated programs always link the standard runtime, so an empty plan is not a
/// meaningful normal-command fallback.
pub fn oven_build_unit_inputs(
    provider_plan: &ProviderPlan,
    requirements: &ProjectRequirements,
    resolved: &ResolvedDependencies,
) -> CliResult<BTreeMap<String, String>> {
    let provider_records = oven_native_provider_records(provider_plan, &semantic_sdk_path_dependencies(requirements))?;
    oven_build_unit_inputs_with_provider_records(requirements, resolved, provider_records)
}

/// Build unit inputs using provider records already checked by the current compilation session.
pub fn oven_build_unit_inputs_with_provider_identities(
    provider_plan: &ProviderPlan,
    requirements: &ProjectRequirements,
    resolved: &ResolvedDependencies,
    semantic_identities: &CheckedProviderSemanticIdentities,
) -> CliResult<BTreeMap<String, String>> {
    let provider_records = oven_native_provider_records_with_checked_identities(
        provider_plan,
        &semantic_sdk_path_dependencies(requirements),
        semantic_identities,
    )?;
    oven_build_unit_inputs_with_provider_records(requirements, resolved, provider_records)
}

/// Finish build-unit identity projection from provider records checked by either supported identity path.
fn oven_build_unit_inputs_with_provider_records(
    requirements: &ProjectRequirements,
    resolved: &ResolvedDependencies,
    provider_records: Vec<String>,
) -> CliResult<BTreeMap<String, String>> {
    let mut dependencies = resolved.dependencies.clone();
    dependencies.extend(resolved.dev_dependencies.clone());
    let dependency_digest = digest_dependency_specs(&dependencies, incan_oven_facet::provider_hooks().as_ref())
        .map_err(|error| CliError::failure(error.to_string()))?;
    runtime_build_unit_inputs(
        &incan_oven_facet::compiler_identity(),
        provider_records,
        &requirements.stdlib_facets,
        dependency_digest,
    )
    .map_err(CliError::failure)
}

/// Encode only the compiler-owned SDK capabilities a generated native crate can exercise.
///
/// The active provider catalog contains every installed SDK component so semantic analysis can resolve imports
/// deterministically. An enabled but unused component contributes neither a generated Rust extern nor a selected
/// implementation facet. Retaining its identity in a Loaf receipt would let an unrelated provider relocation
/// prevent a safe compiler-owned Loaf match. Direct-link roots remain records even without a module claim because a
/// checked project-library projection can require their rlib explicitly.
pub fn oven_native_provider_records(
    provider_plan: &ProviderPlan,
    sdk_path_dependencies: &[DependencySpec],
) -> CliResult<Vec<String>> {
    let semantic_identities =
        provider_semantic_identities(provider_plan, sdk_path_dependencies).map_err(CliError::failure)?;
    oven_native_provider_records_from_map(provider_plan, &semantic_identities)
}

/// Encode selected native provider records from identities checked by the current compilation session.
pub fn oven_native_provider_records_with_checked_identities(
    provider_plan: &ProviderPlan,
    sdk_path_dependencies: &[DependencySpec],
    semantic_identities: &CheckedProviderSemanticIdentities,
) -> CliResult<Vec<String>> {
    let semantic_identities = semantic_identities
        .for_context(provider_plan, sdk_path_dependencies)
        .map_err(CliError::failure)?;
    oven_native_provider_records_from_map(provider_plan, semantic_identities)
}

/// Encode provider records from a map already bound to this exact provider plan.
fn oven_native_provider_records_from_map(
    provider_plan: &ProviderPlan,
    semantic_identities: &BTreeMap<String, String>,
) -> CliResult<Vec<String>> {
    let direct_sdk_link_roots = provider_plan
        .sdk_link_roots()
        .into_iter()
        .map(|provider| provider.identity.stable_key())
        .collect::<BTreeSet<_>>();
    let mut provider_records = Vec::new();
    for provider in provider_plan.active_sdk_records() {
        let used_modules = provider_plan
            .used_modules(provider)
            .into_iter()
            .map(|module| module.join("."))
            .collect::<Vec<_>>();
        let facets = provider_plan
            .linked_implementation_facets(provider)
            .into_iter()
            .map(|facet| facet.id.as_str())
            .collect::<Vec<_>>();
        let direct_link = direct_sdk_link_roots.contains(&provider.identity.stable_key());
        if used_modules.is_empty() && facets.is_empty() && !direct_link {
            continue;
        }
        let raw_identity = provider.identity.stable_key();
        let identity = semantic_identities.get(&raw_identity).ok_or_else(|| {
            CliError::failure(format!(
                "native provider compatibility identity is missing for `{raw_identity}`"
            ))
        })?;
        provider_records.push(format!(
            "{identity}|{}|{}|{}",
            used_modules.join(","),
            facets.join(","),
            if direct_link { "link" } else { "none" }
        ));
    }
    Ok(provider_records)
}

/// Promote the complete canonical normal/dev surface into one generated-test dependency set.
///
/// Cargo unifies features and default-feature activation for duplicate package edges. The synthetic envelope mirrors
/// that behavior once at explicit bake time while preserving the dependency key, package rename, source, and version.
/// Every selected edge becomes non-optional because a generated native test may use any dependency reachable from the
/// checked project/test graph.
pub fn promoted_oven_test_dependencies(resolved: &ResolvedDependencies) -> CliResult<Vec<DependencySpec>> {
    let mut promoted = Vec::new();
    for candidate in resolved.dependencies.iter().chain(&resolved.dev_dependencies) {
        if let Some(existing) = promoted
            .iter_mut()
            .find(|dependency: &&mut DependencySpec| dependency.crate_name == candidate.crate_name)
        {
            if existing.version != candidate.version
                || existing.source != candidate.source
                || existing.package != candidate.package
            {
                return Err(CliError::failure(format!(
                    "test dependency `{}` conflicts between the canonical normal and dev surfaces",
                    candidate.crate_name
                )));
            }
            existing.features.extend(candidate.features.iter().cloned());
            existing.features.sort();
            existing.features.dedup();
            existing.default_features |= candidate.default_features;
            existing.optional = false;
            continue;
        }
        let mut candidate = candidate.clone().normalized();
        candidate.optional = false;
        promoted.push(candidate);
    }
    promoted.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    Ok(promoted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use incan_frontend::library_manifest::LibraryManifest;
    use incan_frontend::library_manifest_index::LibraryManifestIndex;
    use incan_provider::lock_semantics::ProviderSemanticIdentitySession;

    #[test]
    fn native_provider_records_refuse_a_bundle_from_changed_checked_manifest_context()
    -> Result<(), Box<dyn std::error::Error>> {
        let first_plan = ProviderPlan::for_in_memory_sdk_manifest(
            LibraryManifestIndex::default(),
            LibraryManifest::new("checked_provider", "1.0.0"),
        );
        let session = ProviderSemanticIdentitySession::default();
        let checked = session.identities(&first_plan, &[])?;

        let mut changed_manifest = LibraryManifest::new("checked_provider", "1.0.0");
        changed_manifest.contract_metadata.provider.semantic_source_digest = Some(format!("sha256:{}", "b".repeat(64)));
        let changed_plan = ProviderPlan::for_in_memory_sdk_manifest(LibraryManifestIndex::default(), changed_manifest);
        assert!(
            oven_native_provider_records_with_checked_identities(&changed_plan, &[], &checked).is_err(),
            "native provider records must reject identities retained from a changed checked manifest"
        );
        Ok(())
    }
}

//! Checked resolved-provider refresh for an explicit completed-output reuse attempt.

use std::path::Path;

use crate::error::{CliError, CliResult};
use crate::lock::workspace::{collect_project_lock_context, collect_workspace_lock_context};
use incan_provider::FeatureSelection;
use incan_provider::requirements::semantic_sdk_path_dependencies;
use oven_model::lock::{
    CargoFeatureSelection, IncanLock, LockedProvider, SemanticLockState, compute_resolved_fingerprint_with_sdk_paths,
};
use oven_model::manifest::ProjectManifest;
use oven_model::workspace::WorkspaceGraph;

/// Resolve a candidate only when custom-provider content digests are the sole meaningful lock change.
///
/// This miss-only probe uses ordinary lock collection, including source parsing and checked provider discovery,
/// without typechecking, code generation, native compilation, or SDK preparation. It neither publishes the lock
/// nor relaxes normal commands' locked/frozen policy. Package coordinates, features, provider participation,
/// namespace claims, registry records, and Oven configuration must match the current resolution exactly.
pub(crate) fn resolved_provider_refresh(
    manifest: &ProjectManifest,
    entry_file: &Path,
    package_features: &FeatureSelection,
    current: &IncanLock,
) -> CliResult<Option<IncanLock>> {
    if !current.cargo_features.cargo_features.is_empty()
        || current.cargo_features.cargo_no_default_features
        || current.cargo_features.cargo_all_features
    {
        return Ok(None);
    }
    let has_custom_provider = current
        .semantic
        .providers
        .iter()
        .chain(
            current
                .semantic
                .workspace_members
                .iter()
                .flat_map(|member| &member.providers),
        )
        .any(|provider| !provider.identity.starts_with("incan_stdlib_"));
    if !has_custom_provider {
        return Ok(None);
    }
    let workspace =
        WorkspaceGraph::discover(manifest.project_root()).map_err(|error| CliError::failure(error.to_string()))?;
    let cargo_features = CargoFeatureSelection::default();
    let context = if let Some(workspace) = &workspace {
        collect_workspace_lock_context(
            workspace,
            manifest.project_root(),
            Some(entry_file),
            &cargo_features,
            None,
            None,
        )?
    } else {
        let Some(context) = collect_project_lock_context(
            manifest,
            Some(entry_file),
            &cargo_features,
            package_features,
            None,
            None,
            None,
        )?
        else {
            return Ok(None);
        };
        context
    };
    let mut previous = canonical_semantic_selection(&current.semantic);
    let resolved = canonical_semantic_selection(&context.semantic);
    let mut changed = false;
    if !refresh_provider_digests(&mut previous.providers, &resolved.providers, &mut changed)
        || previous.workspace_members.len() != resolved.workspace_members.len()
    {
        return Ok(None);
    }
    for (previous, resolved) in previous.workspace_members.iter_mut().zip(&resolved.workspace_members) {
        if previous.member_root != resolved.member_root
            || !refresh_provider_digests(&mut previous.providers, &resolved.providers, &mut changed)
        {
            return Ok(None);
        }
    }
    if !changed || previous != resolved {
        return Ok(None);
    }
    let root = workspace.as_ref().map_or(manifest.project_root(), WorkspaceGraph::root);
    let sdk_paths = semantic_sdk_path_dependencies(&context.project_requirements);
    let mut candidate = current.clone();
    candidate.semantic = context.semantic;
    candidate.deps_fingerprint = compute_resolved_fingerprint_with_sdk_paths(
        &context.resolved.dependencies,
        &context.resolved.dev_dependencies,
        &cargo_features,
        Some(root),
        &candidate.semantic,
        &sdk_paths,
    );
    Ok(Some(candidate))
}

/// Match the existing source-authority projection's compiler-owned SDK exclusion for comparison only.
fn canonical_semantic_selection(semantic: &SemanticLockState) -> SemanticLockState {
    let mut semantic = semantic.clone();
    semantic.sdk = None;
    semantic
        .providers
        .retain(|provider| !provider.identity.starts_with("incan_stdlib_"));
    for member in &mut semantic.workspace_members {
        member.sdk = None;
        member
            .providers
            .retain(|provider| !provider.identity.starts_with("incan_stdlib_"));
    }
    semantic
}

/// Replace only well-formed content digests, retaining every other provider selection field for equality checking.
fn refresh_provider_digests(previous: &mut [LockedProvider], resolved: &[LockedProvider], changed: &mut bool) -> bool {
    if previous.len() != resolved.len() {
        return false;
    }
    for (previous, resolved) in previous.iter_mut().zip(resolved) {
        let Some(previous_selection) = provider_coordinate_and_features(&previous.identity) else {
            return false;
        };
        if Some(previous_selection) != provider_coordinate_and_features(&resolved.identity) {
            return false;
        }
        if previous.identity != resolved.identity {
            *changed = true;
            previous.identity.clone_from(&resolved.identity);
        }
    }
    true
}

/// Validate the provider identity's SHA-256 slot before comparing its exact coordinates and feature projection.
fn provider_coordinate_and_features(identity: &str) -> Option<(&str, &str)> {
    let (coordinate, content) = identity.split_once("#sha256:")?;
    let (digest, features) = content.split_once('[')?;
    let (name, version) = coordinate.rsplit_once('@')?;
    if name.is_empty()
        || version.is_empty()
        || digest.len() != 64
        || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        || !features.ends_with(']')
    {
        return None;
    }
    Some((coordinate, features))
}

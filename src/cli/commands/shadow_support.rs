//! CLI-owned source-session orchestration for the bounded native shadow comparison.
//!
//! This adapter materializes the compared source, collects its caller-owned graph once, selects the exact prepared
//! provider and Oven authority, and then invokes the backend comparison core. The backend receives finished
//! materialization facts and never rediscovers a project or depends on CLI session construction.

use std::path::Path;

use crate::backend::selection::digest_output;
use crate::backend::shadow::legacy_oven::LegacyOvenCapability;
use crate::backend::shadow::{
    ShadowComparison, ShadowComparisonProfile, ShadowLegacyMaterialization, ShadowUnavailable,
    compare_source_observable_with_materialization, unavailable_source_observable_comparison,
    validate_source_observable_profile,
};
use crate::cli::prelude::ParsedModule;
use crate::dependency_resolver::resolve_reachable_dependencies;
use crate::lockfile::CargoFeatureSelection;
use crate::oven::loaf::OVEN_LOAF_ENV;
use crate::provider::{FeatureSelection, ProviderPlan};

use super::common::{
    CompilationSession, build_source_map, collect_modules_detailed_with_session, collect_project_requirements,
    collect_rust_dependency_uses, extend_requirements_with_provider_plan, format_dependency_error,
    merge_project_requirement_dependencies,
};

/// Run one bounded comparison using provider authority selected from the exact profile source session.
///
/// This CLI-owned adapter materializes the profile source below `workspace`, resolves its provider projection, and
/// selects a staged Oven capability with matching immutable build inputs. The backend comparison core receives only
/// that finished materialization. Every pre-execution failure becomes an unavailable comparison rather than a silent
/// legacy fallback or missing result.
#[must_use]
pub fn compare_source_observable(
    profile: &ShadowComparisonProfile,
    capability: &LegacyOvenCapability,
    workspace: &Path,
) -> ShadowComparison {
    if let Err(unavailable) = validate_source_observable_profile(profile) {
        return unavailable_source_observable_comparison(profile, unavailable);
    }
    let materialization = match materialize_profile_source_session(profile, workspace) {
        Ok(materialization) => materialization,
        Err(unavailable) => return unavailable_source_observable_comparison(profile, unavailable),
    };
    let capability = match capability.select_for_materialization(&materialization) {
        Ok(capability) => capability,
        Err(unavailable) => return unavailable_source_observable_comparison(profile, unavailable),
    };
    compare_source_observable_with_materialization(profile, &materialization, &capability, workspace)
}

/// Resolve the compiler-owned provider closure for the exact source text the comparison will observe.
///
/// The public comparison profile intentionally stores source text rather than a project path. Materializing that text
/// at a private, deterministic path below the caller-owned workspace gives the CLI session resolver the same source
/// bytes while keeping its provider plan, build-input identity, and receipt selection inside this adapter.
fn materialize_profile_source_session(
    profile: &ShadowComparisonProfile,
    workspace: &Path,
) -> Result<ShadowLegacyMaterialization, ShadowUnavailable> {
    let source_directory = workspace.join("shadow-source-session");
    std::fs::create_dir_all(&source_directory).map_err(|error| {
        ShadowUnavailable::new(format!(
            "the legacy comparison could not create its source-session directory {}: {error}",
            source_directory.display()
        ))
    })?;
    let source_path = source_directory.join("profile.incn");
    std::fs::write(&source_path, profile.source()).map_err(|error| {
        ShadowUnavailable::new(format!(
            "the legacy comparison could not materialize its exact source session at {}: {error}",
            source_path.display()
        ))
    })?;
    prepare_shadow_legacy_materialization(&source_path, &FeatureSelection::default(), None)
}

/// Prepare the one source-owned provider projection required for a native shadow comparison.
///
/// This does not publish a provider, bake an Oven plan, or inspect Rust externs. Missing staged inventory, source
/// collection failure, or provider-plan resolution is an explicit [`ShadowUnavailable`] so the caller cannot fall
/// back to a bare legacy emission.
pub(crate) fn prepare_shadow_legacy_materialization(
    entry_path: &Path,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> Result<ShadowLegacyMaterialization, ShadowUnavailable> {
    if std::env::var_os(OVEN_LOAF_ENV).is_some_and(|value| value == "1") {
        return Err(ShadowUnavailable::new(
            "the legacy comparison provider context refuses explicit Oven Loaf publication mode; native shadow \
             comparison only consumes an already staged immutable capability",
        ));
    }
    let session =
        CompilationSession::discover_for_oven(entry_path, package_features, sdk_profile_override).map_err(|error| {
            ShadowUnavailable::new(format!(
                "the legacy comparison provider context is unavailable for {}: {error}",
                entry_path.display()
            ))
        })?;
    let modules = collect_modules_detailed_with_session(entry_path.to_path_buf(), &session).map_err(|failure| {
        ShadowUnavailable::new(format!(
            "the legacy comparison provider context could not collect {}: {}",
            entry_path.display(),
            failure.render_human()
        ))
    })?;
    let canonical_entry = entry_path.canonicalize().unwrap_or_else(|_| entry_path.to_path_buf());
    let entry_module = modules
        .iter()
        .find(|module| {
            module
                .file_path
                .canonicalize()
                .unwrap_or_else(|_| module.file_path.clone())
                == canonical_entry
        })
        .ok_or_else(|| {
            ShadowUnavailable::new(format!(
                "the legacy comparison provider context did not collect its entry source {}",
                entry_path.display()
            ))
        })?;
    let entry_source_identity = digest_output(&[entry_module.source.as_str()]);
    let provider_plan = session.provider_plan_for_modules(&modules).map_err(|error| {
        ShadowUnavailable::new(format!(
            "the legacy comparison provider context could not resolve {}: {error}",
            entry_path.display()
        ))
    })?;
    require_materializable_provider_plan(&provider_plan, entry_path)?;
    let oven_build_unit_inputs = canonical_oven_build_unit_inputs(&session, &modules, &provider_plan)?;
    Ok(ShadowLegacyMaterialization::from_provider_plan(
        provider_plan,
        oven_build_unit_inputs,
        entry_source_identity,
    ))
}

/// Derive the native closure identity inputs from the same source-session path as an ordinary Oven build.
///
/// This is read-only resolution: no provider is published, no lock is written, and no Oven plan is baked. The
/// returned map is later compared byte-for-byte with the adopted native receipt before either shadow materialization
/// or execution can begin.
fn canonical_oven_build_unit_inputs(
    session: &CompilationSession,
    modules: &[ParsedModule],
    provider_plan: &ProviderPlan,
) -> Result<std::collections::BTreeMap<String, String>, ShadowUnavailable> {
    let mut requirements = collect_project_requirements(modules, &session.library_manifest_index).map_err(|error| {
        ShadowUnavailable::new(format!(
            "the legacy comparison provider context could not collect native requirements: {error}"
        ))
    })?;
    extend_requirements_with_provider_plan(&mut requirements, provider_plan).map_err(|error| {
        ShadowUnavailable::new(format!(
            "the legacy comparison provider context could not extend native requirements: {error}"
        ))
    })?;
    let mut inline_imports = modules
        .iter()
        .flat_map(|module| collect_rust_dependency_uses(module, false))
        .collect::<Vec<_>>();
    inline_imports.retain(|import| import.crate_name != "incan_stdlib" && import.crate_name != "std");
    let mut resolved = resolve_reachable_dependencies(
        session.manifest.as_ref(),
        &inline_imports,
        true,
        &CargoFeatureSelection::default(),
    )
    .map_err(|errors| {
        let source_map = build_source_map(modules);
        let rendered = errors
            .iter()
            .map(|error| format_dependency_error(error, &source_map))
            .collect::<String>();
        ShadowUnavailable::new(format!(
            "the legacy comparison provider context could not resolve native dependencies: {}",
            rendered.trim_end()
        ))
    })?;
    merge_project_requirement_dependencies(&mut resolved, &requirements).map_err(|error| {
        ShadowUnavailable::new(format!(
            "the legacy comparison provider context could not merge native requirements: {error}"
        ))
    })?;
    super::build::oven_build_unit_inputs(provider_plan, &requirements, &resolved).map_err(|error| {
        ShadowUnavailable::new(format!(
            "the legacy comparison provider context could not derive native build inputs: {error}"
        ))
    })
}

/// Refuse a plan that cannot materialize the compiler-owned compatibility facade.
///
/// A discover-only session can validly carry no SDK inventory, in which case its provider plan has no compiled
/// SDK root. Passing that empty projection to legacy codegen would recreate the former raw `crate::__incan_std`
/// failure. The provider plan remains the canonical source of this readiness fact; this adapter merely refuses to
/// treat absent provider artifacts as a materialization success.
fn require_materializable_provider_plan(
    provider_plan: &ProviderPlan,
    entry_path: &Path,
) -> Result<(), ShadowUnavailable> {
    provider_plan.validate_compilation_ready().map_err(|error| {
        ShadowUnavailable::new(format!(
            "the legacy comparison provider context is not compilation-ready for {}: {error}",
            entry_path.display()
        ))
    })?;
    if provider_plan.sdk_link_roots().is_empty() {
        return Err(ShadowUnavailable::new(format!(
            "the legacy comparison provider context has no compiled SDK link root for {}; set an existing \
             INCAN_SDK_INVENTORY or use an installed Oven toolchain before native comparison",
            entry_path.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{prepare_shadow_legacy_materialization, require_materializable_provider_plan};
    use crate::provider::{FeatureSelection, ProviderPlan};

    /// A missing caller-owned source is unavailable rather than an invitation to fabricate provider authority.
    #[test]
    fn missing_source_context_is_unavailable() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let missing = workspace.path().join("missing-shadow-source.incn");
        let unavailable = prepare_shadow_legacy_materialization(&missing, &FeatureSelection::default(), None)
            .err()
            .ok_or("missing source unexpectedly prepared a shadow provider context")?;
        assert!(unavailable.reason.contains("legacy comparison provider context"));
        Ok(())
    }

    /// Discover-only session output with no compiled SDK root is unavailable, never a bare-emission fallback.
    #[test]
    fn empty_provider_plan_cannot_prepare_legacy_materialization() -> Result<(), Box<dyn std::error::Error>> {
        let unavailable =
            require_materializable_provider_plan(&ProviderPlan::default(), std::path::Path::new("shadow-profile.incn"))
                .err()
                .ok_or("an empty provider plan must not materialize legacy generated Rust")?;
        assert!(unavailable.reason.contains("no compiled SDK link root"));
        Ok(())
    }
}

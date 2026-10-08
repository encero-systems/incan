//! Session-owned provider preparation for the bounded native shadow comparison.
//!
//! The backend receives a finished provider projection and never rediscovers a project. This adapter is deliberately
//! Oven-only: it discovers an already prepared inventory, collects the caller-owned source graph once, and returns
//! the exact final plan that legacy codegen and generated-project materialization share.

use std::path::Path;

use crate::backend::selection::digest_output;
use crate::backend::shadow::{ShadowLegacyMaterialization, ShadowUnavailable};
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

/// Prepare the one source-owned provider projection required for a native shadow comparison.
///
/// This does not publish a provider, bake an Oven plan, or inspect Rust externs. Missing staged inventory, source
/// collection failure, or provider-plan resolution is an explicit [`ShadowUnavailable`] so the caller cannot fall
/// back to a bare legacy emission.
pub fn prepare_shadow_legacy_materialization(
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

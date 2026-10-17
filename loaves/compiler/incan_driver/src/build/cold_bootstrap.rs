//! Explicit source-built stage-zero handoff to the authored compiler bootstrap (#1337, #1698).
//!
//! A Cargo-built compiler has no native receipt and cannot run ordinary compilation. This removable host producer
//! compiles exactly the first identity engine, policy engine and Incan bootstrap program against an admitted SDK.
//! The resulting compiler is built by the unchanged Incan program through the existing native owner. Nothing here
//! writes a receipt for the Cargo executable or makes missing ordinary compiler authority a fallback condition.

use std::path::Path;
use std::sync::Arc;

use crate::error::{CliError, CliResult};
#[cfg(feature = "rust_inspect")]
use incan_provider::dependency_resolver::ResolvedDependencies;
use incan_provider::dependency_resolver::resolve_reachable_dependencies;
use oven_model::lock::CargoFeatureSelection;
use oven_rustc::rustc::{
    OvenTrustedDirectRustcTargetRequest, bake_trusted_direct_rustc_run_in_store_with_generated_sources,
};
use oven_store::store::OvenStore;
use oven_store::{OvenGeneratedProjectRequest, receipt_generated_project};

use super::checked_emission::CheckedRustSource;
use super::source_authority::{
    baked_project_lock_dependencies_fingerprint, digest_baked_project_source_authority,
    digest_project_development_dependencies, ordinary_preparation_identity_digest,
};
use super::{
    NativeSdkCommandContext, OVEN_PROJECT_OUTPUT_ARTIFACT_PATH, OvenBakeProjectTarget, OvenProjectOutputBakeFile,
    OvenProjectOutputBakeRequest, OvenStoredProjectOutput,
};

/// The first runnable bootstrap program and original leases protecting the two engines it will select.
pub struct ColdCompilerBootstrap {
    program: OvenStoredProjectOutput,
    runtime: OvenStoredProjectOutput,
    policy: OvenStoredProjectOutput,
    /// Native inputs can remain runtime dependencies of the helper; retain the original SDK leases until it exits.
    _native: Arc<NativeSdkCommandContext>,
}

impl ColdCompilerBootstrap {
    /// Borrow the first engine publication for provenance without transferring its execution lease.
    pub fn runtime_output(&self) -> &OvenStoredProjectOutput {
        &self.runtime
    }

    /// Borrow the policy publication for provenance without transferring its execution lease.
    pub fn policy_output(&self) -> &OvenStoredProjectOutput {
        &self.policy
    }

    /// Return the verified program while this owner keeps all three completed publications alive.
    pub fn program_path(&self) -> &Path {
        &self.program.native_output
    }
}

/// Compile the three fixed bootstrap sources from a source-built compiler and an already prepared native SDK.
///
/// `output_root` is caller-owned temporary build storage, outside authored sources. The caller must retain this
/// result while executing its program, with `INCAN_COMPILER_STAGE_ZERO` naming this exact compiler executable and
/// unchanged preparation configuration. The Incan program then selects the genuine completed engine publications.
pub fn prepare_cold_compiler_bootstrap(
    checkout: &Path,
    output_root: &Path,
    rustc: &Path,
    store: &OvenStore,
) -> CliResult<ColdCompilerBootstrap> {
    let checkout = checkout.canonicalize().map_err(failure)?;
    let output_root = std::path::absolute(output_root).map_err(failure)?;
    std::fs::create_dir_all(&output_root).map_err(failure)?;
    let preparation = ordinary_preparation_identity_digest()?;
    let context = NativeSdkCommandContext::discover()?
        .ok_or_else(|| failure("cold compiler bootstrap requires an explicitly prepared native SDK"))?;
    let runtime = prepare_stage(
        &checkout,
        &output_root,
        rustc,
        store,
        &context,
        &preparation,
        BootstrapStage::Runtime,
        None,
    )?;
    let policy = prepare_stage(
        &checkout,
        &output_root,
        rustc,
        store,
        &context,
        &preparation,
        BootstrapStage::Policy,
        Some(&runtime.native_output),
    )?;
    let program = prepare_stage(
        &checkout,
        &output_root,
        rustc,
        store,
        &context,
        &preparation,
        BootstrapStage::Program,
        Some(&runtime.native_output),
    )?;
    Ok(ColdCompilerBootstrap {
        program,
        runtime,
        policy,
        _native: context,
    })
}

/// Fixed compiler-owned roots prevent the engine-free first-stage rule from becoming a general execution API.
#[derive(Clone, Copy)]
enum BootstrapStage {
    Runtime,
    Policy,
    Program,
}

impl BootstrapStage {
    /// Match the exact source/profile contract consumed by compiler-bootstrap's unchanged engine selector.
    fn coordinates(self) -> (&'static str, &'static str, &'static str, &'static str) {
        match self {
            Self::Runtime => (
                "workspaces/native-runtime-inputs",
                "src/main.incn",
                "debug",
                "native-runtime-inputs",
            ),
            Self::Policy => (
                "workspaces/oven",
                "src/plan_json_main.incn",
                "release",
                "oven-policy-engine",
            ),
            Self::Program => (
                "workspaces/compiler-bootstrap",
                "src/main.incn",
                "debug",
                "compiler-bootstrap",
            ),
        }
    }
}

/// Emit, select, execute and publish one fixed bootstrap stage using the existing source and native authorities.
#[allow(clippy::too_many_arguments)]
fn prepare_stage(
    checkout: &Path,
    output_root: &Path,
    rustc: &Path,
    store: &OvenStore,
    context: &NativeSdkCommandContext,
    preparation: &str,
    stage: BootstrapStage,
    engine: Option<&Path>,
) -> CliResult<OvenStoredProjectOutput> {
    let (relative_project, relative_entrypoint, profile, name) = stage.coordinates();
    let project = checkout.join(relative_project).canonicalize().map_err(failure)?;
    let entrypoint = project.join(relative_entrypoint).canonicalize().map_err(failure)?;
    if !project.starts_with(checkout) || !entrypoint.starts_with(&project) {
        return Err(failure(
            "cold bootstrap source escapes its fixed compiler-owned project",
        ));
    }
    let mut source_inputs = super::ProjectSourceAuthorityDigester::observing_authored_sources();
    let source_authority = source_inputs.digest(&project)?;
    source_inputs.ensure_original_sources_current()?;
    let dependencies_authority = digest_project_development_dependencies(&project)?;
    if preparation != ordinary_preparation_identity_digest()? {
        return Err(failure("cold bootstrap configuration changed after native admission"));
    }
    let target = oven_rustc::rustc::rustc_host_target(rustc).map_err(failure)?;
    let toolchain = oven_rustc::rustc::rustc_identity(rustc).map_err(failure)?;

    // ---- Cold emission: downstream receipts bind the actual first engine, not merely the Cargo emitter ----
    // Re-emit these three small authored programs on explicit cold invocation. Existing physical execution reuses
    // exact receipts; a second source-only hit could hide a changed first engine behind an unchanged publisher.
    let generated = output_root.join(name);
    std::fs::create_dir_all(generated.join("src")).map_err(failure)?;
    let checked = CheckedRustSource::discover(&entrypoint)?;
    source_inputs.ensure_original_sources_current()?;
    let manifest = checked
        .session
        .manifest
        .as_ref()
        .ok_or_else(|| failure("cold bootstrap source has no declared project"))?;
    if manifest.project_root().canonicalize().map_err(failure)? != project {
        return Err(failure("cold bootstrap source belongs to a different project"));
    }
    let declared = manifest
        .project
        .as_ref()
        .ok_or_else(|| failure("cold bootstrap declaration has no project identity"))?;
    let providers = checked.session.provider_plan_for_modules(&checked.modules)?;
    let compiled = incan_provider::compiled_sdk::CompiledSdkModules::from_provider_plan(&providers);
    let mut imports = checked
        .modules
        .iter()
        .filter(|module| !compiled.contains_emission_path(&module.path_segments))
        .flat_map(|module| crate::modules::collect_rust_dependency_uses(module, false))
        .collect::<Vec<_>>();
    imports
        .retain(|import| import.crate_name != "std" && !incan_lang::lang::stdlib::facets::is_facet(&import.crate_name));
    let resolved = resolve_reachable_dependencies(Some(manifest), &imports, false, &CargoFeatureSelection::default())
        .map_err(|errors| {
        let sources = crate::modules::build_source_map(&checked.modules);
        failure(
            errors
                .iter()
                .map(|error| crate::modules::format_dependency_error(error, &sources))
                .collect::<String>(),
        )
    })?;
    if matches!(stage, BootstrapStage::Runtime) {
        require_first_engine_roots(&manifest.rust_dependency_values(), &resolved.dependencies)?;
    }
    #[cfg(feature = "rust_inspect")]
    let inspection = prepare_inspection(&checked, &resolved, context, &generated, &target, &toolchain, profile)?;
    let backend_receipt = checked.generate_project(
        &generated,
        declared
            .name
            .as_deref()
            .ok_or_else(|| failure("bootstrap project has no name"))?,
        &resolved.dependencies,
        #[cfg(feature = "rust_inspect")]
        inspection
            .as_ref()
            .map(crate::lock::PreparedRustInspectWorkspace::manifest_dir),
    )?;
    source_inputs.ensure_original_sources_current()?;
    let source = generated.join("src/main.rs");
    let mut request = OvenGeneratedProjectRequest::new(
        &project,
        declared
            .name
            .as_deref()
            .ok_or_else(|| failure("bootstrap project has no name"))?,
        declared
            .version
            .as_deref()
            .ok_or_else(|| failure("bootstrap project has no version"))?,
        &target,
        &toolchain,
        profile,
        Vec::new(),
    )
    .with_generated_source("generated-root", &source)
    .with_generated_source_tree("generated-source-tree", generated.join("src"))
    .with_build_unit_input("bootstrap-source-authority", &source_authority);
    if let Some(engine) = engine {
        request = request.with_generated_source(super::native_runtime_inputs::NATIVE_RUNTIME_ENGINE_SOURCE, engine);
    }
    let source_receipt = receipt_generated_project(&request).map_err(failure)?;

    // ---- First engine has no projection policy to execute; every later stage uses the real Incan engine ----
    let (receipt, selection) = match engine {
        Some(engine) => super::native_sdk::select_prepared_native_sdk_plan_with_engine_and_context(
            store,
            &source_receipt,
            &resolved.dependencies,
            engine,
            context,
        )?,
        None if matches!(stage, BootstrapStage::Runtime) && resolved.dependencies.is_empty() => {
            let receipt = super::native_sdk::bind_native_catalog(&source_receipt, context.receipt_catalog()?)?;
            let selected =
                super::native_sdk_plan::select_native_sdk_plan_with_context(store, &receipt, &[], Some(context))?
                    .ok_or_else(|| failure("first engine has no admitted native plan"))?;
            (receipt, selected.plan_selection)
        }
        None => return Err(failure("later bootstrap stage has no first-engine authority")),
    };
    let plan = selection.source_artifact_plan("generated-root").map_err(failure)?;
    let native = generated.join(name);
    let result = bake_trusted_direct_rustc_run_in_store_with_generated_sources(
        &OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: selection.artifacts(),
            artifact_root: selection.output_guard_root(),
            artifact_plan: Some(&plan),
            rustc,
            source: &source,
            output: &native,
            crate_name: &name.replace('-', "_"),
            edition: "2024",
            source_evidence_key: "generated-root",
            features: &receipt.intent.features,
            prefer_dynamic: false,
        },
        store,
        &request,
    )
    .map_err(failure)?;
    if result.cargo_process_started {
        return Err(failure("cold native bootstrap unexpectedly started Cargo"));
    }

    // ---- Publish only actual completed bytes under the source and preparation generation just compiled ----
    context.verify()?;
    source_inputs.ensure_original_sources_current()?;
    if source_authority != digest_baked_project_source_authority(&project)?
        || dependencies_authority != digest_project_development_dependencies(&project)?
        || preparation != ordinary_preparation_identity_digest()?
    {
        return Err(failure("cold bootstrap authority changed during compilation"));
    }
    let files = vec![OvenProjectOutputBakeFile {
        source_path: native,
        caller_relative_path: format!("target/compiler-bootstrap/{name}"),
        output_relative_path: OVEN_PROJECT_OUTPUT_ARTIFACT_PATH.into(),
    }];
    let mut payload = super::publication::project_output_payload_for_bake(OvenProjectOutputBakeRequest {
        project_root: &project,
        entrypoint: &entrypoint,
        target: OvenBakeProjectTarget::Executable,
        receipt: &receipt,
        plan_identity: selection.report_identity(),
        profile,
        source_authority_digest: &source_authority,
        lock_dependencies_fingerprint: baked_project_lock_dependencies_fingerprint(&project)?,
        files: files.clone(),
        inspection_authority: None,
        required_project_loafs: Vec::new(),
        package_loaf_store_relative_path: None,
        backend_receipt,
        build_report: None,
    })?;
    payload.preparation_identity_digest = Some(preparation.to_owned());
    source_inputs.ensure_original_sources_current()?;
    let output = super::publication::publish_project_output_loaf(store, &receipt, &payload, &files)?;
    let receipt_path = super::source_authority::project_bake_receipt_path(
        &project,
        OvenBakeProjectTarget::Executable,
        &entrypoint,
        profile,
    )?;
    oven_store::write_receipt(&receipt, &receipt_path).map_err(failure)?;
    super::output_selection::remember_project_output(&receipt_path, &output.identity);
    Ok(output)
}

/// Future first-engine dependency additions must use a real earlier engine, never widen the zero-root exception.
fn require_first_engine_roots(
    declared: &[oven_model::manifest::DependencySpec],
    resolved: &[oven_model::manifest::DependencySpec],
) -> CliResult<()> {
    if !declared.is_empty() || !resolved.is_empty() {
        return Err(failure(
            "first native runtime engine must have no declared or inferred direct Rust dependency",
        ));
    }
    Ok(())
}

/// Prepare the established native inspection projection without entering ordinary runtime identity preparation.
#[cfg(feature = "rust_inspect")]
#[allow(clippy::too_many_arguments)]
fn prepare_inspection(
    checked: &CheckedRustSource,
    resolved: &ResolvedDependencies,
    context: &NativeSdkCommandContext,
    generated: &Path,
    target: &str,
    toolchain: &str,
    profile: &str,
) -> CliResult<Option<crate::lock::PreparedRustInspectWorkspace>> {
    let manifest = checked
        .session
        .manifest
        .as_ref()
        .ok_or_else(|| failure("bootstrap manifest missing"))?;
    let project = manifest
        .project
        .as_ref()
        .ok_or_else(|| failure("bootstrap identity missing"))?;
    let name = project
        .name
        .as_deref()
        .ok_or_else(|| failure("bootstrap name missing"))?;
    let version = project
        .version
        .as_deref()
        .ok_or_else(|| failure("bootstrap version missing"))?;
    let queries = crate::rust_inspect_workspace::collect_rust_inspect_query_paths(&checked.modules);
    let derives = crate::rust_inspect_workspace::collect_rust_inspect_derive_probe_paths(&checked.modules);
    let requirements = incan_provider::requirements::collect_project_requirements(
        &checked.modules,
        &checked.session.library_manifest_index,
    )?;
    let inputs = std::collections::BTreeMap::from([(
        "sdk-native-closure".into(),
        oven_store::digest_bytes(context.receipt_catalog()?),
    )]);
    crate::lock::rust_inspect::prepare_rust_inspect_workspace_with_native_sdk(
        crate::lock::RustInspectWorkspaceRequest {
            project_root: manifest.project_root(),
            project_name: name,
            cargo_package_name: name,
            rust_edition: Some("2024".into()),
            resolved,
            project_requirements: &requirements,
            lock_payload: None,
            cargo_lock_projection_root: None,
            clear_cargo_lock: false,
            cargo_policy_flags: Vec::new(),
            cargo_target_dir: &generated.join("inspection"),
            rust_inspect_query_paths: &queries,
            rust_derive_probe_paths: &derives,
            prepare_when_empty: false,
            direct_oven_inspection: true,
            force_direct_prewarm: false,
            oven_source_authority: Some(crate::lock::OvenRustInspectSourceAuthorityRequest {
                project_version: version,
                target,
                toolchain,
                profile,
                features: &[],
                build_unit_inputs: &inputs,
                registry_dependencies: &resolved.dependencies,
            }),
            prepared_project_source_authorities: None,
            explicit_oven_bake: false,
        },
        Some(context),
    )
}

/// Preserve original error context across the explicit bootstrap transport boundary.
fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::require_first_engine_roots;
    use oven_model::manifest::{DependencySource, DependencySpec};

    /// The first executable may use the admitted runtime but cannot silently acquire a declared or inferred root.
    #[test]
    fn cold_first_engine_refuses_declared_and_inferred_rust_roots() -> Result<(), Box<dyn std::error::Error>> {
        require_first_engine_roots(&[], &[])?;
        let dependency = DependencySpec {
            crate_name: "extra".into(),
            version: Some("1".into()),
            features: Vec::new(),
            default_features: true,
            optional: false,
            package: None,
            source: DependencySource::Registry,
        };
        let declared = require_first_engine_roots(std::slice::from_ref(&dependency), &[]);
        assert!(declared.is_err_and(|error| error.message.contains("no declared or inferred")));
        let inferred = require_first_engine_roots(&[], std::slice::from_ref(&dependency));
        assert!(inferred.is_err_and(|error| error.message.contains("no declared or inferred")));
        Ok(())
    }
}

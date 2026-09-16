//! Preparing a project through the generated-Cargo path: session, requirements, provider plan and generated
//! project. Retained for the tests that still exercise that path; the normal build goes through Oven.

#[cfg(test)]
use crate::backend::{IrCodegen, ProjectGenerator};
#[cfg(test)]
use crate::build::library_exports::collect_library_rust_abi_query_paths;
#[cfg(test)]
use crate::build::rust_extern::collect_rust_extern_contexts;
#[cfg(test)]
use crate::cargo_policy::{CargoPolicy, cargo_command_flags, enforce_project_toolchain_constraint};
#[cfg(test)]
use crate::error::{CliError, CliResult};
#[cfg(test)]
use crate::generated_cache::resolve_generated_cargo_target;
#[cfg(test)]
use crate::lock::LockResolutionRequest;
#[cfg(feature = "rust_inspect")]
#[cfg(test)]
use crate::lock::RustInspectWorkspaceRequest;
#[cfg(test)]
use crate::lock::resolution::resolve_lock_context;
#[cfg(feature = "rust_inspect")]
#[cfg(test)]
use crate::lock::rust_inspect::prepare_rust_inspect_workspace;
#[cfg(test)]
use crate::modules::{build_source_map, collect_rust_dependency_uses, format_dependency_error};
#[cfg(test)]
use crate::project::{resolve_project_root, validate_output_dir};
#[cfg(feature = "rust_inspect")]
#[cfg(test)]
use crate::rust_inspect_workspace::collect_rust_inspect_derive_probe_paths;
#[cfg(test)]
use incan_frontend::ParsedModule;
#[cfg(test)]
use incan_provider::FeatureSelection;
#[cfg(test)]
use incan_provider::compiled_sdk::CompiledSdkModules;
#[cfg(test)]
use incan_provider::dependency_resolver::resolve_reachable_dependencies;
#[cfg(test)]
use incan_provider::inventory::extend_requirements_with_provider_plan;
use incan_provider::lock_semantics::semantic_lock_state;
#[cfg(test)]
use incan_provider::requirements::{
    collect_project_requirements, merge_project_requirement_dependencies, semantic_sdk_path_dependencies,
};
#[cfg(test)]
use oven_model::lock::CargoFeatureSelection;
#[cfg(test)]
use oven_model::manifest::ProjectManifest;
#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::env;
#[cfg(test)]
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::Arc;

#[derive(Debug, Clone, Copy, Default)]
#[cfg(test)]
struct PrepareProjectOptions<'a> {
    output_dir: Option<&'a str>,
    project_name_override: Option<&'a str>,
    generated_cargo_target_dir: Option<&'a Path>,
    cargo_profile: &'a str,
    sdk_profile_override: Option<&'a str>,
}

/// Prepare an Incan project for building or running.
///
/// This function performs all the shared setup:
/// 1. Collect and parse modules
/// 2. Type check
/// 3. Configure codegen (serde, async, web, etc.)
/// 4. Add Rust crate dependencies
/// 5. Generate Rust project files
#[allow(clippy::too_many_arguments)] // This orchestration boundary mirrors independent CLI feature and Cargo axes.
#[cfg(test)]
fn prepare_project(
    file_path: &str,
    output_dir: Option<&str>,
    cargo_policy: &CargoPolicy,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
    cargo_profile: &str,
) -> CliResult<()> {
    prepare_project_with_options(
        file_path,
        PrepareProjectOptions {
            output_dir,
            project_name_override: None,
            generated_cargo_target_dir: None,
            cargo_profile,
            sdk_profile_override,
        },
        cargo_policy,
        package_features,
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    )
}

/// Prepare an executable project with optional internal identity overrides for callers that need bounded cache names.
#[cfg(test)]
fn prepare_project_with_options(
    file_path: &str,
    options: PrepareProjectOptions<'_>,
    cargo_policy: &CargoPolicy,
    package_features: &FeatureSelection,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
) -> CliResult<()> {
    let normalized_file_path = if Path::new(file_path).is_absolute() {
        PathBuf::from(file_path)
    } else {
        env::current_dir()
            .map_err(|e| CliError::failure(format!("failed to determine current directory: {e}")))?
            .join(file_path)
    };
    let path = normalized_file_path.as_path();
    let inferred_project_root = resolve_project_root(path);
    let compilation_session = crate::session::CompilationSession::discover_with_selections(
        path,
        package_features,
        options.sdk_profile_override,
    )?;
    let manifest = compilation_session.manifest.clone();
    if let Some(manifest) = manifest.as_ref() {
        enforce_project_toolchain_constraint(manifest)?;
    }

    let modules =
        crate::modules::collect_modules_detailed_with_session(normalized_file_path.clone(), &compilation_session)
            .map_err(|failure| CliError::failure(failure.render_human()))?;
    let rust_extern_contexts = collect_rust_extern_contexts(&modules);

    let Some(main_module) = modules.last() else {
        return Err(CliError::failure("No modules found"));
    };

    let dep_modules = &modules[..modules.len() - 1];
    let project_root = manifest
        .as_ref()
        .map(|manifest| manifest.project_root().to_path_buf())
        .unwrap_or(inferred_project_root);
    let package_feature_plan = compilation_session.package_feature_plan.clone();
    let library_manifest_index = compilation_session.library_manifest_index.clone();
    let mut project_requirements = collect_project_requirements(&modules, &library_manifest_index)?;
    let provider_plan = compilation_session.provider_plan_for_modules(&modules)?;
    let compiled_sdk_modules = CompiledSdkModules::from_provider_plan(&provider_plan);
    extend_requirements_with_provider_plan(&mut project_requirements, &provider_plan)?;
    let semantic_sdk_paths = semantic_sdk_path_dependencies(&project_requirements);
    let semantic = semantic_lock_state(
        &project_root,
        manifest.as_ref().and_then(ProjectManifest::interop_c),
        compilation_session.sdk_inventory.as_deref(),
        compilation_session.sdk_components.as_ref(),
        package_feature_plan.as_ref(),
        &provider_plan,
        &semantic_sdk_paths,
    )
    .map_err(CliError::failure)?;
    // Artifact-owned stdlib modules resolve from checked metadata and are supplied by its linked Rust crate. Keep
    // them out of local emission so consumers cannot materialize a second `__incan_std` tree.
    let emitted_dep_modules: Vec<&ParsedModule> = dep_modules
        .iter()
        .filter(|module| !compiled_sdk_modules.contains_emission_path(&module.path_segments))
        .collect();

    // Derive project name (manifest overrides filename)
    let project_name = options
        .project_name_override
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            manifest
                .as_ref()
                .and_then(|m| m.project.as_ref().and_then(|p| p.name.clone()))
                .unwrap_or_else(|| {
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("incan_project")
                        .to_string()
                })
        });

    let out_dir = options
        .output_dir
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("target/incan/{}", project_name));

    // Validate output directory path to prevent path traversal
    validate_output_dir(&out_dir)?;

    // ---- Setup codegen ----
    let mut codegen = IrCodegen::new();
    codegen.set_preserve_dependency_public_items(false);
    codegen.set_registry_package_identity(Some(project_name.clone()));
    codegen.set_root_source_module_name(path.file_stem().and_then(|stem| stem.to_str()).map(str::to_string));
    if let Some(m) = manifest.as_ref() {
        codegen.set_declared_crate_names(m.declared_rust_crate_names());
    }
    codegen.set_provider_plan(Arc::clone(&provider_plan));
    for module in dep_modules
        .iter()
        .filter(|module| compiled_sdk_modules.contains_emission_path(&module.path_segments))
    {
        codegen.add_dependency_symbol_module_with_path_segments(
            &module.name,
            &module.ast,
            module.path_segments.clone(),
        );
    }
    // Add user dependency modules
    for module in &emitted_dep_modules {
        codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
    }
    // ---- Setup project generator ----
    let mut generator = ProjectGenerator::new(&out_dir, project_name.as_str(), true);
    if let Some(project) = manifest.as_ref().and_then(|manifest| manifest.project.as_ref()) {
        generator.set_package_metadata(project.version.clone(), project.license.clone());
    }
    generator.set_provider_plan(&provider_plan);
    generator.set_sdk_path_dependencies(project_requirements.sdk_path_dependencies.clone());
    generator.set_cargo_target_dir_override(options.generated_cargo_target_dir.map(Path::to_path_buf));
    generator.set_stdlib_facets(project_requirements.stdlib_facets.clone());
    generator.set_include_dev_dependencies(false);
    generator.set_rust_edition(
        manifest
            .as_ref()
            .and_then(|m| m.build.as_ref().and_then(|b| b.rust_edition.clone())),
    );

    let mut inline_imports = collect_rust_dependency_uses(main_module, false);
    for module in &emitted_dep_modules {
        inline_imports.extend(collect_rust_dependency_uses(module, false));
    }
    // RFC 023: Stdlib modules should not have inline rust imports (they use rust.module() + @rust.extern instead),
    // so we skip collecting from them.

    let cargo_features = CargoFeatureSelection {
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    }
    .normalized();

    let mut resolved = match resolve_reachable_dependencies(manifest.as_ref(), &inline_imports, true, &cargo_features) {
        Ok(resolved) => resolved,
        Err(errors) => {
            let mut msg = String::new();
            let sources = build_source_map(&modules);
            for err in errors {
                msg.push_str(&format_dependency_error(&err, &sources));
            }
            return Err(CliError::failure(msg.trim_end()));
        }
    };
    merge_project_requirement_dependencies(&mut resolved, &project_requirements)?;
    #[cfg(feature = "rust_inspect")]
    let metadata_query_paths = collect_library_rust_abi_query_paths(&modules, &rust_extern_contexts);
    #[cfg(not(feature = "rust_inspect"))]
    let metadata_query_paths: Vec<String> = Vec::new();

    // Resolve lock payload before moving deps into generator (borrows resolved)
    let lock_resolution = resolve_lock_context(LockResolutionRequest {
        project_root: &project_root,
        project_name: project_name.as_str(),
        entry_file: Some(&normalized_file_path),
        manifest: manifest.as_ref(),
        resolved: &resolved,
        project_requirements: &project_requirements,
        cargo_features: &cargo_features,
        cargo_policy,
        semantic: Some(&semantic),
        package_features: Some(package_features),
        sdk_profile_override: options.sdk_profile_override,
    })?;
    let cargo_lock_inputs = lock_resolution.cargo_lock_authority.into_generator_inputs();
    let lock_payload = cargo_lock_inputs.payload;
    let cargo_lock_projection_root = cargo_lock_inputs.projection_root;
    let clear_cargo_lock = cargo_lock_inputs.clear_existing;
    let cargo_flags = cargo_command_flags(cargo_policy, &cargo_features);
    resolved = lock_resolution.resolved;
    project_requirements = lock_resolution.project_requirements;
    let cargo_package_name = lock_resolution.cargo_package_name;
    let managed_target = resolve_generated_cargo_target(
        options.generated_cargo_target_dir,
        &project_root,
        Path::new(&out_dir),
        &cargo_package_name,
        options.cargo_profile,
        lock_payload.as_deref(),
        &cargo_features,
        &cargo_flags,
    )
    .map_err(|error| CliError::failure(format!("failed to prepare generated Cargo cache: {error}")))?;
    let (managed_target_path, managed_target_lease, managed_target_identity) = managed_target.into_parts();
    #[cfg(feature = "rust_inspect")]
    let rust_inspect_target = resolve_generated_cargo_target(
        options.generated_cargo_target_dir,
        &project_root,
        &project_root,
        &cargo_package_name,
        "rust-inspect",
        lock_payload.as_deref(),
        &cargo_features,
        &cargo_flags,
    )
    .map_err(|error| CliError::failure(format!("failed to prepare rust-inspect Cargo cache: {error}")))?;
    #[cfg(feature = "rust_inspect")]
    let (rust_inspect_target_path, _rust_inspect_cache_lease, _rust_inspect_cache_identity) =
        rust_inspect_target.into_parts();
    generator.set_cargo_target_dir_override(Some(managed_target_path.clone()));
    generator.set_generated_cache_context(managed_target_lease, managed_target_identity);
    generator.set_package_name(Some(cargo_package_name.clone()));
    generator.set_stdlib_facets(project_requirements.stdlib_facets.clone());
    generator.set_include_dev_dependencies(lock_payload.is_some());
    #[cfg(feature = "rust_inspect")]
    let rust_inspect_manifest_dir = {
        let rust_inspect_manifest_dir = prepare_rust_inspect_workspace(RustInspectWorkspaceRequest {
            project_root: &project_root,
            project_name: project_name.as_str(),
            cargo_package_name: &cargo_package_name,
            rust_edition: manifest
                .as_ref()
                .and_then(|m| m.build.as_ref().and_then(|b| b.rust_edition.clone())),
            resolved: &resolved,
            project_requirements: &project_requirements,
            lock_payload: lock_payload.clone(),
            cargo_lock_projection_root: cargo_lock_projection_root.as_deref(),
            clear_cargo_lock,
            cargo_policy_flags: cargo_flags.clone(),
            cargo_target_dir: &rust_inspect_target_path,
            rust_inspect_query_paths: &metadata_query_paths,
            rust_derive_probe_paths: &collect_rust_inspect_derive_probe_paths(&modules),
            prepare_when_empty: true,
            direct_oven_inspection: false,
            force_direct_prewarm: false,
            oven_source_authority: None,
            prepared_project_source_authorities: None,
            explicit_oven_bake: false,
        })?
        .ok_or_else(|| CliError::failure("rust-inspect workspace preparation did not return a manifest directory"))?;
        codegen.set_rust_inspect_manifest_dir(rust_inspect_manifest_dir.manifest_dir().to_path_buf());
        Some(rust_inspect_manifest_dir)
    };

    // Type check all modules (dependencies + stdlib first), so diagnostics are associated with the correct file.
    //
    // This must run after rust-inspect preparation. Direct Rust calls expose their callable signatures through the
    // prepared metadata workspace; checking before that step degrades those calls to `Unknown` and breaks source-level
    // constructs such as `?` on Rust `Result<T, E>` returns.
    let compilation_analysis = compilation_session
        .analyze_modules(
            &modules,
            #[cfg(feature = "rust_inspect")]
            rust_inspect_manifest_dir
                .as_ref()
                .map(|workspace| workspace.manifest_dir()),
        )
        .map_err(|failure| CliError::failure(failure.render_human()))?;
    let main_type_info = compilation_analysis
        .type_info_for_path(&main_module.file_path)
        .cloned()
        .ok_or_else(|| {
            CliError::failure(format!(
                "missing session analysis for {}",
                main_module.file_path.display()
            ))
        })?;
    let mut dependency_type_info = HashMap::with_capacity(dep_modules.len());
    for module in dep_modules {
        let type_info = compilation_analysis
            .type_info_for_path(&module.file_path)
            .cloned()
            .ok_or_else(|| CliError::failure(format!("missing session analysis for {}", module.file_path.display())))?;
        dependency_type_info.insert(module.path_segments.clone(), type_info);
    }
    codegen.set_stdlib_cache(compilation_analysis.stdlib_cache().clone());
    codegen.set_prechecked_type_info(main_type_info, dependency_type_info);
    generator.set_cargo_lock_payload(lock_payload);
    generator.set_cargo_lock_projection_root(cargo_lock_projection_root);
    generator.set_clear_cargo_lock(clear_cargo_lock);

    generator.set_cargo_policy_flags(cargo_flags);

    generator.set_dependencies(resolved.dependencies);
    generator.set_dev_dependencies(resolved.dev_dependencies);

    // ---- Generate Rust project files ----
    let has_deps = !emitted_dep_modules.is_empty()
        || dep_modules
            .iter()
            .any(|module| compiled_sdk_modules.contains_emission_path(&module.path_segments));
    let _project_changed = if has_deps {
        let module_paths: Vec<Vec<String>> = emitted_dep_modules.iter().map(|m| m.path_segments.clone()).collect();
        let (main_code, rust_modules) = codegen
            .try_generate_multi_file_nested(&main_module.ast, &module_paths)
            .map_err(|e| CliError::failure(format!("Code generation error: {}", e)))?;

        generator
            .generate_nested(&main_code, &rust_modules)
            .map_err(|e| CliError::failure(format!("Error generating project: {}", e)))?
    } else {
        let rust_code = codegen
            .try_generate(&main_module.ast)
            .map_err(|e| CliError::failure(format!("Code generation error: {}", e)))?;
        generator
            .generate(&rust_code)
            .map_err(|e| CliError::failure(format!("Error generating project: {}", e)))?
    };

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::cargo_policy::CargoPolicy;
    use incan_provider::FeatureSelection;
    use oven_model::lock::{CargoFeatureSelection, IncanLock, compute_deps_fingerprint};

    #[test]
    fn run_entrypoint_omits_unused_manifest_rust_dependencies() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let scripts_dir = project_root.join("scripts");
        let declared_unused_rust_dependencies = ["itoa", "ryu"];
        std::fs::create_dir_all(&scripts_dir)?;
        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"unused_rust_dep_run_repro\"\nversion = \"0.1.0\"\n\n[rust-dependencies]\nitoa = \"1\"\nryu = \"1\"\n",
        )?;
        std::fs::write(
            scripts_dir.join("check.incn"),
            "def main() -> None:\n    println(\"ok\")\n",
        )?;

        let cargo_lock_payload =
            std::fs::read_to_string(oven_model::toolchain_layout::development_root().join("Cargo.lock"))?;
        let fingerprint = compute_deps_fingerprint(&[], &[], &CargoFeatureSelection::default(), Some(project_root));
        let incan_lock = IncanLock::new(
            incan_core::version::INCAN_VERSION,
            fingerprint,
            CargoFeatureSelection::default(),
            cargo_lock_payload,
        );
        incan_lock.write(&project_root.join("oven.lock"))?;

        let entry_path = scripts_dir.join("check.incn");
        let output_dir = project_root.join("target").join("incan").join("check");
        let entry_arg = entry_path
            .to_str()
            .ok_or("entry path should be valid utf-8 for prepare_project test")?;
        let output_arg = output_dir
            .to_str()
            .ok_or("output path should be valid utf-8 for prepare_project test")?;

        prepare_project(
            entry_arg,
            Some(output_arg),
            &CargoPolicy::default(),
            &FeatureSelection::default(),
            None,
            Vec::new(),
            false,
            false,
            "release",
        )?;

        let generated_manifest = std::fs::read_to_string(output_dir.join("Cargo.toml"))?;
        let manifest = toml::from_str::<toml::Value>(&generated_manifest)?;
        let dependency_table = manifest
            .get("dependencies")
            .and_then(toml::Value::as_table)
            .ok_or("generated manifest should contain a dependencies table")?;
        let emitted_unused_dependencies = declared_unused_rust_dependencies
            .iter()
            .filter(|dependency| dependency_table.contains_key(**dependency))
            .copied()
            .collect::<Vec<_>>();
        assert!(
            emitted_unused_dependencies.is_empty(),
            "unused package-level rust dependencies should not be emitted for a script run; emitted {emitted_unused_dependencies:?}:\n{generated_manifest}"
        );
        Ok(())
    }
}

//! Build and run pipeline for Incan projects.
//!
//! This module handles the full compilation flow: module collection, type checking, codegen configuration, dependency
//! resolution, project generation, and Cargo build/run.

use std::path::{Path, PathBuf};

use crate::backend::{IrCodegen, ProjectGenerator};
use crate::cli::{CliError, CliResult, ExitCode};
use crate::dependency_resolver::resolve_dependencies;
use crate::frontend::ast::Program;
use crate::frontend::{diagnostics, typechecker};
use crate::lockfile::CargoFeatureSelection;
use crate::manifest::ProjectManifest;

use super::common::{
    build_source_map, cargo_command_flags, collect_inline_rust_imports, collect_modules, format_dependency_error,
    resolve_project_root, validate_output_dir,
};
use super::lock::resolve_lock_payload;
use super::stdlib_loader;

// ============================================================================
// Project Preparation (shared between build and run)
// ============================================================================

/// A prepared Incan project ready to be built or run.
///
/// This struct encapsulates all the setup work shared between `build_file()` and `run_file()`, including module
/// collection, type checking, codegen setup, and project generation.
struct PreparedProject {
    /// The configured project generator
    generator: ProjectGenerator,
    /// Output directory path
    out_dir: String,
    /// Project root directory (used as working dir when running)
    project_root: PathBuf,
}

/// Prepare an Incan project for building or running.
///
/// This function performs all the shared setup:
/// 1. Collect and parse modules
/// 2. Type check
/// 3. Configure codegen (serde, async, web, etc.)
/// 4. Add Rust crate dependencies
/// 5. Generate Rust project files
fn prepare_project(
    file_path: &str,
    output_dir: Option<&str>,
    locked: bool,
    frozen: bool,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
) -> CliResult<PreparedProject> {
    let modules = collect_modules(file_path)?;

    let Some(main_module) = modules.last() else {
        return Err(CliError::failure("No modules found"));
    };

    let dep_modules = &modules[..modules.len() - 1];

    // ---- RFC 023: Load stdlib modules ----
    let stdlib_modules = stdlib_loader::load_stdlib_modules(&modules)?;

    let path = Path::new(file_path);
    let project_root = resolve_project_root(path);

    let manifest = ProjectManifest::discover(&project_root).map_err(|e| CliError::failure(e.to_string()))?;

    // Type check all modules (dependencies + stdlib first), so diagnostics are associated with the correct file.
    let declared = manifest.as_ref().map(|m| m.declared_crate_names());
    let mut all_errors: String = String::new();
    for (idx, module) in modules.iter().enumerate() {
        // Include both user dependencies and stdlib modules as available imports
        let mut deps_for_module: Vec<(&str, &Program)> =
            modules[..idx].iter().map(|m| (m.name.as_str(), &m.ast)).collect();
        for stdlib_mod in &stdlib_modules {
            deps_for_module.push((&stdlib_mod.name, &stdlib_mod.ast));
        }

        let mut checker = typechecker::TypeChecker::new();
        if let Some(names) = declared.clone() {
            checker.set_declared_crate_names(names);
        }

        match checker.check_with_imports(&module.ast, &deps_for_module) {
            Ok(()) => {
                for warn in checker.warnings() {
                    eprint!(
                        "{}",
                        diagnostics::format_error(module.file_path.to_string_lossy().as_ref(), &module.source, warn)
                    );
                }
            }
            Err(errs) => {
                for err in &errs {
                    all_errors.push_str(&diagnostics::format_error(
                        module.file_path.to_string_lossy().as_ref(),
                        &module.source,
                        err,
                    ));
                }
            }
        }
    }
    if !all_errors.is_empty() {
        return Err(CliError::failure(all_errors.trim_end()));
    }

    // Derive project name (manifest overrides filename)
    let project_name = manifest
        .as_ref()
        .and_then(|m| m.project.as_ref().and_then(|p| p.name.clone()))
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("incan_project")
                .to_string()
        });

    let out_dir = output_dir
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("target/incan/{}", project_name));

    // Validate output directory path to prevent path traversal
    validate_output_dir(&out_dir)?;

    // ---- Setup codegen ----
    let mut codegen = IrCodegen::new();
    if let Some(m) = manifest.as_ref() {
        codegen.set_declared_crate_names(m.declared_crate_names());
    }
    // Add user dependency modules
    for module in dep_modules {
        codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
    }
    // RFC 023: Add stdlib modules
    for stdlib_mod in &stdlib_modules {
        codegen.add_module_with_path_segments(&stdlib_mod.name, &stdlib_mod.ast, stdlib_mod.path_segments.clone());
    }

    // Scan for feature requirements (serde, async, web, list helpers)
    codegen.scan_for_serde(&main_module.ast);
    codegen.scan_for_async(&main_module.ast);
    codegen.scan_for_web(&main_module.ast);
    codegen.scan_for_list_helpers(&main_module.ast);
    for module in dep_modules {
        codegen.scan_for_serde(&module.ast);
        codegen.scan_for_async(&module.ast);
        codegen.scan_for_web(&module.ast);
        codegen.scan_for_list_helpers(&module.ast);
    }
    // RFC 023: Scan stdlib modules for features too
    for stdlib_mod in &stdlib_modules {
        codegen.scan_for_serde(&stdlib_mod.ast);
        codegen.scan_for_async(&stdlib_mod.ast);
        codegen.scan_for_web(&stdlib_mod.ast);
        codegen.scan_for_list_helpers(&stdlib_mod.ast);
    }

    let needs_serde = codegen.needs_serde();
    let needs_tokio = codegen.needs_tokio();
    let needs_web = codegen.needs_web();

    // ---- Setup project generator ----
    let mut generator = ProjectGenerator::new(&out_dir, project_name.as_str(), true);
    generator.set_needs_serde(needs_serde);
    generator.set_needs_tokio(needs_tokio);
    generator.set_needs_web(needs_web);
    generator.set_include_dev_dependencies(false);
    generator.set_rust_edition(
        manifest
            .as_ref()
            .and_then(|m| m.build.as_ref().and_then(|b| b.rust_edition.clone())),
    );

    let mut inline_imports = collect_inline_rust_imports(main_module, false);
    for module in dep_modules {
        inline_imports.extend(collect_inline_rust_imports(module, false));
    }
    // RFC 023: Stdlib modules should not have inline rust imports (they use rust.module() + @rust.extern instead),
    // so we skip collecting from them.

    let cargo_features = CargoFeatureSelection {
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    }
    .normalized();

    let resolved = match resolve_dependencies(manifest.as_ref(), &inline_imports, true, &cargo_features) {
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

    // Resolve lock payload before moving deps into generator (borrows resolved)
    let lock_payload = resolve_lock_payload(
        &project_root,
        project_name.as_str(),
        manifest.as_ref(),
        &resolved,
        &cargo_features,
        locked,
        frozen,
    )?;
    generator.set_cargo_lock_payload(lock_payload);

    let cargo_flags = cargo_command_flags(locked, frozen, &cargo_features);
    generator.set_cargo_policy_flags(cargo_flags);

    generator.set_dependencies(resolved.dependencies);
    generator.set_dev_dependencies(resolved.dev_dependencies);

    // ---- Generate Rust project files ----
    let has_deps = !dep_modules.is_empty();
    if has_deps {
        let module_paths: Vec<Vec<String>> = dep_modules.iter().map(|m| m.path_segments.clone()).collect();
        let (main_code, rust_modules) = codegen
            .try_generate_multi_file_nested(&main_module.ast, &module_paths)
            .map_err(|e| CliError::failure(format!("Code generation error: {}", e)))?;

        generator
            .generate_nested(&main_code, &rust_modules)
            .map_err(|e| CliError::failure(format!("Error generating project: {}", e)))?;
    } else {
        let rust_code = codegen
            .try_generate(&main_module.ast)
            .map_err(|e| CliError::failure(format!("Code generation error: {}", e)))?;
        generator
            .generate(&rust_code)
            .map_err(|e| CliError::failure(format!("Error generating project: {}", e)))?;
    }

    Ok(PreparedProject {
        generator,
        out_dir,
        project_root,
    })
}

/// Build an Incan file to a Rust project.
pub fn build_file(
    file_path: &str,
    output_dir: Option<&String>,
    locked: bool,
    frozen: bool,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
) -> CliResult<ExitCode> {
    let prepared = prepare_project(
        file_path,
        output_dir.map(|s| s.as_str()),
        locked,
        frozen,
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    )?;

    println!("Generated Rust project in: {}", prepared.out_dir);
    println!("Building...");

    match prepared.generator.build() {
        Ok(result) => {
            if result.success {
                println!("✓ Build successful!");
                println!("Binary: {}", prepared.generator.binary_path().display());
                Ok(ExitCode::SUCCESS)
            } else {
                Err(CliError::failure(format!("Build failed:\n{}", result.stderr)))
            }
        }
        Err(e) => Err(CliError::failure(format!("Error running cargo: {}", e))),
    }
}

/// Build and run an Incan file.
pub fn run_file(
    file_path: &str,
    locked: bool,
    frozen: bool,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
) -> CliResult<ExitCode> {
    let prepared = prepare_project(
        file_path,
        None,
        locked,
        frozen,
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    )?;

    match prepared.generator.run_with_cwd(&prepared.project_root) {
        Ok(result) => {
            if !result.stdout.is_empty() {
                print!("{}", result.stdout);
            }
            if !result.stderr.is_empty() && !result.success {
                eprint!("{}", result.stderr);
            }
            // Return the program's exit code
            Ok(ExitCode(result.exit_code.unwrap_or(0)))
        }
        Err(e) => Err(CliError::failure(format!("Error running program: {}", e))),
    }
}

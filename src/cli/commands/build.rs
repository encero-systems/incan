//! Build and run pipeline for Incan projects.
//!
//! This module handles the full compilation flow: module collection, type checking, codegen configuration, dependency
//! resolution, project generation, and Cargo build/run.

use std::env;
use std::path::{Path, PathBuf};

use crate::backend::{IrCodegen, ProjectGenerator};
use crate::cli::{CliError, CliResult, ExitCode};
use crate::dependency_resolver::resolve_dependencies;
use crate::frontend::ast::Program;
use crate::frontend::ast::{Declaration, Decorator, Span, Spanned};
use crate::frontend::{diagnostics, typechecker};
use crate::lockfile::CargoFeatureSelection;
use crate::manifest::ProjectManifest;
use std::collections::HashSet;

use super::common::{
    build_source_map, cargo_command_flags, collect_inline_rust_imports, collect_modules, collect_stdlib_usage,
    format_dependency_error, merge_stdlib_extra_dependencies, resolve_project_root, validate_output_dir,
};
use super::lock::{LockResolutionRequest, resolve_lock_payload};
use crate::cli::prelude::ParsedModule;

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
    /// Source contexts for `@rust.extern` declarations, used to enrich downstream Rust/Cargo failures.
    rust_extern_contexts: Vec<RustExternDeclContext>,
}

#[derive(Debug, Clone)]
struct RustExternDeclContext {
    file_path: PathBuf,
    source: String,
    item_name: String,
    rust_module_path: String,
    span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RustExternBuildFailureKind {
    UnresolvedBackingItem,
    SignatureMismatch,
    FeatureGatedBackingPath,
}

fn has_rust_extern_decorator(decorators: &[Spanned<Decorator>]) -> bool {
    decorators
        .iter()
        .any(|d| d.node.path.segments.join(".") == "rust.extern")
}

fn collect_rust_extern_contexts(modules: &[ParsedModule]) -> Vec<RustExternDeclContext> {
    let mut contexts = Vec::new();
    for module in modules {
        let Some(rust_module) = module.ast.rust_module_path.as_ref().map(|p| p.node.clone()) else {
            continue;
        };
        for decl in &module.ast.declarations {
            match &decl.node {
                Declaration::Function(func) => {
                    if has_rust_extern_decorator(&func.decorators) {
                        contexts.push(RustExternDeclContext {
                            file_path: module.file_path.clone(),
                            source: module.source.clone(),
                            item_name: func.name.clone(),
                            rust_module_path: rust_module.clone(),
                            span: decl.span,
                        });
                    }
                }
                Declaration::Trait(tr) => {
                    for method in &tr.methods {
                        if has_rust_extern_decorator(&method.node.decorators) {
                            contexts.push(RustExternDeclContext {
                                file_path: module.file_path.clone(),
                                source: module.source.clone(),
                                item_name: method.node.name.clone(),
                                rust_module_path: rust_module.clone(),
                                span: method.span,
                            });
                        }
                    }
                }
                Declaration::Model(model) => {
                    for method in &model.methods {
                        if method.node.receiver.is_none() && has_rust_extern_decorator(&method.node.decorators) {
                            contexts.push(RustExternDeclContext {
                                file_path: module.file_path.clone(),
                                source: module.source.clone(),
                                item_name: method.node.name.clone(),
                                rust_module_path: rust_module.clone(),
                                span: method.span,
                            });
                        }
                    }
                }
                Declaration::Class(class) => {
                    for method in &class.methods {
                        if method.node.receiver.is_none() && has_rust_extern_decorator(&method.node.decorators) {
                            contexts.push(RustExternDeclContext {
                                file_path: module.file_path.clone(),
                                source: module.source.clone(),
                                item_name: method.node.name.clone(),
                                rust_module_path: rust_module.clone(),
                                span: method.span,
                            });
                        }
                    }
                }
                Declaration::Newtype(nt) => {
                    for method in &nt.methods {
                        if method.node.receiver.is_none() && has_rust_extern_decorator(&method.node.decorators) {
                            contexts.push(RustExternDeclContext {
                                file_path: module.file_path.clone(),
                                source: module.source.clone(),
                                item_name: method.node.name.clone(),
                                rust_module_path: rust_module.clone(),
                                span: method.span,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
    }
    contexts
}

fn classify_rust_extern_build_failure(
    stderr: &str,
    item_name: &str,
    rust_module_path: &str,
) -> Option<RustExternBuildFailureKind> {
    if !stderr.contains(item_name) && !stderr.contains(rust_module_path) {
        return None;
    }
    if stderr.contains("gated behind the")
        || stderr.contains("configured out")
        || stderr.contains("the item is gated behind")
    {
        return Some(RustExternBuildFailureKind::FeatureGatedBackingPath);
    }
    if stderr.contains("mismatched types") || stderr.contains("error[E0308]") {
        return Some(RustExternBuildFailureKind::SignatureMismatch);
    }
    if stderr.contains("cannot find")
        || stderr.contains("failed to resolve")
        || stderr.contains("unresolved import")
        || stderr.contains("error[E0425]")
    {
        return Some(RustExternBuildFailureKind::UnresolvedBackingItem);
    }
    None
}

fn format_rust_extern_wrapped_diagnostics(stderr: &str, contexts: &[RustExternDeclContext]) -> Option<String> {
    let mut rendered = String::new();
    let mut seen: HashSet<String> = HashSet::new();
    for ctx in contexts {
        let Some(kind) = classify_rust_extern_build_failure(stderr, &ctx.item_name, &ctx.rust_module_path) else {
            continue;
        };
        let key = format!(
            "{}:{}:{}:{}",
            ctx.file_path.display(),
            ctx.item_name,
            ctx.span.start,
            ctx.span.end
        );
        if !seen.insert(key) {
            continue;
        }
        let err = match kind {
            RustExternBuildFailureKind::UnresolvedBackingItem => {
                diagnostics::errors::rust_extern_unresolved_backing_item(
                    &ctx.item_name,
                    &ctx.rust_module_path,
                    ctx.span,
                )
            }
            RustExternBuildFailureKind::SignatureMismatch => {
                diagnostics::errors::rust_extern_signature_mismatch(&ctx.item_name, &ctx.rust_module_path, ctx.span)
            }
            RustExternBuildFailureKind::FeatureGatedBackingPath => {
                diagnostics::errors::rust_extern_feature_gated_backing_path(
                    &ctx.item_name,
                    &ctx.rust_module_path,
                    ctx.span,
                )
            }
        };
        rendered.push_str(&diagnostics::format_error(
            ctx.file_path.to_string_lossy().as_ref(),
            &ctx.source,
            &err,
        ));
    }
    if rendered.is_empty() { None } else { Some(rendered) }
}

fn resolve_library_project_root(file_path: Option<&str>) -> CliResult<PathBuf> {
    if let Some(file_path) = file_path {
        return Ok(resolve_project_root(Path::new(file_path)));
    }

    env::current_dir().map_err(|e| CliError::failure(format!("failed to determine current directory: {e}")))
}

fn validate_library_entrypoint(manifest: &ProjectManifest) -> CliResult<PathBuf> {
    let lib_entry = manifest.project_root().join("src").join("lib.incn");
    if !lib_entry.is_file() {
        return Err(CliError::failure(format!(
            "`incan build --lib` requires `{}`",
            lib_entry.display()
        )));
    }
    Ok(lib_entry)
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
    let rust_extern_contexts = collect_rust_extern_contexts(&modules);

    let Some(main_module) = modules.last() else {
        return Err(CliError::failure("No modules found"));
    };

    let dep_modules = &modules[..modules.len() - 1];
    let stdlib_usage = collect_stdlib_usage(&modules);

    let path = Path::new(file_path);
    let project_root = resolve_project_root(path);

    let manifest = ProjectManifest::discover(&project_root).map_err(|e| CliError::failure(e.to_string()))?;

    // Type check all modules (dependencies + stdlib first), so diagnostics are associated with the correct file.
    let declared = manifest.as_ref().map(|m| m.declared_rust_crate_names());
    let mut all_errors: String = String::new();
    for (idx, module) in modules.iter().enumerate() {
        let deps_for_module: Vec<(&str, &Program)> = modules[..idx].iter().map(|m| (m.name.as_str(), &m.ast)).collect();

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
        codegen.set_declared_crate_names(m.declared_rust_crate_names());
    }
    // Add user dependency modules
    for module in dep_modules {
        codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
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
    let needs_serde = codegen.needs_serde() || stdlib_usage.needs_serde;
    let needs_tokio = codegen.needs_tokio() || stdlib_usage.needs_tokio;
    let needs_web = codegen.needs_web() || stdlib_usage.needs_web;

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

    let mut resolved = match resolve_dependencies(manifest.as_ref(), &inline_imports, true, &cargo_features) {
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
    merge_stdlib_extra_dependencies(&mut resolved, &stdlib_usage);

    // Resolve lock payload before moving deps into generator (borrows resolved)
    let lock_payload = resolve_lock_payload(LockResolutionRequest {
        project_root: &project_root,
        project_name: project_name.as_str(),
        manifest: manifest.as_ref(),
        resolved: &resolved,
        stdlib_usage: &stdlib_usage,
        cargo_features: &cargo_features,
        locked,
        frozen,
    })?;
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
        rust_extern_contexts,
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
            } else if let Some(wrapped) =
                format_rust_extern_wrapped_diagnostics(&result.stderr, &prepared.rust_extern_contexts)
            {
                Err(CliError::failure(format!(
                    "Build failed.\n\n{}\nRaw cargo/rustc output:\n{}",
                    wrapped.trim_end(),
                    result.stderr
                )))
            } else {
                Err(CliError::failure(format!("Build failed:\n{}", result.stderr)))
            }
        }
        Err(e) => Err(CliError::failure(format!("Error running cargo: {}", e))),
    }
}

/// Validate RFC 031 library-mode preconditions.
pub fn build_library(
    file_path: Option<&str>,
    _output_dir: Option<&String>,
    _locked: bool,
    _frozen: bool,
    _cargo_features: Vec<String>,
    _cargo_no_default_features: bool,
    _cargo_all_features: bool,
) -> CliResult<ExitCode> {
    let project_root = resolve_library_project_root(file_path)?;
    let Some(manifest) = ProjectManifest::discover(&project_root).map_err(|e| CliError::failure(e.to_string()))? else {
        return Err(CliError::failure(
            "No incan.toml found for `incan build --lib` (run `incan init` first)",
        ));
    };

    let _lib_entry = validate_library_entrypoint(&manifest)?;

    Err(CliError::failure(
        "`incan build --lib` preconditions passed, but library artifact generation is not implemented yet.",
    ))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ProjectManifest;
    use std::fs;

    #[test]
    fn classify_signature_mismatch_for_rust_extern_context() {
        let stderr = "error[E0308]: mismatched types in `incan_stdlib::testing::fail`\n  --> src/main.rs:10:5";
        let kind = classify_rust_extern_build_failure(stderr, "fail", "incan_stdlib::testing");
        assert_eq!(kind, Some(RustExternBuildFailureKind::SignatureMismatch));
    }

    #[test]
    fn classify_unresolved_backing_item_for_rust_extern_context() {
        let stderr = "error[E0425]: cannot find function `fail` in module `incan_stdlib::testing`";
        let kind = classify_rust_extern_build_failure(stderr, "fail", "incan_stdlib::testing");
        assert_eq!(kind, Some(RustExternBuildFailureKind::UnresolvedBackingItem));
    }

    #[test]
    fn wraps_rust_extern_failure_back_to_incan_declaration_span() {
        let stderr = "error[E0425]: cannot find function `fail` in module `incan_stdlib::testing`";
        let contexts = vec![RustExternDeclContext {
            file_path: PathBuf::from("stdlib/testing.incn"),
            source: "rust.module(\"incan_stdlib::testing\")\n@rust.extern\ndef fail(msg: str) -> None:\n  ...\n"
                .to_string(),
            item_name: "fail".to_string(),
            rust_module_path: "incan_stdlib::testing".to_string(),
            span: Span { start: 35, end: 73 },
        }];
        let rendered = format_rust_extern_wrapped_diagnostics(stderr, &contexts);
        let Some(rendered) = rendered else {
            panic!("expected wrapped diagnostic");
        };
        assert!(rendered.contains("Rust backing item"));
        assert!(rendered.contains("incan_stdlib::testing::fail"));
    }

    #[test]
    fn library_entrypoint_precondition_fails_when_missing() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let manifest_path = tmp.path().join("incan.toml");
        let manifest_content = "[project]\nname = \"mylib\"\n";
        fs::write(&manifest_path, manifest_content)?;
        let manifest = ProjectManifest::from_str(manifest_content, &manifest_path)?;

        let err = validate_library_entrypoint(&manifest);
        assert!(err.is_err(), "expected missing src/lib.incn to fail");
        Ok(())
    }

    #[test]
    fn library_entrypoint_precondition_passes_when_present() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let src_dir = tmp.path().join("src");
        fs::create_dir_all(&src_dir)?;
        fs::write(src_dir.join("lib.incn"), "\"\"\"lib\"\"\"\n")?;
        let manifest_path = tmp.path().join("incan.toml");
        let manifest_content = "[project]\nname = \"mylib\"\n";
        fs::write(&manifest_path, manifest_content)?;
        let manifest = ProjectManifest::from_str(manifest_content, &manifest_path)?;

        let lib_path = validate_library_entrypoint(&manifest)?;
        assert!(lib_path.ends_with("src/lib.incn"));
        Ok(())
    }
}

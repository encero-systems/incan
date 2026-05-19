//! Build and run pipeline for Incan projects.
//!
//! This module handles the full compilation flow: module collection, type checking, codegen configuration, dependency
//! resolution, project generation, and Cargo build/run.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::backend::{IrCodegen, ProjectGenerator, RunProfile};
use crate::cli::{CliError, CliResult, ExitCode};
use crate::dependency_resolver::resolve_dependencies;
use crate::frontend::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, CheckedApiPackageIdentity,
    collect_checked_api_metadata, validate_checked_api_docstrings,
};
use crate::frontend::ast::{Declaration, Decorator, ImportKind, Span, Spanned};
use crate::frontend::contract_metadata::{ContractMetadataPackage, read_project_model_bundles};
use crate::frontend::library_exports::{CheckedExportKind, CheckedNamedExport, collect_checked_public_exports};
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::module::canonicalize_source_module_segments;
use crate::frontend::{diagnostics, typechecker};
use crate::library_manifest::LibraryManifest;
#[cfg(feature = "rust_inspect")]
use crate::library_manifest::LibraryRustAbi;
use crate::lockfile::CargoFeatureSelection;
use crate::manifest::ProjectManifest;

use super::common::{
    CargoPolicy, build_source_map, cargo_command_flags, collect_inline_rust_imports, collect_modules,
    collect_project_requirements, enforce_project_toolchain_constraint, format_dependency_error,
    imported_module_deps_for_with_index, merge_project_requirement_dependencies, module_key_index,
    resolve_project_root, typecheck_modules_with_import_graph, validate_output_dir,
};
#[cfg(feature = "rust_inspect")]
use super::common::{collect_rust_inspect_query_paths, ensure_rust_inspect_workspace, prewarm_rust_inspect_workspace};
use super::lock::{LockResolutionRequest, resolve_lock_payload};
use super::vocab_extraction::{PendingDesugarerArtifact, collect_library_vocab_metadata};
use crate::cli::prelude::ParsedModule;
#[cfg(feature = "rust_inspect")]
use crate::rust_inspect::{InspectError, Inspector, InspectorConfig};

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
    /// Whether generating the Rust project changed any on-disk project inputs.
    project_changed: bool,
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
                Declaration::Function(func) if has_rust_extern_decorator(&func.decorators) => {
                    contexts.push(RustExternDeclContext {
                        file_path: module.file_path.clone(),
                        source: module.source.clone(),
                        item_name: func.name.clone(),
                        rust_module_path: rust_module.clone(),
                        span: decl.span,
                    });
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

#[cfg(feature = "rust_inspect")]
/// Collect canonical Rust metadata paths that must be shipped in a library manifest's ABI payload.
fn collect_library_rust_abi_query_paths(
    modules: &[ParsedModule],
    rust_extern_contexts: &[RustExternDeclContext],
) -> Vec<String> {
    let mut paths: BTreeSet<String> = collect_rust_inspect_query_paths(modules).into_iter().collect();
    for context in rust_extern_contexts {
        paths.insert(format!("{}::{}", context.rust_module_path, context.item_name));
    }
    paths.into_iter().collect()
}

#[cfg(feature = "rust_inspect")]
/// Read prewarmed Rust metadata from the generated inspect workspace and package it as manifest ABI.
fn collect_library_rust_abi(
    rust_inspect_manifest_dir: &Path,
    query_paths: &[String],
) -> CliResult<Option<LibraryRustAbi>> {
    if query_paths.is_empty() {
        return Ok(None);
    }

    let inspector = Inspector::new(InspectorConfig::new(rust_inspect_manifest_dir.to_path_buf()));
    let mut items = Vec::new();
    for path in query_paths {
        match inspector.get(path) {
            Ok(result) => items.push((*result.metadata).clone()),
            Err(InspectError::MetadataMiss { .. }) => {}
            Err(err) => {
                return Err(CliError::failure(format!(
                    "failed to read Rust ABI metadata for `{path}` from {}: {err}",
                    rust_inspect_manifest_dir.display()
                )));
            }
        }
    }
    Ok(LibraryRustAbi::from_items(items))
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
        let normalized = if Path::new(file_path).is_absolute() {
            PathBuf::from(file_path)
        } else {
            env::current_dir()
                .map_err(|e| CliError::failure(format!("failed to determine current directory: {e}")))?
                .join(file_path)
        };
        return Ok(resolve_project_root(&normalized));
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

fn module_key(path_segments: &[String]) -> String {
    canonicalize_source_module_segments(path_segments).join("_")
}

/// Rename one checked export while preserving its semantic export kind.
fn rename_checked_export(export: &CheckedNamedExport, exported_name: &str) -> CheckedNamedExport {
    let mut renamed = export.clone();
    renamed.name = exported_name.to_string();

    match &mut renamed.kind {
        CheckedExportKind::Function(function_export) => function_export.name = exported_name.to_string(),
        CheckedExportKind::Partial(partial_export) => partial_export.name = exported_name.to_string(),
        CheckedExportKind::Alias(alias_export) => alias_export.name = exported_name.to_string(),
        CheckedExportKind::TypeAlias(type_alias_export) => type_alias_export.name = exported_name.to_string(),
        CheckedExportKind::Model(model_export) => model_export.name = exported_name.to_string(),
        CheckedExportKind::Class(class_export) => class_export.name = exported_name.to_string(),
        CheckedExportKind::Trait(trait_export) => trait_export.name = exported_name.to_string(),
        CheckedExportKind::Enum(enum_export) => enum_export.name = exported_name.to_string(),
        CheckedExportKind::Newtype(newtype_export) => newtype_export.name = exported_name.to_string(),
        CheckedExportKind::Const(const_export) => const_export.name = exported_name.to_string(),
        CheckedExportKind::Static(static_export) => static_export.name = exported_name.to_string(),
    }

    renamed
}

/// Map exported scalar value enums to the serialized identities used by library consumers.
fn public_ordinal_type_identities(
    lib_module: &ParsedModule,
    project_name: &str,
    selected_exports: &[CheckedNamedExport],
) -> HashMap<String, String> {
    let exported_value_enums = selected_exports
        .iter()
        .filter_map(|export| match &export.kind {
            CheckedExportKind::Enum(enum_export) if enum_export.value_type.is_some() => Some(export.name.as_str()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    if exported_value_enums.is_empty() {
        return HashMap::new();
    }

    let mut identities = HashMap::new();
    for decl in &lib_module.ast.declarations {
        let Declaration::Enum(enum_decl) = &decl.node else {
            continue;
        };
        if !matches!(enum_decl.visibility, crate::frontend::ast::Visibility::Public) {
            continue;
        }
        if exported_value_enums.contains(enum_decl.name.as_str()) {
            identities.insert(
                format!("lib.{}", enum_decl.name),
                format!("{project_name}.{}", enum_decl.name),
            );
        }
    }
    for decl in &lib_module.ast.declarations {
        let Declaration::Import(import) = &decl.node else {
            continue;
        };
        if !matches!(import.visibility, crate::frontend::ast::Visibility::Public) {
            continue;
        }
        let ImportKind::From { module, items } = &import.kind else {
            continue;
        };
        let source_module = canonicalize_source_module_segments(&module.segments).join(".");
        for item in items {
            let exported_name = item.alias.as_deref().unwrap_or(item.name.as_str());
            if exported_value_enums.contains(exported_name) {
                identities.insert(
                    format!("{source_module}.{}", item.name),
                    format!("{project_name}.{exported_name}"),
                );
            }
        }
    }
    identities
}

struct LibraryReexportResolver<'a> {
    module_exports: &'a HashMap<String, HashMap<String, CheckedNamedExport>>,
}

impl<'a> LibraryReexportResolver<'a> {
    fn new(module_exports: &'a HashMap<String, HashMap<String, CheckedNamedExport>>) -> Self {
        Self { module_exports }
    }

    fn resolve(
        &self,
        lib_module: &ParsedModule,
    ) -> Result<Vec<CheckedNamedExport>, Vec<crate::frontend::diagnostics::CompileError>> {
        let mut errors = Vec::new();
        let mut resolved = Vec::new();
        let mut exported_names: HashSet<String> = HashSet::new();
        let known_modules: Vec<String> = self.module_exports.keys().cloned().collect();

        for decl in &lib_module.ast.declarations {
            let Declaration::Import(import) = &decl.node else {
                continue;
            };
            if !matches!(import.visibility, crate::frontend::ast::Visibility::Public) {
                continue;
            }

            let ImportKind::From { module, items } = &import.kind else {
                errors.push(diagnostics::errors::library_pub_reexport_requires_from(decl.span));
                continue;
            };

            let module_name = module_key(&module.segments);
            let Some(exports_by_name) = self.module_exports.get(&module_name) else {
                errors.push(diagnostics::errors::library_reexport_unknown_module(
                    &module.to_rust_path(),
                    &known_modules,
                    decl.span,
                ));
                continue;
            };

            for item in items {
                let exported_name = item.alias.as_ref().unwrap_or(&item.name).clone();
                if !exported_names.insert(exported_name.clone()) {
                    errors.push(diagnostics::errors::duplicate_library_export(&exported_name, decl.span));
                    continue;
                }

                let Some(export) = exports_by_name.get(&item.name) else {
                    let available: Vec<String> = exports_by_name.keys().cloned().collect();
                    errors.push(diagnostics::errors::import_not_exported(
                        &item.name,
                        &module.to_rust_path(),
                        &available,
                        decl.span,
                    ));
                    continue;
                };

                resolved.push(rename_checked_export(export, &exported_name));
            }
        }

        if errors.is_empty() { Ok(resolved) } else { Err(errors) }
    }
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
    cargo_policy: &CargoPolicy,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
) -> CliResult<PreparedProject> {
    let normalized_file_path = if Path::new(file_path).is_absolute() {
        PathBuf::from(file_path)
    } else {
        env::current_dir()
            .map_err(|e| CliError::failure(format!("failed to determine current directory: {e}")))?
            .join(file_path)
    };
    let path = normalized_file_path.as_path();
    let inferred_project_root = resolve_project_root(path);
    let manifest = ProjectManifest::discover(&inferred_project_root).map_err(|e| CliError::failure(e.to_string()))?;
    if let Some(manifest) = manifest.as_ref() {
        enforce_project_toolchain_constraint(manifest)?;
    }

    let normalized_file_path_str = normalized_file_path.to_string_lossy().to_string();
    let modules = collect_modules(&normalized_file_path_str)?;
    let rust_extern_contexts = collect_rust_extern_contexts(&modules);

    let Some(main_module) = modules.last() else {
        return Err(CliError::failure("No modules found"));
    };

    let dep_modules = &modules[..modules.len() - 1];
    let project_root = manifest
        .as_ref()
        .map(|manifest| manifest.project_root().to_path_buf())
        .unwrap_or(inferred_project_root);
    let library_manifest_index = manifest
        .as_ref()
        .map(LibraryManifestIndex::from_project_manifest)
        .unwrap_or_default();
    let project_requirements = collect_project_requirements(&modules, &library_manifest_index)?;

    // Type check all modules (dependencies + stdlib first), so diagnostics are associated with the correct file.
    typecheck_modules_with_import_graph(
        &modules,
        manifest.as_ref(),
        &library_manifest_index,
        #[cfg(feature = "rust_inspect")]
        None,
    )?;

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
    codegen.set_preserve_dependency_public_items(false);
    if let Some(m) = manifest.as_ref() {
        codegen.set_declared_crate_names(m.declared_rust_crate_names());
    }
    codegen.set_library_manifest_index(library_manifest_index.clone());
    // Add user dependency modules
    for module in dep_modules {
        codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
    }
    // ---- Setup project generator ----
    let mut generator = ProjectGenerator::new(&out_dir, project_name.as_str(), true);
    generator.set_stdlib_features(project_requirements.stdlib_features.clone());
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
    merge_project_requirement_dependencies(&mut resolved, &project_requirements)?;
    #[cfg(feature = "rust_inspect")]
    let metadata_query_paths = collect_library_rust_abi_query_paths(&modules, &rust_extern_contexts);

    // Resolve lock payload before moving deps into generator (borrows resolved)
    let lock_payload = resolve_lock_payload(LockResolutionRequest {
        project_root: &project_root,
        project_name: project_name.as_str(),
        manifest: manifest.as_ref(),
        resolved: &resolved,
        project_requirements: &project_requirements,
        cargo_features: &cargo_features,
        cargo_policy,
        #[cfg(feature = "rust_inspect")]
        rust_inspect_query_paths: &metadata_query_paths,
    })?;
    #[cfg(feature = "rust_inspect")]
    {
        let rust_inspect_manifest_dir = ensure_rust_inspect_workspace(
            &project_root,
            project_name.as_str(),
            manifest
                .as_ref()
                .and_then(|m| m.build.as_ref().and_then(|b| b.rust_edition.clone())),
            &resolved,
            &project_requirements,
            lock_payload.clone(),
        )?;
        prewarm_rust_inspect_workspace(&rust_inspect_manifest_dir, &metadata_query_paths)?;
        codegen.set_rust_inspect_manifest_dir(rust_inspect_manifest_dir);
    }
    generator.set_cargo_lock_payload(lock_payload);

    let cargo_flags = cargo_command_flags(cargo_policy, &cargo_features);
    generator.set_cargo_policy_flags(cargo_flags);

    generator.set_dependencies(resolved.dependencies);
    generator.set_dev_dependencies(resolved.dev_dependencies);

    // ---- Generate Rust project files ----
    let has_deps = !dep_modules.is_empty();
    let project_changed = if has_deps {
        let module_paths: Vec<Vec<String>> = dep_modules.iter().map(|m| m.path_segments.clone()).collect();
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

    Ok(PreparedProject {
        generator,
        project_changed,
        out_dir,
        project_root,
        rust_extern_contexts,
    })
}

/// Build an Incan file to a Rust project.
pub fn build_file(
    file_path: &str,
    output_dir: Option<&String>,
    cargo_policy: CargoPolicy,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
) -> CliResult<ExitCode> {
    let prepared = prepare_project(
        file_path,
        output_dir.map(|s| s.as_str()),
        &cargo_policy,
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
    cargo_policy: CargoPolicy,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
) -> CliResult<ExitCode> {
    let project_root = resolve_library_project_root(file_path)?;
    let Some(manifest) = ProjectManifest::discover(&project_root).map_err(|e| CliError::failure(e.to_string()))? else {
        return Err(CliError::failure(
            "No incan.toml found for `incan build --lib` (run `incan init` first)",
        ));
    };
    enforce_project_toolchain_constraint(&manifest)?;

    let lib_entry = validate_library_entrypoint(&manifest)?;
    let lib_entry_str = lib_entry.to_string_lossy().to_string();
    let modules = collect_modules(&lib_entry_str)?;

    let Some(lib_module) = modules.last() else {
        return Err(CliError::failure("No modules found for library build"));
    };
    if lib_module.file_path != lib_entry {
        return Err(CliError::failure(format!(
            "Library entrypoint mismatch: expected `{}`, got `{}`",
            lib_entry.display(),
            lib_module.file_path.display()
        )));
    }

    let declared = manifest.declared_rust_crate_names();
    let library_manifest_index = LibraryManifestIndex::from_project_manifest(&manifest);
    let project_requirements = collect_project_requirements(&modules, &library_manifest_index)?;
    let contract_model_bundles = read_project_model_bundles(&project_root, &manifest.contract_model_bundle_paths())
        .map_err(|error| CliError::failure(error.to_string()))?;
    let rust_extern_contexts = collect_rust_extern_contexts(&modules);
    let dep_modules = &modules[..modules.len() - 1];

    let mut inline_imports = collect_inline_rust_imports(lib_module, false);
    for module in dep_modules {
        inline_imports.extend(collect_inline_rust_imports(module, false));
    }
    let project_name = manifest
        .project
        .as_ref()
        .and_then(|project| project.name.clone())
        .or_else(|| {
            manifest
                .project_root()
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "incan_library".to_string());

    let cargo_features = CargoFeatureSelection {
        cargo_features: cargo_features.clone(),
        cargo_no_default_features,
        cargo_all_features,
    }
    .normalized();

    let mut resolved = match resolve_dependencies(Some(&manifest), &inline_imports, true, &cargo_features) {
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

    let lock_payload_for_typecheck = resolve_lock_payload(LockResolutionRequest {
        project_root: &project_root,
        project_name: project_name.as_str(),
        manifest: Some(&manifest),
        resolved: &resolved,
        project_requirements: &project_requirements,
        cargo_features: &cargo_features,
        cargo_policy: &cargo_policy,
        #[cfg(feature = "rust_inspect")]
        rust_inspect_query_paths: &metadata_query_paths,
    })?;
    #[cfg(feature = "rust_inspect")]
    let rust_inspect_manifest_dir = project_root.join("target").join("incan_lock");
    #[cfg(feature = "rust_inspect")]
    {
        ensure_rust_inspect_workspace(
            &project_root,
            project_name.as_str(),
            manifest.build.as_ref().and_then(|build| build.rust_edition.clone()),
            &resolved,
            &project_requirements,
            lock_payload_for_typecheck.clone(),
        )?;
        prewarm_rust_inspect_workspace(&rust_inspect_manifest_dir, &metadata_query_paths)?;
    }

    let mut all_errors = String::new();
    let mut checked_exports_by_module: HashMap<String, HashMap<String, CheckedNamedExport>> = HashMap::new();
    let mut api_metadata_modules = Vec::new();
    let module_idx_by_key = module_key_index(&modules);

    for (idx, module) in modules.iter().enumerate() {
        let deps_for_module = imported_module_deps_for_with_index(&modules, idx, &module_idx_by_key);
        let mut checker = typechecker::TypeChecker::new();
        checker.set_declared_crate_names(declared.clone());
        checker.set_library_manifest_index(library_manifest_index.clone());
        #[cfg(feature = "rust_inspect")]
        checker.set_rust_inspect_manifest_dir(rust_inspect_manifest_dir.clone());

        match checker.check_with_imports(&module.ast, &deps_for_module) {
            Ok(()) => {
                for warn in checker.warnings() {
                    eprint!(
                        "{}",
                        diagnostics::format_error(module.file_path.to_string_lossy().as_ref(), &module.source, warn)
                    );
                }
                let module_exports = collect_checked_public_exports(&module.ast, &checker);
                api_metadata_modules.push(collect_checked_api_metadata(
                    &module.ast,
                    &checker,
                    module.path_segments.clone(),
                ));
                checked_exports_by_module.insert(
                    module_key(&module.path_segments),
                    module_exports
                        .into_iter()
                        .map(|export| (export.name.clone(), export))
                        .collect(),
                );
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

    for diagnostic in validate_checked_api_docstrings(&api_metadata_modules) {
        if let Some(module) = modules
            .iter()
            .find(|module| module.path_segments == diagnostic.module_path)
        {
            all_errors.push_str(&diagnostics::format_error(
                module.file_path.to_string_lossy().as_ref(),
                &module.source,
                &diagnostic.error,
            ));
        } else {
            all_errors.push_str(&diagnostic.error.message);
            all_errors.push('\n');
        }
    }

    if !all_errors.is_empty() {
        return Err(CliError::failure(all_errors.trim_end()));
    }

    let selected_exports = LibraryReexportResolver::new(&checked_exports_by_module)
        .resolve(lib_module)
        .map_err(|errs| {
            let mut msg = String::new();
            for err in &errs {
                msg.push_str(&diagnostics::format_error(
                    lib_module.file_path.to_string_lossy().as_ref(),
                    &lib_module.source,
                    err,
                ));
            }
            CliError::failure(msg.trim_end())
        })?;

    let project_version = manifest
        .project
        .as_ref()
        .and_then(|project| project.version.clone())
        .unwrap_or_else(|| "0.1.0".to_string());

    let mut library_manifest =
        LibraryManifest::from_checked_exports(project_name.clone(), project_version.clone(), &selected_exports);
    library_manifest.contract_metadata.models = ContractMetadataPackage::new(
        contract_model_bundles
            .into_iter()
            .filter(|bundle| bundle.publishable)
            .collect(),
    );
    library_manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: Some(CheckedApiPackageIdentity {
            name: project_name.clone(),
            version: Some(project_version.clone()),
        }),
        modules: api_metadata_modules,
    });
    #[cfg(feature = "rust_inspect")]
    {
        library_manifest.rust_abi = collect_library_rust_abi(&rust_inspect_manifest_dir, &metadata_query_paths)?;
    }
    let mut pending_desugarer_artifact: Option<PendingDesugarerArtifact> = None;

    if let Some(vocab_extraction) = collect_library_vocab_metadata(&manifest, &project_root)? {
        pending_desugarer_artifact = vocab_extraction.pending_desugarer_artifact;
        library_manifest.vocab = Some(vocab_extraction.payload);
        library_manifest.soft_keywords.activations = vocab_extraction.compatibility_activations;
    }

    let out_dir = project_root.join("target").join("lib");
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| CliError::failure(format!("failed to create {}: {e}", out_dir.display())))?;
    package_desugarer_artifact(&out_dir, pending_desugarer_artifact.as_ref())?;
    let manifest_path = out_dir.join(format!("{project_name}.incnlib"));

    let mut codegen = IrCodegen::new();
    codegen.set_preserve_dependency_public_items(true);
    codegen.set_declared_crate_names(declared);
    codegen.set_library_manifest_index(library_manifest_index.clone());
    codegen.set_public_ordinal_type_identities(public_ordinal_type_identities(
        lib_module,
        project_name.as_str(),
        &selected_exports,
    ));
    for module in dep_modules {
        codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
    }
    let mut generator = ProjectGenerator::new(&out_dir, project_name.as_str(), false);
    generator.set_stdlib_features(project_requirements.stdlib_features.clone());
    generator.set_include_dev_dependencies(false);
    generator.set_rust_edition(manifest.build.as_ref().and_then(|build| build.rust_edition.clone()));
    #[cfg(feature = "rust_inspect")]
    let lock_payload = resolve_lock_payload(LockResolutionRequest {
        project_root: &project_root,
        project_name: project_name.as_str(),
        manifest: Some(&manifest),
        resolved: &resolved,
        project_requirements: &project_requirements,
        cargo_features: &cargo_features,
        cargo_policy: &cargo_policy,
        #[cfg(feature = "rust_inspect")]
        rust_inspect_query_paths: &metadata_query_paths,
    })?;
    #[cfg(feature = "rust_inspect")]
    {
        let rust_inspect_manifest_dir = ensure_rust_inspect_workspace(
            &project_root,
            project_name.as_str(),
            manifest.build.as_ref().and_then(|build| build.rust_edition.clone()),
            &resolved,
            &project_requirements,
            lock_payload.clone(),
        )?;
        prewarm_rust_inspect_workspace(&rust_inspect_manifest_dir, &metadata_query_paths)?;
        codegen.set_rust_inspect_manifest_dir(rust_inspect_manifest_dir);
    }
    generator.set_cargo_lock_payload(lock_payload);
    generator.set_cargo_policy_flags(cargo_command_flags(&cargo_policy, &cargo_features));
    generator.set_dependencies(resolved.dependencies);
    generator.set_dev_dependencies(resolved.dev_dependencies);

    if dep_modules.is_empty() {
        let rust_code = codegen
            .try_generate(&lib_module.ast)
            .map_err(|e| CliError::failure(format!("Code generation error: {e}")))?;
        generator
            .generate(&rust_code)
            .map_err(|e| CliError::failure(format!("Error generating project: {e}")))?;
    } else {
        let module_paths: Vec<Vec<String>> = dep_modules.iter().map(|module| module.path_segments.clone()).collect();
        let (main_code, rust_modules) = codegen
            .try_generate_multi_file_nested(&lib_module.ast, &module_paths)
            .map_err(|e| CliError::failure(format!("Code generation error: {e}")))?;
        generator
            .generate_nested(&main_code, &rust_modules)
            .map_err(|e| CliError::failure(format!("Error generating project: {e}")))?;
    }

    match generator.build() {
        Ok(result) => {
            if !result.success {
                if let Some(wrapped) = format_rust_extern_wrapped_diagnostics(&result.stderr, &rust_extern_contexts) {
                    return Err(CliError::failure(format!(
                        "Library build failed.\n\n{}\nRaw cargo/rustc output:\n{}",
                        wrapped.trim_end(),
                        result.stderr
                    )));
                }
                return Err(CliError::failure(format!("Library build failed:\n{}", result.stderr)));
            }
        }
        Err(err) => {
            return Err(CliError::failure(format!("Error running cargo: {err}")));
        }
    }

    library_manifest
        .write_to_path(&manifest_path)
        .map_err(|err| CliError::failure(format!("failed to write {}: {err}", manifest_path.display())))?;

    println!("✓ Library build successful!");
    println!("Generated Rust crate in: {}", out_dir.display());
    println!("Generated manifest: {}", manifest_path.display());

    Ok(ExitCode::SUCCESS)
}

fn package_desugarer_artifact(out_dir: &Path, artifact: Option<&PendingDesugarerArtifact>) -> CliResult<()> {
    let Some(artifact) = artifact else {
        return Ok(());
    };

    let destination = out_dir.join(&artifact.metadata.relative_path);
    let destination_parent = destination.parent().ok_or_else(|| {
        CliError::failure(format!(
            "invalid desugarer artifact destination path: {}",
            destination.display()
        ))
    })?;

    fs::create_dir_all(destination_parent).map_err(|err| {
        CliError::failure(format!(
            "failed to create desugarer artifact directory {}: {err}",
            destination_parent.display()
        ))
    })?;
    fs::copy(&artifact.source_path, &destination).map_err(|err| {
        CliError::failure(format!(
            "failed to package vocab desugarer artifact {} -> {}: {err}",
            artifact.source_path.display(),
            destination.display()
        ))
    })?;

    Ok(())
}

/// Build and run an Incan file.
pub fn run_file(
    file_path: &str,
    cargo_policy: CargoPolicy,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
    release: bool,
) -> CliResult<ExitCode> {
    let mut prepared = prepare_project(
        file_path,
        None,
        &cargo_policy,
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    )?;
    prepared.generator.set_run_profile(if release {
        RunProfile::Release
    } else {
        RunProfile::Debug
    });

    match prepared
        .generator
        .run_with_cwd(&prepared.project_root, prepared.project_changed)
    {
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
    use crate::frontend::lexer;
    use crate::frontend::parser;
    use crate::frontend::symbols::ResolvedType;
    use crate::lockfile::{IncanLock, compute_deps_fingerprint};
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

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn library_rust_abi_query_paths_include_rust_extern_backing_items() -> Result<(), Box<dyn std::error::Error>> {
        let source =
            "rust.module(\"incan_stdlib::num\")\n@rust.extern\npub def gcd_i64(a: int, b: int) -> int:\n  ...\n";
        let tokens = lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errs| format!("parse errors: {errs:?}"))?;
        let module = ParsedModule {
            name: "lib".to_string(),
            path_segments: vec!["lib".to_string()],
            file_path: PathBuf::from("src/lib.incn"),
            source: source.to_string(),
            ast,
        };

        let modules = vec![module];
        let contexts = collect_rust_extern_contexts(&modules);
        let paths = collect_library_rust_abi_query_paths(&modules, &contexts);

        assert!(
            paths.iter().any(|path| path == "incan_stdlib::num::gcd_i64"),
            "expected rust.extern backing item in ABI query paths, got: {paths:?}"
        );
        Ok(())
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

    #[test]
    fn resolve_library_reexports_success_with_alias() -> Result<(), Box<dyn std::error::Error>> {
        let source = "pub from widgets import Widget as PublicWidget\n";
        let tokens = lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;
        let ast = parser::parse_with_module_path(&tokens, Some("project/src/lib.incn"))
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let lib_module = ParsedModule {
            name: "main".to_string(),
            path_segments: vec!["main".to_string()],
            file_path: PathBuf::from("project/src/lib.incn"),
            source: source.to_string(),
            ast,
        };

        let widget_export = CheckedNamedExport {
            name: "Widget".to_string(),
            kind: CheckedExportKind::TypeAlias(crate::frontend::library_exports::CheckedTypeAliasExport {
                name: "Widget".to_string(),
                type_params: Vec::new(),
                target: ResolvedType::Named("Widget".to_string()),
            }),
        };
        let mut module_exports: HashMap<String, HashMap<String, CheckedNamedExport>> = HashMap::new();
        module_exports.insert(
            "widgets".to_string(),
            HashMap::from([(widget_export.name.clone(), widget_export)]),
        );

        let resolved = LibraryReexportResolver::new(&module_exports)
            .resolve(&lib_module)
            .map_err(|errs| format!("{errs:?}"))?;
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "PublicWidget");
        match &resolved[0].kind {
            CheckedExportKind::TypeAlias(alias) => assert_eq!(alias.name, "PublicWidget"),
            _ => panic!("expected type alias export"),
        }
        Ok(())
    }

    #[test]
    fn resolve_library_reexports_reports_missing_module() -> Result<(), Box<dyn std::error::Error>> {
        let source = "pub from widgets import Widget\n";
        let tokens = lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;
        let ast = parser::parse_with_module_path(&tokens, Some("project/src/lib.incn"))
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let lib_module = ParsedModule {
            name: "main".to_string(),
            path_segments: vec!["main".to_string()],
            file_path: PathBuf::from("project/src/lib.incn"),
            source: source.to_string(),
            ast,
        };

        let module_exports: HashMap<String, HashMap<String, CheckedNamedExport>> = HashMap::new();
        let result = LibraryReexportResolver::new(&module_exports).resolve(&lib_module);
        assert!(result.is_err(), "expected missing module to fail");
        Ok(())
    }

    #[test]
    fn resolve_library_reexports_reports_duplicates() -> Result<(), Box<dyn std::error::Error>> {
        let source = "pub from widgets import Widget\npub from widgets import Widget\n";
        let tokens = lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;
        let ast = parser::parse_with_module_path(&tokens, Some("project/src/lib.incn"))
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let lib_module = ParsedModule {
            name: "main".to_string(),
            path_segments: vec!["main".to_string()],
            file_path: PathBuf::from("project/src/lib.incn"),
            source: source.to_string(),
            ast,
        };

        let widget_export = CheckedNamedExport {
            name: "Widget".to_string(),
            kind: CheckedExportKind::TypeAlias(crate::frontend::library_exports::CheckedTypeAliasExport {
                name: "Widget".to_string(),
                type_params: Vec::new(),
                target: ResolvedType::Named("Widget".to_string()),
            }),
        };
        let mut module_exports: HashMap<String, HashMap<String, CheckedNamedExport>> = HashMap::new();
        module_exports.insert(
            "widgets".to_string(),
            HashMap::from([(widget_export.name.clone(), widget_export)]),
        );

        let result = LibraryReexportResolver::new(&module_exports).resolve(&lib_module);
        assert!(result.is_err(), "expected duplicate export to fail");
        Ok(())
    }

    #[test]
    fn resolve_library_reexports_accepts_directory_entrypoint_spelling() -> Result<(), Box<dyn std::error::Error>> {
        let source = "pub from dataset.mod import DataSet\npub from dataset.ops import filter_ds\n";
        let tokens = lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;
        let ast = parser::parse_with_module_path(&tokens, Some("project/src/lib.incn"))
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let lib_module = ParsedModule {
            name: "main".to_string(),
            path_segments: vec!["main".to_string()],
            file_path: PathBuf::from("project/src/lib.incn"),
            source: source.to_string(),
            ast,
        };

        let dataset_export = CheckedNamedExport {
            name: "DataSet".to_string(),
            kind: CheckedExportKind::TypeAlias(crate::frontend::library_exports::CheckedTypeAliasExport {
                name: "DataSet".to_string(),
                type_params: Vec::new(),
                target: ResolvedType::Named("DataSet".to_string()),
            }),
        };
        let filter_export = CheckedNamedExport {
            name: "filter_ds".to_string(),
            kind: CheckedExportKind::Function(crate::frontend::library_exports::CheckedFunctionExport {
                name: "filter_ds".to_string(),
                type_params: Vec::new(),
                params: Vec::new(),
                return_type: ResolvedType::Named("DataSet".to_string()),
                is_async: false,
            }),
        };
        let mut module_exports: HashMap<String, HashMap<String, CheckedNamedExport>> = HashMap::new();
        module_exports.insert(
            "dataset".to_string(),
            HashMap::from([(dataset_export.name.clone(), dataset_export)]),
        );
        module_exports.insert(
            "dataset_ops".to_string(),
            HashMap::from([(filter_export.name.clone(), filter_export)]),
        );

        let resolved = LibraryReexportResolver::new(&module_exports)
            .resolve(&lib_module)
            .map_err(|errs| format!("{errs:?}"))?;
        assert_eq!(resolved.len(), 2);
        assert!(resolved.iter().any(|export| export.name == "DataSet"));
        assert!(resolved.iter().any(|export| export.name == "filter_ds"));

        Ok(())
    }

    #[test]
    fn resolve_library_reexports_accepts_canonical_nested_module_spelling() -> Result<(), Box<dyn std::error::Error>> {
        let source = "pub from dataset import DataSet\npub from dataset.ops import filter_ds\n";
        let tokens = lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;
        let ast = parser::parse_with_module_path(&tokens, Some("project/src/lib.incn"))
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let lib_module = ParsedModule {
            name: "main".to_string(),
            path_segments: vec!["main".to_string()],
            file_path: PathBuf::from("project/src/lib.incn"),
            source: source.to_string(),
            ast,
        };

        let dataset_export = CheckedNamedExport {
            name: "DataSet".to_string(),
            kind: CheckedExportKind::TypeAlias(crate::frontend::library_exports::CheckedTypeAliasExport {
                name: "DataSet".to_string(),
                type_params: Vec::new(),
                target: ResolvedType::Named("DataSet".to_string()),
            }),
        };
        let filter_export = CheckedNamedExport {
            name: "filter_ds".to_string(),
            kind: CheckedExportKind::Function(crate::frontend::library_exports::CheckedFunctionExport {
                name: "filter_ds".to_string(),
                type_params: Vec::new(),
                params: Vec::new(),
                return_type: ResolvedType::Named("DataSet".to_string()),
                is_async: false,
            }),
        };
        let mut module_exports: HashMap<String, HashMap<String, CheckedNamedExport>> = HashMap::new();
        module_exports.insert(
            "dataset".to_string(),
            HashMap::from([(dataset_export.name.clone(), dataset_export)]),
        );
        module_exports.insert(
            "dataset_ops".to_string(),
            HashMap::from([(filter_export.name.clone(), filter_export)]),
        );

        let resolved = LibraryReexportResolver::new(&module_exports)
            .resolve(&lib_module)
            .map_err(|errs| format!("{errs:?}"))?;
        assert_eq!(resolved.len(), 2);
        assert!(resolved.iter().any(|export| export.name == "DataSet"));
        assert!(resolved.iter().any(|export| export.name == "filter_ds"));

        Ok(())
    }

    #[test]
    fn build_library_accepts_nested_directory_modules() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(src_dir.join("dataset"))?;

        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"nestedlib\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            src_dir.join("lib.incn"),
            "pub from dataset.mod import DataSet\npub from dataset.ops import filter_ds\n",
        )?;
        std::fs::write(
            src_dir.join("dataset").join("mod.incn"),
            "pub trait DataSet[T]:\n    pass\n",
        )?;
        std::fs::write(
            src_dir.join("dataset").join("ops.incn"),
            "from dataset.mod import DataSet\npub def filter_ds[T](ds: DataSet[T]) -> DataSet[T]:\n    return ds\n",
        )?;

        let cargo_lock_payload = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock"))?;
        let fingerprint = compute_deps_fingerprint(&[], &[], &CargoFeatureSelection::default(), Some(project_root));
        let incan_lock = IncanLock::new(fingerprint, CargoFeatureSelection::default(), cargo_lock_payload);
        incan_lock.write(&project_root.join("incan.lock"))?;

        let lib_path = src_dir.join("lib.incn");
        let lib_path_str = lib_path
            .to_str()
            .ok_or("lib path should be valid utf-8 for build_library test")?;
        let exit = build_library(
            Some(lib_path_str),
            None,
            CargoPolicy::default(),
            Vec::new(),
            false,
            false,
        )?;
        assert_eq!(exit, ExitCode::SUCCESS);

        let generated_lib = project_root.join("target").join("lib").join("src").join("lib.rs");
        let generated_dataset = project_root
            .join("target")
            .join("lib")
            .join("src")
            .join("dataset")
            .join("mod.rs");
        let generated_flat_dataset = project_root.join("target").join("lib").join("src").join("dataset.rs");

        let generated_lib_source = std::fs::read_to_string(&generated_lib)?;
        let generated_dataset_source = std::fs::read_to_string(&generated_dataset)?;

        assert!(
            !generated_lib_source.contains("crate::dataset::r#mod"),
            "generated lib.rs should not reference crate::dataset::r#mod"
        );
        assert!(
            !generated_dataset_source.contains("crate::dataset::r#mod"),
            "generated dataset/mod.rs should not reference crate::dataset::r#mod"
        );
        assert!(
            !generated_flat_dataset.exists(),
            "stale flat dataset.rs should not exist after nested library build"
        );

        Ok(())
    }

    #[test]
    fn build_library_accepts_canonical_nested_module_imports() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(src_dir.join("dataset"))?;

        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"nestedlib\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            src_dir.join("lib.incn"),
            "pub from dataset import DataSet\npub from dataset.ops import filter_ds\n",
        )?;
        std::fs::write(
            src_dir.join("dataset").join("mod.incn"),
            "pub trait DataSet[T]:\n    pass\n",
        )?;
        std::fs::write(
            src_dir.join("dataset").join("ops.incn"),
            "from dataset import DataSet\npub def filter_ds[T](ds: DataSet[T]) -> DataSet[T]:\n    return ds\n",
        )?;

        let cargo_lock_payload = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock"))?;
        let fingerprint = compute_deps_fingerprint(&[], &[], &CargoFeatureSelection::default(), Some(project_root));
        let incan_lock = IncanLock::new(fingerprint, CargoFeatureSelection::default(), cargo_lock_payload);
        incan_lock.write(&project_root.join("incan.lock"))?;

        let lib_path = src_dir.join("lib.incn");
        let lib_path_str = lib_path
            .to_str()
            .ok_or("lib path should be valid utf-8 for build_library test")?;
        let exit = build_library(
            Some(lib_path_str),
            None,
            CargoPolicy::default(),
            Vec::new(),
            false,
            false,
        )?;
        assert_eq!(exit, ExitCode::SUCCESS);

        let generated_lib = project_root.join("target").join("lib").join("src").join("lib.rs");
        let generated_dataset = project_root
            .join("target")
            .join("lib")
            .join("src")
            .join("dataset")
            .join("mod.rs");

        let generated_lib_source = std::fs::read_to_string(&generated_lib)?;
        let generated_dataset_source = std::fs::read_to_string(&generated_dataset)?;

        assert!(
            !generated_lib_source.contains("crate::dataset::r#mod"),
            "generated lib.rs should not reference crate::dataset::r#mod"
        );
        assert!(
            !generated_dataset_source.contains("crate::dataset::r#mod"),
            "generated dataset/mod.rs should not reference crate::dataset::r#mod"
        );

        Ok(())
    }
}

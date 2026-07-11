//! Build and run pipeline for Incan projects.
//!
//! This module handles the full compilation flow: module collection, type checking, codegen configuration, dependency
//! resolution, project generation, and Cargo build/run.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::backend::project::generator::GENERATED_CARGO_TARGET_DIR_ENV;
use crate::backend::{IrCodegen, ProjectGenerator, RunProfile};
use crate::cli::{CliError, CliResult, ExitCode};
use crate::dependency_resolver::{ResolvedDependencies, resolve_dependencies, resolve_reachable_dependencies};
use crate::frontend::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, CheckedApiPackageIdentity,
    collect_checked_api_metadata, materialize_api_alias_projections, validate_checked_api_docstrings,
};
use crate::frontend::ast::{Declaration, Decorator, ImportKind, Span, Spanned};
use crate::frontend::contract_metadata::{ContractMetadataPackage, read_project_model_bundles};
use crate::frontend::library_exports::{CheckedExportKind, CheckedNamedExport, collect_checked_public_exports};
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::module::canonicalize_source_module_segments;
use crate::frontend::typechecker::stdlib_loader::StdlibAstCache;
use crate::frontend::{diagnostics, typechecker};
use crate::library_manifest::LibraryManifest;
#[cfg(feature = "rust_inspect")]
use crate::library_manifest::LibraryRustAbi;
use crate::lockfile::CargoFeatureSelection;
use crate::manifest::ProjectManifest;

use super::build_report::{
    BuildReportDraft, BuildReportMode, BuildReportOptions, BuildReportProject, RustInspectionFormat, SourceFileReport,
    artifact_report, cargo_report, dependencies_report, emit_build_report, emit_rust_inspection_report,
    generated_project_report, incan_dependencies_report, interop_report, rust_inspection_report,
};
#[cfg(feature = "rust_inspect")]
use super::common::collect_rust_inspect_query_paths;
use super::common::{
    CargoPolicy, INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV, ProjectRequirements, build_source_map, cargo_command_flags,
    collect_modules, collect_project_requirements, collect_rust_dependency_uses, enforce_project_toolchain_constraint,
    format_dependency_error, imported_module_deps_for_with_index, merge_project_requirement_dependencies,
    module_key_index, resolve_project_root, typecheck_modules_with_import_graph, validate_output_dir,
};
use super::lock::{
    GeneratedLibraryDependencyPreheatRequest, LockResolutionRequest, resolve_lock_payload,
    run_generated_library_dependency_preheat,
};
#[cfg(feature = "rust_inspect")]
use super::lock::{RustInspectWorkspaceRequest, prepare_rust_inspect_workspace};
use super::vocab_extraction::{PendingDesugarerArtifact, collect_library_vocab_metadata};
use crate::cli::prelude::ParsedModule;
#[cfg(feature = "rust_inspect")]
use crate::rust_inspect::{InspectError, Inspector, InspectorConfig};
use incan_core::lang::stdlib;
use sha2::{Digest as _, Sha256};

// ============================================================================
// Project Preparation (shared between build and run)
// ============================================================================

const INLINE_COMMAND_PROJECT_PREFIX: &str = "incan_inline_command";
const INLINE_COMMAND_OUTPUT_PARENT: &str = "target/incan/inline";

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
    /// Machine-readable build report data collected before Cargo is invoked.
    report: BuildReportDraft,
}

#[derive(Debug, Clone, Default)]
pub struct BuildCommandOptions {
    pub cargo_policy: CargoPolicy,
    pub cargo_features: Vec<String>,
    pub cargo_no_default_features: bool,
    pub cargo_all_features: bool,
    pub generated_cargo_target_dir: Option<PathBuf>,
}

impl BuildCommandOptions {
    /// Return the explicit generated Cargo target directory, falling back to the legacy environment default.
    fn effective_generated_cargo_target_dir(&self) -> Option<PathBuf> {
        self.generated_cargo_target_dir.clone().or_else(|| {
            env::var_os(GENERATED_CARGO_TARGET_DIR_ENV)
                .filter(|raw| !raw.is_empty())
                .map(PathBuf::from)
        })
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct PrepareProjectOptions<'a> {
    output_dir: Option<&'a str>,
    project_name_override: Option<&'a str>,
    generated_cargo_target_dir: Option<&'a Path>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InlineCommandProject {
    source_path: PathBuf,
    project_name: String,
    output_dir: String,
}

/// A prepared library project after Incan validation and Rust source generation, before Cargo build.
struct PreparedLibraryProject {
    generator: ProjectGenerator,
    project_root: PathBuf,
    project_name: String,
    rust_edition: Option<String>,
    out_dir: PathBuf,
    manifest_path: PathBuf,
    library_manifest: LibraryManifest,
    resolved_dependencies: ResolvedDependencies,
    project_requirements: ProjectRequirements,
    lock_payload: Option<String>,
    cargo_policy: CargoPolicy,
    cargo_features: CargoFeatureSelection,
    rust_extern_contexts: Vec<RustExternDeclContext>,
    should_preheat_library_dependencies: bool,
    timings_ms: BTreeMap<String, u64>,
    report: BuildReportDraft,
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

/// Return stable `rust.module::item` labels for Rust extern declarations that influenced this generated build.
fn rust_extern_report_paths(contexts: &[RustExternDeclContext]) -> Vec<String> {
    let mut paths = contexts
        .iter()
        .map(|context| format!("{}::{}", context.rust_module_path, context.item_name))
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

/// Build the project identity block used by build and generated Rust inspection reports.
fn manifest_project_report(
    manifest: Option<&ProjectManifest>,
    project_name: &str,
    project_root: &Path,
) -> BuildReportProject {
    BuildReportProject {
        name: project_name.to_string(),
        version: manifest.and_then(|manifest| manifest.project.as_ref().and_then(|project| project.version.clone())),
        project_root: project_root.to_string_lossy().to_string(),
    }
}

/// Convert collected Incan modules into source breadcrumbs for machine-readable reports.
fn source_file_report(modules: &[ParsedModule]) -> Vec<SourceFileReport> {
    modules
        .iter()
        .map(|module| SourceFileReport {
            path: module.file_path.to_string_lossy().to_string(),
            module_path: module.path_segments.clone(),
        })
        .collect()
}

/// Return elapsed milliseconds as a bounded `u64` for report payloads.
fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

/// Record one named build phase timing.
fn record_timing(timings: &mut BTreeMap<String, u64>, name: &str, start: Instant) {
    timings.insert(name.to_string(), elapsed_ms(start));
}

/// Print human build progress to stderr when stdout is reserved for a machine-readable report.
fn print_build_progress(report_options: &BuildReportOptions, message: impl AsRef<str>) {
    if report_options.enabled() {
        eprintln!("{}", message.as_ref());
    } else {
        println!("{}", message.as_ref());
    }
}

/// Return the stable cache key used for one wrapped inline command source from one working directory.
fn inline_command_cache_key(cwd: &Path, wrapped_source: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(cwd.to_string_lossy().as_bytes());
    hasher.update(b"\0");
    hasher.update(wrapped_source.as_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..8])
}

/// Return the stable generated project identity used for one `incan run -c` source.
fn inline_command_project_for_cwd(cwd: &Path, wrapped_source: &str) -> InlineCommandProject {
    let digest = inline_command_cache_key(cwd, wrapped_source);
    let project_name = format!("{INLINE_COMMAND_PROJECT_PREFIX}_{digest}");
    let source_path = env::temp_dir().join(&project_name).join("main.incn");
    let output_dir = format!("{INLINE_COMMAND_OUTPUT_PARENT}/{project_name}");
    InlineCommandProject {
        source_path,
        project_name,
        output_dir,
    }
}

/// Resolve the current invocation's stable inline-command generated project identity.
fn inline_command_project(wrapped_source: &str) -> CliResult<InlineCommandProject> {
    let cwd = env::current_dir().map_err(|err| {
        CliError::failure(format!(
            "failed to determine current directory for inline command cache: {err}"
        ))
    })?;
    Ok(inline_command_project_for_cwd(&cwd, wrapped_source))
}

/// Preserve the legacy `run -c` behavior by adding a no-op `main` only when the snippet did not define one.
fn wrap_inline_command_source(source: &str) -> String {
    if source.contains("def main") {
        source.to_string()
    } else {
        format!("{source}\n\ndef main() -> Unit:\n  pass\n")
    }
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

/// Resolve the project root for library commands from an optional source path or project directory.
fn resolve_library_project_root(file_path: Option<&str>) -> CliResult<PathBuf> {
    if let Some(file_path) = file_path {
        let normalized = if Path::new(file_path).is_absolute() {
            PathBuf::from(file_path)
        } else {
            env::current_dir()
                .map_err(|e| CliError::failure(format!("failed to determine current directory: {e}")))?
                .join(file_path)
        };
        if normalized.is_dir() {
            return Ok(normalized);
        }
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

/// Group checked exports by public source name while preserving same-name function overload entries.
fn checked_exports_by_name(exports: Vec<CheckedNamedExport>) -> HashMap<String, Vec<CheckedNamedExport>> {
    let mut grouped: HashMap<String, Vec<CheckedNamedExport>> = HashMap::new();
    for export in exports {
        grouped.entry(export.name.clone()).or_default().push(export);
    }
    grouped
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
    module_exports: &'a HashMap<String, HashMap<String, Vec<CheckedNamedExport>>>,
}

impl<'a> LibraryReexportResolver<'a> {
    /// Create a resolver over checked exports grouped by canonical source-module name and source export name.
    fn new(module_exports: &'a HashMap<String, HashMap<String, Vec<CheckedNamedExport>>>) -> Self {
        Self { module_exports }
    }

    /// Resolve direct public declarations and `pub from ... import ...` declarations in a library entrypoint into
    /// checked public exports.
    ///
    /// A single source name can map to several checked exports when the provider exposes same-name overloads. The
    /// resolver therefore preserves all matching exports and only applies the consumer-facing alias to each one.
    fn resolve(
        &self,
        lib_module: &ParsedModule,
    ) -> Result<Vec<CheckedNamedExport>, Vec<crate::frontend::diagnostics::CompileError>> {
        let mut errors = Vec::new();
        let mut resolved = Vec::new();
        let mut exported_names: HashSet<String> = HashSet::new();
        let known_modules: Vec<String> = self.module_exports.keys().cloned().collect();

        if let Some(exports_by_name) = self.module_exports.get(&module_key(&lib_module.path_segments)) {
            for (export_name, export_span) in Self::direct_public_exports(lib_module) {
                if !exported_names.insert(export_name.clone()) {
                    errors.push(diagnostics::errors::duplicate_library_export(&export_name, export_span));
                    continue;
                }
                if let Some(exports) = exports_by_name.get(&export_name) {
                    resolved.extend(exports.iter().cloned());
                }
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

                let Some(exports) = exports_by_name.get(&item.name) else {
                    let available: Vec<String> = exports_by_name.keys().cloned().collect();
                    errors.push(diagnostics::errors::import_not_exported(
                        &item.name,
                        &module.to_rust_path(),
                        &available,
                        decl.span,
                    ));
                    continue;
                };

                resolved.extend(
                    exports
                        .iter()
                        .map(|export| rename_checked_export(export, &exported_name)),
                );
            }
        }

        if errors.is_empty() { Ok(resolved) } else { Err(errors) }
    }

    /// Return public names declared directly by the library entrypoint, excluding public imports that are resolved from
    /// their source module below.
    fn direct_public_exports(lib_module: &ParsedModule) -> Vec<(String, crate::frontend::ast::Span)> {
        lib_module
            .ast
            .declarations
            .iter()
            .filter_map(|decl| match &decl.node {
                Declaration::Function(function)
                    if matches!(function.visibility, crate::frontend::ast::Visibility::Public) =>
                {
                    Some((function.name.clone(), decl.span))
                }
                Declaration::Model(model) if matches!(model.visibility, crate::frontend::ast::Visibility::Public) => {
                    Some((model.name.clone(), decl.span))
                }
                Declaration::Class(class) if matches!(class.visibility, crate::frontend::ast::Visibility::Public) => {
                    Some((class.name.clone(), decl.span))
                }
                Declaration::Trait(trait_decl)
                    if matches!(trait_decl.visibility, crate::frontend::ast::Visibility::Public) =>
                {
                    Some((trait_decl.name.clone(), decl.span))
                }
                Declaration::Enum(enum_decl)
                    if matches!(enum_decl.visibility, crate::frontend::ast::Visibility::Public) =>
                {
                    Some((enum_decl.name.clone(), decl.span))
                }
                Declaration::Newtype(newtype_decl)
                    if matches!(newtype_decl.visibility, crate::frontend::ast::Visibility::Public) =>
                {
                    Some((newtype_decl.name.clone(), decl.span))
                }
                Declaration::TypeAlias(alias)
                    if matches!(alias.visibility, crate::frontend::ast::Visibility::Public) =>
                {
                    Some((alias.name.clone(), decl.span))
                }
                Declaration::Const(konst) if matches!(konst.visibility, crate::frontend::ast::Visibility::Public) => {
                    Some((konst.name.clone(), decl.span))
                }
                Declaration::Static(static_decl)
                    if matches!(static_decl.visibility, crate::frontend::ast::Visibility::Public) =>
                {
                    Some((static_decl.name.clone(), decl.span))
                }
                Declaration::Alias(alias) if matches!(alias.visibility, crate::frontend::ast::Visibility::Public) => {
                    Some((alias.name.clone(), decl.span))
                }
                Declaration::Partial(partial)
                    if matches!(partial.visibility, crate::frontend::ast::Visibility::Public) =>
                {
                    Some((partial.name.clone(), decl.span))
                }
                _ => None,
            })
            .collect()
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
    prepare_project_with_options(
        file_path,
        PrepareProjectOptions {
            output_dir,
            project_name_override: None,
            generated_cargo_target_dir: None,
        },
        cargo_policy,
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    )
}

/// Prepare an executable project with optional internal identity overrides for callers that need bounded cache names.
fn prepare_project_with_options(
    file_path: &str,
    options: PrepareProjectOptions<'_>,
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
    // Typechecking still consumes the source-backed stdlib module graph during the compatibility transition, but
    // migrated modules are supplied by the compiled built-in artifact at Rust emission time. Keeping this split here
    // prevents generated consumers from silently materializing a second `__incan_std` implementation. Remove this
    // bridge once the incnlib manifest carries the module-qualified type, trait, decorator, and default-argument
    // metadata needed by the frontend's stdlib import resolver.
    let emitted_dep_modules: Vec<&ParsedModule> = dep_modules
        .iter()
        .filter(|module| !stdlib::is_compiled_builtin_stdlib_emission_path(&module.path_segments))
        .collect();
    let project_root = manifest
        .as_ref()
        .map(|manifest| manifest.project_root().to_path_buf())
        .unwrap_or(inferred_project_root);
    let library_manifest_index = manifest
        .as_ref()
        .map(LibraryManifestIndex::from_project_manifest)
        .unwrap_or_default();
    let project_requirements = collect_project_requirements(&modules, &library_manifest_index)?;

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
    if let Some(m) = manifest.as_ref() {
        codegen.set_declared_crate_names(m.declared_rust_crate_names());
    }
    codegen.set_library_manifest_index(library_manifest_index.clone());
    // Add user dependency modules
    for module in &emitted_dep_modules {
        codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
    }
    // ---- Setup project generator ----
    let mut generator = ProjectGenerator::new(&out_dir, project_name.as_str(), true);
    generator.set_cargo_target_dir_override(options.generated_cargo_target_dir.map(Path::to_path_buf));
    generator.set_stdlib_features(project_requirements.stdlib_features.clone());
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
    let rust_inspect_manifest_dir = {
        let rust_inspect_manifest_dir = prepare_rust_inspect_workspace(RustInspectWorkspaceRequest {
            project_root: &project_root,
            project_name: project_name.as_str(),
            rust_edition: manifest
                .as_ref()
                .and_then(|m| m.build.as_ref().and_then(|b| b.rust_edition.clone())),
            resolved: &resolved,
            project_requirements: &project_requirements,
            lock_payload: lock_payload.clone(),
            rust_inspect_query_paths: &metadata_query_paths,
            prepare_when_empty: true,
        })?
        .ok_or_else(|| CliError::failure("rust-inspect workspace preparation did not return a manifest directory"))?;
        codegen.set_rust_inspect_manifest_dir(rust_inspect_manifest_dir.clone());
        Some(rust_inspect_manifest_dir)
    };

    // Type check all modules (dependencies + stdlib first), so diagnostics are associated with the correct file.
    //
    // This must run after rust-inspect preparation. Direct Rust calls expose their callable signatures through the
    // prepared metadata workspace; checking before that step degrades those calls to `Unknown` and breaks source-level
    // constructs such as `?` on Rust `Result<T, E>` returns.
    typecheck_modules_with_import_graph(
        &modules,
        manifest.as_ref(),
        &library_manifest_index,
        #[cfg(feature = "rust_inspect")]
        rust_inspect_manifest_dir.as_deref(),
    )?;
    generator.set_cargo_lock_payload(lock_payload);

    let cargo_flags = cargo_command_flags(cargo_policy, &cargo_features);
    generator.set_cargo_policy_flags(cargo_flags);

    let rust_dependencies = resolved.dependencies.clone();
    let rust_dev_dependencies = resolved.dev_dependencies.clone();
    let incan_dependencies = manifest
        .as_ref()
        .map(|manifest| incan_dependencies_report(manifest.library_dependencies().iter().collect()))
        .unwrap_or_default();
    let report = BuildReportDraft {
        mode: BuildReportMode::Executable,
        profile: "release".to_string(),
        project: manifest_project_report(manifest.as_ref(), project_name.as_str(), &project_root),
        entrypoint: Some(normalized_file_path_str.clone()),
        library_root: None,
        source_files: source_file_report(&modules),
        generated: generated_project_report(
            generator.output_dir(),
            &generator.crate_root_path(),
            &generator.cargo_target_dir(),
        ),
        artifacts: Vec::new(),
        dependencies: dependencies_report(
            &rust_dependencies,
            &rust_dev_dependencies,
            incan_dependencies,
            project_requirements.stdlib_features.clone(),
        ),
        cargo: cargo_report(
            cargo_policy,
            cargo_features.cargo_features.clone(),
            cargo_features.cargo_no_default_features,
            cargo_features.cargo_all_features,
        ),
        interop: interop_report(
            &inline_imports,
            rust_extern_report_paths(&rust_extern_contexts),
            metadata_query_paths.clone(),
        ),
        notes: vec![
            "Generated Rust is current backend output for inspection and debugging, not a stable Rust ABI.".to_string(),
        ],
    };

    generator.set_dependencies(resolved.dependencies);
    generator.set_dev_dependencies(resolved.dev_dependencies);

    // ---- Generate Rust project files ----
    let has_deps = !emitted_dep_modules.is_empty()
        || dep_modules
            .iter()
            .any(|module| stdlib::is_compiled_builtin_stdlib_emission_path(&module.path_segments));
    let project_changed = if has_deps {
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

    Ok(PreparedProject {
        generator,
        project_changed,
        out_dir,
        project_root,
        rust_extern_contexts,
        report,
    })
}

/// Build an Incan file to a Rust project.
pub fn build_file(
    file_path: &str,
    output_dir: Option<&String>,
    options: BuildCommandOptions,
    report_options: BuildReportOptions,
) -> CliResult<ExitCode> {
    let total_start = Instant::now();
    let prepare_start = Instant::now();
    let generated_cargo_target_dir = options.effective_generated_cargo_target_dir();
    let prepared = prepare_project_with_options(
        file_path,
        PrepareProjectOptions {
            output_dir: output_dir.map(|s| s.as_str()),
            project_name_override: None,
            generated_cargo_target_dir: generated_cargo_target_dir.as_deref(),
        },
        &options.cargo_policy,
        options.cargo_features,
        options.cargo_no_default_features,
        options.cargo_all_features,
    )?;
    let prepare_ms = elapsed_ms(prepare_start);

    print_build_progress(
        &report_options,
        format!("Generated Rust project in: {}", prepared.out_dir),
    );
    print_build_progress(&report_options, "Building...");

    let cargo_start = Instant::now();
    match prepared.generator.build() {
        Ok(result) => {
            let cargo_build_ms = elapsed_ms(cargo_start);
            if result.success {
                print_build_progress(&report_options, "✓ Build successful!");
                print_build_progress(
                    &report_options,
                    format!("Binary: {}", prepared.generator.binary_path().display()),
                );
                let mut report_draft = prepared.report.clone();
                report_draft
                    .artifacts
                    .push(artifact_report("binary", &prepared.generator.binary_path()));
                let report = report_draft.finish(BTreeMap::from([
                    ("prepare".to_string(), prepare_ms),
                    ("cargo_build".to_string(), cargo_build_ms),
                    ("total".to_string(), elapsed_ms(total_start)),
                ]));
                emit_build_report(&report, &report_options)?;
                Ok(ExitCode::SUCCESS)
            } else {
                if let Some(wrapped) =
                    format_rust_extern_wrapped_diagnostics(&result.stderr, &prepared.rust_extern_contexts)
                {
                    return Err(CliError::failure(format!(
                        "Build failed.\n\n{}\nRaw cargo/rustc output:\n{}",
                        wrapped.trim_end(),
                        result.stderr
                    )));
                }
                Err(CliError::failure(format!("Build failed:\n{}", result.stderr)))
            }
        }
        Err(e) => Err(CliError::failure(format!("Error running cargo: {}", e))),
    }
}

/// Validate a library project and generate its Rust project without running Cargo.
fn prepare_library_project(
    file_path: Option<&str>,
    cargo_policy: CargoPolicy,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
    generated_cargo_target_dir: Option<&Path>,
) -> CliResult<PreparedLibraryProject> {
    let prepare_start = Instant::now();
    let mut timings_ms = BTreeMap::new();
    let source_load_start = Instant::now();
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
    record_timing(&mut timings_ms, "library_load_sources", source_load_start);

    let requirements_start = Instant::now();
    let declared = manifest.declared_rust_crate_names();
    let library_manifest_index = LibraryManifestIndex::from_project_manifest(&manifest);
    let project_requirements = collect_project_requirements(&modules, &library_manifest_index)?;
    let contract_model_bundles = read_project_model_bundles(&project_root, &manifest.contract_model_bundle_paths())
        .map_err(|error| CliError::failure(error.to_string()))?;
    let rust_extern_contexts = collect_rust_extern_contexts(&modules);
    let dep_modules = &modules[..modules.len() - 1];

    let mut inline_imports = collect_rust_dependency_uses(lib_module, false);
    for module in dep_modules {
        inline_imports.extend(collect_rust_dependency_uses(module, false));
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
    record_timing(&mut timings_ms, "library_collect_requirements", requirements_start);

    let dependency_start = Instant::now();
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
    record_timing(&mut timings_ms, "library_resolve_dependencies", dependency_start);
    #[cfg(feature = "rust_inspect")]
    let metadata_query_paths = collect_library_rust_abi_query_paths(&modules, &rust_extern_contexts);
    #[cfg(not(feature = "rust_inspect"))]
    let metadata_query_paths: Vec<String> = Vec::new();

    let lock_start = Instant::now();
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
    record_timing(&mut timings_ms, "library_resolve_lock_payload", lock_start);
    let should_preheat_library_dependencies = lock_payload_for_typecheck.is_some()
        && (!resolved.dependencies.is_empty() || !project_requirements.stdlib_features.is_empty());
    let lock_payload_for_preheat = lock_payload_for_typecheck.clone();
    #[cfg(feature = "rust_inspect")]
    let rust_inspect_manifest_dir = {
        let rust_inspect_start = Instant::now();
        let rust_inspect_manifest_dir = prepare_rust_inspect_workspace(RustInspectWorkspaceRequest {
            project_root: &project_root,
            project_name: project_name.as_str(),
            rust_edition: manifest.build.as_ref().and_then(|build| build.rust_edition.clone()),
            resolved: &resolved,
            project_requirements: &project_requirements,
            lock_payload: lock_payload_for_typecheck.clone(),
            rust_inspect_query_paths: &metadata_query_paths,
            prepare_when_empty: true,
        })?
        .ok_or_else(|| CliError::failure("rust-inspect workspace preparation did not return a manifest directory"))?;
        record_timing(&mut timings_ms, "library_rust_inspect_prewarm", rust_inspect_start);
        rust_inspect_manifest_dir
    };

    let typecheck_start = Instant::now();
    let mut all_errors = String::new();
    let mut checked_exports_by_module: HashMap<String, HashMap<String, Vec<CheckedNamedExport>>> = HashMap::new();
    let mut api_metadata_modules = Vec::new();
    let module_idx_by_key = module_key_index(&modules);
    let mut stdlib_cache = StdlibAstCache::new();

    for (idx, module) in modules.iter().enumerate() {
        let deps_for_module = imported_module_deps_for_with_index(&modules, idx, &module_idx_by_key);
        let mut checker = typechecker::TypeChecker::new();
        checker.stdlib_cache = stdlib_cache.clone();
        checker.set_current_module_path(Some(module.path_segments.clone()));
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
                    checked_exports_by_name(module_exports),
                );
                stdlib_cache = checker.stdlib_cache.clone();
            }
            Err(errs) => {
                stdlib_cache = checker.stdlib_cache.clone();
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
    record_timing(&mut timings_ms, "library_typecheck_modules", typecheck_start);

    let api_validation_start = Instant::now();
    materialize_api_alias_projections(&mut api_metadata_modules);

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
    record_timing(&mut timings_ms, "library_validate_api_metadata", api_validation_start);

    let export_start = Instant::now();
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
    record_timing(&mut timings_ms, "library_resolve_exports", export_start);

    let manifest_start = Instant::now();
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
    record_timing(&mut timings_ms, "library_build_manifest_metadata", manifest_start);
    let mut pending_desugarer_artifact: Option<PendingDesugarerArtifact> = None;

    let vocab_start = Instant::now();
    if let Some(vocab_extraction) =
        collect_library_vocab_metadata(&manifest, &project_root, generated_cargo_target_dir)?
    {
        pending_desugarer_artifact = vocab_extraction.pending_desugarer_artifact;
        library_manifest.vocab = Some(vocab_extraction.payload);
        library_manifest.soft_keywords.activations = vocab_extraction.compatibility_activations;
    }
    record_timing(&mut timings_ms, "library_collect_vocab_metadata", vocab_start);

    let out_dir = project_root.join("target").join("lib");
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| CliError::failure(format!("failed to create {}: {e}", out_dir.display())))?;
    package_desugarer_artifact(&out_dir, pending_desugarer_artifact.as_ref())?;
    let manifest_path = out_dir.join(format!("{project_name}.incnlib"));

    let mut codegen = IrCodegen::new();
    codegen.set_preserve_dependency_public_items(true);
    codegen.set_stdlib_cache(stdlib_cache);
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
    generator.set_cargo_target_dir_override(generated_cargo_target_dir.map(Path::to_path_buf));
    generator.set_stdlib_features(project_requirements.stdlib_features.clone());
    generator.set_include_dev_dependencies(false);
    let rust_edition = manifest.build.as_ref().and_then(|build| build.rust_edition.clone());
    generator.set_rust_edition(rust_edition.clone());
    #[cfg(feature = "rust_inspect")]
    codegen.set_rust_inspect_manifest_dir(rust_inspect_manifest_dir.clone());
    generator.set_cargo_lock_payload(lock_payload_for_typecheck);
    generator.set_cargo_policy_flags(cargo_command_flags(&cargo_policy, &cargo_features));
    let rust_dependencies = resolved.dependencies.clone();
    let rust_dev_dependencies = resolved.dev_dependencies.clone();
    let resolved_dependencies_for_preheat = resolved.clone();
    let project_requirements_for_preheat = project_requirements.clone();
    let report_draft = BuildReportDraft {
        mode: BuildReportMode::Library,
        profile: "release".to_string(),
        project: manifest_project_report(Some(&manifest), project_name.as_str(), &project_root),
        entrypoint: Some(lib_entry_str.clone()),
        library_root: Some(project_root.to_string_lossy().to_string()),
        source_files: source_file_report(&modules),
        generated: generated_project_report(
            generator.output_dir(),
            &generator.crate_root_path(),
            &generator.cargo_target_dir(),
        ),
        artifacts: Vec::new(),
        dependencies: dependencies_report(
            &rust_dependencies,
            &rust_dev_dependencies,
            incan_dependencies_report(manifest.library_dependencies().iter().collect()),
            project_requirements.stdlib_features.clone(),
        ),
        cargo: cargo_report(
            &cargo_policy,
            cargo_features.cargo_features.clone(),
            cargo_features.cargo_no_default_features,
            cargo_features.cargo_all_features,
        ),
        interop: interop_report(
            &inline_imports,
            rust_extern_report_paths(&rust_extern_contexts),
            metadata_query_paths.clone(),
        ),
        notes: vec![
            "Generated Rust is current backend output for inspection and debugging, not a stable Rust ABI.".to_string(),
        ],
    };
    generator.set_dependencies(resolved.dependencies);
    generator.set_dev_dependencies(resolved.dev_dependencies);

    let codegen_start = Instant::now();
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
    record_timing(&mut timings_ms, "library_generate_rust", codegen_start);
    record_timing(&mut timings_ms, "library_prepare_total", prepare_start);

    Ok(PreparedLibraryProject {
        generator,
        project_root,
        project_name,
        rust_edition,
        out_dir,
        manifest_path,
        library_manifest,
        resolved_dependencies: resolved_dependencies_for_preheat,
        project_requirements: project_requirements_for_preheat,
        lock_payload: lock_payload_for_preheat,
        cargo_policy,
        cargo_features,
        rust_extern_contexts,
        should_preheat_library_dependencies,
        timings_ms,
        report: report_draft,
    })
}

/// Write the `.incnlib` manifest and build-report artifact paths for a prepared library project.
fn write_library_manifest_artifacts(prepared: &mut PreparedLibraryProject) -> CliResult<()> {
    prepared
        .library_manifest
        .write_to_path(&prepared.manifest_path)
        .map_err(|err| CliError::failure(format!("failed to write {}: {err}", prepared.manifest_path.display())))?;

    prepared
        .report
        .artifacts
        .push(artifact_report("incan_library_manifest", &prepared.manifest_path));
    prepared.report.artifacts.push(artifact_report(
        "generated_cargo_manifest",
        &prepared.generator.cargo_manifest_path(),
    ));
    Ok(())
}

/// Validate RFC 031 library-mode preconditions.
pub fn build_library(
    file_path: Option<&str>,
    _output_dir: Option<&String>,
    options: BuildCommandOptions,
    report_options: BuildReportOptions,
) -> CliResult<ExitCode> {
    let total_start = Instant::now();
    let generated_cargo_target_dir = options.effective_generated_cargo_target_dir();
    let mut prepared = prepare_library_project(
        file_path,
        options.cargo_policy,
        options.cargo_features,
        options.cargo_no_default_features,
        options.cargo_all_features,
        generated_cargo_target_dir.as_deref(),
    )?;
    let artifact_only = env::var_os(INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV).is_some();

    if artifact_only {
        write_library_manifest_artifacts(&mut prepared)?;
        print_build_progress(&report_options, "✓ Library dependency artifact prepared!");
        print_build_progress(
            &report_options,
            format!("Generated manifest: {}", prepared.manifest_path.display()),
        );
        let mut timings_ms = prepared.timings_ms.clone();
        timings_ms.insert("total".to_string(), elapsed_ms(total_start));
        let report = prepared.report.finish(timings_ms);
        emit_build_report(&report, &report_options)?;
        return Ok(ExitCode::SUCCESS);
    }

    let preheat_start = Instant::now();
    if prepared.should_preheat_library_dependencies
        && let Some(lock_payload) = prepared.lock_payload.as_deref()
    {
        run_generated_library_dependency_preheat(GeneratedLibraryDependencyPreheatRequest {
            project_root: &prepared.project_root,
            lock_dir: &prepared.project_root.join("target").join("incan_lock"),
            project_name: &prepared.project_name,
            rust_edition: prepared.rust_edition.clone(),
            resolved: &prepared.resolved_dependencies,
            project_requirements: &prepared.project_requirements,
            cargo_features: &prepared.cargo_features,
            cargo_policy: &prepared.cargo_policy,
            target_dir: &prepared.generator.cargo_target_dir(),
            cargo_lock_payload: lock_payload,
        })?;
    }
    prepared
        .timings_ms
        .insert("library_dependency_preheat".to_string(), elapsed_ms(preheat_start));

    let cargo_start = Instant::now();
    let cargo_build_ms = match prepared.generator.build() {
        Ok(result) => {
            let cargo_build_ms = elapsed_ms(cargo_start);
            if !result.success {
                if let Some(wrapped) =
                    format_rust_extern_wrapped_diagnostics(&result.stderr, &prepared.rust_extern_contexts)
                {
                    return Err(CliError::failure(format!(
                        "Library build failed.\n\n{}\nRaw cargo/rustc output:\n{}",
                        wrapped.trim_end(),
                        result.stderr
                    )));
                }
                return Err(CliError::failure(format!("Library build failed:\n{}", result.stderr)));
            }
            cargo_build_ms
        }
        Err(err) => {
            return Err(CliError::failure(format!("Error running cargo: {err}")));
        }
    };

    write_library_manifest_artifacts(&mut prepared)?;

    print_build_progress(&report_options, "✓ Library build successful!");
    print_build_progress(
        &report_options,
        format!("Generated Rust crate in: {}", prepared.out_dir.display()),
    );
    print_build_progress(
        &report_options,
        format!("Generated manifest: {}", prepared.manifest_path.display()),
    );

    prepared.timings_ms.insert("cargo_build".to_string(), cargo_build_ms);
    prepared.timings_ms.insert("total".to_string(), elapsed_ms(total_start));
    let report = prepared.report.finish(prepared.timings_ms);
    emit_build_report(&report, &report_options)?;

    Ok(ExitCode::SUCCESS)
}

/// Generate and inspect the current Rust backend output without running Cargo.
pub fn inspect_rust(path: &Path, lib_mode: bool, format: RustInspectionFormat) -> CliResult<ExitCode> {
    let path_arg = path.to_string_lossy();
    let report = if lib_mode {
        let prepared = prepare_library_project(
            Some(path_arg.as_ref()),
            CargoPolicy::default(),
            Vec::new(),
            false,
            false,
            None,
        )?;
        rust_inspection_report(
            BuildReportMode::Library,
            prepared.report.generated,
            prepared.report.source_files,
            prepared.report.notes,
        )?
    } else {
        let prepared = prepare_project(
            path_arg.as_ref(),
            None,
            &CargoPolicy::default(),
            Vec::new(),
            false,
            false,
        )?;
        rust_inspection_report(
            BuildReportMode::Executable,
            prepared.report.generated,
            prepared.report.source_files,
            prepared.report.notes,
        )?
    };
    emit_rust_inspection_report(&report, format)?;
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
    let prepared = prepare_project(
        file_path,
        None,
        &cargo_policy,
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    )?;
    run_prepared_project(prepared, release)
}

/// Build and run inline Incan source from `incan run -c`.
pub fn run_inline_source(
    source: &str,
    cargo_policy: CargoPolicy,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
    release: bool,
) -> CliResult<ExitCode> {
    let wrapped_source = wrap_inline_command_source(source);
    let inline_project = inline_command_project(&wrapped_source)?;
    let source_path = inline_project.source_path;
    let source_parent = source_path.parent().ok_or_else(|| {
        CliError::failure(format!(
            "failed to determine temporary inline command directory for {}",
            source_path.display()
        ))
    })?;
    fs::create_dir_all(source_parent).map_err(|err| {
        CliError::failure(format!(
            "Error creating temporary inline command directory {}: {err}",
            source_parent.display()
        ))
    })?;
    fs::write(&source_path, wrapped_source).map_err(|err| {
        CliError::failure(format!(
            "Error writing temporary inline command file {}: {err}",
            source_path.display()
        ))
    })?;

    let source_arg = source_path.to_string_lossy().to_string();
    let result = prepare_project_with_options(
        &source_arg,
        PrepareProjectOptions {
            output_dir: Some(inline_project.output_dir.as_str()),
            project_name_override: Some(inline_project.project_name.as_str()),
            generated_cargo_target_dir: None,
        },
        &cargo_policy,
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    )
    .and_then(|prepared| run_prepared_project(prepared, release));
    let _ = fs::remove_file(&source_path);
    result
}

/// Run a prepared generated project with the same stdout, stderr and exit-code handling used by every `incan run` path.
fn run_prepared_project(mut prepared: PreparedProject, release: bool) -> CliResult<ExitCode> {
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
    use crate::frontend::library_exports::CheckedExportIdentity;
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

    #[test]
    fn inline_command_project_is_stable_for_same_source_and_working_directory() {
        let cwd = Path::new("/tmp/incan-inline-cache/project");
        let source = wrap_inline_command_source("println(\"ok\")");
        let first = inline_command_project_for_cwd(cwd, &source);
        let second = inline_command_project_for_cwd(cwd, &source);

        assert_eq!(first, second);
        assert_eq!(
            first.source_path.file_name().and_then(|name| name.to_str()),
            Some("main.incn")
        );
        let rendered = first.source_path.to_string_lossy();
        assert!(
            rendered.contains("incan_inline_command_"),
            "inline command temp source should use the stable inline-command prefix: {rendered}"
        );
        assert!(
            !rendered.contains("incan_cmd_"),
            "inline command temp source must not use timestamped incan_cmd names: {rendered}"
        );
        assert!(first.project_name.starts_with("incan_inline_command_"));
        assert!(
            first
                .output_dir
                .starts_with("target/incan/inline/incan_inline_command_")
        );
    }

    #[test]
    fn inline_command_project_is_partitioned_by_working_directory() {
        let source = wrap_inline_command_source("println(\"ok\")");
        let first = inline_command_project_for_cwd(Path::new("/tmp/incan-inline-cache/one"), &source);
        let second = inline_command_project_for_cwd(Path::new("/tmp/incan-inline-cache/two"), &source);

        assert_ne!(
            first, second,
            "different working directories should not race on one inline command temp source"
        );
    }

    #[test]
    fn inline_command_project_is_partitioned_by_source_content() {
        let cwd = Path::new("/tmp/incan-inline-cache/project");
        let first = inline_command_project_for_cwd(cwd, &wrap_inline_command_source("println(\"one\")"));
        let second = inline_command_project_for_cwd(cwd, &wrap_inline_command_source("println(\"two\")"));

        assert_ne!(
            first, second,
            "different inline snippets in the same working directory must not race on one generated cargo target"
        );
    }

    #[test]
    fn inline_command_uses_bounded_generated_project_prefixes() {
        assert_eq!(INLINE_COMMAND_PROJECT_PREFIX, "incan_inline_command");
        assert_eq!(INLINE_COMMAND_OUTPUT_PARENT, "target/incan/inline");
    }

    #[test]
    fn inline_command_source_wrapper_preserves_existing_main() {
        let source = "def main() -> None:\n    println(\"ok\")\n";

        assert_eq!(wrap_inline_command_source(source), source);
    }

    #[test]
    fn inline_command_source_wrapper_adds_stub_main_for_expression_snippets() {
        let wrapped = wrap_inline_command_source("println(\"ok\")");

        assert!(
            wrapped.contains("def main() -> Unit:\n  pass"),
            "inline snippets without a main should preserve existing run -c stub behavior: {wrapped}"
        );
    }

    #[test]
    fn run_entrypoint_omits_unused_manifest_rust_dependencies() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let scripts_dir = project_root.join("scripts");
        let declared_unused_rust_dependencies = ["itoa", "ryu"];
        std::fs::create_dir_all(&scripts_dir)?;
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"unused_rust_dep_run_repro\"\nversion = \"0.1.0\"\n\n[rust-dependencies]\nitoa = \"1\"\nryu = \"1\"\n",
        )?;
        std::fs::write(
            scripts_dir.join("check.incn"),
            "def main() -> None:\n    println(\"ok\")\n",
        )?;

        let cargo_lock_payload = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock"))?;
        let fingerprint = compute_deps_fingerprint(&[], &[], &CargoFeatureSelection::default(), Some(project_root));
        let incan_lock = IncanLock::new(fingerprint, CargoFeatureSelection::default(), cargo_lock_payload);
        incan_lock.write(&project_root.join("incan.lock"))?;

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
            Vec::new(),
            false,
            false,
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
            identity: CheckedExportIdentity::direct(vec!["widgets".to_string(), "Widget".to_string()]),
            kind: CheckedExportKind::TypeAlias(crate::frontend::library_exports::CheckedTypeAliasExport {
                name: "Widget".to_string(),
                type_params: Vec::new(),
                target: ResolvedType::Named("Widget".to_string()),
            }),
        };
        let mut module_exports: HashMap<String, HashMap<String, Vec<CheckedNamedExport>>> = HashMap::new();
        module_exports.insert(
            "widgets".to_string(),
            HashMap::from([(widget_export.name.clone(), vec![widget_export])]),
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

        let module_exports: HashMap<String, HashMap<String, Vec<CheckedNamedExport>>> = HashMap::new();
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
            identity: CheckedExportIdentity::direct(vec!["widgets".to_string(), "Widget".to_string()]),
            kind: CheckedExportKind::TypeAlias(crate::frontend::library_exports::CheckedTypeAliasExport {
                name: "Widget".to_string(),
                type_params: Vec::new(),
                target: ResolvedType::Named("Widget".to_string()),
            }),
        };
        let mut module_exports: HashMap<String, HashMap<String, Vec<CheckedNamedExport>>> = HashMap::new();
        module_exports.insert(
            "widgets".to_string(),
            HashMap::from([(widget_export.name.clone(), vec![widget_export])]),
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
            identity: CheckedExportIdentity::direct(vec!["dataset".to_string(), "DataSet".to_string()]),
            kind: CheckedExportKind::TypeAlias(crate::frontend::library_exports::CheckedTypeAliasExport {
                name: "DataSet".to_string(),
                type_params: Vec::new(),
                target: ResolvedType::Named("DataSet".to_string()),
            }),
        };
        let filter_export = CheckedNamedExport {
            name: "filter_ds".to_string(),
            identity: CheckedExportIdentity::direct(vec!["dataset_ops".to_string(), "filter_ds".to_string()]),
            kind: CheckedExportKind::Function(crate::frontend::library_exports::CheckedFunctionExport {
                name: "filter_ds".to_string(),
                emitted_name: None,
                type_params: Vec::new(),
                params: Vec::new(),
                param_defaults: Vec::new(),
                return_type: ResolvedType::Named("DataSet".to_string()),
                is_async: false,
            }),
        };
        let mut module_exports: HashMap<String, HashMap<String, Vec<CheckedNamedExport>>> = HashMap::new();
        module_exports.insert(
            "dataset".to_string(),
            HashMap::from([(dataset_export.name.clone(), vec![dataset_export])]),
        );
        module_exports.insert(
            "dataset_ops".to_string(),
            HashMap::from([(filter_export.name.clone(), vec![filter_export])]),
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
            identity: CheckedExportIdentity::direct(vec!["dataset".to_string(), "DataSet".to_string()]),
            kind: CheckedExportKind::TypeAlias(crate::frontend::library_exports::CheckedTypeAliasExport {
                name: "DataSet".to_string(),
                type_params: Vec::new(),
                target: ResolvedType::Named("DataSet".to_string()),
            }),
        };
        let filter_export = CheckedNamedExport {
            name: "filter_ds".to_string(),
            identity: CheckedExportIdentity::direct(vec!["dataset_ops".to_string(), "filter_ds".to_string()]),
            kind: CheckedExportKind::Function(crate::frontend::library_exports::CheckedFunctionExport {
                name: "filter_ds".to_string(),
                emitted_name: None,
                type_params: Vec::new(),
                params: Vec::new(),
                param_defaults: Vec::new(),
                return_type: ResolvedType::Named("DataSet".to_string()),
                is_async: false,
            }),
        };
        let mut module_exports: HashMap<String, HashMap<String, Vec<CheckedNamedExport>>> = HashMap::new();
        module_exports.insert(
            "dataset".to_string(),
            HashMap::from([(dataset_export.name.clone(), vec![dataset_export])]),
        );
        module_exports.insert(
            "dataset_ops".to_string(),
            HashMap::from([(filter_export.name.clone(), vec![filter_export])]),
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
            BuildCommandOptions::default(),
            BuildReportOptions::default(),
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
            BuildCommandOptions::default(),
            BuildReportOptions::default(),
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

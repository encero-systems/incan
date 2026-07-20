//! Build and run pipeline for Incan projects.
//!
//! This module handles the full compilation flow: module collection, type checking, codegen configuration, dependency
//! resolution, project generation, and Cargo build/run.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::backend::project::generator::GENERATED_CARGO_TARGET_DIR_ENV;
use crate::backend::{IrCodegen, ProjectGenerator, RunProfile};
use crate::cli::{CliError, CliResult, ExitCode};
use crate::compiled_sdk::CompiledSdkModules;
use crate::dependency_resolver::{ResolvedDependencies, resolve_dependencies, resolve_reachable_dependencies};
use crate::frontend::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, CheckedApiPackageIdentity,
    collect_checked_api_metadata, materialize_api_alias_projections, validate_checked_api_docstrings,
};
use crate::frontend::ast::{Declaration, Decorator, Expr, ImportKind, Literal, Span, Spanned, Statement, Visibility};
use crate::frontend::contract_metadata::{ContractMetadataPackage, read_project_model_bundles};
use crate::frontend::library_exports::{CheckedExportKind, CheckedNamedExport, collect_checked_public_exports};
use crate::frontend::library_manifest_index::{LibraryArtifactKind, LibraryManifestIndex, LibraryManifestIndexEntry};
use crate::frontend::module::{
    SourceModuleImportResolution, canonicalize_source_module_segments, resolve_program_source_imports,
    resolve_source_module_import,
};
use crate::frontend::typechecker::stdlib_loader::StdlibAstCache;
use crate::frontend::{diagnostics, typechecker};
#[cfg(feature = "rust_inspect")]
use crate::library_manifest::LibraryRustAbi;
use crate::library_manifest::{
    CompiledProviderMetadata, LibraryManifest, ProviderCargoDependency, ProviderCargoDependencySource,
    ProviderDependencyKind, ProviderDependencyMetadata, ProviderFactKind, ProviderFactRequirement,
    ProviderImplementationFacet, ProviderModuleClaim, digest_provider_artifact,
};
use crate::lockfile::{CargoFeatureSelection, semantic_lock_state};
use crate::manifest::{DependencySource, DependencySpec, ProjectManifest};
use crate::provider::{
    FeatureSelection, PackageFeatureGraph, PackageFeaturePlan, ProviderPlan, SDK_PROVIDER_BUILD_ENV,
};

use super::build_report::{
    BuildReportDraft, BuildReportMode, BuildReportOptions, BuildReportProject, RustInspectionFormat, SourceFileReport,
    artifact_report, cargo_report, dependencies_report, emit_build_report, emit_rust_inspection_report,
    generated_project_report, incan_dependencies_report, interop_report, rust_inspection_report, semantic_report,
};
#[cfg(feature = "rust_inspect")]
use super::common::collect_rust_inspect_query_paths;
use super::common::{
    CargoPolicy, INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV, ProjectRequirements, build_source_map, cargo_command_flags,
    collect_project_requirements, collect_rust_dependency_uses, discover_effective_project_manifest,
    enforce_project_toolchain_constraint, extend_requirements_with_provider_plan, format_dependency_error,
    imported_module_deps_for_with_index, merge_project_requirement_dependencies, module_key_index,
    resolve_project_root, resolve_source_root, semantic_sdk_path_dependencies, validate_output_dir,
};
use super::lock::{
    GeneratedLibraryDependencyPreheatRequest, LockResolution, LockResolutionRequest, resolve_lock_context,
    run_generated_library_dependency_preheat,
};
#[cfg(feature = "rust_inspect")]
use super::lock::{RustInspectWorkspaceRequest, prepare_rust_inspect_workspace};
use super::vocab_extraction::{PendingDesugarerArtifact, collect_library_vocab_metadata};
use crate::cli::prelude::ParsedModule;
#[cfg(feature = "rust_inspect")]
use crate::rust_inspect::{Inspector, InspectorConfig, RustMetadataError};
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
    pub package_features: FeatureSelection,
    pub sdk_profile: Option<String>,
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
    sdk_profile_override: Option<&'a str>,
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
    lock_cargo_package_name: String,
    cargo_lock_projection_root: Option<String>,
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
/// Extract complete Rust metadata from the generated inspect workspace and package it as manifest ABI.
///
/// Prewarm deliberately permits a fast syntax-only fallback. A library artifact is a durable semantic boundary, so
/// publishing whatever happens to be in that shared cache would make its ABI depend on earlier compiler queries.
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
        let Some(lookup_path) = Inspector::normalize_lookup_path(path) else {
            continue;
        };
        match inspector
            .cache()
            .get_or_extract_complete(rust_inspect_manifest_dir, lookup_path, &|_| ())
        {
            Ok(metadata) => items.push((*metadata).clone()),
            Err(
                RustMetadataError::CrateNotFound(_)
                | RustMetadataError::PathNotResolved(_)
                | RustMetadataError::UnsupportedMacro(_),
            ) => {}
            Err(err) => {
                return Err(CliError::failure(format!(
                    "failed to extract complete Rust ABI metadata for `{path}` from {}: {err}",
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
#[allow(clippy::too_many_arguments)] // This orchestration boundary mirrors independent CLI feature and Cargo axes.
fn prepare_project(
    file_path: &str,
    output_dir: Option<&str>,
    cargo_policy: &CargoPolicy,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
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
fn prepare_project_with_options(
    file_path: &str,
    options: PrepareProjectOptions<'_>,
    cargo_policy: &CargoPolicy,
    package_features: &FeatureSelection,
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
    let compilation_session = super::common::CompilationSession::discover_with_selections(
        path,
        package_features,
        options.sdk_profile_override,
    )?;
    let manifest = compilation_session.manifest.clone();
    if let Some(manifest) = manifest.as_ref() {
        enforce_project_toolchain_constraint(manifest)?;
    }

    let modules =
        super::common::collect_modules_detailed_with_session(normalized_file_path.clone(), &compilation_session)
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
        #[cfg(feature = "rust_inspect")]
        rust_inspect_query_paths: &metadata_query_paths,
    })?;
    let (lock_payload, cargo_lock_projection_root) = lock_resolution.cargo_lock_authority.into_generator_inputs();
    resolved = lock_resolution.resolved;
    project_requirements = lock_resolution.project_requirements;
    let cargo_package_name = lock_resolution.cargo_package_name;
    generator.set_package_name(Some(cargo_package_name.clone()));
    generator.set_stdlib_features(project_requirements.stdlib_features.clone());
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
    let compilation_analysis = compilation_session
        .analyze_modules(
            &modules,
            #[cfg(feature = "rust_inspect")]
            rust_inspect_manifest_dir.as_deref(),
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
        entrypoint: Some(normalized_file_path.to_string_lossy().to_string()),
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
        semantic: semantic_report(
            compilation_session.sdk_inventory.as_deref(),
            compilation_session.sdk_components.as_ref(),
            package_feature_plan.as_ref(),
            &provider_plan,
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
            .any(|module| compiled_sdk_modules.contains_emission_path(&module.path_segments));
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
    let report = build_file_report(file_path, output_dir, options, &report_options)?;
    emit_build_report(&report, &report_options)?;
    Ok(ExitCode::SUCCESS)
}

/// Build one executable project and retain its completed report for workspace-level aggregation.
pub(crate) fn build_file_report(
    file_path: &str,
    output_dir: Option<&String>,
    options: BuildCommandOptions,
    report_options: &BuildReportOptions,
) -> CliResult<crate::cli::commands::build_report::BuildReport> {
    let total_start = Instant::now();
    let prepare_start = Instant::now();
    let generated_cargo_target_dir = options.effective_generated_cargo_target_dir();
    let prepared = prepare_project_with_options(
        file_path,
        PrepareProjectOptions {
            output_dir: output_dir.map(|s| s.as_str()),
            project_name_override: None,
            generated_cargo_target_dir: generated_cargo_target_dir.as_deref(),
            sdk_profile_override: options.sdk_profile.as_deref(),
        },
        &options.cargo_policy,
        &options.package_features,
        options.cargo_features,
        options.cargo_no_default_features,
        options.cargo_all_features,
    )?;
    let prepare_ms = elapsed_ms(prepare_start);

    print_build_progress(
        report_options,
        format!("Generated Rust project in: {}", prepared.out_dir),
    );
    print_build_progress(report_options, "Building...");

    let cargo_start = Instant::now();
    match prepared.generator.build() {
        Ok(result) => {
            let cargo_build_ms = elapsed_ms(cargo_start);
            if result.success {
                print_build_progress(report_options, "✓ Build successful!");
                print_build_progress(
                    report_options,
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
                Ok(report)
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

/// Return whether an internal library artifact build must avoid canonical workspace lock resolution.
///
/// Ordinary dependency artifacts are prepared before their parent can finish the canonical workspace lock, so they
/// must retain producer-local resolution. SDK artifacts are different: their publisher supplies an exact lock
/// override that remains part of preparation.
fn dependency_artifact_skips_canonical_lock(artifact_only: bool, sdk_provider_build: bool) -> bool {
    artifact_only && !sdk_provider_build
}

/// Remove path dependencies that point back to the selected project's generated library crate.
///
/// A rooted workspace lock includes the root library as a dependency of its consumers. That aggregate dependency is
/// valid for the synthetic lock/preheat package, but the selected root library artifact cannot depend on its own
/// canonical `target/lib` crate after adopting the producer's Cargo package identity. This comparison deliberately
/// uses the project-owned artifact path rather than a command-specific output override.
fn remove_generated_library_self_dependencies(resolved: &mut ResolvedDependencies, project_root: &Path) {
    let artifact_root = project_root.join("target/lib");
    let canonical_artifact_root = fs::canonicalize(&artifact_root).unwrap_or(artifact_root);
    let points_to_generated_crate = |spec: &DependencySpec| match &spec.source {
        DependencySource::Path { path } => {
            fs::canonicalize(path).unwrap_or_else(|_| path.clone()) == canonical_artifact_root
        }
        DependencySource::Registry | DependencySource::Git { .. } => false,
    };
    resolved.dependencies.retain(|spec| !points_to_generated_crate(spec));
    resolved
        .dev_dependencies
        .retain(|spec| !points_to_generated_crate(spec));
}

/// Validate a library project and generate its Rust project without running Cargo.
#[allow(clippy::too_many_arguments)] // Library preparation receives the same independent CLI selection axes.
fn prepare_library_project(
    file_path: Option<&str>,
    output_dir: Option<&str>,
    cargo_policy: CargoPolicy,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
    generated_cargo_target_dir: Option<&Path>,
) -> CliResult<PreparedLibraryProject> {
    let prepare_start = Instant::now();
    let mut timings_ms = BTreeMap::new();
    let source_load_start = Instant::now();
    let project_root = resolve_library_project_root(file_path)?;
    let Some(manifest) = discover_effective_project_manifest(&project_root)? else {
        return Err(CliError::failure(
            "No incan.toml found for `incan build --lib` (run `incan init` first)",
        ));
    };
    enforce_project_toolchain_constraint(&manifest)?;

    let lib_entry = validate_library_entrypoint(&manifest)?;
    let compilation_session = super::common::CompilationSession::discover_with_selections(
        &lib_entry,
        package_features,
        sdk_profile_override,
    )?;
    let modules = super::common::collect_modules_detailed_with_session(lib_entry.clone(), &compilation_session)
        .map_err(|failure| CliError::failure(failure.render_human()))?;
    let provider_metadata_modules = collect_unprojected_provider_modules(&lib_entry, &compilation_session)?;

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
    let package_feature_plan = compilation_session
        .package_feature_plan
        .clone()
        .ok_or_else(|| CliError::failure("library compilation session is missing its package feature graph"))?;
    let library_manifest_index = compilation_session.library_manifest_index.clone();
    let mut project_requirements = collect_project_requirements(&modules, &library_manifest_index)?;
    let provider_plan = compilation_session.provider_plan_for_modules(&modules)?;
    let compiled_sdk_modules = CompiledSdkModules::from_provider_plan(&provider_plan);
    extend_requirements_with_provider_plan(&mut project_requirements, &provider_plan)?;
    let semantic_sdk_paths = semantic_sdk_path_dependencies(&project_requirements);
    let semantic = semantic_lock_state(
        &project_root,
        compilation_session.sdk_inventory.as_deref(),
        compilation_session.sdk_components.as_ref(),
        Some(&package_feature_plan),
        &provider_plan,
        &semantic_sdk_paths,
    )
    .map_err(CliError::failure)?;
    let contract_model_bundles = read_project_model_bundles(&project_root, &manifest.contract_model_bundle_paths())
        .map_err(|error| CliError::failure(error.to_string()))?;
    let rust_extern_contexts = collect_rust_extern_contexts(&modules);
    let dep_modules = &modules[..modules.len() - 1];
    // Library consumers use the same artifact metadata and linked Rust crate as executable and test-batch consumers;
    // migrated modules must not be generated into a second local `__incan_std` tree.
    let emitted_dep_modules: Vec<&ParsedModule> = dep_modules
        .iter()
        .filter(|module| !compiled_sdk_modules.contains_emission_path(&module.path_segments))
        .collect();

    let mut inline_imports = collect_rust_dependency_uses(lib_module, false);
    for module in &emitted_dep_modules {
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
    let dependency_artifact_only = dependency_artifact_skips_canonical_lock(
        env::var_os(INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV).is_some(),
        env::var_os(SDK_PROVIDER_BUILD_ENV).is_some(),
    );
    let lock_resolution = if dependency_artifact_only {
        // Dependency artifact preparation has no Cargo build to constrain with a lock payload. Resolving the
        // canonical workspace lock here would traverse the consumer that requested this still-missing root artifact
        // and recursively launch the same artifact-only child. Keep the already-resolved producer context intact;
        // the parent command remains the sole owner of canonical lock generation and publication. SDK provider
        // artifact builds are excluded because their parent supplies an exact Cargo.lock payload override.
        LockResolution {
            cargo_lock_authority: super::lock::CargoLockAuthority::None,
            cargo_package_name: project_name.clone(),
            resolved,
            project_requirements,
        }
    } else {
        resolve_lock_context(LockResolutionRequest {
            project_root: &project_root,
            project_name: project_name.as_str(),
            entry_file: Some(&lib_entry),
            manifest: Some(&manifest),
            resolved: &resolved,
            project_requirements: &project_requirements,
            cargo_features: &cargo_features,
            cargo_policy: &cargo_policy,
            semantic: Some(&semantic),
            package_features: Some(package_features),
            sdk_profile_override,
            #[cfg(feature = "rust_inspect")]
            rust_inspect_query_paths: &metadata_query_paths,
        })?
    };
    let (lock_payload_for_typecheck, cargo_lock_projection_root) =
        lock_resolution.cargo_lock_authority.into_generator_inputs();
    resolved = lock_resolution.resolved;
    project_requirements = lock_resolution.project_requirements;
    let lock_cargo_package_name = lock_resolution.cargo_package_name;
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
            cargo_package_name: &lock_cargo_package_name,
            rust_edition: manifest.build.as_ref().and_then(|build| build.rust_edition.clone()),
            resolved: &resolved,
            project_requirements: &project_requirements,
            lock_payload: lock_payload_for_typecheck.clone(),
            cargo_lock_projection_root: cargo_lock_projection_root.as_deref(),
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
    let mut checked_type_info_by_path = BTreeMap::new();

    for (idx, module) in modules.iter().enumerate() {
        let deps_for_module = imported_module_deps_for_with_index(&modules, idx, &module_idx_by_key);
        let mut checker = typechecker::TypeChecker::new();
        checker.stdlib_cache = stdlib_cache.clone();
        checker.set_current_module_path(Some(module.path_segments.clone()));
        checker.set_declared_crate_names(declared.clone());
        checker.set_provider_plan(Arc::clone(&provider_plan));
        #[cfg(feature = "rust_inspect")]
        checker.set_rust_inspect_manifest_dir(rust_inspect_manifest_dir.clone());

        // A provider producer checks its complete source package before publishing the public checked facade.
        let check_result = if provider_plan.bootstrap_sdk_namespace_roots().next().is_some() {
            checker.check_with_imports_allow_private(&module.ast, &deps_for_module)
        } else {
            checker.check_with_imports(&module.ast, &deps_for_module)
        };
        match check_result {
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
                checked_type_info_by_path.insert(module.file_path.clone(), checker.type_info().clone());
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

    let out_dir = match output_dir {
        Some(output_dir) => {
            validate_output_dir(output_dir)?;
            let output_dir = PathBuf::from(output_dir);
            if output_dir.is_absolute() {
                output_dir
            } else {
                project_root.join(output_dir)
            }
        }
        None => project_root.join("target").join("lib"),
    };
    std::fs::create_dir_all(&out_dir)
        .map_err(|error| CliError::failure(format!("failed to create {}: {error}", out_dir.display())))?;

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
    let project_license = manifest.project.as_ref().and_then(|project| project.license.clone());

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
    library_manifest.contract_metadata.provider = compiled_provider_metadata(
        &manifest,
        &package_feature_plan,
        &provider_plan,
        &library_manifest_index,
        &out_dir,
        &provider_metadata_modules,
        lib_module,
    )?;
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

    package_desugarer_artifact(&out_dir, pending_desugarer_artifact.as_ref())?;
    let manifest_path = out_dir.join(format!("{project_name}.incnlib"));

    let mut codegen = IrCodegen::new();
    codegen.set_preserve_dependency_public_items(true);
    codegen.set_stdlib_cache(stdlib_cache);
    codegen.set_declared_crate_names(declared);
    codegen.set_provider_plan(Arc::clone(&provider_plan));
    let main_type_info = checked_type_info_by_path
        .get(&lib_module.file_path)
        .cloned()
        .ok_or_else(|| {
            CliError::failure(format!(
                "missing checked library analysis for {}",
                lib_module.file_path.display()
            ))
        })?;
    let mut dependency_type_info = HashMap::with_capacity(dep_modules.len());
    for module in dep_modules {
        let type_info = checked_type_info_by_path
            .get(&module.file_path)
            .cloned()
            .ok_or_else(|| {
                CliError::failure(format!(
                    "missing checked library analysis for {}",
                    module.file_path.display()
                ))
            })?;
        dependency_type_info.insert(module.path_segments.clone(), type_info);
    }
    codegen.set_prechecked_type_info(main_type_info, dependency_type_info);
    codegen.set_public_ordinal_type_identities(public_ordinal_type_identities(
        lib_module,
        project_name.as_str(),
        &selected_exports,
    ));
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
    for module in &emitted_dep_modules {
        codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
    }
    let mut generator = ProjectGenerator::new(&out_dir, project_name.as_str(), false);
    // Canonical workspace locking uses a synthetic package name so every member resolves one shared Cargo graph.
    // A published library artifact instead has an identity contract across Cargo.toml, `[lib]`, and `.incnlib`, so
    // its generated Cargo package must retain the selected producer project's name.
    generator.set_package_name(Some(project_name.clone()));
    generator.set_package_metadata(Some(project_version.clone()), project_license);
    generator.set_provider_plan(&provider_plan);
    generator.set_sdk_path_dependencies(project_requirements.sdk_path_dependencies.clone());
    generator.set_cargo_target_dir_override(generated_cargo_target_dir.map(Path::to_path_buf));
    generator.set_stdlib_features(project_requirements.stdlib_features.clone());
    generator.set_include_dev_dependencies(lock_payload_for_typecheck.is_some());
    let rust_edition = manifest.build.as_ref().and_then(|build| build.rust_edition.clone());
    generator.set_rust_edition(rust_edition.clone());
    #[cfg(feature = "rust_inspect")]
    codegen.set_rust_inspect_manifest_dir(rust_inspect_manifest_dir.clone());
    generator.set_cargo_lock_payload(lock_payload_for_typecheck);
    generator.set_cargo_lock_projection_root(cargo_lock_projection_root.clone());
    generator.set_cargo_policy_flags(cargo_command_flags(&cargo_policy, &cargo_features));
    let resolved_dependencies_for_preheat = resolved.clone();
    let project_requirements_for_preheat = project_requirements.clone();
    remove_generated_library_self_dependencies(&mut resolved, &project_root);
    let rust_dependencies = resolved.dependencies.clone();
    let rust_dev_dependencies = resolved.dev_dependencies.clone();
    let report_draft = BuildReportDraft {
        mode: BuildReportMode::Library,
        profile: "release".to_string(),
        project: manifest_project_report(Some(&manifest), project_name.as_str(), &project_root),
        entrypoint: Some(lib_entry.to_string_lossy().to_string()),
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
        semantic: semantic_report(
            compilation_session.sdk_inventory.as_deref(),
            compilation_session.sdk_components.as_ref(),
            Some(&package_feature_plan),
            &provider_plan,
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
    if emitted_dep_modules.is_empty() {
        let rust_code = codegen
            .try_generate(&lib_module.ast)
            .map_err(|e| CliError::failure(format!("Code generation error: {e}")))?;
        generator
            .generate(&rust_code)
            .map_err(|e| CliError::failure(format!("Error generating project: {e}")))?;
    } else {
        let module_paths: Vec<Vec<String>> = emitted_dep_modules
            .iter()
            .map(|module| module.path_segments.clone())
            .collect();
        let (main_code, rust_modules) = codegen
            .try_generate_multi_file_nested(&lib_module.ast, &module_paths)
            .map_err(|e| CliError::failure(format!("Code generation error: {e}")))?;
        generator
            .generate_nested(&main_code, &rust_modules)
            .map_err(|e| CliError::failure(format!("Error generating project: {e}")))?;
    }
    synchronize_projected_provider_dependencies(
        &mut library_manifest,
        &out_dir,
        &generator.effective_dependencies().map_err(|error| {
            CliError::failure(format!("failed to resolve projected provider dependencies: {error}"))
        })?,
    )?;
    record_timing(&mut timings_ms, "library_generate_rust", codegen_start);
    record_timing(&mut timings_ms, "library_prepare_total", prepare_start);

    Ok(PreparedLibraryProject {
        generator,
        project_root,
        lock_cargo_package_name,
        cargo_lock_projection_root,
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

/// Synchronize newly published public dependency metadata with the exact projected paths rendered into Cargo.toml.
fn synchronize_projected_provider_dependencies(
    library_manifest: &mut LibraryManifest,
    artifact_root: &Path,
    dependencies: &[DependencySpec],
) -> CliResult<()> {
    for descriptor in library_manifest
        .contract_metadata
        .provider
        .provider_dependencies
        .iter_mut()
        .filter(|dependency| dependency.kind == ProviderDependencyKind::PublicPackage)
    {
        let Some(dependency) = dependencies
            .iter()
            .find(|dependency| dependency.crate_name == descriptor.dependency_key)
        else {
            continue;
        };
        let cargo_package = dependency.package.as_deref().unwrap_or(dependency.crate_name.as_str());
        if cargo_package != descriptor.provider_name {
            return Err(CliError::failure(format!(
                "projected Cargo dependency `{}` names package `{cargo_package}`, but its checked provider edge names `{}`",
                descriptor.dependency_key, descriptor.provider_name
            )));
        }
        let DependencySource::Path { path } = &dependency.source else {
            continue;
        };
        descriptor.relative_artifact_path = relative_provider_artifact_path(artifact_root, path)?;
        descriptor.artifact_digest = digest_provider_artifact(path).map_err(|error| {
            CliError::failure(format!(
                "failed to hash projected provider dependency `{}` artifact {}: {error}",
                descriptor.dependency_key,
                path.display()
            ))
        })?;
    }
    Ok(())
}

/// Build transport-stable provider facts from the checked physical artifact projection.
fn compiled_provider_metadata(
    manifest: &ProjectManifest,
    feature_plan: &PackageFeaturePlan,
    provider_plan: &ProviderPlan,
    library_manifest_index: &LibraryManifestIndex,
    artifact_root: &Path,
    modules: &[ParsedModule],
    active_library_entrypoint: &ParsedModule,
) -> CliResult<CompiledProviderMetadata> {
    let graph = PackageFeatureGraph::from_manifest(manifest).map_err(|error| CliError::failure(error.to_string()))?;
    let root_features = feature_plan
        .root_package()
        .map(|package| &package.features)
        .ok_or_else(|| CliError::failure("resolved package feature plan is missing its root package"))?;
    let library_entrypoint = modules
        .iter()
        .find(|module| module.file_path == active_library_entrypoint.file_path)
        .ok_or_else(|| CliError::failure("unprojected provider graph is missing its library entrypoint"))?;
    let source_root = resolve_source_root(manifest.project_root(), Some(manifest));
    let module_requirements = provider_module_reachability_requirements(modules, library_entrypoint, &source_root)?;
    let mut namespace_claims = modules
        .iter()
        .filter(|module| {
            module.file_path != active_library_entrypoint.file_path
                && !module.path_segments.is_empty()
                && !module_is_owned_by_dependency_provider(provider_plan, &module.path_segments)
        })
        .flat_map(|module| {
            module_requirements
                .get(&module.path_segments)
                .into_iter()
                .flatten()
                .map(|required_features| ProviderModuleClaim {
                    module_path: module.path_segments.clone(),
                    required_features: required_features.clone(),
                })
        })
        .collect::<Vec<_>>();
    namespace_claims.sort();
    namespace_claims.dedup();

    let public_features = graph.provider_metadata();
    let mut fact_requirements = Vec::new();
    for module in modules
        .iter()
        .filter(|module| !module_is_owned_by_dependency_provider(provider_plan, &module.path_segments))
    {
        let requirements = module_requirements.get(&module.path_segments).ok_or_else(|| {
            CliError::failure(format!(
                "unprojected provider module `{}` has no reachability predicate from the library entrypoint",
                module.path_segments.join(".")
            ))
        })?;
        fact_requirements.extend(provider_fact_requirements(module, requirements));
    }
    fact_requirements.extend(
        namespace_claims
            .iter()
            .filter(|claim| !claim.required_features.is_empty())
            .map(|claim| ProviderFactRequirement {
                kind: ProviderFactKind::Module,
                identity: claim.module_path.join("."),
                required_features: claim.required_features.clone(),
            }),
    );
    fact_requirements.extend(public_features.iter().flat_map(|(feature, metadata)| {
        metadata
            .required_sdk_components
            .iter()
            .map(move |component| ProviderFactRequirement {
                kind: ProviderFactKind::ComponentRequirement,
                identity: component.clone(),
                required_features: BTreeSet::from([feature.clone()]),
            })
    }));
    fact_requirements.sort();
    fact_requirements.dedup();

    let provider_dependencies =
        compiled_provider_dependencies(feature_plan, library_manifest_index, provider_plan, artifact_root)?;
    let implementation_facets = provider_implementation_facets(&namespace_claims);
    Ok(CompiledProviderMetadata {
        namespace_claims,
        public_features,
        active_features: root_features.active_features.clone(),
        provider_dependencies,
        fact_requirements,
        required_sdk_components: root_features.required_sdk_components.clone(),
        implementation_facets,
        ..CompiledProviderMetadata::default()
    })
}

/// Freeze the active Incan dependency edges into artifact-owned, relocation-safe provider metadata.
fn compiled_provider_dependencies(
    feature_plan: &PackageFeaturePlan,
    library_manifest_index: &LibraryManifestIndex,
    provider_plan: &ProviderPlan,
    artifact_root: &Path,
) -> CliResult<Vec<ProviderDependencyMetadata>> {
    let mut dependencies = Vec::new();
    for edge in feature_plan
        .edges()
        .filter(|edge| edge.from.as_path() == feature_plan.root())
    {
        let entry = library_manifest_index.get(&edge.dependency_key).ok_or_else(|| {
            CliError::failure(format!(
                "active provider dependency `pub::{}` is missing from the checked library manifest index",
                edge.dependency_key
            ))
        })?;
        let (manifest, metadata) = match entry {
            LibraryManifestIndexEntry::Loaded { manifest, metadata } => (manifest, metadata),
            LibraryManifestIndexEntry::Failed(failure) => {
                return Err(CliError::failure(format!(
                    "active provider dependency `pub::{}` could not be loaded from {}: {}",
                    edge.dependency_key,
                    failure.path.display(),
                    failure.message
                )));
            }
        };
        if metadata.kind != LibraryArtifactKind::Materialized {
            return Err(CliError::failure(format!(
                "active provider dependency `pub::{}` has parser-only metadata; build its compiled artifact before publishing this provider",
                edge.dependency_key
            )));
        }
        let artifact_digest = digest_provider_artifact(&metadata.crate_root).map_err(|error| {
            CliError::failure(format!(
                "failed to hash provider dependency `pub::{}` artifact {}: {error}",
                edge.dependency_key,
                metadata.crate_root.display()
            ))
        })?;
        dependencies.push(ProviderDependencyMetadata {
            kind: crate::library_manifest::ProviderDependencyKind::PublicPackage,
            dependency_key: edge.dependency_key.clone(),
            provider_name: manifest.name.clone(),
            provider_version: manifest.version.clone(),
            artifact_digest,
            relative_artifact_path: relative_provider_artifact_path(artifact_root, &metadata.crate_root)?,
            requested_features: edge.requested_features.clone(),
            default_features: edge.default_features,
            optional: edge.optional,
        });
    }
    for provider in provider_plan.sdk_link_roots() {
        let Some(metadata) = provider.artifact.as_ref() else {
            continue;
        };
        let artifact_digest = digest_provider_artifact(&metadata.crate_root).map_err(|error| {
            CliError::failure(format!(
                "failed to hash private SDK provider dependency `{}` artifact {}: {error}",
                provider.identity.name,
                metadata.crate_root.display()
            ))
        })?;
        dependencies.push(ProviderDependencyMetadata {
            kind: crate::library_manifest::ProviderDependencyKind::PrivateImplementation,
            dependency_key: metadata.dependency_key.clone(),
            provider_name: provider.identity.name.clone(),
            provider_version: provider.identity.version.clone(),
            artifact_digest,
            relative_artifact_path: relative_provider_artifact_path(artifact_root, &metadata.crate_root)?,
            requested_features: provider.identity.feature_projection.clone(),
            default_features: false,
            optional: false,
        });
    }
    dependencies.sort();
    dependencies.dedup();
    Ok(dependencies)
}

/// Compute one normalized portable path between two existing provider artifact roots.
fn relative_provider_artifact_path(from: &Path, to: &Path) -> CliResult<String> {
    let from = fs::canonicalize(from).map_err(|error| {
        CliError::failure(format!(
            "failed to canonicalize provider artifact root {}: {error}",
            from.display()
        ))
    })?;
    let to = fs::canonicalize(to).map_err(|error| {
        CliError::failure(format!(
            "failed to canonicalize dependency artifact root {}: {error}",
            to.display()
        ))
    })?;
    let from_components = from.components().collect::<Vec<_>>();
    let to_components = to.components().collect::<Vec<_>>();
    let common = from_components
        .iter()
        .zip(&to_components)
        .take_while(|(left, right)| left == right)
        .count();
    if common == 0 {
        return Err(CliError::failure(format!(
            "provider artifact roots {} and {} have no relocatable filesystem ancestor",
            from.display(),
            to.display()
        )));
    }
    let mut relative = PathBuf::new();
    for _ in common..from_components.len() {
        relative.push("..");
    }
    for component in &to_components[common..] {
        relative.push(component.as_os_str());
    }
    let rendered = relative.to_string_lossy().replace('\\', "/");
    if rendered.is_empty() {
        return Err(CliError::failure("a provider artifact cannot depend on itself"));
    }
    Ok(rendered)
}

/// Parse the complete local provider graph without dropping inactive feature-conditioned declarations.
///
/// The checked API and generated Rust remain specialized to the selected feature projection. This parallel metadata
/// view preserves the complete positive condition inventory so consumers and inspection can explain inactive facts
/// without reparsing provider source.
fn collect_unprojected_provider_modules(
    library_entrypoint: &Path,
    session: &super::common::CompilationSession,
) -> CliResult<Vec<ParsedModule>> {
    let mut pending = vec![(
        library_entrypoint.to_path_buf(),
        "main".to_string(),
        vec!["main".to_string()],
    )];
    let mut processed = HashSet::new();
    let mut modules = Vec::new();

    while let Some((file_path, module_name, path_segments)) = pending.pop() {
        let canonical_path = file_path.canonicalize().unwrap_or_else(|_| file_path.clone());
        if !processed.insert(canonical_path) {
            continue;
        }
        let source = fs::read_to_string(&file_path)
            .map_err(|error| CliError::failure(format!("failed to read {}: {error}", file_path.display())))?;
        let ast = session
            .parse_source_unprojected(&file_path, &source, false)
            .map_err(|errors| {
                let rendered = errors
                    .iter()
                    .map(|error| diagnostics::format_error(file_path.to_string_lossy().as_ref(), &source, error))
                    .collect::<String>();
                CliError::failure(rendered.trim_end())
            })?;
        session.validate_parsed_program_features(&ast).map_err(|errors| {
            let rendered = errors
                .iter()
                .map(|error| diagnostics::format_error(file_path.to_string_lossy().as_ref(), &source, error))
                .collect::<String>();
            CliError::failure(rendered.trim_end())
        })?;
        let base_dir = file_path.parent().unwrap_or(session.source_root.as_path());
        for resolved in resolve_program_source_imports(&ast, base_dir, Some(&session.source_root)) {
            if let SourceModuleImportResolution::Local(module) = resolved.resolution {
                pending.push((module.file_path, module.module_name, module.path_segments));
            }
        }
        modules.push(ParsedModule {
            name: module_name,
            path_segments,
            file_path,
            source,
            ast,
        });
    }

    Ok(modules)
}

/// Freeze the source SDK publisher's current Rust-backend mappings into provider-owned artifact facets.
///
/// Consumers read these mappings from `.incnlib`; they never rediscover Cargo features or dependencies from a
/// compiler-side stdlib module inventory. This bootstrap adapter can disappear once provider source can author the
/// equivalent backend mappings directly.
fn provider_implementation_facets(namespace_claims: &[ProviderModuleClaim]) -> Vec<ProviderImplementationFacet> {
    if env::var_os(SDK_PROVIDER_BUILD_ENV).is_none() {
        return Vec::new();
    }
    let roots = namespace_claims
        .iter()
        .filter_map(|claim| claim.module_path.first().cloned())
        .collect::<BTreeSet<_>>();
    roots
        .into_iter()
        .filter_map(|root| {
            let namespace = incan_core::lang::stdlib::find_namespace(&root)?;
            let required_modules = namespace_claims
                .iter()
                .filter(|claim| claim.module_path.first() == Some(&root))
                .map(|claim| claim.module_path.clone())
                .collect();
            let cargo_features = namespace
                .feature
                .map(|feature| {
                    BTreeMap::from([(
                        crate::backend::project::INCAN_STDLIB_CRATE_NAME.to_string(),
                        BTreeSet::from([feature.to_string()]),
                    )])
                })
                .unwrap_or_default();
            let cargo_dependencies = namespace
                .extra_crate_deps
                .iter()
                .map(|dependency| ProviderCargoDependency {
                    crate_name: dependency.crate_name.to_string(),
                    package: incan_core::lang::stdlib::extra_crate_package_alias(dependency.crate_name)
                        .map(str::to_string),
                    version: match dependency.source {
                        incan_core::lang::stdlib::StdlibExtraCrateSource::Version(version) => Some(version.to_string()),
                        incan_core::lang::stdlib::StdlibExtraCrateSource::Path(_) => None,
                    },
                    features: dependency
                        .features
                        .iter()
                        .map(|feature| (*feature).to_string())
                        .collect(),
                    default_features: true,
                    source: match dependency.source {
                        incan_core::lang::stdlib::StdlibExtraCrateSource::Version(_) => {
                            ProviderCargoDependencySource::Registry
                        }
                        incan_core::lang::stdlib::StdlibExtraCrateSource::Path(relative_path) => {
                            ProviderCargoDependencySource::Toolchain {
                                relative_path: relative_path.to_string(),
                            }
                        }
                    },
                })
                .collect();
            Some(ProviderImplementationFacet {
                id: format!("rust_{root}"),
                required_modules,
                required_features: BTreeSet::new(),
                cargo_features,
                cargo_dependencies,
            })
        })
        .collect()
}

/// Return whether an already-linked SDK provider owns this emitted `__incan_std.*` module.
fn module_is_owned_by_dependency_provider(provider_plan: &ProviderPlan, emission_path: &[String]) -> bool {
    let prefix = [incan_core::lang::stdlib::INCAN_STD_NAMESPACE.to_string()];
    let relative = if let Some(relative) = emission_path.strip_prefix(prefix.as_slice()) {
        relative
    } else if env::var_os(SDK_PROVIDER_BUILD_ENV).is_some() {
        emission_path
    } else {
        return false;
    };
    let mut canonical = vec![incan_core::lang::stdlib::STDLIB_ROOT.to_string()];
    canonical.extend(relative.iter().cloned());
    provider_plan.active_sdk_provider_for_module(&canonical).is_some()
}

/// Derive the positive feature predicates under which every local provider module is reachable from the entrypoint.
///
/// Multiple incomparable predicates represent alternative additive paths. A broader predicate subsumes narrower paths,
/// so an unconditional import collapses every conditional route to the same module. Conditions accumulate across
/// nested imports instead of being inferred only from the library entrypoint.
fn provider_module_reachability_requirements(
    modules: &[ParsedModule],
    entrypoint: &ParsedModule,
    source_root: &Path,
) -> CliResult<BTreeMap<Vec<String>, Vec<BTreeSet<String>>>> {
    let modules_by_path = modules
        .iter()
        .map(|module| (canonical_provider_source_path(&module.file_path), module))
        .collect::<BTreeMap<_, _>>();
    let entrypoint_path = canonical_provider_source_path(&entrypoint.file_path);
    if !modules_by_path.contains_key(&entrypoint_path) {
        return Err(CliError::failure(
            "unprojected provider graph does not contain its library entrypoint",
        ));
    }

    let mut requirements = BTreeMap::new();
    insert_provider_feature_predicate(&mut requirements, entrypoint.path_segments.clone(), BTreeSet::new());
    let mut pending = vec![(entrypoint_path, BTreeSet::new())];

    while let Some((module_path, inherited_features)) = pending.pop() {
        let Some(module) = modules_by_path.get(&module_path) else {
            return Err(CliError::failure(format!(
                "unprojected provider graph lost module {}",
                module_path.display()
            )));
        };
        let base_dir = module.file_path.parent().unwrap_or(source_root);
        for declaration in &module.ast.declarations {
            let Declaration::Import(import) = &declaration.node else {
                continue;
            };
            let SourceModuleImportResolution::Local(target) =
                resolve_source_module_import(base_dir, Some(source_root), import)
            else {
                continue;
            };
            let target_path = canonical_provider_source_path(&target.file_path);
            let Some(target_module) = modules_by_path.get(&target_path) else {
                return Err(CliError::failure(format!(
                    "unprojected provider graph is missing imported module `{}` at {}",
                    target.path_segments.join("."),
                    target.file_path.display()
                )));
            };
            let mut required_features = inherited_features.clone();
            required_features.extend(declaration.required_features.iter().cloned());
            if insert_provider_feature_predicate(
                &mut requirements,
                target_module.path_segments.clone(),
                required_features.clone(),
            ) {
                pending.push((target_path, required_features));
            }
        }
    }

    Ok(requirements)
}

/// Canonicalize source identity when possible while retaining useful fixture paths when it is not.
fn canonical_provider_source_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Insert one predicate into a deterministic minimal antichain for a provider module.
fn insert_provider_feature_predicate(
    requirements: &mut BTreeMap<Vec<String>, Vec<BTreeSet<String>>>,
    module_path: Vec<String>,
    candidate: BTreeSet<String>,
) -> bool {
    let predicates = requirements.entry(module_path).or_default();
    if predicates.iter().any(|existing| existing.is_subset(&candidate)) {
        return false;
    }
    predicates.retain(|existing| !candidate.is_subset(existing));
    predicates.push(candidate);
    predicates.sort();
    true
}

/// Preserve positive feature predicates on checked declarations for inspection and artifact projection.
fn provider_fact_requirements(
    module: &ParsedModule,
    module_requirements: &[BTreeSet<String>],
) -> Vec<ProviderFactRequirement> {
    let module_name = module.path_segments.join(".");
    let mut requirements = Vec::new();
    for declaration in &module.ast.declarations {
        let mut combined_requirements = Vec::new();
        for module_requirement in module_requirements {
            let mut combined = module_requirement.clone();
            combined.extend(declaration.required_features.iter().cloned());
            if !combined.is_empty()
                && !combined_requirements
                    .iter()
                    .any(|existing: &BTreeSet<String>| existing.is_subset(&combined))
            {
                combined_requirements.retain(|existing| !combined.is_subset(existing));
                combined_requirements.push(combined);
            }
        }
        combined_requirements.sort();

        for required_features in combined_requirements {
            match &declaration.node {
                Declaration::Import(import) => {
                    requirements.push(ProviderFactRequirement {
                        kind: ProviderFactKind::ProviderDependency,
                        identity: format!("{module_name}::{}", provider_import_identity(&import.kind)),
                        required_features: required_features.clone(),
                    });
                    if import.visibility == Visibility::Public {
                        let reexported_items = match &import.kind {
                            ImportKind::From { items, .. } | ImportKind::PubFrom { items, .. } => items.as_slice(),
                            _ => &[],
                        };
                        requirements.extend(reexported_items.iter().map(|item| ProviderFactRequirement {
                            kind: ProviderFactKind::Export,
                            identity: format!("{module_name}::{}", item.alias.as_deref().unwrap_or(item.name.as_str())),
                            required_features: required_features.clone(),
                        }));
                    }
                }
                Declaration::Docstring(_) => requirements.push(ProviderFactRequirement {
                    kind: ProviderFactKind::Documentation,
                    identity: format!("{module_name}::module-docstring"),
                    required_features,
                }),
                Declaration::TestModule(test_module) => {
                    requirements.push(ProviderFactRequirement {
                        kind: ProviderFactKind::Export,
                        identity: format!("{module_name}::{}", test_module.name),
                        required_features: required_features.clone(),
                    });
                    requirements.extend(provider_nested_test_fact_requirements(
                        &module_name,
                        &test_module.body,
                        &required_features,
                    ));
                }
                declaration => {
                    let Some(name) = provider_declaration_name(declaration) else {
                        continue;
                    };
                    let identity = format!("{module_name}::{name}");
                    requirements.push(ProviderFactRequirement {
                        kind: if provider_declaration_is_public(declaration) {
                            ProviderFactKind::Export
                        } else {
                            ProviderFactKind::ImplementationFacet
                        },
                        identity: identity.clone(),
                        required_features: required_features.clone(),
                    });
                    if provider_declaration_has_docstring(declaration) {
                        requirements.push(ProviderFactRequirement {
                            kind: ProviderFactKind::Documentation,
                            identity: identity.clone(),
                            required_features: required_features.clone(),
                        });
                    }
                    if provider_declaration_is_registry_entry(declaration) {
                        requirements.push(ProviderFactRequirement {
                            kind: ProviderFactKind::RegistryEntry,
                            identity,
                            required_features,
                        });
                    }
                }
            }
        }
    }
    requirements
}

/// Preserve nested inline-test predicates together with their enclosing test-module predicate.
fn provider_nested_test_fact_requirements(
    module_name: &str,
    declarations: &[Spanned<Declaration>],
    parent_features: &BTreeSet<String>,
) -> Vec<ProviderFactRequirement> {
    declarations
        .iter()
        .filter_map(|declaration| {
            let name = provider_declaration_name(&declaration.node)?;
            let mut required_features = parent_features.clone();
            required_features.extend(declaration.required_features.iter().cloned());
            Some(ProviderFactRequirement {
                kind: ProviderFactKind::ImplementationFacet,
                identity: format!("{module_name}::tests::{name}"),
                required_features,
            })
        })
        .collect()
}

/// Render a stable provider-local import identity without depending on source offsets.
fn provider_import_identity(import: &ImportKind) -> String {
    match import {
        ImportKind::Module(path) => format!("import:{}", path.segments.join(".")),
        ImportKind::From { module, .. } => format!("from:{}", module.segments.join(".")),
        ImportKind::PubLibrary { library } => format!("import:pub::{library}"),
        ImportKind::PubFrom { library, .. } => format!("from:pub::{library}"),
        ImportKind::Python(module) => format!("import:python:{module}"),
        ImportKind::RustCrate { crate_name, path, .. } => {
            format!("import:rust::{crate_name}::{}", path.join("::"))
        }
        ImportKind::RustFrom { crate_name, path, .. } => {
            format!("from:rust::{crate_name}::{}", path.join("::"))
        }
    }
}

/// Return one declaration's stable local name.
fn provider_declaration_name(declaration: &Declaration) -> Option<&str> {
    match declaration {
        Declaration::Const(item) => Some(&item.name),
        Declaration::Static(item) => Some(&item.name),
        Declaration::Model(item) => Some(&item.name),
        Declaration::Class(item) => Some(&item.name),
        Declaration::Trait(item) => Some(&item.name),
        Declaration::Alias(item) => Some(&item.name),
        Declaration::Partial(item) => Some(&item.name),
        Declaration::TypeAlias(item) => Some(&item.name),
        Declaration::Newtype(item) => Some(&item.name),
        Declaration::Enum(item) => Some(&item.name),
        Declaration::Function(item) => Some(&item.name),
        Declaration::TestModule(item) => Some(&item.name),
        Declaration::Import(_) | Declaration::Docstring(_) => None,
    }
}

/// Return whether one declaration contributes to the package's public checked surface.
fn provider_declaration_is_public(declaration: &Declaration) -> bool {
    let visibility = match declaration {
        Declaration::Const(item) => item.visibility,
        Declaration::Static(item) => item.visibility,
        Declaration::Model(item) => item.visibility,
        Declaration::Class(item) => item.visibility,
        Declaration::Trait(item) => item.visibility,
        Declaration::Alias(item) => item.visibility,
        Declaration::Partial(item) => item.visibility,
        Declaration::TypeAlias(item) => item.visibility,
        Declaration::Newtype(item) => item.visibility,
        Declaration::Enum(item) => item.visibility,
        Declaration::Function(item) => item.visibility,
        Declaration::Import(item) => item.visibility,
        Declaration::TestModule(_) | Declaration::Docstring(_) => Visibility::Private,
    };
    matches!(visibility, Visibility::Public)
}

/// Return whether the declaration owns checked source documentation.
fn provider_declaration_has_docstring(declaration: &Declaration) -> bool {
    match declaration {
        Declaration::Function(item) => item.body.first().is_some_and(|statement| {
            matches!(
                &statement.node,
                Statement::Expr(expression)
                    if matches!(&expression.node, Expr::Literal(Literal::String(_)))
            )
        }),
        Declaration::Model(item) => item.docstring.is_some(),
        Declaration::Class(item) => item.docstring.is_some(),
        Declaration::Trait(item) => item.docstring.is_some(),
        Declaration::Newtype(item) => item.docstring.is_some(),
        Declaration::Enum(item) => item.docstring.is_some(),
        _ => false,
    }
}

/// Return whether the declaration is a checked `std.registry` entry described by `@describe`.
fn provider_declaration_is_registry_entry(declaration: &Declaration) -> bool {
    let decorators = match declaration {
        Declaration::Model(item) => &item.decorators,
        Declaration::Class(item) => &item.decorators,
        Declaration::Trait(item) => &item.decorators,
        Declaration::Newtype(item) => &item.decorators,
        Declaration::Enum(item) => &item.decorators,
        Declaration::Function(item) => &item.decorators,
        _ => return false,
    };
    decorators.iter().any(|decorator| decorator.node.name == "describe")
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
    output_dir: Option<&String>,
    options: BuildCommandOptions,
    report_options: BuildReportOptions,
) -> CliResult<ExitCode> {
    let report = build_library_report(file_path, output_dir, options, &report_options)?;
    emit_build_report(&report, &report_options)?;
    Ok(ExitCode::SUCCESS)
}

/// Build one library project and retain its completed report for workspace-level aggregation.
pub(crate) fn build_library_report(
    file_path: Option<&str>,
    output_dir: Option<&String>,
    options: BuildCommandOptions,
    report_options: &BuildReportOptions,
) -> CliResult<crate::cli::commands::build_report::BuildReport> {
    let total_start = Instant::now();
    let generated_cargo_target_dir = options.effective_generated_cargo_target_dir();
    let mut prepared = prepare_library_project(
        file_path,
        output_dir.map(String::as_str),
        options.cargo_policy,
        &options.package_features,
        options.sdk_profile.as_deref(),
        options.cargo_features,
        options.cargo_no_default_features,
        options.cargo_all_features,
        generated_cargo_target_dir.as_deref(),
    )?;
    let artifact_only = env::var_os(INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV).is_some();

    if artifact_only {
        write_library_manifest_artifacts(&mut prepared)?;
        print_build_progress(report_options, "✓ Library dependency artifact prepared!");
        print_build_progress(
            report_options,
            format!("Generated manifest: {}", prepared.manifest_path.display()),
        );
        let mut timings_ms = prepared.timings_ms.clone();
        timings_ms.insert("total".to_string(), elapsed_ms(total_start));
        let report = prepared.report.finish(timings_ms);
        return Ok(report);
    }

    let preheat_start = Instant::now();
    if prepared.should_preheat_library_dependencies
        && let Some(lock_payload) = prepared.lock_payload.as_deref()
    {
        run_generated_library_dependency_preheat(GeneratedLibraryDependencyPreheatRequest {
            project_root: &prepared.project_root,
            lock_dir: &crate::lockfile::compiler_lock_state_dir(&prepared.project_root),
            project_name: &prepared.lock_cargo_package_name,
            rust_edition: prepared.rust_edition.clone(),
            resolved: &prepared.resolved_dependencies,
            project_requirements: &prepared.project_requirements,
            cargo_features: &prepared.cargo_features,
            cargo_policy: &prepared.cargo_policy,
            target_dir: &prepared.generator.cargo_target_dir(),
            cargo_lock_payload: lock_payload,
            cargo_lock_projection_root: prepared.cargo_lock_projection_root.as_deref(),
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

    print_build_progress(report_options, "✓ Library build successful!");
    print_build_progress(
        report_options,
        format!("Generated Rust crate in: {}", prepared.out_dir.display()),
    );
    print_build_progress(
        report_options,
        format!("Generated manifest: {}", prepared.manifest_path.display()),
    );

    prepared.timings_ms.insert("cargo_build".to_string(), cargo_build_ms);
    prepared.timings_ms.insert("total".to_string(), elapsed_ms(total_start));
    let report = prepared.report.finish(prepared.timings_ms);
    Ok(report)
}

/// Generate and inspect the current Rust backend output without running Cargo.
pub fn inspect_rust(path: &Path, lib_mode: bool, format: RustInspectionFormat) -> CliResult<ExitCode> {
    let path_arg = path.to_string_lossy();
    let report = if lib_mode {
        let prepared = prepare_library_project(
            Some(path_arg.as_ref()),
            None,
            CargoPolicy::default(),
            &FeatureSelection::default(),
            None,
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
            &FeatureSelection::default(),
            None,
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
#[allow(clippy::too_many_arguments)] // Public CLI dispatch keeps the parsed command axes explicit at this boundary.
pub fn run_file(
    file_path: &str,
    cargo_policy: CargoPolicy,
    package_features: FeatureSelection,
    sdk_profile: Option<String>,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
    release: bool,
) -> CliResult<ExitCode> {
    let prepared = prepare_project(
        file_path,
        None,
        &cargo_policy,
        &package_features,
        sdk_profile.as_deref(),
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    )?;
    run_prepared_project(prepared, release)
}

/// Build and run inline Incan source from `incan run -c`.
#[allow(clippy::too_many_arguments)] // Inline and file execution intentionally share the explicit CLI contract.
pub fn run_inline_source(
    source: &str,
    cargo_policy: CargoPolicy,
    package_features: FeatureSelection,
    sdk_profile: Option<String>,
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
    let _inline_command_lock = crate::lockfile::acquire_publication_lock(&source_path).map_err(|error| {
        CliError::failure(format!(
            "failed to coordinate temporary inline command project {}: {error}",
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
            sdk_profile_override: sdk_profile.as_deref(),
        },
        &cargo_policy,
        &package_features,
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
    fn dependency_artifact_only_build_skips_canonical_lock_issue908() {
        assert!(dependency_artifact_skips_canonical_lock(true, false));
        assert!(!dependency_artifact_skips_canonical_lock(true, true));
        assert!(!dependency_artifact_skips_canonical_lock(false, false));
    }

    #[test]
    fn emitted_library_metadata_tracks_projected_dependency_issue911() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let artifact_root = workspace.path().join("published");
        let projected_root = workspace.path().join("projected");
        fs::create_dir_all(&artifact_root)?;
        fs::create_dir_all(projected_root.join("src"))?;
        fs::write(
            projected_root.join("Cargo.toml"),
            "[package]\nname = \"projected_provider\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(projected_root.join("src/lib.rs"), "pub fn marker() {}\n")?;
        let mut manifest = LibraryManifest::new("published", "0.1.0");
        manifest
            .contract_metadata
            .provider
            .provider_dependencies
            .push(ProviderDependencyMetadata {
                kind: ProviderDependencyKind::PublicPackage,
                dependency_key: "provider_alias".to_string(),
                provider_name: "projected_provider".to_string(),
                provider_version: "0.1.0".to_string(),
                artifact_digest: "sha256:stale".to_string(),
                relative_artifact_path: "../stale".to_string(),
                requested_features: BTreeSet::new(),
                default_features: false,
                optional: false,
            });
        let dependencies = vec![DependencySpec {
            crate_name: "provider_alias".to_string(),
            version: None,
            features: Vec::new(),
            default_features: false,
            source: DependencySource::Path {
                path: projected_root.clone(),
            },
            optional: false,
            package: Some("projected_provider".to_string()),
        }];

        synchronize_projected_provider_dependencies(&mut manifest, &artifact_root, &dependencies)?;

        let descriptor = &manifest.contract_metadata.provider.provider_dependencies[0];
        assert_eq!(descriptor.artifact_digest, digest_provider_artifact(&projected_root)?);
        assert_eq!(
            fs::canonicalize(artifact_root.join(&descriptor.relative_artifact_path))?,
            fs::canonicalize(projected_root)?
        );
        Ok(())
    }

    #[test]
    fn rooted_library_removes_selected_project_self_dependency_issue909() -> Result<(), Box<dyn std::error::Error>> {
        let project_root = tempfile::tempdir()?;
        let artifact_root = project_root.path().join("target/lib");
        let external_root = project_root.path().join("external/artifact");
        fs::create_dir_all(&artifact_root)?;
        fs::create_dir_all(&external_root)?;
        let path_dependency = |crate_name: &str, path: PathBuf| DependencySpec {
            crate_name: crate_name.to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path { path },
            optional: false,
            package: None,
        };
        let mut resolved = ResolvedDependencies {
            dependencies: vec![
                path_dependency("root_lib", artifact_root.clone()),
                path_dependency("external", external_root),
            ],
            dev_dependencies: vec![path_dependency("root_lib_dev_alias", artifact_root)],
        };

        remove_generated_library_self_dependencies(&mut resolved, project_root.path());

        assert_eq!(resolved.dependencies.len(), 1);
        assert_eq!(resolved.dependencies[0].crate_name, "external");
        assert!(resolved.dev_dependencies.is_empty());
        Ok(())
    }

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
            &FeatureSelection::default(),
            None,
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

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn library_rust_abi_is_independent_of_partial_prewarm_cache_issue922() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let root = workspace.path().join("root");
        let dependency = workspace.path().join("source-dep");
        fs::create_dir_all(root.join("src"))?;
        fs::create_dir_all(dependency.join("src"))?;
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"root\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nsource-dep = { path = \"../source-dep\" }\n",
        )?;
        fs::write(root.join("src/lib.rs"), "pub fn keep() {}\n")?;
        fs::write(
            dependency.join("Cargo.toml"),
            "[package]\nname = \"source-dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nname = \"source_dep\"\n",
        )?;
        fs::write(
            dependency.join("src/lib.rs"),
            r#"
pub struct ChildId(String);

impl ChildId {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}
"#,
        )?;

        let query_path = "source_dep::ChildId".to_string();
        let inspector = Inspector::new(InspectorConfig::new(root.clone()));
        inspector.prewarm([query_path.clone()], &|_| ())?;
        let prewarmed = inspector.get(&query_path)?;
        let incan_core::interop::RustItemKind::Type(prewarmed_type) = &prewarmed.metadata.kind else {
            return Err("expected prewarmed ChildId type metadata".into());
        };
        assert!(
            !prewarmed_type.metadata_completeness.has_methods(),
            "the regression requires the fast prewarm route to persist partial source metadata"
        );

        let query_paths = vec![query_path.clone()];
        let cold = collect_library_rust_abi(&root, &query_paths)?.ok_or("expected cold library Rust ABI")?;
        inspector.cache().get_or_extract_complete(&root, &query_path, &|_| ())?;
        let warm = collect_library_rust_abi(&root, &query_paths)?.ok_or("expected warm library Rust ABI")?;

        assert_eq!(
            cold, warm,
            "library ABI publication must not depend on whether a previous compiler query upgraded the shared cache"
        );
        let child_id = warm.get(&query_path).ok_or("expected ChildId ABI item")?;
        let incan_core::interop::RustItemKind::Type(child_id_type) = &child_id.kind else {
            return Err("expected ChildId ABI type metadata".into());
        };
        assert!(child_id_type.metadata_completeness.has_methods());
        assert!(child_id_type.methods.iter().any(|method| method.name == "as_str"));
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

    #[test]
    fn feature_conditions_are_preserved_for_provider_exports_docs_registries_and_reexports()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"when feature("catalog"):
    @describe(summary="Catalog entry")
    pub def catalog_entry() -> str:
        """Return the selected catalog entry."""
        return "catalog"

when feature("widgets"):
    pub from widgets import Widget
"#;
        let tokens = lexer::lex(source).map_err(|errors| format!("lex errors: {errors:?}"))?;
        let ast = parser::parse_with_module_path(&tokens, Some("project/src/lib.incn"))
            .map_err(|errors| format!("parse errors: {errors:?}"))?;
        let module = ParsedModule {
            name: "main".to_string(),
            path_segments: vec!["main".to_string()],
            file_path: PathBuf::from("project/src/lib.incn"),
            source: source.to_string(),
            ast,
        };

        let requirements = provider_fact_requirements(&module, &[BTreeSet::new()]);
        let catalog_features = BTreeSet::from(["catalog".to_string()]);
        for kind in [
            ProviderFactKind::Export,
            ProviderFactKind::Documentation,
            ProviderFactKind::RegistryEntry,
        ] {
            assert!(requirements.iter().any(|requirement| {
                requirement.kind == kind
                    && requirement.identity == "main::catalog_entry"
                    && requirement.required_features == catalog_features
            }));
        }
        assert!(requirements.iter().any(|requirement| {
            requirement.kind == ProviderFactKind::ProviderDependency
                && requirement.identity == "main::from:widgets"
                && requirement.required_features == BTreeSet::from(["widgets".to_string()])
        }));
        assert!(requirements.iter().any(|requirement| {
            requirement.kind == ProviderFactKind::Export
                && requirement.identity == "main::Widget"
                && requirement.required_features == BTreeSet::from(["widgets".to_string()])
        }));
        Ok(())
    }

    #[test]
    fn provider_module_conditions_preserve_nested_and_alternative_feature_paths()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let source_root = project.path().join("src");
        fs::create_dir_all(&source_root)?;
        let entry_path = source_root.join("lib.incn");
        let nested_path = source_root.join("nested.incn");
        let leaf_path = source_root.join("leaf.incn");
        let entry_source = r#"when feature("outer"):
    from nested import Nested

when feature("alternate"):
    from leaf import Leaf
"#;
        let nested_source = r#"when feature("inner"):
    from leaf import Leaf

pub model Nested:
    value: int
"#;
        let leaf_source = "pub model Leaf:\n    value: int\n";
        fs::write(&entry_path, entry_source)?;
        fs::write(&nested_path, nested_source)?;
        fs::write(&leaf_path, leaf_source)?;

        let parse_module = |name: &str,
                            path_segments: Vec<String>,
                            file_path: PathBuf,
                            source: &str|
         -> Result<ParsedModule, Box<dyn std::error::Error>> {
            let tokens = lexer::lex(source).map_err(|errors| format!("lex errors: {errors:?}"))?;
            let ast = parser::parse_with_module_path(&tokens, file_path.to_str())
                .map_err(|errors| format!("parse errors: {errors:?}"))?;
            Ok(ParsedModule {
                name: name.to_string(),
                path_segments,
                file_path,
                source: source.to_string(),
                ast,
            })
        };
        let entry = parse_module("main", vec!["main".to_string()], entry_path, entry_source)?;
        let nested = parse_module("nested", vec!["nested".to_string()], nested_path, nested_source)?;
        let leaf = parse_module("leaf", vec!["leaf".to_string()], leaf_path, leaf_source)?;
        let modules = vec![entry.clone(), nested, leaf.clone()];

        let requirements = provider_module_reachability_requirements(&modules, &entry, &source_root)?;
        let nested_key = vec!["nested".to_string()];
        let leaf_key = vec!["leaf".to_string()];
        assert_eq!(
            requirements.get(&nested_key),
            Some(&vec![BTreeSet::from(["outer".to_string()])])
        );
        assert_eq!(
            requirements.get(&leaf_key),
            Some(&vec![
                BTreeSet::from(["alternate".to_string()]),
                BTreeSet::from(["inner".to_string(), "outer".to_string()]),
            ])
        );

        let leaf_facts = provider_fact_requirements(
            &leaf,
            requirements
                .get(&leaf_key)
                .ok_or("leaf reachability should be present")?,
        );
        assert!(leaf_facts.iter().any(|fact| {
            fact.kind == ProviderFactKind::Export
                && fact.identity == "leaf::Leaf"
                && fact.required_features == BTreeSet::from(["alternate".to_string()])
        }));
        assert!(leaf_facts.iter().any(|fact| {
            fact.kind == ProviderFactKind::Export
                && fact.identity == "leaf::Leaf"
                && fact.required_features == BTreeSet::from(["inner".to_string(), "outer".to_string()])
        }));
        Ok(())
    }

    #[test]
    fn unprojected_provider_collection_rejects_unknown_features_in_inactive_modules()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let source_root = project.path().join("src");
        fs::create_dir_all(&source_root)?;
        fs::write(
            project.path().join("incan.toml"),
            "[project]\nname = \"provider_features\"\n\n[project.features]\ndefault = []\nouter = []\n",
        )?;
        let entry_path = source_root.join("lib.incn");
        fs::write(&entry_path, "when feature(\"outer\"):\n    from nested import Nested\n")?;
        fs::write(
            source_root.join("nested.incn"),
            "when feature(\"missing\"):\n    pub model Nested:\n        value: int\n",
        )?;

        let session = super::super::common::CompilationSession::discover_with_feature_selection(
            &entry_path,
            &FeatureSelection::default(),
        )?;
        let error = collect_unprojected_provider_modules(&entry_path, &session)
            .err()
            .ok_or("unknown feature in inactive provider module should fail collection")?;

        assert!(error.message.contains("Unknown package feature `missing`"));
        Ok(())
    }
}

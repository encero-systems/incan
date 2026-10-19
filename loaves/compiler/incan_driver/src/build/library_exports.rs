//! What a library publishes: its Rust ABI query paths, its checked re-export resolution and its ordinal type
//! identities.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
#[cfg(feature = "rust_inspect")]
use std::time::Instant;

use crate::build::rust_extern::RustExternDeclContext;
use crate::error::{CliError, CliResult};
use crate::project::resolve_project_root;
#[cfg(feature = "rust_inspect")]
use ::rust_inspect::Inspector;
#[cfg(feature = "rust_inspect")]
use ::rust_inspect::InspectorConfig;
#[cfg(feature = "rust_inspect")]
use ::rust_inspect::RustMetadataError;
use incan_frontend::ast::{Declaration, ImportKind};
use incan_frontend::library_exports::{CheckedExportKind, CheckedNamedExport, LibraryExportBindingRegistry};
#[cfg(feature = "rust_inspect")]
use incan_frontend::library_manifest::LibraryRustAbi;
use incan_frontend::module::canonicalize_source_module_segments;
use incan_frontend::{ParsedModule, diagnostics};
use oven_model::manifest::ProjectManifest;

#[cfg(feature = "rust_inspect")]
/// Collect canonical Rust metadata paths that must be shipped in a library manifest's ABI payload.
pub fn collect_library_rust_abi_query_paths(
    modules: &[ParsedModule],
    rust_extern_contexts: &[RustExternDeclContext],
) -> Vec<String> {
    // Consumer prewarming can skip runtime facets. A durable publication must retain its own imported native ABI
    // as well, regardless of that performance policy or the ambient prewarm environment.
    let mut paths = BTreeSet::new();
    for module in modules {
        for declaration in &module.ast.declarations {
            let Declaration::Import(import) = &declaration.node else {
                continue;
            };
            let ImportKind::RustFrom {
                crate_name,
                path,
                items,
                ..
            } = &import.kind
            else {
                continue;
            };
            let base = std::iter::once(crate_name.replace('-', "_"))
                .chain(path.iter().cloned())
                .collect::<Vec<_>>()
                .join("::");
            if matches!(base.as_str(), "std::primitive" | "core::primitive") {
                continue;
            }
            for item in items {
                paths.insert(format!("{base}::{}", item.name));
            }
        }
    }
    for context in rust_extern_contexts {
        paths.insert(format!("{}::{}", context.rust_module_path, context.item_name));
    }
    paths.into_iter().collect()
}

/// Collect ordinary ABI coverage through original source ownership while retaining legacy extern report labels.
///
/// Backend-provided protocol methods require the actual native helper's complete ABI. This adapter changes only
/// genuine ordinary source publication queries; an ordinary consumer or legacy collector cannot acquire it by name.
#[cfg(feature = "rust_inspect")]
pub(crate) fn collect_ordinary_library_rust_abi_query_paths(
    modules: &[ParsedModule],
    contexts: &[RustExternDeclContext],
    plan: &incan_provider::ProviderPlan,
) -> CliResult<Vec<String>> {
    let mut paths = collect_library_rust_abi_query_paths(modules, &[])
        .into_iter()
        .collect::<BTreeSet<_>>();
    for context in contexts {
        paths.insert(crate::build::rust_extern::ordinary_rust_extern_abi_path(
            context, modules, plan,
        )?);
    }
    Ok(paths.into_iter().collect())
}

#[cfg(feature = "rust_inspect")]
/// Extract complete Rust metadata from the generated inspect workspace and package it as manifest ABI.
///
/// Prewarm deliberately permits a fast syntax-only fallback. A library artifact is a durable semantic boundary, so
/// publishing whatever happens to be in that shared cache would make its ABI depend on earlier compiler queries.
pub fn collect_library_rust_abi(
    rust_inspect_manifest_dir: &Path,
    query_paths: &[String],
) -> CliResult<Option<LibraryRustAbi>> {
    if query_paths.is_empty() {
        return Ok(None);
    }

    let inspector = Inspector::new(InspectorConfig::new(rust_inspect_manifest_dir.to_path_buf()));
    let extraction_start = Instant::now();
    let mut items = Vec::new();
    let mut complete_requests = 0;
    for path in query_paths {
        let Some(lookup_path) = Inspector::normalize_lookup_path(path) else {
            continue;
        };
        complete_requests += 1;
        match inspector.cache().get_or_extract_complete_deferred_persist(
            rust_inspect_manifest_dir,
            lookup_path,
            &|_| (),
        ) {
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
    let complete_extraction_ms = extraction_start.elapsed().as_secs_f64() * 1000.0;
    let persistence_start = Instant::now();
    // Persisting metadata is an accelerator, not publication authority; retain the original best-effort behavior.
    if complete_requests > 0
        && let Err(error) = inspector.cache().persist_manifest_dir(rust_inspect_manifest_dir)
    {
        tracing::warn!(
            root = %rust_inspect_manifest_dir.display(),
            error = %error,
            "failed to persist batched public Rust ABI metadata"
        );
    }
    tracing::debug!(
        complete_abi_requests = complete_requests,
        complete_abi_items = items.len(),
        persistence_attempts = usize::from(complete_requests > 0),
        complete_extraction_ms,
        persistence_ms = persistence_start.elapsed().as_secs_f64() * 1000.0,
        root = %rust_inspect_manifest_dir.display(),
        "public Rust ABI metadata batch completed"
    );
    Ok(LibraryRustAbi::from_items(items))
}

/// Select a shipped ABI record only when it can satisfy durable publication without source extraction.
#[cfg(feature = "rust_inspect")]
fn complete_shipped_rust_abi_item(
    index: &incan_frontend::library_manifest_index::LibraryManifestIndex,
    path: &str,
) -> Option<incan_lang::interop::RustItemMetadata> {
    index.rust_abi_item(path).filter(|metadata| match &metadata.kind {
        incan_lang::interop::RustItemKind::Type(info) => {
            info.metadata_completeness.has_methods() && info.metadata_completeness.has_trait_impls()
        }
        _ => true,
    })
}

/// Prepare inspection only for queries not already answered by complete ABI in the checker's dependency index.
#[cfg(feature = "rust_inspect")]
pub(crate) fn library_rust_abi_source_queries(
    query_paths: &[String],
    index: &incan_frontend::library_manifest_index::LibraryManifestIndex,
) -> Vec<String> {
    query_paths
        .iter()
        .filter(|path| complete_shipped_rust_abi_item(index, path).is_none())
        .cloned()
        .collect()
}

/// Reuse the checker's complete shipped ABI records and inspect only demands not covered by that same index.
///
/// The caller retains the dependency owners behind the index throughout checking and publication. Copying their
/// checked facts avoids reconstructing a dependency's inspection workspace. Partial type records still require
/// complete extraction and cannot satisfy a durable ABI requirement by themselves.
#[cfg(feature = "rust_inspect")]
pub(crate) fn collect_library_rust_abi_with_shipped(
    rust_inspect_manifest_dir: Option<&Path>,
    query_paths: &[String],
    index: &incan_frontend::library_manifest_index::LibraryManifestIndex,
) -> CliResult<Option<LibraryRustAbi>> {
    let mut items = Vec::new();
    let mut source_queries = Vec::new();
    for path in query_paths {
        if let Some(metadata) = complete_shipped_rust_abi_item(index, path) {
            items.push(metadata);
        } else {
            source_queries.push(path.clone());
        }
    }
    if !source_queries.is_empty()
        && let Some(directory) = rust_inspect_manifest_dir
        && let Some(extracted) = collect_library_rust_abi(directory, &source_queries)?
    {
        items.extend(extracted.items);
    }
    Ok(LibraryRustAbi::from_items(items))
}

/// Resolve the project root for library commands from an optional source path or project directory.
pub fn resolve_library_project_root(file_path: Option<&str>) -> CliResult<PathBuf> {
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

/// Resolve and require the project's canonical `src/lib.incn` entrypoint.
pub fn validate_library_entrypoint(manifest: &ProjectManifest) -> CliResult<PathBuf> {
    let lib_entry = manifest.project_root().join("src").join("lib.incn");
    if !lib_entry.is_file() {
        return Err(CliError::failure(format!(
            "`incan build --lib` requires `{}`",
            lib_entry.display()
        )));
    }
    Ok(lib_entry)
}

/// Return the generated module key for canonicalized source path segments.
pub fn module_key(path_segments: &[String]) -> String {
    canonicalize_source_module_segments(path_segments).join("_")
}

/// Map exported scalar value enums to the serialized identities used by library consumers.
pub fn public_ordinal_type_identities(
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
        if !matches!(enum_decl.visibility, incan_frontend::ast::Visibility::Public) {
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
        if !matches!(import.visibility, incan_frontend::ast::Visibility::Public) {
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

pub struct LibraryReexportResolver<'a> {
    pub module_exports: &'a HashMap<String, HashMap<String, Vec<CheckedNamedExport>>>,
}

impl<'a> LibraryReexportResolver<'a> {
    /// Create a resolver over checked exports grouped by canonical source-module name and source export name.
    pub fn new(module_exports: &'a HashMap<String, HashMap<String, Vec<CheckedNamedExport>>>) -> Self {
        Self { module_exports }
    }

    /// Resolve direct public declarations and `pub from ... import ...` declarations in a library entrypoint into
    /// checked public exports.
    ///
    /// A single source name can map to several checked exports when the provider exposes same-name overloads. The
    /// resolver therefore preserves all matching exports and only applies the consumer-facing alias to each one.
    pub fn resolve(
        &self,
        lib_module: &ParsedModule,
    ) -> Result<Vec<CheckedNamedExport>, Vec<incan_frontend::diagnostics::CompileError>> {
        let mut errors = Vec::new();
        let mut resolved = Vec::new();
        let mut exported_names = LibraryExportBindingRegistry::default();
        let known_modules: Vec<String> = self.module_exports.keys().cloned().collect();
        let entrypoint_exports = self.module_exports.get(&module_key(&lib_module.path_segments));

        if let Some(exports_by_name) = self.module_exports.get(&module_key(&lib_module.path_segments)) {
            let mut direct_groups = HashSet::new();
            for (export_name, export_span) in Self::direct_public_exports(lib_module) {
                let exports = exports_by_name.get(&export_name);
                // The checked map owns one binding group, including all selected overload declarations. Register
                // that group once; later, distinct import projections still pass through the collision registry.
                if exports.is_some() && !direct_groups.insert(export_name.clone()) {
                    continue;
                }
                if let Err(error) = exported_names.register(&export_name, export_span) {
                    errors.push(error);
                    continue;
                }
                if let Some(exports) = exports {
                    resolved.extend(exports.iter().cloned());
                }
            }
        }

        for decl in &lib_module.ast.declarations {
            let Declaration::Import(import) = &decl.node else {
                continue;
            };
            if !matches!(import.visibility, incan_frontend::ast::Visibility::Public) {
                continue;
            }

            let checked_external_import = match &import.kind {
                ImportKind::RustFrom {
                    crate_name,
                    path,
                    items,
                    ..
                } => Some(("rust", crate_name, path, items)),
                ImportKind::PubFrom { library, path, items } => Some(("pub", library, path, items)),
                _ => None,
            };
            if let Some((namespace, provider, path, items)) = checked_external_import {
                let Some(exports_by_name) = self.module_exports.get(&module_key(&lib_module.path_segments)) else {
                    errors.push(diagnostics::errors::library_reexport_unknown_module(
                        &module_key(&lib_module.path_segments),
                        &known_modules,
                        decl.span,
                    ));
                    continue;
                };
                let mut source_segments = vec![namespace.to_string(), provider.clone()];
                source_segments.extend(path.iter().cloned());
                let source_path = source_segments.join("::");

                for item in items {
                    let exported_name = item.alias.as_ref().unwrap_or(&item.name).clone();
                    if let Err(error) = exported_names.register(&exported_name, decl.span) {
                        errors.push(error);
                        continue;
                    }

                    let Some(exports) = exports_by_name.get(&exported_name) else {
                        let available: Vec<String> = exports_by_name.keys().cloned().collect();
                        errors.push(diagnostics::errors::import_not_exported(
                            &item.name,
                            &source_path,
                            &available,
                            decl.span,
                        ));
                        continue;
                    };
                    resolved.extend(exports.iter().cloned());
                }
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
                if let Err(error) = exported_names.register(&exported_name, decl.span) {
                    errors.push(error);
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
                        .map(|export| export.projected_through_reexport(&exported_name, entrypoint_exports)),
                );
            }
        }

        if errors.is_empty() { Ok(resolved) } else { Err(errors) }
    }

    /// Return public names declared directly by the library entrypoint, excluding public imports that are resolved from
    /// their source module below.
    pub fn direct_public_exports(lib_module: &ParsedModule) -> Vec<(String, incan_frontend::ast::Span)> {
        lib_module
            .ast
            .declarations
            .iter()
            .filter_map(|decl| match &decl.node {
                Declaration::Function(function)
                    if matches!(function.visibility, incan_frontend::ast::Visibility::Public) =>
                {
                    Some((function.name.clone(), decl.span))
                }
                Declaration::Model(model) if matches!(model.visibility, incan_frontend::ast::Visibility::Public) => {
                    Some((model.name.clone(), decl.span))
                }
                Declaration::Class(class) if matches!(class.visibility, incan_frontend::ast::Visibility::Public) => {
                    Some((class.name.clone(), decl.span))
                }
                Declaration::Trait(trait_decl)
                    if matches!(trait_decl.visibility, incan_frontend::ast::Visibility::Public) =>
                {
                    Some((trait_decl.name.clone(), decl.span))
                }
                Declaration::Enum(enum_decl)
                    if matches!(enum_decl.visibility, incan_frontend::ast::Visibility::Public) =>
                {
                    Some((enum_decl.name.clone(), decl.span))
                }
                Declaration::Newtype(newtype_decl)
                    if matches!(newtype_decl.visibility, incan_frontend::ast::Visibility::Public) =>
                {
                    Some((newtype_decl.name.clone(), decl.span))
                }
                Declaration::TypeAlias(alias)
                    if matches!(alias.visibility, incan_frontend::ast::Visibility::Public) =>
                {
                    Some((alias.name.clone(), decl.span))
                }
                Declaration::Const(konst) if matches!(konst.visibility, incan_frontend::ast::Visibility::Public) => {
                    Some((konst.name.clone(), decl.span))
                }
                Declaration::Static(static_decl)
                    if matches!(static_decl.visibility, incan_frontend::ast::Visibility::Public) =>
                {
                    Some((static_decl.name.clone(), decl.span))
                }
                Declaration::Alias(alias) if matches!(alias.visibility, incan_frontend::ast::Visibility::Public) => {
                    Some((alias.name.clone(), decl.span))
                }
                Declaration::Partial(partial)
                    if matches!(partial.visibility, incan_frontend::ast::Visibility::Public) =>
                {
                    Some((partial.name.clone(), decl.span))
                }
                _ => None,
            })
            .collect()
    }
}

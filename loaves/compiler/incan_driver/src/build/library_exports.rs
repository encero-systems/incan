//! What a library publishes: its Rust ABI query paths, its checked re-export resolution and its ordinal type
//! identities.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};

use crate::build::rust_extern::RustExternDeclContext;
use crate::error::{CliError, CliResult};
use crate::project::resolve_project_root;
#[cfg(feature = "rust_inspect")]
use crate::rust_inspect_workspace::collect_rust_inspect_query_paths;
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
pub fn collect_library_rust_abi(
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;

    use crate::build::rust_extern::collect_rust_extern_contexts;
    use incan_frontend::library_exports::{
        CheckedExportIdentity, CheckedExportKind, CheckedNamedExport, checked_exports_by_name,
        collect_checked_public_exports,
    };
    use incan_frontend::symbols::ResolvedType;
    use incan_frontend::{ParsedModule, lexer, parser};
    use oven_model::manifest::ProjectManifest;

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn library_rust_abi_query_paths_include_rust_extern_backing_items() -> Result<(), Box<dyn std::error::Error>> {
        let source =
            "rust.module(\"incan_std_core::num\")\n@rust.extern\npub def gcd_i64(a: int, b: int) -> int:\n  ...\n";
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
            paths.iter().any(|path| path == "incan_std_core::num::gcd_i64"),
            "expected rust.extern backing item in ABI query paths, got: {paths:?}"
        );
        Ok(())
    }

    /// Proves complete published ABI extraction is independent of unrelated downstream impl crates in the graph.
    #[cfg(feature = "rust_inspect")]
    #[test]
    fn library_rust_abi_ignores_loaded_downstream_trait_impls_issue924() -> Result<(), Box<dyn std::error::Error>> {
        // ---- Fixture: clean and downstream-loaded views of one Rust surface ----
        let workspace = tempfile::tempdir()?;
        let trait_api = workspace.path().join("trait-api");
        let surface_api = workspace.path().join("surface-api");
        let downstream_api = workspace.path().join("downstream-api");
        let clean_probe = workspace.path().join("clean-probe");
        let polluted_probe = workspace.path().join("polluted-probe");
        for root in [&trait_api, &surface_api, &downstream_api, &clean_probe, &polluted_probe] {
            fs::create_dir_all(root.join("src"))?;
        }
        fs::write(
            trait_api.join("Cargo.toml"),
            "[package]\nname = \"abi_trait_api\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        fs::write(trait_api.join("src/lib.rs"), "pub trait Intrinsic {}\n")?;
        fs::write(
            surface_api.join("Cargo.toml"),
            "[package]\nname = \"abi_surface_api\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nabi_trait_api = { path = \"../trait-api\" }\n",
        )?;
        fs::write(
            surface_api.join("src/lib.rs"),
            "pub struct Thing;\n\nimpl abi_trait_api::Intrinsic for Thing {}\n",
        )?;
        fs::write(
            downstream_api.join("Cargo.toml"),
            "[package]\nname = \"abi_downstream_api\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nabi_surface_api = { path = \"../surface-api\" }\n",
        )?;
        fs::write(
            downstream_api.join("src/lib.rs"),
            "pub trait Ambient {}\n\nimpl Ambient for abi_surface_api::Thing {}\n",
        )?;
        fs::write(
            clean_probe.join("Cargo.toml"),
            "[package]\nname = \"abi_clean_probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nabi_surface_api = { path = \"../surface-api\" }\n",
        )?;
        fs::write(clean_probe.join("src/lib.rs"), "pub fn load_surface() {}\n")?;
        fs::write(
            polluted_probe.join("Cargo.toml"),
            "[package]\nname = \"abi_polluted_probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nabi_surface_api = { path = \"../surface-api\" }\nabi_downstream_api = { path = \"../downstream-api\" }\n",
        )?;
        fs::write(polluted_probe.join("src/lib.rs"), "pub fn load_graph() {}\n")?;

        // ---- Publication: complete ABI must converge across both loaded graphs ----
        let query_paths = vec!["abi_surface_api::Thing".to_string()];
        let clean = collect_library_rust_abi(&clean_probe, &query_paths)?.ok_or("expected clean library Rust ABI")?;
        let polluted =
            collect_library_rust_abi(&polluted_probe, &query_paths)?.ok_or("expected polluted library Rust ABI")?;

        assert_eq!(
            clean, polluted,
            "published library ABI must not depend on unrelated downstream impl crates"
        );

        // ---- Contract: preserve intrinsic traits and exclude downstream-only traits ----
        let thing = polluted
            .get("abi_surface_api::Thing")
            .ok_or("expected Thing ABI item")?;
        let incan_lang::interop::RustItemKind::Type(thing_type) = &thing.kind else {
            return Err("expected Thing ABI type metadata".into());
        };
        assert!(
            thing_type
                .implemented_traits
                .iter()
                .any(|implemented| implemented.path == "abi_trait_api::Intrinsic")
        );
        assert!(
            thing_type
                .implemented_traits
                .iter()
                .all(|implemented| implemented.path != "abi_downstream_api::Ambient")
        );
        Ok(())
    }

    #[test]
    fn library_entrypoint_precondition_fails_when_missing() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let manifest_path = tmp.path().join("loaf.toml");
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
        let manifest_path = tmp.path().join("loaf.toml");
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
            kind: CheckedExportKind::TypeAlias(incan_frontend::library_exports::CheckedTypeAliasExport {
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
        let public_widget_projection = CheckedNamedExport {
            name: "PublicWidget".to_string(),
            identity: CheckedExportIdentity::reexport(
                vec!["widgets".to_string(), "Widget".to_string()],
                vec!["widgets".to_string(), "Widget".to_string()],
            ),
            kind: CheckedExportKind::Alias(incan_frontend::library_exports::CheckedAliasExport {
                name: "PublicWidget".to_string(),
                target_path: vec!["widgets".to_string(), "Widget".to_string()],
                projected_type: None,
                projected_function: None,
            }),
        };
        module_exports.insert(
            "main".to_string(),
            HashMap::from([("PublicWidget".to_string(), vec![public_widget_projection])]),
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
        assert!(
            matches!(
                resolved[0].identity.projection,
                incan_frontend::library_exports::CheckedExportProjection::Reexport { .. }
            ),
            "the package-root export must retain the checked entrypoint re-export projection"
        );
        Ok(())
    }

    /// A renamed re-export of a callable alias must republish the callable under the new public name.
    ///
    /// The manifest's callable projection describes the binding a consumer resolves at this public name, so leaving
    /// the inner hop's name on it makes the manifest advertise `run` for an export named `public_target`.
    #[test]
    fn resolve_library_reexports_renames_the_callable_an_alias_projects() -> Result<(), Box<dyn std::error::Error>> {
        let source = "pub from provider import run as public_target\n";
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

        let callable = incan_frontend::library_exports::CheckedFunctionExport {
            name: "run".to_string(),
            emitted_name: None,
            type_params: Vec::new(),
            params: Vec::new(),
            param_defaults: Vec::new(),
            return_type: ResolvedType::Int,
            is_async: false,
        };
        let run_export = CheckedNamedExport {
            name: "run".to_string(),
            identity: CheckedExportIdentity::alias(
                vec!["provider".to_string(), "run".to_string()],
                vec!["provider".to_string(), "helper".to_string()],
            ),
            kind: CheckedExportKind::Alias(incan_frontend::library_exports::CheckedAliasExport {
                name: "run".to_string(),
                target_path: vec!["provider".to_string(), "helper".to_string()],
                projected_type: None,
                projected_function: Some(callable),
            }),
        };
        let mut module_exports: HashMap<String, HashMap<String, Vec<CheckedNamedExport>>> = HashMap::new();
        module_exports.insert(
            "provider".to_string(),
            HashMap::from([("run".to_string(), vec![run_export])]),
        );

        let resolved = LibraryReexportResolver::new(&module_exports)
            .resolve(&lib_module)
            .map_err(|errs| format!("{errs:?}"))?;
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "public_target");
        match &resolved[0].kind {
            CheckedExportKind::Alias(alias) => {
                assert_eq!(alias.name, "public_target");
                let projected = alias
                    .projected_function
                    .as_ref()
                    .ok_or("the renamed re-export must keep the callable the alias projects")?;
                assert_eq!(
                    projected.name, "public_target",
                    "the projected callable must carry the name the re-export published it under"
                );
            }
            other => panic!("expected an alias export, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn resolve_library_reexports_accepts_checked_rust_imports() -> Result<(), Box<dyn std::error::Error>> {
        let source = "pub from rust::receiver_factory import PairFactory as PublicPairFactory\n";
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

        let pair_factory_export = CheckedNamedExport {
            name: "PublicPairFactory".to_string(),
            identity: CheckedExportIdentity::reexport(
                vec![
                    "rust".to_string(),
                    "receiver_factory".to_string(),
                    "PairFactory".to_string(),
                ],
                vec![
                    "rust".to_string(),
                    "receiver_factory".to_string(),
                    "PairFactory".to_string(),
                ],
            ),
            kind: CheckedExportKind::Alias(incan_frontend::library_exports::CheckedAliasExport {
                name: "PublicPairFactory".to_string(),
                target_path: vec![
                    "rust".to_string(),
                    "receiver_factory".to_string(),
                    "PairFactory".to_string(),
                ],
                projected_type: None,
                projected_function: None,
            }),
        };
        let mut module_exports: HashMap<String, HashMap<String, Vec<CheckedNamedExport>>> = HashMap::new();
        module_exports.insert(
            "main".to_string(),
            HashMap::from([(pair_factory_export.name.clone(), vec![pair_factory_export])]),
        );

        let resolved = LibraryReexportResolver::new(&module_exports)
            .resolve(&lib_module)
            .map_err(|errs| format!("{errs:?}"))?;
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "PublicPairFactory");
        let incan_frontend::library_exports::CheckedExportProjection::Reexport { target_path } =
            &resolved[0].identity.projection
        else {
            return Err("expected Rust import reexport identity".into());
        };
        assert_eq!(
            target_path,
            &[
                "rust".to_string(),
                "receiver_factory".to_string(),
                "PairFactory".to_string(),
            ]
        );
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
            kind: CheckedExportKind::TypeAlias(incan_frontend::library_exports::CheckedTypeAliasExport {
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

    /// Direct overload declarations share one checked public binding; an additional import projection still collides.
    #[test]
    fn resolve_library_direct_overload_group_once() -> Result<(), Box<dyn std::error::Error>> {
        let source = "pub def select(value: int) -> int:\n    return value\npub def select(value: str) -> str:\n    return value\n";
        let ast = parser::parse(&lexer::lex(source).map_err(|errors| format!("{errors:?}"))?)
            .map_err(|errors| format!("{errors:?}"))?;
        let mut checker = incan_frontend::typechecker::TypeChecker::new();
        checker.set_current_package_identity(Some("producer".into()));
        checker.set_current_module_path(Some(vec!["lib".into()]));
        checker.check_program(&ast).map_err(|errors| format!("{errors:?}"))?;
        let exports = collect_checked_public_exports(&ast, &checker);
        assert_eq!(exports.len(), 2);
        let mut module_exports = HashMap::from([("lib".to_string(), checked_exports_by_name(exports.clone()))]);
        let mut module = ParsedModule {
            name: "lib".into(),
            path_segments: vec!["lib".into()],
            file_path: PathBuf::from("project/src/lib.incn"),
            source: source.into(),
            ast,
        };
        let resolved = LibraryReexportResolver::new(&module_exports)
            .resolve(&module)
            .map_err(|errors| format!("{errors:?}"))?;
        assert_eq!(resolved.len(), 2, "the complete group must be emitted exactly once");
        assert_eq!(resolved[0].identity.canonical, exports[0].identity.canonical);
        assert_eq!(resolved[1].identity.canonical, exports[1].identity.canonical);
        assert_ne!(resolved[0].identity.canonical, resolved[1].identity.canonical);

        module_exports.insert("other".into(), checked_exports_by_name(exports));
        module.source.push_str("pub from other import select\n");
        module.ast = parser::parse(&lexer::lex(&module.source).map_err(|errors| format!("{errors:?}"))?)
            .map_err(|errors| format!("{errors:?}"))?;
        let errors = LibraryReexportResolver::new(&module_exports)
            .resolve(&module)
            .err()
            .ok_or("distinct public projection did not collide with the checked overload binding")?;
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("Duplicate library export `select`"))
        );
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
            kind: CheckedExportKind::TypeAlias(incan_frontend::library_exports::CheckedTypeAliasExport {
                name: "DataSet".to_string(),
                type_params: Vec::new(),
                target: ResolvedType::Named("DataSet".to_string()),
            }),
        };
        let filter_export = CheckedNamedExport {
            name: "filter_ds".to_string(),
            identity: CheckedExportIdentity::direct(vec!["dataset_ops".to_string(), "filter_ds".to_string()]),
            kind: CheckedExportKind::Function(incan_frontend::library_exports::CheckedFunctionExport {
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
            kind: CheckedExportKind::TypeAlias(incan_frontend::library_exports::CheckedTypeAliasExport {
                name: "DataSet".to_string(),
                type_params: Vec::new(),
                target: ResolvedType::Named("DataSet".to_string()),
            }),
        };
        let filter_export = CheckedNamedExport {
            name: "filter_ds".to_string(),
            identity: CheckedExportIdentity::direct(vec!["dataset_ops".to_string(), "filter_ds".to_string()]),
            kind: CheckedExportKind::Function(incan_frontend::library_exports::CheckedFunctionExport {
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
}

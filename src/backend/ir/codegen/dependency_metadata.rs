//! Dependency metadata planning for IR code generation.

use std::collections::{HashMap, HashSet};

use crate::frontend::ast::{self, Declaration, Expr, ImportKind, ImportPath, Program};
use crate::frontend::decorator_resolution;
use crate::frontend::module::canonicalize_source_module_segments;
use crate::frontend::typechecker::stdlib_loader::StdlibAstCache;
use incan_core::lang::stdlib;
use incan_core::lang::traits::{self as core_traits, TraitId};

pub(super) fn collect_model_field_aliases(
    main: &Program,
    deps: &[(&str, &Program)],
) -> HashMap<String, HashMap<String, String>> {
    let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();

    let mut visit = |p: &Program| {
        for decl in &p.declarations {
            let Declaration::Model(m) = &decl.node else {
                continue;
            };

            let mut map: HashMap<String, String> = HashMap::new();
            for f in &m.fields {
                if let Some(alias) = &f.node.metadata.alias {
                    map.insert(alias.clone(), f.node.name.clone());
                }
            }

            if !map.is_empty() {
                out.entry(m.name.clone()).or_default().extend(map);
            }
        }
    };

    visit(main);
    for (_, dep) in deps {
        visit(dep);
    }

    out
}

/// Resolve a source import path to the generated Rust module path used for dependency emission.
fn generated_module_path_for_source_import(path: &ImportPath, current_module_path: &[String]) -> Option<Vec<String>> {
    let resolved_segments = if path.parent_levels > 0 {
        let keep = current_module_path.len().checked_sub(path.parent_levels)?;
        let mut resolved = current_module_path[..keep].to_vec();
        resolved.extend(path.segments.clone());
        resolved
    } else {
        path.segments.clone()
    };
    let mut segments = canonicalize_source_module_segments(&resolved_segments);

    if segments.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT) {
        segments[0] = stdlib::INCAN_STD_NAMESPACE.to_string();
    }

    Some(segments)
}

/// True when a dependency module should keep its public API even if the main module does not import every item.
pub(super) fn should_preserve_dependency_public_items(
    module_path: &[String],
    preserve_non_stdlib_public_items: bool,
) -> bool {
    if matches!(
        module_path.first().map(String::as_str),
        Some(stdlib::STDLIB_ROOT | stdlib::INCAN_STD_NAMESPACE)
    ) {
        return true;
    }
    preserve_non_stdlib_public_items
}

/// Return whether a function carries the stdlib-backed web route decorator that lowers to a Rust proc-macro attribute.
fn has_web_route_passthrough_decorator(
    func: &ast::FunctionDecl,
    aliases: &HashMap<String, Vec<String>>,
    stdlib_cache: &mut StdlibAstCache,
) -> bool {
    func.decorators.iter().any(|decorator| {
        let resolved = decorator_resolution::resolve_decorator_path(&decorator.node, aliases);
        if resolved.len() < 2 {
            return false;
        }
        let module_segments = &resolved[..resolved.len() - 1];
        let name = &resolved[resolved.len() - 1];
        if name != "route" {
            return false;
        }
        let Some(meta) = stdlib_cache.lookup_function_meta(module_segments, name) else {
            return false;
        };
        meta.is_rust_extern && meta.rust_module_path.as_deref() == Some("incan_web_macros")
    })
}

/// Collect dependency-module declarations that must remain reachable from externally visible roots such as imports,
/// ambient logging, and web route registration.
pub(super) fn collect_externally_reachable_items_by_module(
    main: &Program,
    dependency_modules: &[(&str, &Program, Option<Vec<String>>)],
) -> HashMap<Vec<String>, HashSet<String>> {
    let module_paths: HashSet<Vec<String>> = dependency_modules
        .iter()
        .map(|(name, _, path_segments)| path_segments.clone().unwrap_or_else(|| vec![(*name).to_string()]))
        .collect();

    fn record_imports(
        reachable: &mut HashMap<Vec<String>, HashSet<String>>,
        program: &Program,
        current_module_path: &[String],
        module_paths: &HashSet<Vec<String>>,
    ) {
        if crate::frontend::surface_semantics::uses_ambient_log_surface(program) {
            reachable
                .entry(vec!["std".to_string(), "logging".to_string()])
                .or_default()
                .insert("get_logger".to_string());
        }
        let mut module_import_bindings: HashMap<String, Vec<String>> = HashMap::new();
        for decl in &program.declarations {
            let Declaration::Import(import) = &decl.node else {
                continue;
            };
            match &import.kind {
                ImportKind::From { module, items } => {
                    let Some(module_path) = generated_module_path_for_source_import(module, current_module_path) else {
                        continue;
                    };
                    let reachable_items = reachable.entry(module_path).or_default();
                    for item in items {
                        reachable_items.insert(item.name.clone());
                    }
                }
                ImportKind::Module(path) => {
                    let Some(segments) = generated_module_path_for_source_import(path, current_module_path) else {
                        continue;
                    };
                    if module_paths.contains(&segments) {
                        if let Some(binding) = import.alias.clone().or_else(|| path.segments.last().cloned()) {
                            module_import_bindings.insert(binding, segments);
                        }
                        continue;
                    }
                    let Some(item_name) = segments.last() else {
                        continue;
                    };
                    for module_path in module_paths {
                        if segments.len() == module_path.len() + 1 && segments.starts_with(module_path) {
                            reachable
                                .entry(module_path.clone())
                                .or_default()
                                .insert(item_name.clone());
                            break;
                        }
                    }
                }
                ImportKind::PubLibrary { .. }
                | ImportKind::PubFrom { .. }
                | ImportKind::RustCrate { .. }
                | ImportKind::RustFrom { .. }
                | ImportKind::Python(_) => {}
            }
        }
        if !module_import_bindings.is_empty() {
            let _ = crate::frontend::ast_walk::any_expr_in_program(program, |expr| {
                if let Expr::Field(object, field) = expr
                    && let Expr::Ident(binding) = &object.node
                    && let Some(module_path) = module_import_bindings.get(binding)
                {
                    reachable.entry(module_path.clone()).or_default().insert(field.clone());
                }
                if let Expr::MethodCall(object, method, _, _) = expr
                    && let Expr::Ident(binding) = &object.node
                    && let Some(module_path) = module_import_bindings.get(binding)
                {
                    reachable.entry(module_path.clone()).or_default().insert(method.clone());
                }
                false
            });
        }
        if module_paths.contains(current_module_path) {
            let aliases = decorator_resolution::collect_import_aliases(program);
            let mut stdlib_cache = StdlibAstCache::new();
            for decl in &program.declarations {
                let Declaration::Function(func) = &decl.node else {
                    continue;
                };
                if has_web_route_passthrough_decorator(func, &aliases, &mut stdlib_cache) {
                    reachable
                        .entry(current_module_path.to_vec())
                        .or_default()
                        .insert(func.name.clone());
                }
            }
        }
    }

    let mut reachable = HashMap::new();
    record_imports(&mut reachable, main, &[String::from("main")], &module_paths);
    for (name, program, path_segments) in dependency_modules {
        let module_path = path_segments.clone().unwrap_or_else(|| vec![(*name).to_string()]);
        record_imports(&mut reachable, program, &module_path, &module_paths);
    }
    reachable
}

/// Dependency symbol facts gathered during codegen setup and reused by module emission.
#[derive(Debug, Clone, Default)]
pub(super) struct DependencySymbolMetadata {
    pub(super) module_paths: HashMap<String, Vec<String>>,
    pub(super) ambiguous_type_names: HashSet<String>,
    pub(super) value_module_paths: HashMap<String, Vec<String>>,
    pub(super) ambiguous_value_names: HashSet<String>,
    pub(super) enum_type_names: HashSet<String>,
    pub(super) error_trait_type_names: HashSet<String>,
}

/// Collect dependency symbol metadata needed by IR emission for cross-module nominal types and values.
pub(super) fn collect_dependency_symbol_metadata(
    deps: &[(&str, &Program, Option<Vec<String>>)],
) -> DependencySymbolMetadata {
    let mut paths: HashMap<String, Vec<String>> = HashMap::new();
    let mut ambiguous: HashSet<String> = HashSet::new();
    let mut value_paths: HashMap<String, Vec<String>> = HashMap::new();
    let mut ambiguous_values: HashSet<String> = HashSet::new();
    let mut enum_type_names: HashSet<String> = HashSet::new();
    let mut non_enum_type_names: HashSet<String> = HashSet::new();
    let mut error_trait_type_names: HashSet<String> = HashSet::new();
    let error_trait_name = core_traits::as_str(TraitId::Error);

    for (_name, program, path_segments) in deps {
        for decl in &program.declarations {
            if let Some(segs) = path_segments.as_ref()
                && let Some(name) = match &decl.node {
                    Declaration::Const(c) => Some(&c.name),
                    Declaration::Static(s) => Some(&s.name),
                    Declaration::Function(f) => Some(&f.name),
                    Declaration::Partial(p) => Some(&p.name),
                    Declaration::Alias(a) => Some(&a.name),
                    Declaration::Import(_)
                    | Declaration::Model(_)
                    | Declaration::Class(_)
                    | Declaration::Trait(_)
                    | Declaration::TypeAlias(_)
                    | Declaration::Newtype(_)
                    | Declaration::Enum(_)
                    | Declaration::TestModule(_)
                    | Declaration::Docstring(_) => None,
                }
            {
                if let Some(existing) = value_paths.get(name) {
                    if existing != segs {
                        ambiguous_values.insert(name.clone());
                    }
                } else {
                    value_paths.insert(name.clone(), segs.clone());
                }
            }

            let type_name = match &decl.node {
                Declaration::Model(m) => {
                    if m.traits.iter().any(|bound| bound.node.name == error_trait_name) {
                        error_trait_type_names.insert(m.name.clone());
                    }
                    Some((&m.name, false))
                }
                Declaration::Class(c) => {
                    if c.traits.iter().any(|bound| bound.node.name == error_trait_name) {
                        error_trait_type_names.insert(c.name.clone());
                    }
                    Some((&c.name, false))
                }
                Declaration::Enum(e) => Some((&e.name, true)),
                Declaration::TypeAlias(a) => Some((&a.name, false)),
                Declaration::Newtype(n) => Some((&n.name, false)),
                _ => None,
            };
            let Some((name, is_enum)) = type_name else {
                continue;
            };

            if is_enum {
                enum_type_names.insert(name.clone());
            } else {
                non_enum_type_names.insert(name.clone());
            }

            let Some(segs) = path_segments.as_ref() else {
                continue;
            };

            if let Some(existing) = paths.get(name) {
                if existing != segs {
                    ambiguous.insert(name.clone());
                }
            } else {
                paths.insert(name.clone(), segs.clone());
            }
        }
    }

    for name in &ambiguous {
        paths.remove(name);
    }
    for name in &ambiguous_values {
        value_paths.remove(name);
    }
    enum_type_names.retain(|name| !ambiguous.contains(name) && !non_enum_type_names.contains(name));

    DependencySymbolMetadata {
        module_paths: paths,
        ambiguous_type_names: ambiguous,
        value_module_paths: value_paths,
        ambiguous_value_names: ambiguous_values,
        enum_type_names,
        error_trait_type_names,
    }
}

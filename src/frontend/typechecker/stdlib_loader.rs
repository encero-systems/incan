//! RFC 023: Stdlib `.incn` file loader.
//!
//! This module provides infrastructure for loading, parsing, and extracting function and trait signatures from stdlib
//! `.incn` files. It replaces hardcoded function registries with signatures derived from the actual source files.
//!
//! ## Design
//!
//! 1. **Discovery**: finds stdlib `.incn` files using `incan_core::lang::stdlib::stdlib_stub_path`.
//! 2. **Parsing**: lexes and parses the file through the normal Incan frontend pipeline.
//! 3. **Extraction**: walks the parsed AST to extract `FunctionInfo` entries for each `def` and `TraitInfo` entries for
//!    each `trait`.
//! 4. **Caching**: results are cached per module path in a `HashMap` to avoid redundant parsing.
//!
//! ## Re-export resolution
//!
//! Modules with submodules (e.g. `std.web`) resolve to a prelude file (e.g. `stdlib/web/prelude.incn`).
//! The prelude typically only contains `from std.web.<sub> import ...` re-export statements, not direct declarations.
//! To support `from std.web import route` (where `route` is declared in `std.web.routing`), the loader follows these
//! re-export imports and merges the referenced submodule metadata into the parent.
//!
//! ## Limitations
//!
//! - Function signatures are extracted from top-level `def` declarations (not methods on classes/models).
//! - Trait signatures are extracted from top-level `trait` declarations with their methods.
//! - Default parameter values are not captured (only the parameter name and type).
//! - Complex types beyond the common set (`int`, `str`, `bool`, `Option[T]`, etc.) are treated as `Named`.
//! - Parse failures are logged and the module is treated as unavailable for AST-derived signature lookup.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::frontend::ast;
use crate::frontend::symbols::VariableInfo;
use crate::frontend::symbols::{FunctionInfo, MethodInfo, ResolvedType, TraitInfo};
use incan_core::lang::conventions;
use incan_core::lang::decorators::{self, DecoratorId};
use incan_core::lang::stdlib;
use incan_core::lang::surface::functions::{self as surface_functions, SurfaceFnId};
use incan_core::lang::types::collections::{self as collection_types, CollectionTypeId};
use incan_core::lang::types::numerics::{self as numeric_types, NumericTypeId};
use incan_core::lang::types::stringlike::{self as string_types, StringLikeId};

#[derive(Debug, Clone, Default)]
struct StdlibModuleData {
    functions: Vec<(String, FunctionInfo)>,
    traits: Vec<(String, TraitInfo)>,
    constants: Vec<(String, VariableInfo)>,
    function_meta: HashMap<String, FunctionMeta>,
    trait_meta: HashMap<String, TraitMeta>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FunctionMeta {
    pub is_rust_extern: bool,
    pub rust_module_path: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TraitMeta {
    pub rust_module_path: Option<String>,
}

/// Cached stdlib module signatures keyed by dot-joined module path (e.g. `"std.testing"`).
#[derive(Debug, Default)]
pub(crate) struct StdlibAstCache {
    /// Map from module path (dot-joined) to extracted stdlib module data.
    cache: HashMap<String, StdlibModuleData>,
}

impl StdlibAstCache {
    pub fn new() -> Self {
        Self { cache: HashMap::new() }
    }

    /// Look up a specific function in a stdlib module.
    ///
    /// Returns `Some(FunctionInfo)` if the module has been loaded and contains a function with the given name.
    pub fn lookup_function(&mut self, module_path: &[String], function_name: &str) -> Option<FunctionInfo> {
        self.ensure_loaded(module_path);
        let key = module_path.join(".");
        self.cache
            .get(&key)?
            .functions
            .iter()
            .find(|(name, _)| name == function_name)
            .map(|(_, info)| info.clone())
    }

    /// Look up a specific trait in a stdlib module.
    ///
    /// Returns `Some(TraitInfo)` if the module has been loaded and contains a trait with the given name.
    pub fn lookup_trait(&mut self, module_path: &[String], trait_name: &str) -> Option<TraitInfo> {
        self.ensure_loaded(module_path);
        let key = module_path.join(".");
        self.cache
            .get(&key)?
            .traits
            .iter()
            .find(|(name, _)| name == trait_name)
            .map(|(_, info)| info.clone())
    }

    /// Look up a specific const binding in a stdlib module.
    pub fn lookup_constant(&mut self, module_path: &[String], const_name: &str) -> Option<VariableInfo> {
        self.ensure_loaded(module_path);
        let key = module_path.join(".");
        self.cache
            .get(&key)?
            .constants
            .iter()
            .find(|(name, _)| name == const_name)
            .map(|(_, info)| info.clone())
    }

    /// Look up metadata for a specific function in a stdlib module.
    pub fn lookup_function_meta(&mut self, module_path: &[String], function_name: &str) -> Option<FunctionMeta> {
        self.ensure_loaded(module_path);
        let key = module_path.join(".");
        self.cache.get(&key)?.function_meta.get(function_name).cloned()
    }

    /// Look up metadata for a specific trait in a stdlib module.
    pub fn lookup_trait_meta(&mut self, module_path: &[String], trait_name: &str) -> Option<TraitMeta> {
        self.ensure_loaded(module_path);
        let key = module_path.join(".");
        self.cache.get(&key)?.trait_meta.get(trait_name).cloned()
    }

    /// Ensure a module is loaded into the cache, loading it on first access.
    fn ensure_loaded(&mut self, module_path: &[String]) {
        let key = module_path.join(".");
        self.cache
            .entry(key)
            .or_insert_with(|| load_stdlib_module_data(module_path).unwrap_or_default());
    }
}

/// Load and parse a stdlib `.incn` file, extracting module data.
///
/// For modules with submodules (whose stub path resolves to a prelude file), this function also follows
/// `from std.<ns>.<submodule> import <name>` re-exports: it loads each referenced submodule and merges
/// the imported names' metadata into the parent module. This enables `from std.web import route` to resolve
/// decorator metadata even though `route` is declared in `std.web.routing`, not the prelude itself.
///
/// Returns `None` if the file cannot be found or parsed.
fn load_stdlib_module_data(module_path: &[String]) -> Option<StdlibModuleData> {
    let relative = stdlib::stdlib_stub_path(module_path)?;
    let abs_path = find_stdlib_file(&relative)?;

    let source = std::fs::read_to_string(&abs_path)
        .map_err(|e| {
            tracing::debug!(path = %abs_path.display(), error = %e, "failed to read stdlib file");
        })
        .ok()?;

    let tokens = crate::frontend::lexer::lex(&source)
        .map_err(|e| {
            tracing::debug!(path = %abs_path.display(), error = ?e, "failed to lex stdlib file");
        })
        .ok()?;

    let program = crate::frontend::parser::parse(&tokens)
        .map_err(|e| {
            tracing::debug!(path = %abs_path.display(), error = ?e, "failed to parse stdlib file");
        })
        .ok()?;

    let mut functions = extract_function_signatures(&program);
    let mut traits = extract_trait_signatures(&program);
    let mut constants = extract_const_signatures(&program);
    let mut function_meta = extract_function_meta(&program);
    let mut trait_meta = extract_trait_meta(&program);

    // ---- Follow prelude re-exports ----
    // If this module is a prelude (has submodules), its declarations are just `from ... import ...` re-exports.
    // We recursively load each referenced submodule and merge the imported names' metadata so that
    // `lookup_function_meta(["std", "web"], "route")` finds `route` even though it's declared in
    // `std.web.routing`.
    merge_reexported_metadata(
        &program,
        &mut functions,
        &mut traits,
        &mut constants,
        &mut function_meta,
        &mut trait_meta,
    );

    Some(StdlibModuleData {
        functions,
        traits,
        constants,
        function_meta,
        trait_meta,
    })
}

/// Scan a program's import declarations and merge metadata from referenced stdlib submodules.
///
/// For each `from std.<ns>.<sub> import name1, name2` statement, loads the submodule and copies the
/// corresponding function/trait signatures and metadata into the parent module's collections.
fn merge_reexported_metadata(
    program: &ast::Program,
    functions: &mut Vec<(String, FunctionInfo)>,
    traits: &mut Vec<(String, TraitInfo)>,
    constants: &mut Vec<(String, VariableInfo)>,
    function_meta: &mut HashMap<String, FunctionMeta>,
    trait_meta: &mut HashMap<String, TraitMeta>,
) {
    for decl in &program.declarations {
        let ast::Declaration::Import(import) = &decl.node else {
            continue;
        };
        let ast::ImportKind::From { module, items } = &import.kind else {
            continue;
        };

        // Only follow stdlib re-exports (paths starting with "std").
        if module.segments.first().is_none_or(|s| s != stdlib::STDLIB_ROOT) {
            continue;
        }
        if module.segments.len() < 3 {
            continue;
        }

        let Some(sub_data) = load_stdlib_module_data(&module.segments) else {
            continue;
        };

        for item in items {
            let effective_name = item.alias.as_deref().unwrap_or(&item.name);

            // Merge function signature.
            if let Some((_, info)) = sub_data.functions.iter().find(|(n, _)| n == &item.name)
                && !functions.iter().any(|(n, _)| n == effective_name)
            {
                functions.push((effective_name.to_string(), info.clone()));
            }

            // Merge trait signature.
            if let Some((_, info)) = sub_data.traits.iter().find(|(n, _)| n == &item.name)
                && !traits.iter().any(|(n, _)| n == effective_name)
            {
                traits.push((effective_name.to_string(), info.clone()));
            }

            // Merge const signature.
            if let Some((_, info)) = sub_data.constants.iter().find(|(n, _)| n == &item.name)
                && !constants.iter().any(|(n, _)| n == effective_name)
            {
                constants.push((effective_name.to_string(), info.clone()));
            }

            // Merge function meta.
            if let Some(meta) = sub_data.function_meta.get(&item.name) {
                function_meta
                    .entry(effective_name.to_string())
                    .or_insert_with(|| meta.clone());
            }

            // Merge trait meta.
            if let Some(meta) = sub_data.trait_meta.get(&item.name) {
                trait_meta
                    .entry(effective_name.to_string())
                    .or_insert_with(|| meta.clone());
            }
        }
    }
}

/// Find the absolute path for a stdlib file given its relative path (e.g. `"stdlib/testing.incn"`).
///
/// Search order:
/// 1. `$INCAN_STDLIB_DIR/<relative>` if the env var is set
/// 2. `$CARGO_MANIFEST_DIR/crates/incan_stdlib/<relative>` (stdlib crate-local stubs)
/// 3. `$CARGO_MANIFEST_DIR/<relative>` (workspace-root stubs)
/// 4. `$CWD/crates/incan_stdlib/<relative>`
/// 5. `$CWD/<relative>`
fn find_stdlib_file(relative: &str) -> Option<PathBuf> {
    // 1. Explicit override root.
    if let Ok(dir) = std::env::var("INCAN_STDLIB_DIR") {
        let p = PathBuf::from(dir).join(relative);
        if p.exists() {
            return Some(p);
        }
    }

    // 2-3. Development builds (CARGO_MANIFEST_DIR points to workspace root for `incan`).
    if let Ok(dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let manifest_dir = PathBuf::from(dir);
        let crate_local = manifest_dir.join("crates/incan_stdlib").join(relative);
        if crate_local.exists() {
            return Some(crate_local);
        }
        let workspace_local = manifest_dir.join(relative);
        if workspace_local.exists() {
            return Some(workspace_local);
        }
    }

    // 4-5. Relative to current working directory.
    if let Ok(cwd) = std::env::current_dir() {
        let crate_local = cwd.join("crates/incan_stdlib").join(relative);
        if crate_local.exists() {
            return Some(crate_local);
        }
        let local = cwd.join(relative);
        if local.exists() {
            return Some(local);
        }
    }

    tracing::debug!(relative_path = %relative, "stdlib file not found in any search path");
    None
}

/// Extract function signatures from a parsed stdlib `.incn` program.
///
/// Only top-level `def` declarations are extracted. Methods and other declarations are ignored.
fn extract_function_signatures(program: &ast::Program) -> Vec<(String, FunctionInfo)> {
    let mut fns = Vec::new();
    for decl in &program.declarations {
        if let ast::Declaration::Function(func) = &decl.node {
            let info = function_decl_to_info(func);
            fns.push((func.name.clone(), info));
            continue;
        }

        if let ast::Declaration::Import(import) = &decl.node {
            let ast::ImportKind::RustFrom { items, .. } = &import.kind else {
                continue;
            };
            for item in items {
                let local_name = item.alias.as_deref().unwrap_or(&item.name);
                if let Some(info) = imported_runtime_function_info(local_name)
                    && !fns.iter().any(|(name, _)| name == local_name)
                {
                    fns.push((local_name.to_string(), info));
                }
            }
        }
    }
    fns
}

/// Extract public const bindings from a parsed stdlib `.incn` program.
fn extract_const_signatures(program: &ast::Program) -> Vec<(String, VariableInfo)> {
    let mut consts = Vec::new();
    for decl in &program.declarations {
        let ast::Declaration::Const(konst) = &decl.node else {
            continue;
        };
        let ty = konst
            .ty
            .as_ref()
            .map(|ty| ast_type_to_resolved(&ty.node, &[]))
            .unwrap_or(ResolvedType::Unknown);
        consts.push((
            konst.name.clone(),
            VariableInfo {
                ty,
                is_mutable: false,
                is_used: false,
            },
        ));
    }
    consts
}

/// Extract trait signatures from a parsed stdlib `.incn` program.
///
/// Top-level `trait` declarations are extracted with their method signatures. `@requires` decorators are not
/// resolved (requires fields are left empty) since stdlib traits typically don't use them.
fn extract_trait_signatures(program: &ast::Program) -> Vec<(String, TraitInfo)> {
    let mut traits = Vec::new();
    for decl in &program.declarations {
        if let ast::Declaration::Trait(tr) = &decl.node {
            let tp_names: Vec<String> = tr.type_params.iter().map(|tp| tp.name.clone()).collect();
            let methods = extract_method_signatures(&tr.methods, &tp_names);
            traits.push((
                tr.name.clone(),
                TraitInfo {
                    type_params: tp_names,
                    supertraits: Vec::new(),
                    methods,
                    requires: Vec::new(),
                },
            ));
        }
    }
    traits
}

/// Extract method signatures from AST method declarations.
///
/// Converts each `MethodDecl` into a `MethodInfo` using lightweight type resolution (no full typechecker needed).
fn extract_method_signatures(
    methods: &[ast::Spanned<ast::MethodDecl>],
    type_params: &[String],
) -> HashMap<String, MethodInfo> {
    methods
        .iter()
        .map(|m| {
            let params: Vec<(String, ResolvedType)> = m
                .node
                .params
                .iter()
                .map(|p| (p.node.name.clone(), ast_type_to_resolved(&p.node.ty.node, type_params)))
                .collect();
            let return_type = ast_type_to_resolved(&m.node.return_type.node, type_params);
            (
                m.node.name.clone(),
                MethodInfo {
                    receiver: m.node.receiver,
                    params,
                    return_type,
                    is_async: m.node.is_async(),
                    has_body: m.node.body.is_some(),
                },
            )
        })
        .collect()
}

/// Convert an AST `FunctionDecl` to a typechecker `FunctionInfo`.
fn function_decl_to_info(func: &ast::FunctionDecl) -> FunctionInfo {
    // Extract just the type parameter names for type resolution.
    let tp_names: Vec<String> = func.type_params.iter().map(|tp| tp.name.clone()).collect();
    let tp_bounds: HashMap<String, Vec<String>> = func
        .type_params
        .iter()
        .map(|tp| {
            (
                tp.name.clone(),
                tp.bounds.iter().map(|bound| bound.name.clone()).collect(),
            )
        })
        .collect();

    let params: Vec<(String, ResolvedType)> = func
        .params
        .iter()
        .map(|p| (p.node.name.clone(), ast_type_to_resolved(&p.node.ty.node, &tp_names)))
        .collect();

    let return_type = ast_type_to_resolved(&func.return_type.node, &tp_names);

    FunctionInfo {
        params,
        return_type,
        is_async: func.is_async(),
        type_params: tp_names,
        type_param_bounds: tp_bounds,
    }
}

/// Build a lightweight `FunctionInfo` for the remaining generic Rust leaves that still need direct stdlib imports.
///
/// We intentionally keep this list narrow: only helpers whose Rust-side bounds are not yet representable by the
/// language surface stay on this path. Public stdlib functions that can be declared locally should prefer real `.incn`
/// definitions so their signatures come straight from the AST.
fn imported_runtime_function_info(name: &str) -> Option<FunctionInfo> {
    let (params, return_type, is_async) = match surface_functions::from_str(name)? {
        SurfaceFnId::Timeout => (
            vec![
                ("seconds".to_string(), ResolvedType::Float),
                ("task".to_string(), ResolvedType::Unknown),
            ],
            ResolvedType::Unknown,
            true,
        ),
        SurfaceFnId::TimeoutMs => (
            vec![
                ("milliseconds".to_string(), ResolvedType::Int),
                ("task".to_string(), ResolvedType::Unknown),
            ],
            ResolvedType::Unknown,
            true,
        ),
        SurfaceFnId::SelectTimeout => (
            vec![
                ("seconds".to_string(), ResolvedType::Float),
                ("task".to_string(), ResolvedType::Unknown),
            ],
            ResolvedType::Unknown,
            true,
        ),
        SurfaceFnId::Spawn => (
            vec![("task".to_string(), ResolvedType::Unknown)],
            ResolvedType::Unknown,
            false,
        ),
        SurfaceFnId::SpawnBlocking => (
            vec![("task".to_string(), ResolvedType::Unknown)],
            ResolvedType::Unknown,
            true,
        ),
        _ => return None,
    };

    Some(FunctionInfo {
        params,
        return_type,
        is_async,
        type_params: Vec::new(),
        type_param_bounds: HashMap::new(),
    })
}

/// Extract function metadata from a stdlib module's AST.
///
/// Walks top-level function declarations and records:
/// - Whether the function has the `@rust.extern` decorator (indicating delegation to a Rust backing module).
/// - The `rust.module()` path declared on the program, if any.
///
/// This metadata is stored in [`StdlibAstCache`] and used during lowering to decide whether a decorator reference
/// should be emitted as a Rust attribute (passthrough) or compiled normally.
fn extract_function_meta(program: &ast::Program) -> HashMap<String, FunctionMeta> {
    let mut meta = HashMap::new();
    let rust_module_path = program.rust_module_path.as_ref().map(|sp| sp.node.clone());
    for decl in &program.declarations {
        if let ast::Declaration::Function(func) = &decl.node {
            let is_rust_extern = func
                .decorators
                .iter()
                .any(|d| decorators::from_str(&d.node.path.segments.join(".")) == Some(DecoratorId::RustExtern));
            meta.insert(
                func.name.clone(),
                FunctionMeta {
                    is_rust_extern,
                    rust_module_path: rust_module_path.clone(),
                },
            );
        }
    }
    meta
}

/// Extract trait metadata from a stdlib module's AST.
///
/// Walks top-level trait declarations and records the `rust.module()` backing path (if any).
/// This metadata is used by [`LoweringHelpers::resolve_derive_module_path`] to map `@derive(Trait)` references
/// to their Rust proc-macro crate paths for derive passthrough.
fn extract_trait_meta(program: &ast::Program) -> HashMap<String, TraitMeta> {
    let mut meta = HashMap::new();
    let rust_module_path = program.rust_module_path.as_ref().map(|sp| sp.node.clone());
    for decl in &program.declarations {
        if let ast::Declaration::Trait(tr) = &decl.node {
            meta.insert(
                tr.name.clone(),
                TraitMeta {
                    rust_module_path: rust_module_path.clone(),
                },
            );
        }
    }
    meta
}

/// Convert an AST `Type` to a `ResolvedType`.
///
/// Type parameter names (from the enclosing function's `type_params`) are resolved to `TypeVar`.
/// Primitive type names are resolved to their concrete `ResolvedType` variants.
/// Unknown types are resolved to `Named(name)`.
fn ast_type_to_resolved(ty: &ast::Type, type_params: &[String]) -> ResolvedType {
    match ty {
        ast::Type::Unit => ResolvedType::Unit,
        ast::Type::SelfType => ResolvedType::SelfType,
        ast::Type::Simple(name) => {
            // Check if it's a type parameter first.
            if type_params.contains(name) {
                return ResolvedType::TypeVar(name.clone());
            }

            // Resolve through incan_core registries (numerics, strings, unit).
            if let Some(id) = numeric_types::from_str(name) {
                return match id {
                    NumericTypeId::Int => ResolvedType::Int,
                    NumericTypeId::Float => ResolvedType::Float,
                    NumericTypeId::Bool => ResolvedType::Bool,
                };
            }
            if let Some(id) = string_types::from_str(name) {
                return match id {
                    StringLikeId::Str => ResolvedType::Str,
                    StringLikeId::Bytes => ResolvedType::Bytes,
                    // Frozen variants are named types in this context.
                    _ => ResolvedType::Named(name.clone()),
                };
            }
            if name == conventions::NONE_TYPE_NAME {
                return ResolvedType::Unit;
            }

            ResolvedType::Named(name.clone())
        }
        ast::Type::Generic(name, args) => {
            let resolved_args: Vec<ResolvedType> = args
                .iter()
                .map(|a| ast_type_to_resolved(&a.node, type_params))
                .collect();

            let collection_id = collection_types::from_str(name);
            match collection_id {
                Some(CollectionTypeId::Option) => {
                    let canonical = collection_types::as_str(CollectionTypeId::Option).to_string();
                    if let Some(inner) = resolved_args.into_iter().next() {
                        ResolvedType::Generic(canonical, vec![inner])
                    } else {
                        ResolvedType::Named(canonical)
                    }
                }
                Some(CollectionTypeId::Result) => {
                    let canonical = collection_types::as_str(CollectionTypeId::Result).to_string();
                    if resolved_args.len() == 2 {
                        ResolvedType::Generic(canonical, resolved_args)
                    } else {
                        ResolvedType::Named(canonical)
                    }
                }
                Some(CollectionTypeId::List) => {
                    let canonical = collection_types::as_str(CollectionTypeId::List).to_string();
                    if let Some(inner) = resolved_args.into_iter().next() {
                        ResolvedType::Generic(canonical, vec![inner])
                    } else {
                        ResolvedType::Named(canonical)
                    }
                }
                _ => ResolvedType::Generic(name.clone(), resolved_args),
            }
        }
        ast::Type::Function(params, ret) => {
            let param_types: Vec<ResolvedType> = params
                .iter()
                .map(|p| ast_type_to_resolved(&p.node, type_params))
                .collect();
            let ret_type = ast_type_to_resolved(&ret.node, type_params);
            ResolvedType::Function(param_types, Box::new(ret_type))
        }
        ast::Type::Tuple(elems) => {
            let elem_types: Vec<ResolvedType> = elems
                .iter()
                .map(|e| ast_type_to_resolved(&e.node, type_params))
                .collect();
            ResolvedType::Tuple(elem_types)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_testing_module() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "testing".to_string()];
        let module = load_stdlib_module_data(&path);

        // The stdlib/testing.incn file should be findable and parseable.
        let module = module.ok_or("failed to load stdlib/testing.incn")?;
        let fns = module.functions;
        assert!(!fns.is_empty(), "should have extracted function signatures");

        // Check a few known functions.
        let fail_fn = fns.iter().find(|(name, _)| name == "fail");
        assert!(fail_fn.is_some(), "should find 'fail' function");
        let fail_info = &fail_fn.ok_or("fail not found")?.1;
        assert_eq!(fail_info.params.len(), 1);
        assert_eq!(fail_info.params[0].0, "msg");
        assert!(matches!(fail_info.params[0].1, ResolvedType::Str));
        assert!(fail_info.type_params.is_empty());
        assert!(matches!(fail_info.return_type, ResolvedType::Unit));

        let fail_t_fn = fns.iter().find(|(name, _)| name == "fail_t");
        assert!(fail_t_fn.is_some(), "should find 'fail_t' function");
        let fail_t_info = &fail_t_fn.ok_or("fail_t not found")?.1;
        assert_eq!(fail_t_info.params.len(), 1);
        assert_eq!(fail_t_info.params[0].0, "msg");
        assert!(matches!(fail_t_info.params[0].1, ResolvedType::Str));
        assert_eq!(fail_t_info.type_params, vec!["T".to_string()]);
        assert!(matches!(fail_t_info.return_type, ResolvedType::TypeVar(ref s) if s == "T"));

        let assert_eq_fn = fns.iter().find(|(name, _)| name == "assert_eq");
        assert!(assert_eq_fn.is_some(), "should find 'assert_eq' function");
        let assert_eq_info = &assert_eq_fn.ok_or("assert_eq not found")?.1;
        assert_eq!(assert_eq_info.params.len(), 3);
        assert_eq!(assert_eq_info.type_params, vec!["T".to_string()]);
        assert!(matches!(assert_eq_info.params[0].1, ResolvedType::TypeVar(ref s) if s == "T"));
        assert!(matches!(assert_eq_info.params[1].1, ResolvedType::TypeVar(ref s) if s == "T"));
        assert!(matches!(assert_eq_info.params[2].1, ResolvedType::Str));

        Ok(())
    }

    #[test]
    fn test_cache_lookup() -> Result<(), Box<dyn std::error::Error>> {
        let mut cache = StdlibAstCache::new();
        let path = vec!["std".to_string(), "testing".to_string()];

        // First lookup loads the module.
        let fail_info = cache.lookup_function(&path, "fail");
        assert!(fail_info.is_some(), "should find 'fail' from cache");

        // Second lookup uses the cache.
        let assert_eq_info = cache.lookup_function(&path, "assert_eq");
        assert!(assert_eq_info.is_some(), "should find 'assert_eq' from cache");

        // Unknown function returns None.
        let unknown = cache.lookup_function(&path, "nonexistent_function");
        assert!(unknown.is_none(), "should not find unknown function");

        Ok(())
    }

    #[test]
    fn test_load_async_time_module() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "async".to_string(), "time".to_string()];
        let module = load_stdlib_module_data(&path);
        let fns = module.ok_or("failed to load stdlib/async/time.incn")?.functions;
        let sleep_fn = fns.iter().find(|(name, _)| name == "sleep");
        assert!(sleep_fn.is_some(), "should find 'sleep' function");
        let timeout_fn = fns.iter().find(|(name, _)| name == "timeout");
        assert!(timeout_fn.is_some(), "should find 'timeout' function");
        Ok(())
    }

    #[test]
    fn test_load_async_prelude_module() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "async".to_string()];
        let module = load_stdlib_module_data(&path);
        let fns = module.ok_or("failed to load stdlib/async/prelude.incn")?.functions;
        let sleep_fn = fns.iter().find(|(name, _)| name == "sleep");
        assert!(sleep_fn.is_some(), "should resolve prelude re-export 'sleep'");
        let spawn_fn = fns.iter().find(|(name, _)| name == "spawn");
        assert!(spawn_fn.is_some(), "should resolve prelude re-export 'spawn'");
        Ok(())
    }

    #[test]
    fn test_load_async_channel_module() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "async".to_string(), "channel".to_string()];
        let module = load_stdlib_module_data(&path);

        let fns = module.ok_or("failed to load stdlib/async/channel.incn")?.functions;

        let channel_fn = fns.iter().find(|(name, _)| name == "channel");
        assert!(channel_fn.is_some(), "should find 'channel' function");
        let oneshot_fn = fns.iter().find(|(name, _)| name == "oneshot");
        assert!(oneshot_fn.is_some(), "should find 'oneshot' function");

        Ok(())
    }

    #[test]
    fn test_load_async_task_module() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "async".to_string(), "task".to_string()];
        let module = load_stdlib_module_data(&path);
        let fns = module.ok_or("failed to load stdlib/async/task.incn")?.functions;

        assert!(fns.iter().any(|(name, _)| name == "spawn"));
        assert!(fns.iter().any(|(name, _)| name == "spawn_blocking"));
        assert!(fns.iter().any(|(name, _)| name == "yield_now"));
        Ok(())
    }

    #[test]
    fn test_load_async_select_module_only_exports_supported_surface() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "async".to_string(), "select".to_string()];
        let module = load_stdlib_module_data(&path);
        let fns = module.ok_or("failed to load stdlib/async/select.incn")?.functions;

        assert!(fns.iter().any(|(name, _)| name == "select_timeout"));
        assert!(!fns.iter().any(|(name, _)| name == "select2"));
        assert!(!fns.iter().any(|(name, _)| name == "race"));
        Ok(())
    }

    // ---- Phase 6: Derive trait extraction tests ----

    use incan_core::lang::derives::{self as derive_reg, DeriveId};

    /// Helper: canonical derive name from the registry (avoids stringly-typed vocab checks).
    fn derive_name(id: DeriveId) -> &'static str {
        derive_reg::as_str(id)
    }

    #[test]
    fn test_load_derives_comparison_traits() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "derives".to_string(), "comparison".to_string()];
        let module = load_stdlib_module_data(&path);
        let module = module.ok_or("failed to load stdlib/derives/comparison.incn")?;

        // Should have no top-level functions, only traits.
        assert!(
            module.functions.is_empty(),
            "comparison.incn has no top-level functions"
        );
        assert!(!module.traits.is_empty(), "should have extracted trait signatures");

        // Eq trait: __eq__ (extern, no body) and __ne__ (default, has body).
        let eq_name = derive_name(DeriveId::Eq);
        let eq_trait = module.traits.iter().find(|(name, _)| name == eq_name);
        assert!(eq_trait.is_some(), "should find Eq trait");
        let eq_info = &eq_trait.ok_or("Eq not found")?.1;
        assert!(eq_info.type_params.is_empty());
        assert!(eq_info.methods.contains_key("__eq__"), "Eq should have __eq__");
        assert!(eq_info.methods.contains_key("__ne__"), "Eq should have __ne__");
        let ne = &eq_info.methods["__ne__"];
        assert!(ne.has_body, "__ne__ is a default method with a body");
        assert!(matches!(ne.return_type, ResolvedType::Bool));

        // Ord trait: __lt__ (extern) + __le__, __gt__, __ge__ (defaults).
        let ord_name = derive_name(DeriveId::Ord);
        let ord_trait = module.traits.iter().find(|(name, _)| name == ord_name);
        assert!(ord_trait.is_some(), "should find Ord trait");
        let ord_info = &ord_trait.ok_or("Ord not found")?.1;
        assert_eq!(ord_info.methods.len(), 4);
        assert!(!ord_info.methods["__lt__"].has_body, "__lt__ is abstract (extern)");
        assert!(ord_info.methods["__le__"].has_body, "__le__ is a default method");
        assert!(ord_info.methods["__gt__"].has_body, "__gt__ is a default method");
        assert!(ord_info.methods["__ge__"].has_body, "__ge__ is a default method");

        // Hash trait: single __hash__ (extern).
        let hash_name = derive_name(DeriveId::Hash);
        let hash_trait = module.traits.iter().find(|(name, _)| name == hash_name);
        assert!(hash_trait.is_some(), "should find Hash trait");
        let hash_info = &hash_trait.ok_or("Hash not found")?.1;
        assert_eq!(hash_info.methods.len(), 1);
        assert!(hash_info.methods.contains_key("__hash__"));
        assert!(matches!(hash_info.methods["__hash__"].return_type, ResolvedType::Int));

        Ok(())
    }

    #[test]
    fn test_load_derives_copying_traits() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "derives".to_string(), "copying".to_string()];
        let module = load_stdlib_module_data(&path);
        let module = module.ok_or("failed to load stdlib/derives/copying.incn")?;

        assert!(module.functions.is_empty(), "copying.incn has no top-level functions");

        // Clone trait: single clone(self) -> Self method.
        let clone_name = derive_name(DeriveId::Clone);
        let clone_trait = module.traits.iter().find(|(name, _)| name == clone_name);
        assert!(clone_trait.is_some(), "should find Clone trait");
        let clone_info = &clone_trait.ok_or("Clone not found")?.1;
        assert_eq!(clone_info.methods.len(), 1);
        assert!(clone_info.methods.contains_key("clone"));
        assert!(matches!(
            clone_info.methods["clone"].return_type,
            ResolvedType::SelfType
        ));

        // Copy trait: marker trait with no methods.
        let copy_name = derive_name(DeriveId::Copy);
        let copy_trait = module.traits.iter().find(|(name, _)| name == copy_name);
        assert!(copy_trait.is_some(), "should find Copy trait");
        let copy_info = &copy_trait.ok_or("Copy not found")?.1;
        assert!(copy_info.methods.is_empty(), "Copy is a marker trait with no methods");

        // Default trait: default() -> Self (no receiver, associated function).
        let default_name = derive_name(DeriveId::Default);
        let default_trait = module.traits.iter().find(|(name, _)| name == default_name);
        assert!(default_trait.is_some(), "should find Default trait");
        let default_info = &default_trait.ok_or("Default not found")?.1;
        assert_eq!(default_info.methods.len(), 1);
        let default_method = &default_info.methods["default"];
        assert!(
            default_method.receiver.is_none(),
            "default() is an associated function (no receiver)"
        );
        assert!(matches!(default_method.return_type, ResolvedType::SelfType));

        Ok(())
    }

    #[test]
    fn test_load_derives_string_traits() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "derives".to_string(), "string".to_string()];
        let module = load_stdlib_module_data(&path);
        let module = module.ok_or("failed to load stdlib/derives/string.incn")?;

        assert!(module.functions.is_empty(), "string.incn has no top-level functions");

        // Debug trait: __repr__(self) -> str.
        let debug_name = derive_name(DeriveId::Debug);
        let debug_trait = module.traits.iter().find(|(name, _)| name == debug_name);
        assert!(debug_trait.is_some(), "should find Debug trait");
        let debug_info = &debug_trait.ok_or("Debug not found")?.1;
        assert_eq!(debug_info.methods.len(), 1);
        assert!(debug_info.methods.contains_key("__repr__"));
        assert!(matches!(debug_info.methods["__repr__"].return_type, ResolvedType::Str));

        // Display trait: __str__(self) -> str (abstract, no body).
        let display_name = derive_name(DeriveId::Display);
        let display_trait = module.traits.iter().find(|(name, _)| name == display_name);
        assert!(display_trait.is_some(), "should find Display trait");
        let display_info = &display_trait.ok_or("Display not found")?.1;
        assert_eq!(display_info.methods.len(), 1);
        assert!(display_info.methods.contains_key("__str__"));
        assert!(
            !display_info.methods["__str__"].has_body,
            "__str__ is abstract (no body)"
        );

        Ok(())
    }

    #[test]
    fn test_cache_trait_lookup() -> Result<(), Box<dyn std::error::Error>> {
        let mut cache = StdlibAstCache::new();
        let path = vec!["std".to_string(), "derives".to_string(), "comparison".to_string()];

        // First lookup loads the module and finds the trait.
        let eq_name = derive_name(DeriveId::Eq);
        let eq_info = cache.lookup_trait(&path, eq_name);
        assert!(eq_info.is_some(), "should find Eq trait from cache");

        // Second lookup uses the cache.
        let ord_name = derive_name(DeriveId::Ord);
        let ord_info = cache.lookup_trait(&path, ord_name);
        assert!(ord_info.is_some(), "should find Ord trait from cache");

        // Function lookup on a trait-only module returns None.
        let no_fn = cache.lookup_function(&path, eq_name);
        assert!(no_fn.is_none(), "should not find Eq as a function");

        // Unknown trait returns None.
        let unknown = cache.lookup_trait(&path, "NonExistent");
        assert!(unknown.is_none(), "should not find unknown trait");

        Ok(())
    }

    #[test]
    fn test_ast_type_conversion() {
        let type_params = vec!["T".to_string()];

        assert!(matches!(
            ast_type_to_resolved(&ast::Type::Simple("int".to_string()), &type_params),
            ResolvedType::Int
        ));
        assert!(matches!(
            ast_type_to_resolved(&ast::Type::Simple("str".to_string()), &type_params),
            ResolvedType::Str
        ));
        assert!(matches!(
            ast_type_to_resolved(&ast::Type::Simple("bool".to_string()), &type_params),
            ResolvedType::Bool
        ));
        assert!(matches!(
            ast_type_to_resolved(&ast::Type::Simple("T".to_string()), &type_params),
            ResolvedType::TypeVar(ref s) if s == "T"
        ));
        assert!(matches!(
            ast_type_to_resolved(&ast::Type::Unit, &type_params),
            ResolvedType::Unit
        ));
    }
}

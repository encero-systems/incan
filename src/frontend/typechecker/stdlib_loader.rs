//! RFC 023: Stdlib `.incn` file loader.
//!
//! This module provides infrastructure for loading, parsing, and extracting function signatures from stdlib `.incn`
//! files. It replaces hardcoded function registries with signatures derived from the actual source files.
//!
//! ## Design
//!
//! 1. **Discovery**: finds stdlib `.incn` files using `incan_core::lang::stdlib::stdlib_stub_path`.
//! 2. **Parsing**: lexes and parses the file through the normal Incan frontend pipeline.
//! 3. **Extraction**: walks the parsed AST to extract `FunctionInfo` entries for each `def`.
//! 4. **Caching**: results are cached per module path in a `HashMap` to avoid redundant parsing.
//!
//! ## Limitations
//!
//! - Function signatures are extracted from top-level `def` declarations (not methods on classes/models).
//! - Default parameter values are not captured (only the parameter name and type).
//! - Complex types beyond the common set (`int`, `str`, `bool`, `Option[T]`, etc.) are treated as `Named`.
//! - Parse failures are logged and cause a graceful fallback to hardcoded registries.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::frontend::ast;
use crate::frontend::symbols::{FunctionInfo, ResolvedType};
use incan_core::lang::conventions;
use incan_core::lang::stdlib;
use incan_core::lang::types::collections::{self as collection_types, CollectionTypeId};
use incan_core::lang::types::numerics::{self as numeric_types, NumericTypeId};
use incan_core::lang::types::stringlike::{self as string_types, StringLikeId};

#[derive(Debug, Clone, Default)]
struct StdlibModuleData {
    functions: Vec<(String, FunctionInfo)>,
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
        let key = module_path.join(".");
        // Load on first access.
        if !self.cache.contains_key(&key) {
            if let Some(module_data) = load_stdlib_module_data(module_path) {
                self.cache.insert(key.clone(), module_data);
            } else {
                // Cache an empty entry to avoid re-trying.
                self.cache.insert(key.clone(), StdlibModuleData::default());
            }
        }

        self.cache
            .get(&key)?
            .functions
            .iter()
            .find(|(name, _)| name == function_name)
            .map(|(_, info)| info.clone())
    }
}

/// Load and parse a stdlib `.incn` file, extracting module data.
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

    let functions = extract_function_signatures(&program);
    Some(StdlibModuleData { functions })
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
        }
    }
    fns
}

/// Convert an AST `FunctionDecl` to a typechecker `FunctionInfo`.
fn function_decl_to_info(func: &ast::FunctionDecl) -> FunctionInfo {
    // Extract just the type parameter names for type resolution.
    let tp_names: Vec<String> = func.type_params.iter().map(|tp| tp.name.clone()).collect();

    let params: Vec<(String, ResolvedType)> = func
        .params
        .iter()
        .map(|p| (p.node.name.clone(), ast_type_to_resolved(&p.node.ty.node, &tp_names)))
        .collect();

    let return_type = ast_type_to_resolved(&func.return_type.node, &tp_names);

    FunctionInfo {
        params,
        return_type,
        is_async: func.is_async,
        type_params: tp_names,
    }
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
    fn test_load_async_time_module_falls_back() {
        // stdlib/async/time.incn contains `model Duration:` with methods and `async def`, which the current parser
        // can't handle in stub extraction mode. The loader returns None and the typechecker uses the hardcoded
        // async_import_function_info() fallback.
        let path = vec!["std".to_string(), "async".to_string(), "time".to_string()];
        let module = load_stdlib_module_data(&path);
        let fns = module.map(|m| m.functions);
        // Currently returns None because the parser can't handle models + async defs in the same file.
        // This test documents the current behavior; it will start passing once the parser handles these.
        assert!(
            fns.is_none(),
            "async/time.incn parse currently fails; fallback expected"
        );
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

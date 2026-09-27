//! Canonical paths for the stdlib consts that parameter defaults name.
//!
//! A parameter default is written in its declaring module but expanded at every call site that omits the argument,
//! which lies in another module or another crate. A default spelled as a bare const name (`chunk_size: int =
//! DEFAULT_CHUNK_SIZE`) names nothing there, so the loader records which declaration each such spelling denotes while
//! the declaring module's own bindings are still in view (#1771).

use std::collections::{HashMap, HashSet};

use super::{StdlibModuleData, load_stdlib_module_data_inner};
use crate::ast;
use incan_lang::lang::stdlib;
use incan_semantics_core::{SemanticSourceTargetKind, SymbolOrigin};

/// Resolve every parameter default that is exactly a const name to the canonical `std.*` path of that const.
///
/// The map is keyed by the spelling the declaring module uses. A default of any other shape, or a name that is not a
/// const of the declaring module, is left out: lowering then keeps the default as written.
pub(super) fn param_default_const_paths(
    params: &[ast::Spanned<ast::Param>],
    module_path: &[String],
    program: &ast::Program,
    loading: &mut HashSet<String>,
    loaded: &mut HashMap<String, StdlibModuleData>,
) -> HashMap<String, Vec<String>> {
    let mut paths = HashMap::new();
    for param in params {
        let Some(ast::Expr::Ident(name)) = param.node.default.as_ref().map(|default| &default.node) else {
            continue;
        };
        if paths.contains_key(name) {
            continue;
        }
        if let Some(path) = module_const_path(name, module_path, program, loading, loaded) {
            paths.insert(name.clone(), path);
        }
    }
    paths
}

/// Return the canonical path of the const a module binds under `name`, following a stdlib import to its declaration.
///
/// A const the module declares is `module_path.name`. A const it imports from another stdlib module is resolved
/// through that module's declaration identities, so an import through a facade (`from std.hash import ...`) still
/// reaches the module that declares the const. A cyclic or unavailable import resolves to nothing.
fn module_const_path(
    name: &str,
    module_path: &[String],
    program: &ast::Program,
    loading: &mut HashSet<String>,
    loaded: &mut HashMap<String, StdlibModuleData>,
) -> Option<Vec<String>> {
    let declares_const = program
        .declarations
        .iter()
        .any(|declaration| matches!(&declaration.node, ast::Declaration::Const(konst) if konst.name == name));
    if declares_const {
        let mut path = module_path.to_vec();
        path.push(name.to_string());
        return Some(path);
    }
    let (source_module, imported_name) = program.declarations.iter().find_map(|declaration| {
        let ast::Declaration::Import(import) = &declaration.node else {
            return None;
        };
        let ast::ImportKind::From { module, items } = &import.kind else {
            return None;
        };
        if module.segments.first().is_none_or(|root| root != stdlib::STDLIB_ROOT) {
            return None;
        }
        items
            .iter()
            .find(|item| item.alias.as_deref().unwrap_or(&item.name) == name)
            .map(|item| (module.segments.clone(), item.name.clone()))
    })?;
    let key = source_module.join(".");
    if !loaded.contains_key(&key) {
        load_stdlib_module_data_inner(&source_module, loading, loaded)?;
    }
    let identity = loaded
        .get(&key)?
        .identities
        .get(&imported_name)
        .filter(|identity| identity.kind == SemanticSourceTargetKind::Const)?;
    let SymbolOrigin::Module(owner) = &identity.origin else {
        return None;
    };
    let mut path = owner.clone();
    path.push(identity.declaration_name.clone());
    Some(path)
}

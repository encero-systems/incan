//! Retain proven stdlib constant values once by checked source identity, without changing legacy evaluation.

use super::{CanonicalSymbolId, ResolvedType, SemanticSourceTargetKind, TypeCheckInfo, ast, bir};
use crate::typechecker::stdlib_loader::StdlibAstCache;
use incan_semantics_core::SymbolOrigin;
use std::collections::BTreeSet;

/// Resolve referenced constants through their full source/package identity; aliases deduplicate to one declaration.
pub(super) fn collect(type_info: &TypeCheckInfo) -> Vec<bir::ConstantDeclaration> {
    let identities = type_info
        .references
        .resolved_identities
        .values()
        .filter(|identity| identity.kind == SemanticSourceTargetKind::Const)
        .collect::<BTreeSet<_>>();
    let mut cache = type_info.body_ir_stdlib_cache();
    identities
        .into_iter()
        .filter_map(|identity| {
            Some(bir::ConstantDeclaration {
                canonical: identity.clone(),
                value: constant_value(identity, &mut cache)?,
            })
        })
        .collect()
}

/// Require a source-verified float constant initialized by one exact Rust import before retaining its native value.
/// Symbolic expressions, other Rust items and unrelated declarations remain unretained globals.
pub(super) fn constant_value(identity: &CanonicalSymbolId, cache: &mut StdlibAstCache) -> Option<bir::Constant> {
    let source_identity = cache.callable_source_identity(identity)?;
    let SymbolOrigin::Module(path) = &source_identity.origin else {
        return None;
    };
    if cache.lookup_constant(path, &identity.declaration_name)?.ty != ResolvedType::Float {
        return None;
    }
    let program = cache.body_ir_constant_program(identity)?;
    let item = program
        .declarations
        .iter()
        .find_map(|declaration| match &declaration.node {
            ast::Declaration::Const(item) => Some(item),
            _ => None,
        })?;
    let ast::Expr::Ident(binding) = &item.value.node else {
        return None;
    };
    let target = rust_import_path(&program, binding)?;
    Some(bir::Constant::Float(rust_float_constant(&target)?.to_string()))
}

/// Follow only the selected constant's authored Rust import alias. Source names never select a numeric value.
fn rust_import_path(program: &ast::Program, binding: &str) -> Option<String> {
    program.declarations.iter().find_map(|declaration| {
        let ast::Declaration::Import(import) = &declaration.node else {
            return None;
        };
        let ast::ImportKind::RustFrom {
            crate_name,
            path,
            items,
            ..
        } = &import.kind
        else {
            return None;
        };
        let item = items
            .iter()
            .find(|item| item.alias.as_deref().unwrap_or(&item.name) == binding)?;
        Some(
            std::iter::once(crate_name.as_str())
                .chain(path.iter().map(String::as_str))
                .chain(std::iter::once(item.name.as_str()))
                .collect::<Vec<_>>()
                .join("::"),
        )
    })
}

/// Use the pinned Rust compiler's f64 constants for the exact proven Rust item. Nonfinite values retain their native
/// spelling in Body IR and are parsed by the existing float carrier; no user source arithmetic is evaluated here.
fn rust_float_constant(path: &str) -> Option<f64> {
    match path {
        "std::f64::consts::PI" => Some(std::f64::consts::PI),
        "std::f64::consts::E" => Some(std::f64::consts::E),
        "std::f64::consts::TAU" => Some(std::f64::consts::TAU),
        "std::f64::INFINITY" => Some(f64::INFINITY),
        "std::f64::NAN" => Some(f64::NAN),
        _ => None,
    }
}

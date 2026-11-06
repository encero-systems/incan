//! The value each parameter of a stdlib crate call takes when a call omits it.
//!
//! A default is written in the stdlib module that declares the callable, but a call that omits its argument supplies
//! it, in another module or crate. Body IR records such a default only as a closed constant the call site passes: a
//! literal the parameter's checked type admits, or a stdlib const as checking evaluates it in the module that declares
//! it, which is the value legacy reads when it names that const through its declaring module (#1771). Any other
//! default stays [`bir::CallableParamDefault::Unsupported`], so a call that omits it refuses rather than rebuilding the
//! default's source semantics at the call site.

use std::collections::HashMap;

use incan_semantics_core::IncanPrimitiveType;

use super::primitives::{lower_checked_literal, lower_checked_negative_literal};
use super::{IncanType, ast, bir, hir_span};
use crate::typechecker::{ConstValue, TypeChecker, stdlib_loader::StdlibAstCache};

/// What each parameter of a stdlib function's crate call takes when a call omits it, read from the module's only
/// declaration under that name; `types` are the parameter types the crate call passes.
///
/// A function the module does not declare once under its name, such as one overload of a same-name set, records no
/// defaults, so a call that omits one of its arguments refuses.
pub(super) fn function_defaults(
    path: &[String],
    name: &str,
    types: &[IncanType],
    cache: &mut StdlibAstCache,
) -> Vec<bir::CallableParamDefault> {
    cache
        .lookup_function_source(path, name)
        .and_then(|source| declared_defaults(&source.declaration.params, &source.default_const_paths, types, cache))
        .unwrap_or_default()
}

/// What each parameter of a stdlib method's crate call takes when a call omits it: its receiver, when it has one, is
/// required, then each parameter its owner's declaration lists; `types` are the ordinary parameter types.
pub(super) fn method_defaults(
    path: &[String],
    owner: &str,
    name: &str,
    receiver: bool,
    types: &[IncanType],
    cache: &mut StdlibAstCache,
) -> Vec<bir::CallableParamDefault> {
    let Some(source) = cache.lookup_type_method_source(path, owner, name) else {
        return Vec::new();
    };
    let Some(declared) = declared_defaults(&source.declaration.params, &source.default_const_paths, types, cache)
    else {
        return Vec::new();
    };
    let mut defaults = Vec::new();
    if receiver {
        defaults.push(bir::CallableParamDefault::Required);
    }
    defaults.extend(declared);
    defaults
}

/// Record what each declared parameter takes when a call omits it, given the parameter types the crate call passes.
///
/// `const_paths` maps each const name a default spells to the const's canonical `std.*` path. A declaration whose
/// parameters do not pair with `types` yields nothing.
fn declared_defaults(
    params: &[ast::Spanned<ast::Param>],
    const_paths: &HashMap<String, Vec<String>>,
    types: &[IncanType],
    cache: &mut StdlibAstCache,
) -> Option<Vec<bir::CallableParamDefault>> {
    if params.len() != types.len() {
        return None;
    }
    let defaults = params
        .iter()
        .zip(types)
        .map(|(param, ty)| match &param.node.default {
            None => bir::CallableParamDefault::Required,
            Some(default) => constant_default(default, ty, const_paths, cache),
        })
        .collect();
    Some(defaults)
}

/// Record one written default as the constant a call passes for it, or name why it has none.
fn constant_default(
    default: &ast::Spanned<ast::Expr>,
    ty: &IncanType,
    const_paths: &HashMap<String, Vec<String>>,
    cache: &mut StdlibAstCache,
) -> bir::CallableParamDefault {
    let constant = match &default.node {
        ast::Expr::Literal(literal) => Some(lower_checked_literal(literal, ty)),
        ast::Expr::Unary(ast::UnaryOp::Neg, inner) => match &inner.node {
            ast::Expr::Literal(literal) => negated_literal(literal, ty),
            _ => None,
        },
        ast::Expr::Ident(name) => const_paths.get(name).and_then(|path| const_value(path, ty, cache)),
        _ => None,
    };
    match constant.filter(|constant| admits(ty, constant)) {
        Some(constant) => bir::CallableParamDefault::Source(Box::new(bir::DefaultComputation {
            span: hir_span(default.span),
            stmts: Vec::new(),
            result: bir::Operand::Constant(constant),
        })),
        None => bir::CallableParamDefault::Unsupported {
            span: hir_span(default.span),
            description: "stdlib default other than a literal or a const of the parameter's type".to_owned(),
        },
    }
}

/// Fold a negated literal into one constant. Ordinary lowering keeps a negated `int` literal an operation, which a
/// record cannot run, so its value is folded here; `int`'s most negative value needs the checked fold.
fn negated_literal(literal: &ast::Literal, ty: &IncanType) -> Option<bir::Constant> {
    let folded = lower_checked_negative_literal(literal, ty);
    match (literal, ty, folded) {
        (_, _, Some(constant)) => Some(constant),
        (ast::Literal::Int(value), IncanType::Primitive(IncanPrimitiveType::Int), None) if value.suffix.is_none() => {
            i64::try_from(value.magnitude)
                .ok()
                .map(|magnitude| bir::Constant::Int(-magnitude))
        }
        _ => None,
    }
}

/// The value of the stdlib const at `path`, as checking evaluates it in the module that declares it.
fn const_value(path: &[String], ty: &IncanType, cache: &mut StdlibAstCache) -> Option<bir::Constant> {
    let (name, module_path) = path.split_last()?;
    let program = cache.constants_program(module_path)?;
    let mut checker = TypeChecker::new();
    checker.stdlib_cache = cache.clone();
    checker.set_current_module_path(Some(module_path.to_vec()));
    checker.check_program(&program).ok()?;
    match (checker.type_info().const_value(name)?, ty) {
        (ConstValue::Int(value), IncanType::Primitive(IncanPrimitiveType::Int)) => Some(bir::Constant::Int(*value)),
        _ => None,
    }
}

/// Whether a parameter of this checked type takes this constant as the call's argument.
fn admits(ty: &IncanType, constant: &bir::Constant) -> bool {
    if let (IncanType::Generic { base, args }, bir::Constant::None) = (ty, constant) {
        return base == "Option" && args.len() == 1;
    }
    matches!(
        (ty, constant),
        (IncanType::Primitive(IncanPrimitiveType::Int), bir::Constant::Int(_))
            | (IncanType::Primitive(IncanPrimitiveType::Bool), bir::Constant::Bool(_))
            | (IncanType::Primitive(IncanPrimitiveType::Str), bir::Constant::Str(_))
            | (IncanType::Primitive(IncanPrimitiveType::Bytes), bir::Constant::Bytes(_))
    )
}

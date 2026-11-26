//! Generic call-site inference, monomorph recording, explicit bound validation and undetermined type parameters.

use std::collections::HashMap;

use super::TypeChecker;
use crate::ast::{CallArg, DictEntry, Expr, ListEntry, Literal, ParamKind, Span, Spanned, Type, UnaryOp};
use crate::diagnostics::CompileError;
use crate::diagnostics::errors::{self, TypeArgumentOrigin};
use crate::resolved_type_subst::{substitute_resolved_type, type_param_subst_map_call_site};
use crate::symbols::{CallableParam, FunctionInfo, MethodInfo, ResolvedType, TypeInfo};
use crate::typechecker::helpers::collection_type_id;
use crate::typechecker::type_info::BodyIrDecoratorApplication;
use incan_lang::interop::is_rust_capability_bound;
use incan_lang::lang::callables;
use incan_lang::lang::derives::{self, DeriveId};
use incan_lang::lang::surface::constructors::ConstructorId;
use incan_lang::lang::traits::{self as builtin_traits, TraitId};
use incan_lang::lang::types::collections::CollectionTypeId;
use incan_semantics_core::CanonicalSymbolId;

mod argument_bindings;
mod function_calls;
mod instantiations_and_methods;
mod open_type_params;
mod type_param_inference;

use open_type_params::OpenTypeParamCall;

/// Settled function-call facts consumed by result and monomorph publication. Owned inventories move from the
/// validator; argument type references still borrow its live substituted parameter inventory.
struct FunctionCallCompletion<'call> {
    func_name: &'call str,
    info: &'call FunctionInfo,
    explicit_type_args: &'call [Spanned<Type>],
    args: &'call [CallArg],
    call_span: Span,
    errors_before_call: usize,
    callee_identity: Option<CanonicalSymbolId>,
    contextual_params: Vec<CallableParam>,
    type_bindings: HashMap<String, ResolvedType>,
    instantiation: HashMap<String, ResolvedType>,
    arguments: Vec<(&'call Spanned<Expr>, &'call ResolvedType)>,
}

/// Settled method-call facts retained after receiver substitution and argument validation. The method's owned
/// declaration and result carrier move into completion without adding declaration or type snapshots.
struct MethodCallCompletion<'call> {
    method: &'call str,
    method_info: MethodInfo,
    explicit_type_args: &'call [Spanned<Type>],
    args: &'call [CallArg],
    call_site_span: Span,
    receiver_ty: &'call ResolvedType,
    expected_return_ty: Option<&'call ResolvedType>,
    errors_before_call: usize,
    explicit_arity_ok: bool,
    contextual_params: Vec<CallableParam>,
    type_bindings: HashMap<String, ResolvedType>,
    instantiation: HashMap<String, ResolvedType>,
    arguments: Vec<(&'call Spanned<Expr>, &'call ResolvedType)>,
    return_type: ResolvedType,
}

/// Whether an inferred bound is the builtin `Eq` or `Hash` a hashed type parameter needs (#1758).
fn is_hash_key_bound(bound: &crate::symbols::TypeBoundInfo) -> bool {
    matches!(
        bound.name.rsplit("::").next().and_then(derives::from_str),
        Some(DeriveId::Eq | DeriveId::Hash)
    )
}

/// Return whether a type-parameter binding leaves the parameter open: unknown, or naming a type parameter still to be
/// inferred (the `T` a `[]` checked against `list[T]` binds `T` to).
fn is_open_binding(ty: &ResolvedType, type_params: &[String]) -> bool {
    matches!(ty, ResolvedType::Unknown | ResolvedType::CallSiteInfer)
        || first_open_type_param(ty, type_params).is_some()
}

/// Return the first of `type_params` that `ty` names, at any depth, as a type variable still to be inferred.
///
/// A callee's parameter types name its type parameters as type variables; a type parameter of the caller's own body is
/// a named type there, and is not open.
pub(in crate::typechecker::check_expr) fn first_open_type_param(
    ty: &ResolvedType,
    type_params: &[String],
) -> Option<String> {
    match ty {
        ResolvedType::TypeVar(name) => type_params.contains(name).then(|| name.clone()),
        ResolvedType::Generic(_, args) | ResolvedType::Tuple(args) => {
            args.iter().find_map(|arg| first_open_type_param(arg, type_params))
        }
        ResolvedType::FrozenList(inner)
        | ResolvedType::FrozenSet(inner)
        | ResolvedType::TypeToken(inner)
        | ResolvedType::Ref(inner)
        | ResolvedType::RefMut(inner) => first_open_type_param(inner, type_params),
        ResolvedType::FrozenDict(key, value) => {
            first_open_type_param(key, type_params).or_else(|| first_open_type_param(value, type_params))
        }
        ResolvedType::Function(params, ret) => params
            .iter()
            .find_map(|param| first_open_type_param(&param.ty, type_params))
            .or_else(|| first_open_type_param(ret, type_params)),
        _ => None,
    }
}

/// Return whether `expr` is an integer literal or a negated one, looking through parentheses.
fn is_integer_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Paren(inner) => is_integer_literal(&inner.node),
        Expr::Literal(Literal::Int(_)) => true,
        Expr::Unary(UnaryOp::Neg, operand) => matches!(operand.node, Expr::Literal(Literal::Int(_))),
        _ => false,
    }
}

/// Return whether `expr` is the `None` literal, looking through parentheses.
fn is_none_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Paren(inner) => is_none_literal(&inner.node),
        Expr::Literal(Literal::None) => true,
        _ => false,
    }
}

/// Return whether `expr` is a list, dict, set or tuple literal, looking through parentheses.
fn is_collection_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Paren(inner) => is_collection_literal(&inner.node),
        Expr::List(_) | Expr::Dict(_) | Expr::Set(_) | Expr::Tuple(_) => true,
        _ => false,
    }
}

/// Return whether `expr` is built from literals alone: a literal (`None` included), a negated number literal, or a
/// list, dict, set or tuple literal whose entries are built from literals alone, with no spread.
///
/// Checking such an expression a second time records its types again and does nothing else.
fn is_built_from_literals(expr: &Expr) -> bool {
    match expr {
        Expr::Literal(_) => true,
        Expr::Paren(inner) => is_built_from_literals(&inner.node),
        Expr::Unary(UnaryOp::Neg, operand) => {
            matches!(operand.node, Expr::Literal(Literal::Int(_) | Literal::Float(_)))
        }
        Expr::List(entries) => entries.iter().all(|entry| match entry {
            ListEntry::Element(item) => is_built_from_literals(&item.node),
            ListEntry::Spread(_) => false,
        }),
        Expr::Dict(entries) => entries.iter().all(|entry| match entry {
            DictEntry::Pair(key, value) => is_built_from_literals(&key.node) && is_built_from_literals(&value.node),
            DictEntry::Spread(_) => false,
        }),
        Expr::Set(items) | Expr::Tuple(items) => items.iter().all(|item| is_built_from_literals(&item.node)),
        _ => false,
    }
}

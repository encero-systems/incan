//! Check unary and binary operators.
//!
//! These helpers validate operator semantics (e.g., numeric ops, boolean ops) and compute the resulting type, emitting
//! diagnostics on mismatches.
//!
//! Numeric semantics follow RFC 009 and Python's division rules:
//!
//! - Same-type integer arithmetic yields that type (`i8 + i8` is an `i8`); an unsuffixed integer literal beside an
//!   exact-width integer takes its type, and operands of two different integer types are refused
//! - `/` always yields `float` (even `int / int`)
//! - `%` supports floats with Python remainder semantics
//! - `**` keeps an integer base's type only for a non-negative integer literal exponent; otherwise it yields `float`
//! - `+` supports string and list concatenation before numeric fallback
//! - Mixed numeric comparisons are allowed (promote to float for comparison)

use crate::ast::*;
use crate::diagnostics::errors;
use crate::numeric_adapters::{numeric_op_from_ast, numeric_ty_from_resolved, pow_exponent_kind_from_ast};
use crate::symbols::{ResolvedType, TypeBoundInfo, TypeInfo};
use crate::typechecker::derive_requirements::DeriveSupport;
use crate::typechecker::{
    BodyIrIterationHooksInfo, BodyIrProtocolCallInfo, MemberBindingSurface, ProtocolIterationInfo,
    ResolvedMethodDispatch, ResolvedOperatorKind, numeric_type_id_for_compat,
};
use incan_lang::lang::derives::{self, DeriveId};
use incan_lang::lang::magic_methods::{self, MagicMethodId};
use incan_lang::lang::traits as builtin_traits;
use incan_lang::{NumericTy, result_numeric_type};

use super::TypeChecker;

mod binary_ops;
mod operator_dunders;
mod unary_and_iteration;
use crate::typechecker::helpers::{collection_type_id, decimal_shape, is_str_like};
use incan_lang::lang::types::collections::CollectionTypeId;
use incan_lang::lang::types::numerics::{self, NumericFamily, NumericTypeId};

/// Check whether a resolved type is a runtime `List[T]` with one element slot.
fn is_runtime_list(ty: &ResolvedType) -> bool {
    matches!(
        ty,
        ResolvedType::Generic(name, args)
            if collection_type_id(name.as_str()) == Some(CollectionTypeId::List) && args.len() == 1
    )
}

/// Return the element type for a runtime `List[T]`, if known.
fn runtime_list_elem_type(ty: &ResolvedType) -> Option<&ResolvedType> {
    match ty {
        ResolvedType::Generic(name, args)
            if collection_type_id(name.as_str()) == Some(CollectionTypeId::List) && args.len() == 1 =>
        {
            args.first()
        }
        _ => None,
    }
}

/// Whether an expression is an empty list literal, looking through parentheses.
fn is_empty_list_literal(expr: &Expr) -> bool {
    match expr {
        Expr::List(entries) => entries.is_empty(),
        Expr::Paren(inner) => is_empty_list_literal(&inner.node),
        _ => false,
    }
}

/// Return whether a resolved type is one of the exact-width integer types (`i8`, `u16`, `i64`, ...).
///
/// `int` is not one of them: it is the ordinary spelling an unsuffixed integer literal already has.
fn is_exact_integer_type(ty: &ResolvedType) -> bool {
    matches!(ty, ResolvedType::Numeric(id) if numerics::is_integer(*id))
}

/// Return whether an AST expression is an unsuffixed integer literal, negated or parenthesized or not.
///
/// These are the operands whose type comes from their context: a suffixed literal (`7i8`) names its own type.
pub(super) fn is_unsuffixed_integer_literal(expr: &Spanned<Expr>) -> bool {
    match &expr.node {
        Expr::Literal(Literal::Int(value)) => value.suffix.is_none(),
        Expr::Unary(UnaryOp::Neg, inner) => {
            matches!(&inner.node, Expr::Literal(Literal::Int(value)) if value.suffix.is_none())
        }
        Expr::Paren(inner) => is_unsuffixed_integer_literal(inner),
        _ => false,
    }
}

/// Return whether an unsuffixed integer literal operand, negated or parenthesized or not, holds a value of the integer
/// type `ty`.
pub(super) fn integer_literal_fits(expr: &Spanned<Expr>, ty: &ResolvedType) -> bool {
    let Some(id) = numeric_type_id_for_compat(ty) else {
        return false;
    };
    let (negative, literal) = match &expr.node {
        Expr::Paren(inner) => return integer_literal_fits(inner, ty),
        Expr::Literal(Literal::Int(value)) => (false, value),
        Expr::Unary(UnaryOp::Neg, inner) => match &inner.node {
            Expr::Literal(Literal::Int(value)) => (true, value),
            _ => return false,
        },
        _ => return false,
    };
    match incan_lang::numeric_values::integer_bounds(id) {
        Some(incan_lang::numeric_values::IntegerBounds::Signed { minimum, maximum }) => {
            if negative {
                literal.magnitude <= minimum.unsigned_abs()
            } else {
                literal.magnitude <= maximum.unsigned_abs()
            }
        }
        Some(incan_lang::numeric_values::IntegerBounds::Unsigned { maximum }) => {
            literal.magnitude <= maximum && (!negative || literal.magnitude == 0)
        }
        None => false,
    }
}

/// Admit a contextual exact-width integer candidate only for an unsuffixed literal within its declared range.
/// Native trait selection uses the same literal rules as operators; ordinary arguments and suffixed literals
/// retain their fixed checked carriers and cannot use this contextual match.
pub(super) fn contextual_integer_literal_fits(expr: &Spanned<Expr>, ty: &ResolvedType) -> bool {
    is_exact_integer_type(ty) && is_unsuffixed_integer_literal(expr) && integer_literal_fits(expr, ty)
}

/// Fully resolved hook pair for one structural iteration protocol.
struct ResolvedIterationHooks {
    iterator_type: ResolvedType,
    next_type: ResolvedType,
    iter_dispatch: Option<ResolvedMethodDispatch>,
    next_dispatch: Option<ResolvedMethodDispatch>,
    body_ir: BodyIrIterationHooksInfo,
}

/// Preserve a trait's defining module when synthetic protocol-hook resolution follows a source trait call.
fn inherit_same_trait_dispatch_module(
    prior: Option<&ResolvedMethodDispatch>,
    current: &mut Option<ResolvedMethodDispatch>,
) {
    let (
        Some(ResolvedMethodDispatch::Trait {
            trait_name: prior_trait,
            module_path: Some(prior_module),
            ..
        }),
        Some(ResolvedMethodDispatch::Trait {
            trait_name,
            module_path,
            ..
        }),
    ) = (prior, current.as_mut())
    else {
        return;
    };
    if module_path.is_none() && trait_name == prior_trait {
        *module_path = Some(prior_module.clone());
    }
}

/// Return the dunder hook for an arithmetic operator whose semantics only the concrete numeric types carry.
///
/// `/` yields `float` for any operands, `//` and `%` round toward negative infinity and `**` picks its result type
/// from the exponent: these are the language's numeric rules, provided by the runtime for `int`, `float` and the
/// exact-width numerics, with no trait standing for them. `+`, `-` and `*` are not in this set: they lower to the
/// operator traits and become inferred bounds on a type parameter (RFC 023). The hook is returned so a diagnostic can
/// name what a bound trait would have to define for the operator to resolve through RFC 028 dispatch instead.
fn numeric_only_operator_dunder(op: BinaryOp) -> Option<&'static str> {
    match op {
        BinaryOp::Div | BinaryOp::FloorDiv | BinaryOp::Mod | BinaryOp::Pow => binary_operator_dunder(op),
        _ => None,
    }
}

/// Return the dunder hook for a binary operator that can participate in RFC 028 dispatch.
fn binary_operator_dunder(op: BinaryOp) -> Option<&'static str> {
    match op {
        BinaryOp::Add => Some("__add__"),
        BinaryOp::Sub => Some("__sub__"),
        BinaryOp::Mul => Some("__mul__"),
        BinaryOp::Div => Some("__div__"),
        BinaryOp::FloorDiv => Some("__floordiv__"),
        BinaryOp::Mod => Some("__mod__"),
        BinaryOp::Pow => Some("__pow__"),
        BinaryOp::MatMul => Some("__matmul__"),
        BinaryOp::PipeForward => Some("__pipe_forward__"),
        BinaryOp::PipeBackward => Some("__pipe_backward__"),
        BinaryOp::BitAnd => Some("__and__"),
        BinaryOp::BitOr => Some("__or__"),
        BinaryOp::BitXor => Some("__xor__"),
        BinaryOp::Shl => Some("__lshift__"),
        BinaryOp::Shr => Some("__rshift__"),
        BinaryOp::Eq => Some("__eq__"),
        BinaryOp::NotEq => Some("__ne__"),
        BinaryOp::Lt => Some("__lt__"),
        BinaryOp::Gt => Some("__gt__"),
        BinaryOp::LtEq => Some("__le__"),
        BinaryOp::GtEq => Some("__ge__"),
        BinaryOp::And | BinaryOp::Or | BinaryOp::In | BinaryOp::NotIn | BinaryOp::Is | BinaryOp::IsNot => None,
    }
}

/// Return the explicit in-place dunder hook for a compound-assignment operator.
fn compound_in_place_dunder(op: CompoundOp) -> &'static str {
    match op {
        CompoundOp::Add => "__iadd__",
        CompoundOp::Sub => "__isub__",
        CompoundOp::Mul => "__imul__",
        CompoundOp::Div => "__idiv__",
        CompoundOp::FloorDiv => "__ifloordiv__",
        CompoundOp::Mod => "__imod__",
        CompoundOp::MatMul => "__imatmul__",
        CompoundOp::BitAnd => "__iand__",
        CompoundOp::BitOr => "__ior__",
        CompoundOp::BitXor => "__ixor__",
        CompoundOp::Shl => "__ilshift__",
        CompoundOp::Shr => "__irshift__",
    }
}

/// Return the binary operator used when compound assignment falls back to `a = a <op> b`.
pub(in crate::typechecker) fn compound_binary_op(op: CompoundOp) -> BinaryOp {
    match op {
        CompoundOp::Add => BinaryOp::Add,
        CompoundOp::Sub => BinaryOp::Sub,
        CompoundOp::Mul => BinaryOp::Mul,
        CompoundOp::Div => BinaryOp::Div,
        CompoundOp::FloorDiv => BinaryOp::FloorDiv,
        CompoundOp::Mod => BinaryOp::Mod,
        CompoundOp::MatMul => BinaryOp::MatMul,
        CompoundOp::BitAnd => BinaryOp::BitAnd,
        CompoundOp::BitOr => BinaryOp::BitOr,
        CompoundOp::BitXor => BinaryOp::BitXor,
        CompoundOp::Shl => BinaryOp::Shl,
        CompoundOp::Shr => BinaryOp::Shr,
    }
}

/// Return all comparison dunders that make comparison fallback explicit for a user type.
fn comparison_dunders() -> &'static [&'static str] {
    &["__eq__", "__ne__", "__lt__", "__le__", "__gt__", "__ge__"]
}

/// Return the derive whose Rust trait implements a comparison operator: `PartialEq` for `==`, `!=` and list
/// membership, `PartialOrd` for the orderings.
fn comparison_operator_derive(op: BinaryOp) -> Option<DeriveId> {
    match op {
        BinaryOp::Eq | BinaryOp::NotEq | BinaryOp::In | BinaryOp::NotIn => Some(DeriveId::PartialEq),
        BinaryOp::Lt | BinaryOp::Gt | BinaryOp::LtEq | BinaryOp::GtEq => Some(DeriveId::PartialOrd),
        _ => None,
    }
}

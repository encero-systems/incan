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
    MemberBindingSurface, ProtocolIterationInfo, ResolvedMethodDispatch, ResolvedOperatorKind,
    numeric_type_id_for_compat,
};
use incan_lang::lang::derives::{self, DeriveId};
use incan_lang::lang::magic_methods::{self, MagicMethodId};
use incan_lang::{NumericTy, result_numeric_type};

use super::TypeChecker;
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
fn is_unsuffixed_integer_literal(expr: &Spanned<Expr>) -> bool {
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
fn integer_literal_fits(expr: &Spanned<Expr>, ty: &ResolvedType) -> bool {
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

/// Fully resolved hook pair for one structural iteration protocol.
struct ResolvedIterationHooks {
    iterator_type: ResolvedType,
    next_type: ResolvedType,
    iter_dispatch: Option<ResolvedMethodDispatch>,
    next_dispatch: Option<ResolvedMethodDispatch>,
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

impl TypeChecker {
    /// Give an unsuffixed integer literal operand the exact-width integer type of the other operand.
    ///
    /// RFC 009 types an unsuffixed integer literal from its context, and for `+`, `-`, `*`, `//`, `%` and the
    /// comparisons the other operand is that context: `n + 1` adds two `i8` values when `n` is an `i8`, and `n == 0`
    /// compares two. The literal is checked again against that type as it is at a destination of that type, which
    /// records the type for lowering; in arithmetic that refuses a value outside the type's range (`n + 300`, or
    /// `b - -1` for a `u8` `b`), while a comparison keeps such a literal an `int` and compares in a type holding both
    /// (`b == -1` is `false`). `/` is left alone because it divides as floats whatever its operands are, and so is the
    /// exponent of `**`, which counts rather than being a value of the base's type. Returns the operand types after
    /// the literal has taken its partner's type.
    fn integer_literal_operand_takes_partner_type(
        &mut self,
        (left, left_ty): (&Spanned<Expr>, &ResolvedType),
        op: BinaryOp,
        (right, right_ty): (&Spanned<Expr>, &ResolvedType),
    ) -> (ResolvedType, ResolvedType) {
        if !matches!(
            op,
            BinaryOp::Add
                | BinaryOp::Sub
                | BinaryOp::Mul
                | BinaryOp::FloorDiv
                | BinaryOp::Mod
                | BinaryOp::Eq
                | BinaryOp::NotEq
                | BinaryOp::Lt
                | BinaryOp::LtEq
                | BinaryOp::Gt
                | BinaryOp::GtEq
        ) {
            return (left_ty.clone(), right_ty.clone());
        }
        let comparison = matches!(
            op,
            BinaryOp::Eq | BinaryOp::NotEq | BinaryOp::Lt | BinaryOp::LtEq | BinaryOp::Gt | BinaryOp::GtEq
        );
        let takes = |literal: &Spanned<Expr>, partner_ty: &ResolvedType| {
            is_exact_integer_type(partner_ty) && (!comparison || integer_literal_fits(literal, partner_ty))
        };
        match (
            is_unsuffixed_integer_literal(left),
            is_unsuffixed_integer_literal(right),
        ) {
            (false, true) if takes(right, left_ty) => {
                let right_ty = self.check_expr_with_expected(right, Some(left_ty));
                (left_ty.clone(), right_ty)
            }
            (true, false) if takes(left, right_ty) => {
                let left_ty = self.check_expr_with_expected(left, Some(right_ty));
                (left_ty, right_ty.clone())
            }
            _ => (left_ty.clone(), right_ty.clone()),
        }
    }

    /// Return the type of an arithmetic operation whose result the `int`/`float` table makes an integer.
    ///
    /// RFC 009: same-type integer arithmetic yields that type. `+`, `-`, `*`, `//` and `%` keep their operands' one
    /// integer type (`int` and `i64` are one type), and `**` with a non-negative integer literal exponent keeps its
    /// base's type. Operands of two different integer types are refused, because mixed-width integer arithmetic needs
    /// an explicit conversion of one operand; the refusal is recorded and yields `Unknown`.
    fn integer_arithmetic_result_type(
        &mut self,
        left_ty: &ResolvedType,
        op: BinaryOp,
        right_ty: &ResolvedType,
        span: Span,
    ) -> ResolvedType {
        if matches!(op, BinaryOp::Pow) {
            return left_ty.clone();
        }
        let left_id = numeric_type_id_for_compat(left_ty);
        if left_id.is_some() && left_id == numeric_type_id_for_compat(right_ty) {
            return left_ty.clone();
        }
        self.errors.push(errors::mixed_width_integer_arithmetic(
            &left_ty.to_string(),
            &op.to_string(),
            &right_ty.to_string(),
            span,
        ));
        ResolvedType::Unknown
    }

    /// Return the result type of an arithmetic operator over two numeric operands, by the operator result table.
    ///
    /// The table's one home for `a op b` and for `x op= y` (checked as `x = x op y`). An unsuffixed integer literal
    /// beside an exact-width integer first takes that integer's type. Two `f32` operands keep `f32`. An operation the
    /// `int`/`float` table gives an integer result keeps its operands' one integer type, and two different integer
    /// types are refused; every other operation yields `float`. A refusal is recorded and yields `Unknown`.
    pub(in crate::typechecker) fn numeric_arithmetic_result_type(
        &mut self,
        (left, left_ty): (&Spanned<Expr>, &ResolvedType),
        op: BinaryOp,
        (right, right_ty): (&Spanned<Expr>, &ResolvedType),
        span: Span,
    ) -> ResolvedType {
        let (Some(lhs), Some(rhs)) = (numeric_ty_from_resolved(left_ty), numeric_ty_from_resolved(right_ty)) else {
            let found = format!("{left_ty} {op} {right_ty}");
            self.errors.push(errors::type_mismatch("numeric", &found, span));
            return ResolvedType::Unknown;
        };
        let Some(num_op) = numeric_op_from_ast(&op) else {
            self.errors
                .push(errors::type_mismatch("numeric operator", &op.to_string(), span));
            return ResolvedType::Unknown;
        };
        let (left_ty, right_ty) =
            self.integer_literal_operand_takes_partner_type((left, left_ty), op, (right, right_ty));
        let pow_exp = if matches!(op, BinaryOp::Pow) {
            Some(pow_exponent_kind_from_ast(right, &right_ty))
        } else {
            None
        };
        // `f32` operands must not flow through the two-class numeric promotion table: it only distinguishes `int` and
        // `float`, so `f32 + f32` would incorrectly become `float`. Other float pairings still use that table.
        if left_ty == right_ty && matches!(left_ty, ResolvedType::Numeric(NumericTypeId::F32)) {
            return left_ty;
        }
        match result_numeric_type(num_op, lhs, rhs, pow_exp) {
            NumericTy::Int => self.integer_arithmetic_result_type(&left_ty, op, &right_ty, span),
            NumericTy::Float => ResolvedType::Float,
        }
    }

    /// Type-check a binary operation and return its result type.
    pub(in crate::typechecker::check_expr) fn check_binary(
        &mut self,
        left: &Spanned<Expr>,
        op: BinaryOp,
        right: &Spanned<Expr>,
        span: Span,
    ) -> ResolvedType {
        self.check_binary_with_expected(left, op, right, span, None)
    }

    /// Give an empty list literal compared against a list the partner operand's type, and record it (#1476).
    ///
    /// `values == []` checks `[]` as `List[Unknown]` and accepts the comparison by compatibility, but the recorded
    /// expression type is what lowering carries onto the literal and what emission spells. A bare `vec![]` beside a
    /// `Vec<String>` leaves rustc with an ambiguous `PartialEq` (E0283), and nothing downstream of the checker knows
    /// the element type: the comparison is the only place the two operands meet. Re-checking the empty literal
    /// against the partner's type records the unified `List[T]` for its span. A populated literal, a non-list
    /// partner, or a partner whose own element type is still unknown is left as checked, and no compatibility rule
    /// changes: an empty literal already compares against any list.
    fn adopt_partner_type_for_empty_list_operand(
        &mut self,
        operand: &Spanned<Expr>,
        operand_ty: ResolvedType,
        partner_ty: &ResolvedType,
    ) -> ResolvedType {
        if !is_empty_list_literal(&operand.node) {
            return operand_ty;
        }
        match runtime_list_elem_type(partner_ty) {
            Some(elem_ty) if !matches!(elem_ty, ResolvedType::Unknown) => {
                self.check_expr_with_expected(operand, Some(partner_ty))
            }
            _ => operand_ty,
        }
    }

    /// Type-check a binary operation with an optional contextual result type for overload disambiguation.
    pub(in crate::typechecker::check_expr) fn check_binary_with_expected(
        &mut self,
        left: &Spanned<Expr>,
        op: BinaryOp,
        right: &Spanned<Expr>,
        span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        if matches!(
            op,
            BinaryOp::Add
                | BinaryOp::Sub
                | BinaryOp::Mul
                | BinaryOp::Div
                | BinaryOp::FloorDiv
                | BinaryOp::Mod
                | BinaryOp::Pow
        ) && let Some(expected_ty) = expected_return_ty
        {
            self.validate_exact_float_literals_in_arithmetic(left, expected_ty);
            self.validate_exact_float_literals_in_arithmetic(right, expected_ty);
        }
        let left_ty = self.check_expr(left);
        let right_ty = self.check_expr(right);

        match op {
            BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::Mul
            | BinaryOp::Div
            | BinaryOp::FloorDiv
            | BinaryOp::Mod
            | BinaryOp::Pow => {
                // String concatenation special case (both sides must be str).
                if matches!(op, BinaryOp::Add) {
                    let lhs_is_str = is_str_like(&left_ty);
                    let rhs_is_str = is_str_like(&right_ty);
                    if lhs_is_str && rhs_is_str {
                        return ResolvedType::Str;
                    }
                    if lhs_is_str || rhs_is_str {
                        if self.is_user_operator_receiver(&left_ty)
                            && let Some(method) = binary_operator_dunder(op)
                        {
                            let args = vec![CallArg::Positional(right.clone())];
                            let arg_types = vec![right_ty.clone()];
                            if let Some(ret) = self.resolve_operator_dunder(
                                &left_ty,
                                method,
                                &args,
                                &arg_types,
                                span,
                                expected_return_ty,
                            ) {
                                self.type_info.record_resolved_operator_call(
                                    span,
                                    method,
                                    ResolvedOperatorKind::Binary,
                                );
                                return ret;
                            }
                        }
                        self.errors.push(errors::type_mismatch(
                            "str",
                            &format!("{} {} {}", left_ty, op, right_ty),
                            span,
                        ));
                        return ResolvedType::Unknown;
                    }
                }

                if matches!(op, BinaryOp::Add)
                    && is_runtime_list(&left_ty)
                    && is_runtime_list(&right_ty)
                    && self.types_compatible(&left_ty, &right_ty)
                {
                    if let Some(elem_ty) = runtime_list_elem_type(&left_ty)
                        && !self.is_copy_type(elem_ty)
                        && !self.is_clone_type(elem_ty)
                    {
                        self.errors
                            .push(errors::list_concat_requires_clone(&elem_ty.to_string(), span));
                        return ResolvedType::Unknown;
                    }
                    return left_ty.clone();
                }

                // RFC 023: allow arithmetic on generic type variables.
                //
                // The Rust backend infers and emits the trait bound the operator needs (`T: Add<Output = T>`), so the
                // typechecker stays permissive for `+`, `-` and `*` and generic stdlib helpers typecheck. A bound
                // trait's RFC 028 hook is consulted first, so `T with Remainder` resolves `%` through the trait.
                if self.is_generic_placeholder_type(&left_ty)
                    && let Some(method) = binary_operator_dunder(op)
                {
                    let args = vec![CallArg::Positional(right.clone())];
                    let arg_types = vec![right_ty.clone()];
                    if let Some(ret) =
                        self.resolve_operator_dunder(&left_ty, method, &args, &arg_types, span, expected_return_ty)
                    {
                        self.type_info
                            .record_resolved_operator_call(span, method, ResolvedOperatorKind::Binary);
                        return ret;
                    }
                }
                match (
                    self.generic_placeholder_name(&left_ty),
                    self.generic_placeholder_name(&right_ty),
                ) {
                    (Some(left_name), Some(right_name)) if left_name == right_name => {
                        // `/`, `//`, `%` and `**` have no bound a type argument could satisfy (#1715): their
                        // Python-shaped numeric semantics belong to the concrete numeric types, not to a trait.
                        if let Some(dunder) = numeric_only_operator_dunder(op) {
                            self.errors.push(errors::operator_has_no_type_parameter_bound(
                                &op.to_string(),
                                left_name,
                                dunder,
                                span,
                            ));
                            return ResolvedType::Unknown;
                        }
                        return left_ty.clone();
                    }
                    (Some(_), None) if matches!(right_ty, ResolvedType::Unknown) => return left_ty.clone(),
                    (None, Some(_)) if matches!(left_ty, ResolvedType::Unknown) => return right_ty.clone(),
                    _ => {}
                }

                // Check both operands are numeric
                let lhs_num = numeric_ty_from_resolved(&left_ty);
                let rhs_num = numeric_ty_from_resolved(&right_ty);

                match (lhs_num, rhs_num) {
                    (Some(_), Some(_)) => {
                        self.numeric_arithmetic_result_type((left, &left_ty), op, (right, &right_ty), span)
                    }
                    // Allow Unknown with numeric partner (treat as that numeric type): an integer partner keeps its
                    // own integer type, as same-type integer arithmetic does.
                    (Some(n), None) if matches!(right_ty, ResolvedType::Unknown | ResolvedType::RustPath(_)) => match n
                    {
                        NumericTy::Int => left_ty.clone(),
                        NumericTy::Float => ResolvedType::Float,
                    },
                    (None, Some(n)) if matches!(left_ty, ResolvedType::Unknown | ResolvedType::RustPath(_)) => {
                        match n {
                            NumericTy::Int => right_ty.clone(),
                            NumericTy::Float => ResolvedType::Float,
                        }
                    }
                    _ => {
                        if let Some(method) = binary_operator_dunder(op) {
                            let args = vec![CallArg::Positional(right.clone())];
                            let arg_types = vec![right_ty.clone()];
                            if let Some(ret) = self.resolve_operator_dunder(
                                &left_ty,
                                method,
                                &args,
                                &arg_types,
                                span,
                                expected_return_ty,
                            ) {
                                self.type_info.record_resolved_operator_call(
                                    span,
                                    method,
                                    ResolvedOperatorKind::Binary,
                                );
                                return ret;
                            }
                            if self.is_user_operator_receiver(&left_ty) {
                                self.errors
                                    .push(errors::missing_method(&left_ty.to_string(), method, span));
                                return ResolvedType::Unknown;
                            }
                        }
                        self.errors.push(errors::type_mismatch(
                            "numeric",
                            &format!("{} {} {}", left_ty, op, right_ty),
                            span,
                        ));
                        ResolvedType::Unknown
                    }
                }
            }
            BinaryOp::MatMul
            | BinaryOp::PipeForward
            | BinaryOp::PipeBackward
            | BinaryOp::BitAnd
            | BinaryOp::BitOr
            | BinaryOp::BitXor
            | BinaryOp::Shl
            | BinaryOp::Shr => {
                if matches!(
                    op,
                    BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor | BinaryOp::Shl | BinaryOp::Shr
                ) {
                    match (numeric_ty_from_resolved(&left_ty), numeric_ty_from_resolved(&right_ty)) {
                        (Some(NumericTy::Int), Some(NumericTy::Int)) => return ResolvedType::Int,
                        (Some(NumericTy::Int), None) if matches!(right_ty, ResolvedType::Unknown) => {
                            return ResolvedType::Int;
                        }
                        (None, Some(NumericTy::Int)) if matches!(left_ty, ResolvedType::Unknown) => {
                            return ResolvedType::Int;
                        }
                        _ => {}
                    }
                }

                let Some(method) = binary_operator_dunder(op) else {
                    self.errors
                        .push(errors::type_mismatch("overloadable operator", &op.to_string(), span));
                    return ResolvedType::Unknown;
                };
                let args = vec![CallArg::Positional(right.clone())];
                let arg_types = vec![right_ty.clone()];
                if let Some(ret) =
                    self.resolve_operator_dunder(&left_ty, method, &args, &arg_types, span, expected_return_ty)
                {
                    self.type_info
                        .record_resolved_operator_call(span, method, ResolvedOperatorKind::Binary);
                    return ret;
                }
                if self.is_user_operator_receiver(&left_ty) {
                    self.errors
                        .push(errors::missing_method(&left_ty.to_string(), method, span));
                } else {
                    self.errors.push(errors::type_mismatch(
                        &format!("operator receiver with {}", method),
                        &format!("{} {} {}", left_ty, op, right_ty),
                        span,
                    ));
                }
                ResolvedType::Unknown
            }
            // Comparisons: allow mixed numeric types (promote for comparison), result is Bool
            BinaryOp::Eq | BinaryOp::NotEq | BinaryOp::Lt | BinaryOp::Gt | BinaryOp::LtEq | BinaryOp::GtEq => {
                let left_ty = self.adopt_partner_type_for_empty_list_operand(left, left_ty, &right_ty);
                let right_ty = self.adopt_partner_type_for_empty_list_operand(right, right_ty, &left_ty);
                // If both are numeric, allow mixed comparisons
                let lhs_num = numeric_ty_from_resolved(&left_ty);
                let rhs_num = numeric_ty_from_resolved(&right_ty);
                if lhs_num.is_some() && rhs_num.is_some() {
                    let (left_ty, right_ty) =
                        self.integer_literal_operand_takes_partner_type((left, &left_ty), op, (right, &right_ty));
                    // Mixed numeric comparison is valid (promotion handled at codegen): two integer types compare in
                    // the narrowest integer type both widen to, and a pair no integer type holds is refused.
                    if let (Some(left_id), Some(right_id)) = (
                        numeric_type_id_for_compat(&left_ty),
                        numeric_type_id_for_compat(&right_ty),
                    ) && numerics::is_integer(left_id)
                        && numerics::is_integer(right_id)
                        && incan_lang::numeric_values::common_lossless_integer_type(left_id, right_id).is_none()
                    {
                        self.errors.push(errors::incomparable_integer_types(
                            &left_ty.to_string(),
                            &op.to_string(),
                            &right_ty.to_string(),
                            span,
                        ));
                    }
                    ResolvedType::Bool
                } else if (is_str_like)(&left_ty) && (is_str_like)(&right_ty) {
                    ResolvedType::Bool
                } else if decimal_shape(&left_ty).is_some() && decimal_shape(&right_ty).is_some() {
                    // Decimal values compare by value, whatever precision and scale each side declares (#1810);
                    // assignability between the two shapes does not enter into it.
                    ResolvedType::Bool
                } else if let Some(method) = binary_operator_dunder(op)
                    && self.type_has_derive_backed_comparison_operator(&left_ty, op)
                    && !self.type_has_inherent_operator_method(&left_ty, method)
                    && self.types_compatible(&left_ty, &right_ty)
                {
                    ResolvedType::Bool
                } else if let Some(method) = binary_operator_dunder(op)
                    && self.is_user_operator_receiver(&left_ty)
                {
                    let args = vec![CallArg::Positional(right.clone())];
                    let arg_types = vec![right_ty.clone()];
                    if let Some(ret) = self.resolve_operator_dunder(
                        &left_ty,
                        method,
                        &args,
                        &arg_types,
                        span,
                        Some(&ResolvedType::Bool),
                    ) {
                        self.type_info
                            .record_resolved_operator_call(span, method, ResolvedOperatorKind::Binary);
                        if !self.types_compatible(&ret, &ResolvedType::Bool) {
                            self.errors.push(errors::type_mismatch("bool", &ret.to_string(), span));
                        }
                        ResolvedType::Bool
                    } else if !self.is_generic_placeholder_type(&left_ty)
                        && (matches!(op, BinaryOp::Lt | BinaryOp::Gt | BinaryOp::LtEq | BinaryOp::GtEq)
                            || self.type_has_any_operator_method(&left_ty, comparison_dunders(), span))
                    {
                        self.errors
                            .push(errors::missing_method(&left_ty.to_string(), method, span));
                        ResolvedType::Unknown
                    } else if self.refuse_comparison_without_derive(&left_ty, op, span) {
                        ResolvedType::Unknown
                    } else if left_ty == right_ty || self.types_compatible(&left_ty, &right_ty) {
                        ResolvedType::Bool
                    } else {
                        self.errors.push(errors::type_mismatch(
                            &format!("comparable to {}", left_ty),
                            &right_ty.to_string(),
                            span,
                        ));
                        ResolvedType::Bool
                    }
                } else if left_ty == right_ty || self.types_compatible(&left_ty, &right_ty) {
                    // Same-type or compatible comparison, which the operands' type must implement.
                    if self.refuse_comparison_without_derive(&left_ty, op, span) {
                        return ResolvedType::Unknown;
                    }
                    ResolvedType::Bool
                } else {
                    // Different non-numeric types
                    self.errors.push(errors::type_mismatch(
                        &format!("comparable to {}", left_ty),
                        &right_ty.to_string(),
                        span,
                    ));
                    ResolvedType::Bool
                }
            }
            BinaryOp::And | BinaryOp::Or => ResolvedType::Bool,
            BinaryOp::In | BinaryOp::NotIn => {
                let lhs_is_str = is_str_like(&left_ty);
                let rhs_is_str = is_str_like(&right_ty);

                // str in str
                if lhs_is_str && rhs_is_str {
                    return ResolvedType::Bool;
                }

                // A `const` frozen collection answers membership as its mutable form does: an element of a
                // `FrozenList` or `FrozenSet`, a key of a `FrozenDict`. A text element or key takes any text probe.
                let frozen_member_ty = match &right_ty {
                    ResolvedType::FrozenList(elem) | ResolvedType::FrozenSet(elem) => Some(elem.as_ref()),
                    ResolvedType::FrozenDict(key, _) => Some(key.as_ref()),
                    _ => None,
                };
                if let Some(member_ty) = frozen_member_ty {
                    let text_probe_for_text_member = lhs_is_str && is_str_like(member_ty);
                    if !text_probe_for_text_member && self.types_compatible(&left_ty, member_ty) {
                        // A numeric probe of a narrower type is widened to the member type where it is lowered.
                        self.record_value_destination_if_compatible(left.span, &left_ty, member_ty);
                    } else if !text_probe_for_text_member && !matches!(left_ty, ResolvedType::Unknown) {
                        self.errors.push(errors::type_mismatch(
                            &member_ty.to_string(),
                            &left_ty.to_string(),
                            span,
                        ));
                    }
                    return ResolvedType::Bool;
                }

                // List/Set membership: "<item> in <collection>"
                if let ResolvedType::Generic(name, args) = &right_ty {
                    match collection_type_id(name.as_str()) {
                        Some(CollectionTypeId::List | CollectionTypeId::Set) if !args.is_empty() => {
                            let elem_ty = &args[0];
                            if self.types_compatible(&left_ty, elem_ty) || matches!(left_ty, ResolvedType::Unknown) {
                                // A numeric probe of a narrower type is widened to the element type where it is
                                // lowered (RFC 009).
                                self.record_value_destination_if_compatible(left.span, &left_ty, elem_ty);
                                // A list finds its item by equality, which the element type must implement (#1870); a
                                // set's element type is held to `Eq` and `Hash` where the set type is written.
                                if collection_type_id(name.as_str()) == Some(CollectionTypeId::List) {
                                    self.refuse_comparison_without_derive(elem_ty, op, span);
                                }
                                return ResolvedType::Bool;
                            }
                            self.errors
                                .push(errors::type_mismatch(&elem_ty.to_string(), &left_ty.to_string(), span));
                            return ResolvedType::Bool;
                        }
                        Some(CollectionTypeId::Dict) if args.len() >= 2 => {
                            let key_ty = &args[0];
                            if self.types_compatible(&left_ty, key_ty) || matches!(left_ty, ResolvedType::Unknown) {
                                self.record_value_destination_if_compatible(left.span, &left_ty, key_ty);
                                return ResolvedType::Bool;
                            }
                            self.errors
                                .push(errors::type_mismatch(&key_ty.to_string(), &left_ty.to_string(), span));
                            return ResolvedType::Bool;
                        }
                        _ => {}
                    }
                }

                if self.is_user_operator_receiver(&right_ty) {
                    let _ = self.resolve_contains_dunder(&right_ty, left, &left_ty, span);
                    return ResolvedType::Bool;
                }

                // Fallback: keep previous permissive behavior but note mismatch.
                self.errors.push(errors::type_mismatch(
                    "supported membership (str, list, set, dict)",
                    &format!("{} {} {}", left_ty, op, right_ty),
                    span,
                ));
                ResolvedType::Bool
            }
            BinaryOp::Is | BinaryOp::IsNot => ResolvedType::Bool,
        }
    }

    /// Type-check a unary operation and return its result type.
    pub(in crate::typechecker::check_expr) fn check_unary(
        &mut self,
        op: UnaryOp,
        operand: &Spanned<Expr>,
        span: Span,
    ) -> ResolvedType {
        self.check_unary_with_expected(op, operand, span, None)
    }

    /// Type-check a unary operation with an optional contextual result type for overload disambiguation.
    pub(in crate::typechecker::check_expr) fn check_unary_with_expected(
        &mut self,
        op: UnaryOp,
        operand: &Spanned<Expr>,
        span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        if matches!(op, UnaryOp::Neg)
            && let Expr::Literal(Literal::Int(value)) = &operand.node
            && let Some(suffix) = value.suffix
        {
            let operand_ty = self.check_suffixed_int_literal(value, true, operand.span);
            self.record_expr_type(operand.span, operand_ty.clone());
            if numerics::info_for(suffix).family == NumericFamily::UnsignedInteger {
                self.errors
                    .push(errors::type_mismatch("signed numeric", &operand_ty.to_string(), span));
                return ResolvedType::Unknown;
            }
            return operand_ty;
        }
        let operand_ty = self.check_expr(operand);
        match op {
            UnaryOp::Neg => {
                let exact_family = match &operand_ty {
                    ResolvedType::Numeric(id) => Some(numerics::info_for(*id).family),
                    _ => None,
                };
                if matches!(
                    exact_family,
                    Some(NumericFamily::SignedInteger | NumericFamily::BinaryFloat)
                ) {
                    // A negated exact-width value keeps its type, as same-type arithmetic does.
                    operand_ty
                } else if exact_family == Some(NumericFamily::UnsignedInteger) {
                    self.errors
                        .push(errors::type_mismatch("signed numeric", &operand_ty.to_string(), span));
                    ResolvedType::Unknown
                } else if self.types_compatible(&operand_ty, &ResolvedType::Int) {
                    ResolvedType::Int
                } else if self.types_compatible(&operand_ty, &ResolvedType::Float) {
                    ResolvedType::Float
                } else {
                    let method = "__neg__";
                    if let Some(ret) =
                        self.resolve_operator_dunder(&operand_ty, method, &[], &[], span, expected_return_ty)
                    {
                        self.type_info
                            .record_resolved_operator_call(span, method, ResolvedOperatorKind::Unary);
                        return ret;
                    }
                    if self.is_user_operator_receiver(&operand_ty) {
                        self.errors
                            .push(errors::missing_method(&operand_ty.to_string(), method, span));
                        return ResolvedType::Unknown;
                    }
                    self.errors
                        .push(errors::type_mismatch("numeric", &operand_ty.to_string(), span));
                    ResolvedType::Unknown
                }
            }
            UnaryOp::Not => {
                if !self.types_compatible(&operand_ty, &ResolvedType::Bool) {
                    self.errors
                        .push(errors::type_mismatch("bool", &operand_ty.to_string(), span));
                }
                ResolvedType::Bool
            }
            UnaryOp::Invert => {
                if self.types_compatible(&operand_ty, &ResolvedType::Int) {
                    ResolvedType::Int
                } else {
                    let method = "__invert__";
                    if let Some(ret) =
                        self.resolve_operator_dunder(&operand_ty, method, &[], &[], span, expected_return_ty)
                    {
                        self.type_info
                            .record_resolved_operator_call(span, method, ResolvedOperatorKind::Unary);
                        return ret;
                    }
                    if self.is_user_operator_receiver(&operand_ty) {
                        self.errors
                            .push(errors::missing_method(&operand_ty.to_string(), method, span));
                        return ResolvedType::Unknown;
                    }
                    self.errors
                        .push(errors::type_mismatch("int", &operand_ty.to_string(), span));
                    ResolvedType::Unknown
                }
            }
        }
    }

    /// Resolve a user-defined indexing operation (`base[index]`) through `__getitem__`, if available.
    pub(in crate::typechecker) fn resolve_index_dunder(
        &mut self,
        base_ty: &ResolvedType,
        index: &Spanned<Expr>,
        index_ty: &ResolvedType,
        span: Span,
    ) -> Option<ResolvedType> {
        let method = "__getitem__";
        let args = vec![CallArg::Positional(index.clone())];
        let arg_types = vec![index_ty.clone()];
        let ret = self.resolve_operator_dunder(base_ty, method, &args, &arg_types, span, None)?;
        self.type_info
            .record_resolved_operator_call(span, method, ResolvedOperatorKind::Index);
        Some(ret)
    }

    /// Validate a boolean control-flow condition using RFC 068 structural `__bool__` when needed.
    pub(in crate::typechecker) fn validate_truthiness_condition(&mut self, cond_ty: &ResolvedType, span: Span) {
        if self.types_compatible(cond_ty, &ResolvedType::Bool) || matches!(cond_ty, ResolvedType::Unknown) {
            return;
        }
        if cond_ty.is_option() || cond_ty.is_result() {
            self.errors
                .push(errors::type_mismatch("bool", &cond_ty.to_string(), span));
            return;
        }

        if self.is_user_operator_receiver(cond_ty) {
            let method = "__bool__";
            let args: Vec<CallArg> = Vec::new();
            let arg_types: Vec<ResolvedType> = Vec::new();
            match self.resolve_operator_dunder(cond_ty, method, &args, &arg_types, span, Some(&ResolvedType::Bool)) {
                Some(ret) => {
                    if !self.types_compatible(&ret, &ResolvedType::Bool) {
                        self.errors.push(errors::type_mismatch("bool", &ret.to_string(), span));
                        return;
                    }
                    self.type_info
                        .record_resolved_operator_call(span, method, ResolvedOperatorKind::Truthiness);
                }
                None => self
                    .errors
                    .push(errors::missing_method(&cond_ty.to_string(), method, span)),
            }
            return;
        }

        self.errors
            .push(errors::type_mismatch("bool", &cond_ty.to_string(), span));
    }

    /// Resolve `len(x)` for a user-defined receiver through `__len__(self) -> int`.
    pub(in crate::typechecker) fn resolve_len_dunder(
        &mut self,
        receiver_ty: &ResolvedType,
        span: Span,
    ) -> Option<ResolvedType> {
        let method = "__len__";
        let args: Vec<CallArg> = Vec::new();
        let arg_types: Vec<ResolvedType> = Vec::new();
        let ret = self.resolve_operator_dunder(receiver_ty, method, &args, &arg_types, span, Some(&ResolvedType::Int));
        match ret {
            Some(ret) => {
                if !self.types_compatible(&ret, &ResolvedType::Int) {
                    self.errors.push(errors::type_mismatch("int", &ret.to_string(), span));
                    return Some(ResolvedType::Unknown);
                }
                self.type_info
                    .record_resolved_operator_call(span, method, ResolvedOperatorKind::Len);
                Some(ResolvedType::Int)
            }
            None => {
                self.errors
                    .push(errors::missing_method(&receiver_ty.to_string(), method, span));
                None
            }
        }
    }

    /// Resolve `item in receiver` for a user-defined receiver through `__contains__(self, item) -> bool`.
    pub(in crate::typechecker) fn resolve_contains_dunder(
        &mut self,
        receiver_ty: &ResolvedType,
        item: &Spanned<Expr>,
        item_ty: &ResolvedType,
        span: Span,
    ) -> Option<ResolvedType> {
        let method = "__contains__";
        let args = vec![CallArg::Positional(item.clone())];
        let arg_types = vec![item_ty.clone()];
        let ret = self.resolve_operator_dunder(receiver_ty, method, &args, &arg_types, span, Some(&ResolvedType::Bool));
        match ret {
            Some(ret) => {
                if !self.types_compatible(&ret, &ResolvedType::Bool) {
                    self.errors.push(errors::type_mismatch("bool", &ret.to_string(), span));
                    return Some(ResolvedType::Unknown);
                }
                self.type_info
                    .record_resolved_operator_call(span, method, ResolvedOperatorKind::Contains);
                Some(ResolvedType::Bool)
            }
            None => {
                self.errors
                    .push(errors::missing_method(&receiver_ty.to_string(), method, span));
                None
            }
        }
    }

    /// Resolve `receiver(...)` for callable user-defined objects through `__call__`.
    pub(in crate::typechecker) fn resolve_call_dunder(
        &mut self,
        receiver_ty: &ResolvedType,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
    ) -> Option<ResolvedType> {
        let method = "__call__";
        let ret = self.resolve_operator_dunder(receiver_ty, method, args, arg_types, span, None);
        match ret {
            Some(ret) => {
                self.type_info
                    .record_resolved_operator_call(span, method, ResolvedOperatorKind::Call);
                Some(ret)
            }
            None => {
                self.errors
                    .push(errors::missing_method(&receiver_ty.to_string(), method, span));
                None
            }
        }
    }

    /// Resolve custom `for` iteration through `__iter__(self)` and `iterator.__next__() -> Option[T]`.
    pub(in crate::typechecker) fn resolve_iteration_protocol(
        &mut self,
        receiver_ty: &ResolvedType,
        span: Span,
    ) -> Option<ResolvedType> {
        let iter_method = magic_methods::as_str(MagicMethodId::Iter);
        let next_method = magic_methods::as_str(MagicMethodId::Next);
        let hooks = self.resolve_iteration_protocol_hooks(receiver_ty, span, span)?;
        let iterator_ty = hooks.iterator_type;
        let next_ret = hooks.next_type;

        let Some(item_ty) = next_ret.option_inner_type().cloned() else {
            self.errors
                .push(errors::type_mismatch("Option[_]", &next_ret.to_string(), span));
            return Some(ResolvedType::Unknown);
        };

        self.type_info.record_protocol_iteration(
            span,
            ProtocolIterationInfo {
                iter_method: iter_method.to_string(),
                iterator_type: iterator_ty,
                next_method: next_method.to_string(),
                item_type: item_ty.clone(),
                iter_dispatch: hooks.iter_dispatch,
                next_dispatch: hooks.next_dispatch,
                fallible_error_type: None,
            },
        );
        Some(item_ty)
    }

    /// Resolve `for item in iterable?` through `__iter__(self)` and a fallible `__next__() -> Result[Option[T], E]`
    /// protocol.
    pub(in crate::typechecker) fn resolve_fallible_iteration_protocol(
        &mut self,
        receiver_ty: &ResolvedType,
        receiver_span: Span,
        protocol_span: Span,
    ) -> Option<ResolvedType> {
        let iter_method = magic_methods::as_str(MagicMethodId::Iter);
        let next_method = magic_methods::as_str(MagicMethodId::Next);
        let hooks = self.resolve_iteration_protocol_hooks(receiver_ty, receiver_span, protocol_span)?;
        let iterator_ty = hooks.iterator_type;
        let next_ret = hooks.next_type;

        let Some(item_ty) = next_ret
            .result_ok_type()
            .and_then(ResolvedType::option_inner_type)
            .cloned()
        else {
            self.errors.push(errors::type_mismatch(
                "Result[Option[_], _]",
                &next_ret.to_string(),
                protocol_span,
            ));
            return Some(ResolvedType::Unknown);
        };
        let Some(error_ty) = next_ret.result_err_type().cloned() else {
            return Some(ResolvedType::Unknown);
        };

        match self.current_return_error_type.clone() {
            Some(expected_error) if !self.types_compatible(&error_ty, &expected_error) => {
                self.errors.push(errors::incompatible_error_type(
                    &expected_error.to_string(),
                    &error_ty.to_string(),
                    protocol_span,
                ));
            }
            None => self.errors.push(errors::try_without_result_return(protocol_span)),
            _ => {}
        }

        self.type_info.record_protocol_iteration(
            protocol_span,
            ProtocolIterationInfo {
                iter_method: iter_method.to_string(),
                iterator_type: iterator_ty,
                next_method: next_method.to_string(),
                item_type: item_ty.clone(),
                iter_dispatch: hooks.iter_dispatch,
                next_dispatch: hooks.next_dispatch,
                fallible_error_type: Some(error_ty),
            },
        );
        Some(item_ty)
    }

    /// Probe the fallible iteration shape while rolling back diagnostics and semantic artifacts.
    pub(in crate::typechecker) fn probe_fallible_iteration_protocol(
        &mut self,
        receiver_ty: &ResolvedType,
        span: Span,
    ) -> bool {
        if !self.is_user_operator_receiver(receiver_ty) {
            return false;
        }

        let baseline_errors = self.errors.clone();
        let baseline_warnings = self.warnings.clone();
        let baseline_type_info = self.type_info.clone();
        let baseline_consumed_iterator_bindings = self.consumed_iterator_bindings.clone();
        let result = self
            .resolve_iteration_protocol_hooks(receiver_ty, span, span)
            .is_some_and(|hooks| {
                hooks
                    .next_type
                    .result_ok_type()
                    .and_then(ResolvedType::option_inner_type)
                    .is_some()
                    && hooks.next_type.result_err_type().is_some()
            });
        self.errors = baseline_errors;
        self.warnings = baseline_warnings;
        self.type_info = baseline_type_info;
        self.consumed_iterator_bindings = baseline_consumed_iterator_bindings;
        result
    }

    /// Resolve the shared `__iter__` and `__next__` hook pair for one custom iteration route.
    fn resolve_iteration_protocol_hooks(
        &mut self,
        receiver_ty: &ResolvedType,
        receiver_span: Span,
        protocol_span: Span,
    ) -> Option<ResolvedIterationHooks> {
        let args: Vec<CallArg> = Vec::new();
        let arg_types: Vec<ResolvedType> = Vec::new();
        let iter_method = magic_methods::as_str(MagicMethodId::Iter);
        let next_method = magic_methods::as_str(MagicMethodId::Next);
        let receiver_dispatch = self
            .type_info
            .resolved_method_call(receiver_span)
            .map(|call| call.dispatch.clone());
        let iterator_ty = match self.resolve_protocol_operator_dunder(
            receiver_ty,
            iter_method,
            &args,
            &arg_types,
            protocol_span,
            receiver_dispatch.as_ref(),
        ) {
            Some(iterator_ty) => iterator_ty,
            None => {
                self.errors.push(errors::missing_method(
                    &receiver_ty.to_string(),
                    iter_method,
                    protocol_span,
                ));
                return None;
            }
        };
        let mut iter_dispatch = self
            .type_info
            .resolved_method_call(protocol_span)
            .filter(|call| call.method == iter_method)
            .map(|call| call.dispatch.clone());
        inherit_same_trait_dispatch_module(receiver_dispatch.as_ref(), &mut iter_dispatch);
        let next_ret = match self.resolve_protocol_operator_dunder(
            &iterator_ty,
            next_method,
            &args,
            &arg_types,
            protocol_span,
            iter_dispatch.as_ref(),
        ) {
            Some(next_ret) => next_ret,
            None => {
                self.errors.push(errors::missing_method(
                    &iterator_ty.to_string(),
                    next_method,
                    protocol_span,
                ));
                return None;
            }
        };
        let mut next_dispatch = self
            .type_info
            .resolved_method_call(protocol_span)
            .filter(|call| call.method == next_method)
            .map(|call| call.dispatch.clone());
        inherit_same_trait_dispatch_module(iter_dispatch.as_ref(), &mut next_dispatch);
        Some(ResolvedIterationHooks {
            iterator_type: iterator_ty,
            next_type: next_ret,
            iter_dispatch,
            next_dispatch,
        })
    }

    /// Resolve a synthetic protocol hook, retaining the exact trait provenance selected by the source expression.
    ///
    /// A source method can return its owning trait without making that trait name directly visible to the consumer.
    /// Ordinary operator lookup therefore has no unqualified trait symbol to consult. The source call's recorded
    /// dispatch remains authoritative: use its canonical module only when the returned receiver is that same trait.
    fn resolve_protocol_operator_dunder(
        &mut self,
        receiver_ty: &ResolvedType,
        method: &str,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
        source_dispatch: Option<&ResolvedMethodDispatch>,
    ) -> Option<ResolvedType> {
        if let Some(resolved) = self.resolve_operator_dunder(receiver_ty, method, args, arg_types, span, None) {
            return Some(resolved);
        }

        let (receiver_name, receiver_args) = match receiver_ty {
            ResolvedType::Named(name) => (name, Vec::new()),
            ResolvedType::Generic(name, type_args) => (name, type_args.clone()),
            _ => return None,
        };
        let Some(ResolvedMethodDispatch::Trait {
            trait_name,
            module_path: Some(module_path),
            ..
        }) = source_dispatch
        else {
            return None;
        };
        if receiver_name != trait_name {
            return None;
        }

        let adoption = TypeBoundInfo {
            name: trait_name.clone(),
            source_name: None,
            type_args: receiver_args,
            module_path: Some(module_path.clone()),
            implementation_type_params: Vec::new(),
            inferred: false,
        };
        self.resolve_named_method(
            &std::collections::HashMap::new(),
            None,
            Some(std::slice::from_ref(&adoption)),
            method,
            MemberBindingSurface::Instance,
            &[],
            args,
            arg_types,
            span,
            receiver_ty,
            None,
        )
    }

    /// Resolve a user-defined index assignment (`base[index] = value`) through `__setitem__`, if available.
    pub(in crate::typechecker) fn resolve_index_set_dunder(
        &mut self,
        base_ty: &ResolvedType,
        index: &Spanned<Expr>,
        index_ty: &ResolvedType,
        value: &Spanned<Expr>,
        value_ty: &ResolvedType,
        span: Span,
    ) -> Option<ResolvedType> {
        let method = "__setitem__";
        let args = vec![CallArg::Positional(index.clone()), CallArg::Positional(value.clone())];
        let arg_types = vec![index_ty.clone(), value_ty.clone()];
        let expected_return_ty = ResolvedType::Unit;
        let ret = self.resolve_operator_dunder(base_ty, method, &args, &arg_types, span, Some(&expected_return_ty))?;
        if !self.types_compatible(&ret, &expected_return_ty) {
            self.errors.push(errors::type_mismatch(
                &expected_return_ty.to_string(),
                &ret.to_string(),
                span,
            ));
        }
        self.type_info
            .record_resolved_operator_call(span, method, ResolvedOperatorKind::IndexAssign);
        Some(ret)
    }

    /// Resolve compound assignment as an in-place hook first, then the ordinary binary hook.
    pub(in crate::typechecker) fn resolve_compound_assignment_operator(
        &mut self,
        receiver_ty: &ResolvedType,
        op: CompoundOp,
        value: &Spanned<Expr>,
        value_ty: &ResolvedType,
        span: Span,
    ) -> Option<ResolvedType> {
        let args = vec![CallArg::Positional(value.clone())];
        let arg_types = vec![value_ty.clone()];
        let in_place = compound_in_place_dunder(op);
        if let Some(ret) =
            self.resolve_operator_dunder(receiver_ty, in_place, &args, &arg_types, span, Some(receiver_ty))
        {
            self.type_info
                .record_resolved_operator_call(span, in_place, ResolvedOperatorKind::Binary);
            return Some(ret);
        }
        let binary = binary_operator_dunder(compound_binary_op(op))?;
        let ret = self.resolve_operator_dunder(receiver_ty, binary, &args, &arg_types, span, Some(receiver_ty))?;
        self.type_info
            .record_resolved_operator_call(span, binary, ResolvedOperatorKind::Binary);
        Some(ret)
    }

    /// Resolve one operator dunder on a user type or generic placeholder.
    ///
    /// Also resolves the `message()` an `Error` adopter displays through (`check_expr/error_display.rs`), which, like a
    /// dunder, is a method call the program never wrote.
    pub(in crate::typechecker::check_expr) fn resolve_operator_dunder(
        &mut self,
        receiver_ty: &ResolvedType,
        method: &str,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        if self.is_generic_placeholder_type(receiver_ty) {
            let placeholder_name = self.generic_placeholder_name(receiver_ty)?.to_string();
            return self.resolve_generic_placeholder_method(
                &placeholder_name,
                method,
                &[],
                args,
                arg_types,
                span,
                receiver_ty,
                expected_return_ty,
            );
        }
        match receiver_ty {
            ResolvedType::Generic(type_name, _type_args) => {
                if let Some(type_info) = self.lookup_semantic_type_info(type_name).cloned() {
                    self.resolve_operator_dunder_on_type_info(
                        &type_info,
                        method,
                        args,
                        arg_types,
                        span,
                        receiver_ty,
                        expected_return_ty,
                    )
                } else {
                    self.resolve_trait_receiver_method(
                        receiver_ty,
                        method,
                        MemberBindingSurface::Instance,
                        &[],
                        args,
                        arg_types,
                        span,
                        expected_return_ty,
                    )
                }
            }
            ResolvedType::Named(type_name) => {
                if let Some(type_info) = self.lookup_semantic_type_info(type_name).cloned() {
                    self.resolve_operator_dunder_on_type_info(
                        &type_info,
                        method,
                        args,
                        arg_types,
                        span,
                        receiver_ty,
                        expected_return_ty,
                    )
                } else {
                    self.resolve_trait_receiver_method(
                        receiver_ty,
                        method,
                        MemberBindingSurface::Instance,
                        &[],
                        args,
                        arg_types,
                        span,
                        expected_return_ty,
                    )
                }
            }
            _ => None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    /// Resolve an operator dunder against concrete type metadata and its adopted traits.
    fn resolve_operator_dunder_on_type_info(
        &mut self,
        type_info: &TypeInfo,
        method: &str,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
        receiver_ty: &ResolvedType,
        expected_return_ty: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        match type_info {
            TypeInfo::Model(model) => {
                let trait_adoptions = self.trait_adoptions_for_type_methods(&model.trait_adoptions, &model.derives);
                self.resolve_named_method(
                    &model.methods,
                    Some(&model.method_overloads),
                    Some(&trait_adoptions),
                    method,
                    MemberBindingSurface::Instance,
                    &[],
                    args,
                    arg_types,
                    span,
                    receiver_ty,
                    expected_return_ty,
                )
            }
            TypeInfo::Class(class) => {
                let trait_adoptions = self.trait_adoptions_for_type_methods(&class.trait_adoptions, &class.derives);
                self.resolve_named_method(
                    &class.methods,
                    Some(&class.method_overloads),
                    Some(&trait_adoptions),
                    method,
                    MemberBindingSurface::Instance,
                    &[],
                    args,
                    arg_types,
                    span,
                    receiver_ty,
                    expected_return_ty,
                )
            }
            TypeInfo::Enum(en) => {
                let trait_adoptions = self.trait_adoptions_for_type_methods(&en.trait_adoptions, &en.derives);
                self.resolve_named_method(
                    &en.methods,
                    Some(&en.method_overloads),
                    Some(&trait_adoptions),
                    method,
                    MemberBindingSurface::Instance,
                    &[],
                    args,
                    arg_types,
                    span,
                    receiver_ty,
                    expected_return_ty,
                )
            }
            TypeInfo::Newtype(newtype) => {
                let resolved_method = self.resolve_newtype_method_name(newtype, method);
                self.resolve_named_method(
                    &newtype.methods,
                    Some(&newtype.method_overloads),
                    Some(&newtype.trait_adoptions),
                    resolved_method,
                    MemberBindingSurface::Instance,
                    &[],
                    args,
                    arg_types,
                    span,
                    receiver_ty,
                    expected_return_ty,
                )
            }
            TypeInfo::Builtin | TypeInfo::TypeAlias => None,
        }
    }

    /// Return whether a type can participate in user-defined operator dispatch.
    pub(in crate::typechecker) fn is_user_operator_receiver(&self, ty: &ResolvedType) -> bool {
        if self.is_generic_placeholder_type(ty) {
            return true;
        }
        match ty {
            ResolvedType::Generic(name, _) | ResolvedType::Named(name) => {
                matches!(
                    self.lookup_semantic_type_info(name),
                    Some(TypeInfo::Class(_) | TypeInfo::Model(_) | TypeInfo::Enum(_) | TypeInfo::Newtype(_))
                ) || self.lookup_semantic_trait_info(name).is_some()
            }
            _ => false,
        }
    }

    /// Return whether a type has Rust-backed comparison derives for this operator: the operator's derive declared or
    /// implied (`Ord` implies `PartialEq`), spelled in `@rust.derive(...)`, or an enum's automatic `PartialEq`.
    fn type_has_derive_backed_comparison_operator(&self, ty: &ResolvedType, op: BinaryOp) -> bool {
        comparison_operator_derive(op).is_some_and(|derive| self.comparison_is_derived(ty, derive))
    }

    /// Refuse a comparison whose operand type is known not to implement the operator's trait (#1870).
    ///
    /// Called once the operator's dunders have not resolved it: `==`, `!=` and list membership need `PartialEq` and the
    /// orderings need `PartialOrd`, of the operand and of every type inside it, as the generated comparison does. For
    /// membership `ty` is the list's element type. A type the derive relation cannot decide is accepted. Returns
    /// whether the comparison was refused.
    fn refuse_comparison_without_derive(&mut self, ty: &ResolvedType, op: BinaryOp, span: Span) -> bool {
        let Some(derive) = comparison_operator_derive(op) else {
            return false;
        };
        let DeriveSupport::Missing(holder) = self.derive_support(ty, derive) else {
            return false;
        };
        // The catalog derive that provides the operator; `Eq` and `Ord` bring `PartialEq` and `PartialOrd` with them.
        let providing_derive = if derive == DeriveId::PartialEq {
            DeriveId::Eq
        } else {
            DeriveId::Ord
        };
        // Membership compares items with `==`, so its dunder is `__eq__`.
        let dunder_op = if matches!(op, BinaryOp::In | BinaryOp::NotIn) {
            BinaryOp::Eq
        } else {
            op
        };
        let dunder = self
            .is_user_operator_receiver(&holder)
            .then(|| binary_operator_dunder(dunder_op))
            .flatten();
        self.errors.push(errors::operator_not_provided(
            &ty.to_string(),
            &holder.to_string(),
            &op.to_string(),
            derives::as_str(providing_derive),
            dunder,
            span,
        ));
        true
    }

    /// Return whether a type directly declares an operator method, excluding derived trait defaults.
    fn type_has_inherent_operator_method(&self, ty: &ResolvedType, method: &str) -> bool {
        match ty {
            ResolvedType::Generic(type_name, _) | ResolvedType::Named(type_name) => {
                let Some(type_info) = self.lookup_semantic_type_info(type_name) else {
                    return false;
                };
                match type_info {
                    TypeInfo::Model(model) => {
                        model.methods.contains_key(method) || model.method_overloads.contains_key(method)
                    }
                    TypeInfo::Class(class) => {
                        class.methods.contains_key(method) || class.method_overloads.contains_key(method)
                    }
                    TypeInfo::Enum(en) => en.methods.contains_key(method) || en.method_overloads.contains_key(method),
                    TypeInfo::Newtype(newtype) => {
                        let resolved = self.resolve_newtype_method_name(newtype, method);
                        newtype.methods.contains_key(resolved) || newtype.method_overloads.contains_key(resolved)
                    }
                    TypeInfo::Builtin | TypeInfo::TypeAlias => false,
                }
            }
            _ => false,
        }
    }

    /// Return whether a type exposes any method in a candidate operator method set.
    fn type_has_any_operator_method(&mut self, ty: &ResolvedType, methods: &[&str], span: Span) -> bool {
        methods
            .iter()
            .any(|method| self.type_has_operator_method(ty, method, span))
    }

    /// Return whether a type exposes one operator method directly or through an adopted trait.
    fn type_has_operator_method(&mut self, ty: &ResolvedType, method: &str, span: Span) -> bool {
        match ty {
            ResolvedType::Generic(type_name, _) | ResolvedType::Named(type_name) => {
                let Some(type_info) = self.lookup_semantic_type_info(type_name).cloned() else {
                    return false;
                };
                match type_info {
                    TypeInfo::Model(model) => {
                        if model.methods.contains_key(method) || model.method_overloads.contains_key(method) {
                            return true;
                        }
                        let trait_adoptions =
                            self.trait_adoptions_for_type_methods(&model.trait_adoptions, &model.derives);
                        trait_adoptions.iter().any(|adoption| {
                            self.trait_method_info_resolved_for_adoption(adoption, method, span)
                                .is_some()
                        })
                    }
                    TypeInfo::Class(class) => {
                        if class.methods.contains_key(method) || class.method_overloads.contains_key(method) {
                            return true;
                        }
                        let trait_adoptions =
                            self.trait_adoptions_for_type_methods(&class.trait_adoptions, &class.derives);
                        trait_adoptions.iter().any(|adoption| {
                            self.trait_method_info_resolved_for_adoption(adoption, method, span)
                                .is_some()
                        })
                    }
                    TypeInfo::Enum(en) => {
                        if en.methods.contains_key(method) || en.method_overloads.contains_key(method) {
                            return true;
                        }
                        let trait_adoptions = self.trait_adoptions_for_type_methods(&en.trait_adoptions, &en.derives);
                        trait_adoptions.iter().any(|adoption| {
                            self.trait_method_info_resolved_for_adoption(adoption, method, span)
                                .is_some()
                        })
                    }
                    TypeInfo::Newtype(newtype) => {
                        let resolved = self.resolve_newtype_method_name(&newtype, method);
                        if newtype.methods.contains_key(resolved) || newtype.method_overloads.contains_key(resolved) {
                            return true;
                        }
                        newtype.trait_adoptions.iter().any(|adoption| {
                            self.trait_method_info_resolved_for_adoption(adoption, resolved, span)
                                .is_some()
                        })
                    }
                    TypeInfo::Builtin | TypeInfo::TypeAlias => false,
                }
            }
            _ => false,
        }
    }
}

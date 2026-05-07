//! Check unary and binary operators.
//!
//! These helpers validate operator semantics (e.g., numeric ops, boolean ops) and compute the resulting type, emitting
//! diagnostics on mismatches.
//!
//! Numeric semantics follow Python-like rules:
//!
//! - `/` always yields `Float` (even `int / int`)
//! - `%` supports floats with Python remainder semantics
//! - `**` yields `Int` only for non-negative int literal exponents; otherwise `Float`
//! - `+` supports string and list concatenation before numeric fallback
//! - Mixed numeric comparisons are allowed (promote to float for comparison)

use crate::frontend::ast::*;
use crate::frontend::diagnostics::errors;
use crate::frontend::symbols::{ResolvedType, TypeInfo};
use crate::frontend::typechecker::{ProtocolIterationInfo, ResolvedOperatorKind};
use crate::numeric_adapters::{numeric_op_from_ast, numeric_ty_from_resolved, pow_exponent_kind_from_ast};
use incan_core::lang::derives::{self, DeriveId};
use incan_core::{NumericTy, result_numeric_type};

use super::TypeChecker;
use crate::frontend::typechecker::helpers::{collection_type_id, is_str_like};
use incan_core::lang::types::collections::CollectionTypeId;

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
pub(in crate::frontend::typechecker) fn compound_binary_op(op: CompoundOp) -> BinaryOp {
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

/// Return whether a derive set supplies Rust-backed comparison for an operator.
fn derives_support_comparison_operator(derives: &[String], op: BinaryOp) -> bool {
    let has = |id| derives.iter().any(|derive| derive == derives::as_str(id));
    match op {
        BinaryOp::Eq | BinaryOp::NotEq => {
            has(DeriveId::Eq) || has(DeriveId::PartialEq) || has(DeriveId::Ord) || has(DeriveId::PartialOrd)
        }
        BinaryOp::Lt | BinaryOp::Gt | BinaryOp::LtEq | BinaryOp::GtEq => {
            has(DeriveId::Ord) || has(DeriveId::PartialOrd)
        }
        _ => false,
    }
}

impl TypeChecker {
    /// Type-check a binary operation and return its result type.
    pub(in crate::frontend::typechecker::check_expr) fn check_binary(
        &mut self,
        left: &Spanned<Expr>,
        op: BinaryOp,
        right: &Spanned<Expr>,
        span: Span,
    ) -> ResolvedType {
        self.check_binary_with_expected(left, op, right, span, None)
    }

    /// Type-check a binary operation with an optional contextual result type for overload disambiguation.
    pub(in crate::frontend::typechecker::check_expr) fn check_binary_with_expected(
        &mut self,
        left: &Spanned<Expr>,
        op: BinaryOp,
        right: &Spanned<Expr>,
        span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
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
                // The Rust backend will infer and emit the appropriate trait bounds (e.g. `T: Add<Output = T>`).
                // Here we keep the typechecker permissive so generic stdlib helpers can typecheck.
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
                    (Some(left_name), Some(right_name)) if left_name == right_name => return left_ty.clone(),
                    (Some(_), None) if matches!(right_ty, ResolvedType::Unknown) => return left_ty.clone(),
                    (None, Some(_)) if matches!(left_ty, ResolvedType::Unknown) => return right_ty.clone(),
                    _ => {}
                }

                // Check both operands are numeric
                let lhs_num = numeric_ty_from_resolved(&left_ty);
                let rhs_num = numeric_ty_from_resolved(&right_ty);

                match (lhs_num, rhs_num) {
                    (Some(lhs), Some(rhs)) => {
                        let Some(num_op) = numeric_op_from_ast(&op) else {
                            self.errors
                                .push(errors::type_mismatch("numeric operator", &op.to_string(), span));
                            return ResolvedType::Unknown;
                        };
                        let pow_exp = if matches!(op, BinaryOp::Pow) {
                            Some(pow_exponent_kind_from_ast(right, &right_ty))
                        } else {
                            None
                        };
                        let result = result_numeric_type(num_op, lhs, rhs, pow_exp);
                        match result {
                            NumericTy::Int => ResolvedType::Int,
                            NumericTy::Float => ResolvedType::Float,
                        }
                    }
                    // Allow Unknown with numeric partner (treat as that numeric type).
                    (Some(n), None) if matches!(right_ty, ResolvedType::Unknown | ResolvedType::RustPath(_)) => match n
                    {
                        NumericTy::Int => ResolvedType::Int,
                        NumericTy::Float => ResolvedType::Float,
                    },
                    (None, Some(n)) if matches!(left_ty, ResolvedType::Unknown | ResolvedType::RustPath(_)) => {
                        match n {
                            NumericTy::Int => ResolvedType::Int,
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
                // If both are numeric, allow mixed comparisons
                let lhs_num = numeric_ty_from_resolved(&left_ty);
                let rhs_num = numeric_ty_from_resolved(&right_ty);
                if lhs_num.is_some() && rhs_num.is_some() {
                    // Mixed numeric comparison is valid (promotion handled at codegen)
                    ResolvedType::Bool
                } else if (is_str_like)(&left_ty) && (is_str_like)(&right_ty) {
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
                    // Same-type or compatible comparison
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

                // List/Set membership: "<item> in <collection>"
                if let ResolvedType::Generic(name, args) = &right_ty {
                    match collection_type_id(name.as_str()) {
                        Some(CollectionTypeId::List | CollectionTypeId::Set) if !args.is_empty() => {
                            let elem_ty = &args[0];
                            if self.types_compatible(&left_ty, elem_ty) || matches!(left_ty, ResolvedType::Unknown) {
                                return ResolvedType::Bool;
                            }
                            self.errors
                                .push(errors::type_mismatch(&elem_ty.to_string(), &left_ty.to_string(), span));
                            return ResolvedType::Bool;
                        }
                        Some(CollectionTypeId::Dict) if args.len() >= 2 => {
                            let key_ty = &args[0];
                            if self.types_compatible(&left_ty, key_ty) || matches!(left_ty, ResolvedType::Unknown) {
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
    pub(in crate::frontend::typechecker::check_expr) fn check_unary(
        &mut self,
        op: UnaryOp,
        operand: &Spanned<Expr>,
        span: Span,
    ) -> ResolvedType {
        self.check_unary_with_expected(op, operand, span, None)
    }

    /// Type-check a unary operation with an optional contextual result type for overload disambiguation.
    pub(in crate::frontend::typechecker::check_expr) fn check_unary_with_expected(
        &mut self,
        op: UnaryOp,
        operand: &Spanned<Expr>,
        span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        let operand_ty = self.check_expr(operand);
        match op {
            UnaryOp::Neg => {
                if self.types_compatible(&operand_ty, &ResolvedType::Int) {
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
    pub(in crate::frontend::typechecker) fn resolve_index_dunder(
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
    pub(in crate::frontend::typechecker) fn validate_truthiness_condition(
        &mut self,
        cond_ty: &ResolvedType,
        span: Span,
    ) {
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
    pub(in crate::frontend::typechecker) fn resolve_len_dunder(
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
    pub(in crate::frontend::typechecker) fn resolve_contains_dunder(
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
    pub(in crate::frontend::typechecker) fn resolve_call_dunder(
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
    pub(in crate::frontend::typechecker) fn resolve_iteration_protocol(
        &mut self,
        receiver_ty: &ResolvedType,
        span: Span,
    ) -> Option<ResolvedType> {
        let iter_method = "__iter__";
        let next_method = "__next__";
        let args: Vec<CallArg> = Vec::new();
        let arg_types: Vec<ResolvedType> = Vec::new();
        let iterator_ty = match self.resolve_operator_dunder(receiver_ty, iter_method, &args, &arg_types, span, None) {
            Some(iterator_ty) => iterator_ty,
            None => {
                self.errors
                    .push(errors::missing_method(&receiver_ty.to_string(), iter_method, span));
                return None;
            }
        };

        let next_ret = match self.resolve_operator_dunder(&iterator_ty, next_method, &args, &arg_types, span, None) {
            Some(next_ret) => next_ret,
            None => {
                self.errors
                    .push(errors::missing_method(&iterator_ty.to_string(), next_method, span));
                return None;
            }
        };

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
            },
        );
        Some(item_ty)
    }

    /// Resolve a user-defined index assignment (`base[index] = value`) through `__setitem__`, if available.
    pub(in crate::frontend::typechecker) fn resolve_index_set_dunder(
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
    pub(in crate::frontend::typechecker) fn resolve_compound_assignment_operator(
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
    fn resolve_operator_dunder(
        &mut self,
        receiver_ty: &ResolvedType,
        method: &str,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        match receiver_ty {
            ResolvedType::Generic(type_name, _type_args) => {
                let type_info = self.lookup_semantic_type_info(type_name).cloned()?;
                self.resolve_operator_dunder_on_type_info(
                    &type_info,
                    method,
                    args,
                    arg_types,
                    span,
                    receiver_ty,
                    expected_return_ty,
                )
            }
            ResolvedType::Named(type_name) => {
                let type_info = self.lookup_semantic_type_info(type_name).cloned()?;
                self.resolve_operator_dunder_on_type_info(
                    &type_info,
                    method,
                    args,
                    arg_types,
                    span,
                    receiver_ty,
                    expected_return_ty,
                )
            }
            _ if self.is_generic_placeholder_type(receiver_ty) => {
                let placeholder_name = self.generic_placeholder_name(receiver_ty)?.to_string();
                self.resolve_generic_placeholder_method(
                    &placeholder_name,
                    method,
                    &[],
                    args,
                    arg_types,
                    span,
                    receiver_ty,
                    expected_return_ty,
                )
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
    pub(in crate::frontend::typechecker) fn is_user_operator_receiver(&self, ty: &ResolvedType) -> bool {
        match ty {
            ResolvedType::Generic(name, _) | ResolvedType::Named(name) => matches!(
                self.lookup_semantic_type_info(name),
                Some(TypeInfo::Class(_) | TypeInfo::Model(_) | TypeInfo::Enum(_) | TypeInfo::Newtype(_))
            ),
            _ => self.is_generic_placeholder_type(ty),
        }
    }

    /// Return whether a type has Rust-backed comparison derives for this operator.
    fn type_has_derive_backed_comparison_operator(&self, ty: &ResolvedType, op: BinaryOp) -> bool {
        match ty {
            ResolvedType::Generic(type_name, _) | ResolvedType::Named(type_name) => {
                let Some(type_info) = self.lookup_semantic_type_info(type_name) else {
                    return false;
                };
                let derives = match type_info {
                    TypeInfo::Model(model) => model.derives.as_slice(),
                    TypeInfo::Class(class) => class.derives.as_slice(),
                    TypeInfo::Enum(en) => en.derives.as_slice(),
                    TypeInfo::Newtype(_) | TypeInfo::Builtin | TypeInfo::TypeAlias => &[],
                };
                derives_support_comparison_operator(derives, op)
            }
            _ => false,
        }
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

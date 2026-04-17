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
use crate::frontend::symbols::ResolvedType;
use crate::numeric_adapters::{numeric_op_from_ast, numeric_ty_from_resolved, pow_exponent_kind_from_ast};
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

impl TypeChecker {
    /// Type-check a binary operation and return its result type.
    pub(in crate::frontend::typechecker::check_expr) fn check_binary(
        &mut self,
        left: &Spanned<Expr>,
        op: BinaryOp,
        right: &Spanned<Expr>,
        span: Span,
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
                    // Allow Unknown with numeric partner (treat as that numeric type)
                    (Some(n), None) | (None, Some(n)) => match n {
                        NumericTy::Int => ResolvedType::Int,
                        NumericTy::Float => ResolvedType::Float,
                    },
                    _ => {
                        self.errors.push(errors::type_mismatch(
                            "numeric",
                            &format!("{} {} {}", left_ty, op, right_ty),
                            span,
                        ));
                        ResolvedType::Unknown
                    }
                }
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

                // Fallback: keep previous permissive behavior but note mismatch.
                self.errors.push(errors::type_mismatch(
                    "supported membership (str, list, set, dict)",
                    &format!("{} {} {}", left_ty, op, right_ty),
                    span,
                ));
                ResolvedType::Bool
            }
            BinaryOp::Is => ResolvedType::Bool,
        }
    }

    /// Type-check a unary operation and return its result type.
    pub(in crate::frontend::typechecker::check_expr) fn check_unary(
        &mut self,
        op: UnaryOp,
        operand: &Spanned<Expr>,
        span: Span,
    ) -> ResolvedType {
        let operand_ty = self.check_expr(operand);
        match op {
            UnaryOp::Neg => {
                if self.types_compatible(&operand_ty, &ResolvedType::Int) {
                    ResolvedType::Int
                } else if self.types_compatible(&operand_ty, &ResolvedType::Float) {
                    ResolvedType::Float
                } else {
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
        }
    }
}

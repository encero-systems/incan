//! Const-evaluation / const-validation for RFC 008.
//!
//! This module does not compute runtime values; it validates that an initializer is const-evaluable,
//! determines its type, classifies it (Rust-native vs frozen), and detects const dependency cycles.
//!
//! Numeric semantics follow Python-like rules (via `crate::numeric`):
//! - `/` always yields `Float` (even `int / int`)
//! - `%` supports floats with Python remainder semantics
//! - `**` yields `Int` only for non-negative int literal exponents; otherwise `Float`

use crate::frontend::ast::*;
use crate::frontend::diagnostics::{CompileError, errors};
use crate::frontend::symbols::{ResolvedType, SymbolKind, TypeInfo};
use crate::numeric_adapters::{numeric_op_from_ast, numeric_ty_from_resolved, pow_exponent_kind_from_ast};
use incan_core::lang::types::numerics::NumericTypeId;
use incan_core::strings::{self, StringAccessError};
use incan_core::{NumericTy, result_numeric_type};

use super::TypeChecker;
use crate::frontend::typechecker::helpers::{
    freeze_const_type, frozen_bytes_ty, frozen_str_ty, is_frozen_str, is_intlike_for_index, is_str_like,
};

/// Const category used by RFC 008.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstKind {
    /// Can be emitted as a Rust `const` directly.
    RustNative,
    /// Needs frozen stdlib wrappers (deep immutability / baked static data).
    Frozen,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    FrozenStr(String),
    FrozenBytes(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConstEvalResult {
    pub ty: ResolvedType,
    pub kind: ConstKind,
    pub value: Option<ConstValue>,
}

fn const_str(value: &ConstValue) -> Option<&str> {
    match value {
        ConstValue::FrozenStr(s) => Some(s.as_str()),
        _ => None,
    }
}

fn const_int(value: &ConstValue) -> Option<i64> {
    match value {
        ConstValue::Int(i) => Some(*i),
        _ => None,
    }
}

/// Return the inclusive integer range accepted by an exact-width numeric const annotation.
fn const_integer_value_bounds(id: NumericTypeId) -> Option<(i128, i128)> {
    match id {
        NumericTypeId::I8 => Some((i128::from(i8::MIN), i128::from(i8::MAX))),
        NumericTypeId::I16 => Some((i128::from(i16::MIN), i128::from(i16::MAX))),
        NumericTypeId::I32 => Some((i128::from(i32::MIN), i128::from(i32::MAX))),
        NumericTypeId::I64 => Some((i128::from(i64::MIN), i128::from(i64::MAX))),
        NumericTypeId::I128 => Some((i128::MIN, i128::MAX)),
        NumericTypeId::U8 => Some((0, i128::from(u8::MAX))),
        NumericTypeId::U16 => Some((0, i128::from(u16::MAX))),
        NumericTypeId::U32 => Some((0, i128::from(u32::MAX))),
        NumericTypeId::U64 => Some((0, i128::from(u64::MAX))),
        NumericTypeId::U128 => Some((0, i128::MAX)),
        NumericTypeId::ISize => Some((isize::MIN as i128, isize::MAX as i128)),
        NumericTypeId::USize => Some((0, usize::MAX as i128)),
        NumericTypeId::F32 | NumericTypeId::F64 | NumericTypeId::Bool => None,
    }
}

/// Return whether a non-negative integer literal can initialize an exact-width numeric type.
fn unsigned_int_literal_fits_numeric_target(lit: &IntLiteral, id: NumericTypeId) -> bool {
    match id {
        NumericTypeId::I8 => lit.magnitude <= i8::MAX as u128,
        NumericTypeId::I16 => lit.magnitude <= i16::MAX as u128,
        NumericTypeId::I32 => lit.magnitude <= i32::MAX as u128,
        NumericTypeId::I64 => lit.magnitude <= i64::MAX as u128,
        NumericTypeId::I128 => lit.magnitude <= i128::MAX as u128,
        NumericTypeId::U8 => lit.magnitude <= u8::MAX as u128,
        NumericTypeId::U16 => lit.magnitude <= u16::MAX as u128,
        NumericTypeId::U32 => lit.magnitude <= u32::MAX as u128,
        NumericTypeId::U64 => lit.magnitude <= u64::MAX as u128,
        NumericTypeId::U128 => true,
        NumericTypeId::ISize => lit.magnitude <= isize::MAX as u128,
        NumericTypeId::USize => lit.magnitude <= usize::MAX as u128,
        NumericTypeId::F32 | NumericTypeId::F64 | NumericTypeId::Bool => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstEvalState {
    NotStarted,
    InProgress,
    Done,
}

impl TypeChecker {
    /// Convert a user-written type annotation in a `const` declaration to its frozen form.
    ///
    /// This makes `const X: List[T] = [...]` behave as `const X: FrozenList[T] = [...]`, ensuring
    /// the resulting constant has a deeply immutable type (no mutating APIs).
    fn freeze_const_annotation(&self, ty: ResolvedType) -> ResolvedType {
        freeze_const_type(ty)
    }

    /// Evaluate, type-check, classify, and publish one `const` declaration for later compiler stages.
    pub(crate) fn check_and_resolve_const(&mut self, konst: &ConstDecl, decl_span: Span) {
        // Evaluate (with cycle detection) and update the symbol table entry.
        let mut stack = Vec::new();
        let Some(mut result) = self.eval_const_by_name(&konst.name, &mut stack) else {
            return;
        };

        // Publish classification for downstream stages.
        self.type_info
            .consts
            .const_kinds
            .insert(konst.name.clone(), result.kind);
        if let Some(val) = result.value.clone() {
            self.type_info.consts.const_values.insert(konst.name.clone(), val);
        }

        // If an annotation exists, require compatibility.
        if let Some(ann) = &konst.ty {
            let resolved = self.resolve_type_checked(ann);
            let expected = self.freeze_const_annotation(resolved);
            if self.types_compatible(&result.ty, &expected) {
                result.ty = expected;
            } else {
                match self.const_int_value_checked_against_numeric_expected(&result, &expected, konst.value.span) {
                    Some(true) => result.ty = expected,
                    Some(false) => {}
                    None => {
                        self.errors.push(errors::type_mismatch(
                            &expected.to_string(),
                            &result.ty.to_string(),
                            konst.value.span,
                        ));
                    }
                }
            }
        } else if matches!(result.ty, ResolvedType::Unknown) {
            self.errors
                .push(errors::const_missing_type_annotation(&konst.name, decl_span));
        }

        // Record the root initializer type so lowering/codegen can use it.
        self.record_expr_type(konst.value.span, result.ty.clone());
        self.const_eval_cache.insert(konst.name.clone(), result.clone());

        // Update the symbol table type (so later expressions see the refined type).
        if let Some(var_info) = self.lookup_local_variable_info_mut(&konst.name) {
            var_info.ty = result.ty.clone();
        }
    }

    /// Validate a const integer value against an exact-width numeric annotation without widening runtime integers.
    fn const_int_value_checked_against_numeric_expected(
        &mut self,
        result: &ConstEvalResult,
        expected: &ResolvedType,
        span: Span,
    ) -> Option<bool> {
        let target = super::numeric_type_id_for_compat(expected)?;
        let value = result.value.as_ref().and_then(const_int)?;
        let (min, max) = const_integer_value_bounds(target)?;
        if i128::from(value) < min || i128::from(value) > max {
            self.errors.push(CompileError::type_error(
                format!("Integer literal {value} does not fit in {expected}; valid range is {min}..={max}"),
                span,
            ));
            return Some(false);
        }
        Some(true)
    }

    fn eval_const_by_name(&mut self, name: &str, stack: &mut Vec<String>) -> Option<ConstEvalResult> {
        if let Some(res) = self.const_eval_cache.get(name).cloned() {
            return Some(res);
        }

        let state = self
            .const_eval_state
            .get(name)
            .copied()
            .unwrap_or(ConstEvalState::NotStarted);
        match state {
            ConstEvalState::Done => return self.const_eval_cache.get(name).cloned(),
            ConstEvalState::InProgress => {
                // Cycle: stack + name
                let mut cycle = stack.clone();
                cycle.push(name.to_string());
                let cycle_str = cycle.join(" -> ");
                let span = self.const_decls.get(name).map(|(_, s)| *s).unwrap_or_default();
                self.errors.push(errors::const_dependency_cycle(&cycle_str, span));
                return None;
            }
            ConstEvalState::NotStarted => {}
        }

        let Some((decl, decl_span)) = self.const_decls.get(name).cloned() else {
            self.errors.push(errors::unknown_symbol(name, Span::default()));
            return None;
        };

        self.const_eval_state
            .insert(name.to_string(), ConstEvalState::InProgress);
        stack.push(name.to_string());

        let expected = decl.ty.as_ref().map(|t| self.resolve_type_checked(t));
        let expected = expected.map(|t| self.freeze_const_annotation(t));
        let result = self.eval_const_expr(&decl.value, expected.as_ref(), stack, decl_span);

        stack.pop();
        self.const_eval_state.insert(name.to_string(), ConstEvalState::Done);

        if let Some(res) = &result {
            self.const_eval_cache.insert(name.to_string(), res.clone());
        }

        result
    }

    /// Evaluate a const expression into the limited compile-time value domain used by declarations and attributes.
    fn eval_const_expr(
        &mut self,
        expr: &Spanned<Expr>,
        expected: Option<&ResolvedType>,
        stack: &mut Vec<String>,
        decl_span: Span,
    ) -> Option<ConstEvalResult> {
        match &expr.node {
            Expr::Literal(lit) => Some(self.eval_const_literal(lit, expected, expr.span, decl_span)),
            Expr::Ident(name) => {
                // Other Incan consts are resolved by name.
                if self.const_decls.contains_key(name) {
                    return self.eval_const_by_name(name, stack);
                }
                // Rust imports (e.g. `from rust::std::f64::consts import PI`) are valid const references — Rust can
                // evaluate them at compile time. We treat them as opaque `RustNative` values with no known Incan value;
                // the type is inferred from the enclosing const annotation when available, otherwise left `Unknown`.
                // Any actual type mismatch is caught by Rust's compiler.
                if let Some(sym) = self.lookup_symbol(name)
                    && match &sym.kind {
                        SymbolKind::RustItem(_) => true,
                        SymbolKind::Module(info) => info.path.first().is_some_and(|seg| seg == "rust"),
                        _ => false,
                    }
                {
                    let ty = expected.cloned().unwrap_or(ResolvedType::Unknown);
                    return Some(ConstEvalResult {
                        ty,
                        kind: ConstKind::RustNative,
                        value: None,
                    });
                }
                self.errors.push(errors::const_non_const_name(name, expr.span));
                None
            }
            Expr::Tuple(items) => {
                let mut tys = Vec::with_capacity(items.len());
                let mut kind = ConstKind::RustNative;
                for item in items {
                    let r = self.eval_const_expr(item, None, stack, decl_span)?;
                    tys.push(r.ty);
                    if r.kind == ConstKind::Frozen {
                        kind = ConstKind::Frozen;
                    }
                }
                Some(ConstEvalResult {
                    ty: ResolvedType::Tuple(tys),
                    kind,
                    value: None,
                })
            }
            Expr::Unary(op, inner) => {
                let r = self.eval_const_expr(inner, None, stack, decl_span)?;
                match op {
                    UnaryOp::Neg => {
                        if matches!(r.ty, ResolvedType::Int | ResolvedType::Float) {
                            let value = match r.value.as_ref() {
                                Some(ConstValue::Int(n)) => Some(ConstValue::Int(-n)),
                                Some(ConstValue::Float(f)) => Some(ConstValue::Float(-f)),
                                _ => None,
                            };
                            Some(ConstEvalResult {
                                ty: r.ty,
                                kind: r.kind,
                                value,
                            })
                        } else {
                            self.errors
                                .push(errors::const_unary_op_not_supported("-", &r.ty.to_string(), expr.span));
                            None
                        }
                    }
                    UnaryOp::Not => {
                        if matches!(r.ty, ResolvedType::Bool) {
                            let value = match r.value.as_ref() {
                                Some(ConstValue::Bool(b)) => Some(ConstValue::Bool(!b)),
                                _ => None,
                            };
                            Some(ConstEvalResult {
                                ty: ResolvedType::Bool,
                                kind: r.kind,
                                value,
                            })
                        } else {
                            self.errors.push(errors::const_unary_op_not_supported(
                                "not",
                                &r.ty.to_string(),
                                expr.span,
                            ));
                            None
                        }
                    }
                    UnaryOp::Invert => {
                        if matches!(r.ty, ResolvedType::Int) {
                            let value = match r.value.as_ref() {
                                Some(ConstValue::Int(n)) => Some(ConstValue::Int(!n)),
                                _ => None,
                            };
                            Some(ConstEvalResult {
                                ty: ResolvedType::Int,
                                kind: r.kind,
                                value,
                            })
                        } else {
                            self.errors
                                .push(errors::const_unary_op_not_supported("~", &r.ty.to_string(), expr.span));
                            None
                        }
                    }
                }
            }
            Expr::Binary(left, op, right) => {
                let mut l = self.eval_const_expr(left, None, stack, decl_span)?;
                let mut r = self.eval_const_expr(right, None, stack, decl_span)?;

                // When one operand is a Rust-imported constant whose type couldn't be determined (Unknown), coerce it
                // to the other operand's numeric type. Rust's own compiler will catch real type mismatches later.
                if l.kind == ConstKind::RustNative
                    && l.ty == ResolvedType::Unknown
                    && numeric_ty_from_resolved(&r.ty).is_some()
                {
                    l.ty = r.ty.clone();
                } else if r.kind == ConstKind::RustNative
                    && r.ty == ResolvedType::Unknown
                    && numeric_ty_from_resolved(&l.ty).is_some()
                {
                    r.ty = l.ty.clone();
                }

                // String concatenation (str + str)
                if matches!(op, BinaryOp::Add) && is_str_like(&l.ty) && is_str_like(&r.ty) {
                    let value = match (
                        l.value.as_ref().and_then(const_str),
                        r.value.as_ref().and_then(const_str),
                    ) {
                        (Some(lhs), Some(rhs)) => Some(ConstValue::FrozenStr(strings::str_concat(lhs, rhs))),
                        _ => None,
                    };
                    return Some(ConstEvalResult {
                        ty: frozen_str_ty(),
                        kind: ConstKind::Frozen,
                        value,
                    });
                }

                // String comparisons
                if matches!(
                    op,
                    BinaryOp::Eq | BinaryOp::NotEq | BinaryOp::Lt | BinaryOp::Gt | BinaryOp::LtEq | BinaryOp::GtEq
                ) && is_str_like(&l.ty)
                    && is_str_like(&r.ty)
                {
                    return Some(ConstEvalResult {
                        ty: ResolvedType::Bool,
                        kind: ConstKind::RustNative,
                        value: None,
                    });
                }

                // String membership
                if matches!(op, BinaryOp::In | BinaryOp::NotIn) && is_str_like(&l.ty) && is_str_like(&r.ty) {
                    let value = match (
                        l.value.as_ref().and_then(const_str),
                        r.value.as_ref().and_then(const_str),
                    ) {
                        (Some(needle), Some(haystack)) => {
                            let contains = strings::str_contains(haystack, needle);
                            let result = if matches!(op, BinaryOp::NotIn) {
                                !contains
                            } else {
                                contains
                            };
                            Some(ConstValue::Bool(result))
                        }
                        _ => None,
                    };
                    return Some(ConstEvalResult {
                        ty: ResolvedType::Bool,
                        kind: ConstKind::RustNative,
                        value,
                    });
                }

                let (result_ty, result_kind, value) = match op {
                    // Numeric ops (Python-like semantics via numeric policy)
                    BinaryOp::Add
                    | BinaryOp::Sub
                    | BinaryOp::Mul
                    | BinaryOp::Div
                    | BinaryOp::FloorDiv
                    | BinaryOp::Mod
                    | BinaryOp::Pow => {
                        // Convert to NumericTy
                        let lhs_num = numeric_ty_from_resolved(&l.ty);
                        let rhs_num = numeric_ty_from_resolved(&r.ty);

                        match (lhs_num, rhs_num) {
                            (Some(lhs), Some(rhs)) => {
                                let Some(num_op) = numeric_op_from_ast(op) else {
                                    self.errors.push(errors::const_binary_op_not_supported(
                                        &op.to_string(),
                                        &l.ty.to_string(),
                                        &r.ty.to_string(),
                                        expr.span,
                                    ));
                                    return None;
                                };
                                let pow_exp = if matches!(op, BinaryOp::Pow) {
                                    Some(pow_exponent_kind_from_ast(right, &r.ty))
                                } else {
                                    None
                                };
                                let result = result_numeric_type(num_op, lhs, rhs, pow_exp);
                                let ty = match result {
                                    NumericTy::Int => ResolvedType::Int,
                                    NumericTy::Float => ResolvedType::Float,
                                };
                                (ty, ConstKind::RustNative, None)
                            }
                            _ => {
                                self.errors.push(errors::const_binary_op_not_supported(
                                    &op.to_string(),
                                    &l.ty.to_string(),
                                    &r.ty.to_string(),
                                    expr.span,
                                ));
                                return None;
                            }
                        }
                    }
                    BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor | BinaryOp::Shl | BinaryOp::Shr => {
                        if !matches!((&l.ty, &r.ty), (ResolvedType::Int, ResolvedType::Int)) {
                            self.errors.push(errors::const_binary_op_not_supported(
                                &op.to_string(),
                                &l.ty.to_string(),
                                &r.ty.to_string(),
                                expr.span,
                            ));
                            return None;
                        }
                        let value = match (l.value.as_ref(), r.value.as_ref(), op) {
                            (Some(ConstValue::Int(lhs)), Some(ConstValue::Int(rhs)), BinaryOp::BitAnd) => {
                                Some(ConstValue::Int(lhs & rhs))
                            }
                            (Some(ConstValue::Int(lhs)), Some(ConstValue::Int(rhs)), BinaryOp::BitOr) => {
                                Some(ConstValue::Int(lhs | rhs))
                            }
                            (Some(ConstValue::Int(lhs)), Some(ConstValue::Int(rhs)), BinaryOp::BitXor) => {
                                Some(ConstValue::Int(lhs ^ rhs))
                            }
                            (Some(ConstValue::Int(lhs)), Some(ConstValue::Int(rhs)), BinaryOp::Shl) if *rhs >= 0 => {
                                u32::try_from(*rhs)
                                    .ok()
                                    .and_then(|rhs| lhs.checked_shl(rhs))
                                    .map(ConstValue::Int)
                            }
                            (Some(ConstValue::Int(lhs)), Some(ConstValue::Int(rhs)), BinaryOp::Shr) if *rhs >= 0 => {
                                u32::try_from(*rhs)
                                    .ok()
                                    .and_then(|rhs| lhs.checked_shr(rhs))
                                    .map(ConstValue::Int)
                            }
                            _ => None,
                        };
                        (ResolvedType::Int, ConstKind::RustNative, value)
                    }
                    // Comparisons always yield bool (mixed numeric allowed)
                    BinaryOp::Eq | BinaryOp::NotEq | BinaryOp::Lt | BinaryOp::Gt | BinaryOp::LtEq | BinaryOp::GtEq => {
                        // Validate operands are comparable (same type or both numeric)
                        let lhs_num = numeric_ty_from_resolved(&l.ty);
                        let rhs_num = numeric_ty_from_resolved(&r.ty);
                        if lhs_num.is_some() && rhs_num.is_some() {
                            // Mixed numeric comparison is valid
                            (ResolvedType::Bool, ConstKind::RustNative, None)
                        } else if self.types_compatible(&l.ty, &r.ty) {
                            (ResolvedType::Bool, ConstKind::RustNative, None)
                        } else {
                            self.errors.push(errors::const_compare_incompatible(
                                &l.ty.to_string(),
                                &r.ty.to_string(),
                                expr.span,
                            ));
                            return None;
                        }
                    }
                    BinaryOp::And | BinaryOp::Or => {
                        if matches!(l.ty, ResolvedType::Bool) && matches!(r.ty, ResolvedType::Bool) {
                            let value = match (l.value.as_ref(), r.value.as_ref()) {
                                (Some(ConstValue::Bool(lb)), Some(ConstValue::Bool(rb))) => {
                                    let res = if matches!(op, BinaryOp::And) {
                                        *lb && *rb
                                    } else {
                                        *lb || *rb
                                    };
                                    Some(ConstValue::Bool(res))
                                }
                                _ => None,
                            };
                            (ResolvedType::Bool, ConstKind::RustNative, value)
                        } else {
                            self.errors.push(errors::const_logical_op_requires_bool(
                                &op.to_string(),
                                &l.ty.to_string(),
                                &r.ty.to_string(),
                                expr.span,
                            ));
                            return None;
                        }
                    }
                    BinaryOp::MatMul
                    | BinaryOp::PipeForward
                    | BinaryOp::PipeBackward
                    | BinaryOp::In
                    | BinaryOp::NotIn
                    | BinaryOp::Is
                    | BinaryOp::IsNot => {
                        self.errors
                            .push(errors::const_operator_not_allowed(&op.to_string(), expr.span));
                        return None;
                    }
                };

                Some(ConstEvalResult {
                    ty: result_ty,
                    kind: result_kind,
                    value,
                })
            }
            Expr::List(items) => {
                if items.iter().any(|item| matches!(item, ListEntry::Spread(_))) {
                    self.errors.push(errors::const_expression_not_allowed(expr.span));
                    return None;
                }

                let elem_expected = expected.and_then(|t| match t {
                    ResolvedType::FrozenList(elem) => Some(elem.as_ref()),
                    ResolvedType::Generic(name, args)
                        if crate::frontend::typechecker::helpers::collection_type_id(name.as_str())
                            == Some(incan_core::lang::types::collections::CollectionTypeId::FrozenList)
                            && !args.is_empty() =>
                    {
                        Some(&args[0])
                    }
                    _ => None,
                });

                let elem_ty = if items.is_empty() {
                    elem_expected.cloned().unwrap_or(ResolvedType::Unknown)
                } else {
                    let ListEntry::Element(ast_first) = &items[0] else {
                        unreachable!("list spreads were rejected before const list evaluation");
                    };
                    let first = self.eval_const_expr(ast_first, elem_expected, stack, decl_span)?;
                    // Evaluate the rest just for validation.
                    for it in items.iter().skip(1) {
                        let ListEntry::Element(value) = it else {
                            unreachable!("list spreads were rejected before const list evaluation");
                        };
                        self.eval_const_expr(value, elem_expected, stack, decl_span)?;
                    }
                    first.ty
                };

                if items.is_empty() && matches!(elem_ty, ResolvedType::Unknown) {
                    self.errors.push(errors::const_empty_list_type_inference(expr.span));
                }

                Some(ConstEvalResult {
                    ty: ResolvedType::FrozenList(Box::new(elem_ty)),
                    kind: ConstKind::Frozen,
                    value: None,
                })
            }
            Expr::Set(items) => {
                let elem_expected = expected.and_then(|t| match t {
                    ResolvedType::FrozenSet(elem) => Some(elem.as_ref()),
                    ResolvedType::Generic(name, args)
                        if crate::frontend::typechecker::helpers::collection_type_id(name.as_str())
                            == Some(incan_core::lang::types::collections::CollectionTypeId::FrozenSet)
                            && !args.is_empty() =>
                    {
                        Some(&args[0])
                    }
                    _ => None,
                });

                let elem_ty = if items.is_empty() {
                    elem_expected.cloned().unwrap_or(ResolvedType::Unknown)
                } else {
                    let first = self.eval_const_expr(&items[0], elem_expected, stack, decl_span)?;
                    for it in items.iter().skip(1) {
                        self.eval_const_expr(it, elem_expected, stack, decl_span)?;
                    }
                    first.ty
                };

                if items.is_empty() && matches!(elem_ty, ResolvedType::Unknown) {
                    self.errors.push(errors::const_empty_set_type_inference(expr.span));
                }

                Some(ConstEvalResult {
                    ty: ResolvedType::FrozenSet(Box::new(elem_ty)),
                    kind: ConstKind::Frozen,
                    value: None,
                })
            }
            Expr::Dict(pairs) => {
                let (k_expected, v_expected) = match expected {
                    Some(ResolvedType::FrozenDict(k, v)) => (Some(k.as_ref()), Some(v.as_ref())),
                    Some(ResolvedType::Generic(name, args))
                        if crate::frontend::typechecker::helpers::collection_type_id(name.as_str())
                            == Some(incan_core::lang::types::collections::CollectionTypeId::FrozenDict)
                            && args.len() >= 2 =>
                    {
                        (Some(&args[0]), Some(&args[1]))
                    }
                    _ => (None, None),
                };

                let (key_ty, val_ty) = if pairs.is_empty() {
                    (
                        k_expected.cloned().unwrap_or(ResolvedType::Unknown),
                        v_expected.cloned().unwrap_or(ResolvedType::Unknown),
                    )
                } else {
                    let DictEntry::Pair(k0, v0) = &pairs[0] else {
                        self.errors.push(errors::const_expression_not_allowed(expr.span));
                        return None;
                    };
                    let kk = self.eval_const_expr(k0, k_expected, stack, decl_span)?;
                    let vv = self.eval_const_expr(v0, v_expected, stack, decl_span)?;
                    for entry in pairs.iter().skip(1) {
                        let DictEntry::Pair(k, v) = entry else {
                            self.errors.push(errors::const_expression_not_allowed(expr.span));
                            return None;
                        };
                        self.eval_const_expr(k, k_expected, stack, decl_span)?;
                        self.eval_const_expr(v, v_expected, stack, decl_span)?;
                    }
                    (kk.ty, vv.ty)
                };

                if pairs.is_empty()
                    && (matches!(key_ty, ResolvedType::Unknown) || matches!(val_ty, ResolvedType::Unknown))
                {
                    self.errors.push(errors::const_empty_dict_type_inference(expr.span));
                }

                Some(ConstEvalResult {
                    ty: ResolvedType::FrozenDict(Box::new(key_ty), Box::new(val_ty)),
                    kind: ConstKind::Frozen,
                    value: None,
                })
            }

            Expr::Index(base, idx) => {
                let b = self.eval_const_expr(base, None, stack, decl_span)?;
                let i = self.eval_const_expr(idx, None, stack, decl_span)?;
                if !is_frozen_str(&b.ty) && !matches!(b.ty, ResolvedType::Str) {
                    self.errors.push(errors::const_indexing_requires_string(expr.span));
                    return None;
                }
                if !is_intlike_for_index(&i.ty) {
                    self.errors
                        .push(errors::const_string_index_requires_int(&i.ty.to_string(), idx.span));
                    return None;
                }
                let mut value = None;
                if let (Some(base_str), Some(idx_val)) = (
                    b.value.as_ref().and_then(const_str),
                    i.value.as_ref().and_then(const_int),
                ) {
                    match strings::str_char_at(base_str, idx_val) {
                        Ok(ch) => value = Some(ConstValue::FrozenStr(ch)),
                        Err(StringAccessError::IndexOutOfRange) => {
                            self.errors.push(errors::const_string_index_out_of_range(expr.span));
                            return None;
                        }
                        Err(StringAccessError::SliceStepZero) => unreachable!("step zero is not used for index"),
                    }
                }
                Some(ConstEvalResult {
                    ty: frozen_str_ty(),
                    kind: ConstKind::Frozen,
                    value,
                })
            }

            Expr::Slice(base, slice) => {
                let b = self.eval_const_expr(base, None, stack, decl_span)?;
                if !is_frozen_str(&b.ty) && !matches!(b.ty, ResolvedType::Str) {
                    self.errors.push(errors::const_slicing_requires_string(base.span));
                    return None;
                }

                let mut start_val = None;
                if let Some(s) = &slice.start {
                    let ty = self.eval_const_expr(s, None, stack, decl_span)?;
                    if !is_intlike_for_index(&ty.ty) {
                        self.errors.push(errors::const_slice_component_requires_int(
                            "start",
                            &ty.ty.to_string(),
                            s.span,
                        ));
                        return None;
                    }
                    start_val = ty.value.as_ref().and_then(const_int);
                }
                let mut end_val = None;
                if let Some(e) = &slice.end {
                    let ty = self.eval_const_expr(e, None, stack, decl_span)?;
                    if !is_intlike_for_index(&ty.ty) {
                        self.errors.push(errors::const_slice_component_requires_int(
                            "end",
                            &ty.ty.to_string(),
                            e.span,
                        ));
                        return None;
                    }
                    end_val = ty.value.as_ref().and_then(const_int);
                }
                let mut step_val = None;
                if let Some(st) = &slice.step {
                    let ty = self.eval_const_expr(st, None, stack, decl_span)?;
                    if !is_intlike_for_index(&ty.ty) {
                        self.errors.push(errors::const_slice_component_requires_int(
                            "step",
                            &ty.ty.to_string(),
                            st.span,
                        ));
                        return None;
                    }
                    step_val = ty.value.as_ref().and_then(const_int);
                }

                let mut value = None;
                if let Some(base_str) = b.value.as_ref().and_then(const_str) {
                    match strings::str_slice(base_str, start_val, end_val, step_val) {
                        Ok(out) => value = Some(ConstValue::FrozenStr(out)),
                        Err(StringAccessError::SliceStepZero) => {
                            let span = slice.step.as_ref().map(|s| s.span).unwrap_or(expr.span);
                            self.errors.push(errors::const_slice_step_zero(span));
                            return None;
                        }
                        Err(StringAccessError::IndexOutOfRange) => {
                            // Should not normally occur due to clamping but keep in sync with semantics.
                            self.errors.push(errors::const_string_index_out_of_range(expr.span));
                            return None;
                        }
                    }
                }

                Some(ConstEvalResult {
                    ty: frozen_str_ty(),
                    kind: ConstKind::Frozen,
                    value,
                })
            }

            Expr::Call(callee, type_args, args) if type_args.is_empty() => {
                if let Expr::Ident(callee_name) = &callee.node
                    && self.is_const_model_constructor_name(callee_name)
                {
                    return self.eval_const_model_constructor(callee_name, args, expected, stack, decl_span, expr.span);
                }

                let Some(ResolvedType::Named(expected_name)) = expected else {
                    self.errors.push(errors::const_expression_not_allowed(expr.span));
                    return None;
                };
                let Expr::Ident(callee_name) = &callee.node else {
                    self.errors.push(errors::const_expression_not_allowed(expr.span));
                    return None;
                };
                let Some(TypeInfo::Newtype(newtype)) = self.lookup_type_info(callee_name).cloned() else {
                    self.errors.push(errors::const_expression_not_allowed(expr.span));
                    return None;
                };
                if callee_name != expected_name {
                    self.errors.push(errors::const_expression_not_allowed(expr.span));
                    return None;
                }
                let [CallArg::Positional(arg)] = args.as_slice() else {
                    self.errors.push(errors::const_expression_not_allowed(expr.span));
                    return None;
                };
                let Some(target) = super::numeric_type_id_for_compat(&newtype.underlying) else {
                    self.errors.push(errors::const_expression_not_allowed(expr.span));
                    return None;
                };
                let Expr::Literal(Literal::Int(lit)) = &arg.node else {
                    self.errors.push(errors::const_expression_not_allowed(expr.span));
                    return None;
                };
                if !unsigned_int_literal_fits_numeric_target(lit, target) {
                    self.errors.push(CompileError::type_error(
                        format!("Integer literal {} does not fit in {}", lit.repr, newtype.underlying),
                        arg.span,
                    ));
                    return None;
                }
                Some(ConstEvalResult {
                    ty: ResolvedType::Named(expected_name.clone()),
                    kind: ConstKind::RustNative,
                    value: None,
                })
            }
            Expr::Constructor(name, args) if self.is_const_model_constructor_name(name) => {
                self.eval_const_model_constructor(name, args, expected, stack, decl_span, expr.span)
            }

            // Disallowed constructs for RFC 008 phase 1.
            Expr::Call(_, _, _)
            | Expr::MethodCall(_, _, _, _)
            | Expr::Partial(_)
            | Expr::Generator(_)
            | Expr::ListComp(_)
            | Expr::DictComp(_)
            | Expr::Match(_, _)
            | Expr::If(_)
            | Expr::Loop(_)
            | Expr::Closure(_, _)
            | Expr::Yield(_)
            | Expr::Range { .. }
            | Expr::Field(_, _)
            | Expr::Surface(_)
            | Expr::Try(_)
            | Expr::Paren(_)
            | Expr::Constructor(_, _)
            | Expr::FString(_) => {
                self.errors.push(errors::const_expression_not_allowed(expr.span));
                None
            }
            Expr::SelfExpr => {
                self.errors.push(errors::const_self_not_allowed(expr.span));
                None
            }
        }
    }

    /// Return whether a name resolves to a model constructor that can be considered for const literal validation.
    fn is_const_model_constructor_name(&self, name: &str) -> bool {
        self.lookup_type_info(name)
            .is_some_and(|info| matches!(info, TypeInfo::Model(_)))
    }

    /// Evaluate a model constructor in a const initializer.
    ///
    /// This keeps `const` model literals declaration-safe: every provided field must itself be const-evaluable, all
    /// required fields must be explicit, and omitted defaults are rejected because model defaults are ordinary runtime
    /// expressions rather than const metadata.
    fn eval_const_model_constructor(
        &mut self,
        type_name: &str,
        args: &[CallArg],
        expected: Option<&ResolvedType>,
        stack: &mut Vec<String>,
        decl_span: Span,
        call_span: Span,
    ) -> Option<ConstEvalResult> {
        if let Some(expected_ty) = expected
            && !matches!(expected_ty, ResolvedType::Named(name) if name == type_name)
            && !matches!(expected_ty, ResolvedType::Unknown)
        {
            return Some(ConstEvalResult {
                ty: ResolvedType::Named(type_name.to_string()),
                kind: ConstKind::RustNative,
                value: None,
            });
        }

        let Some(TypeInfo::Model(model)) = self.lookup_type_info(type_name).cloned() else {
            self.errors.push(errors::const_expression_not_allowed(call_span));
            return None;
        };

        let mut provided = std::collections::HashSet::new();
        let mut result_kind = ConstKind::RustNative;
        let mut had_error = false;

        for arg in args {
            let CallArg::Named(field_name, value) = arg else {
                self.errors
                    .push(errors::positional_constructor_args_not_supported(type_name, call_span));
                had_error = true;
                continue;
            };

            let Some((canonical_name, field_info)) = Self::resolve_const_model_field(&model.fields, field_name) else {
                self.eval_const_expr(value, None, stack, decl_span);
                self.errors
                    .push(errors::missing_field(type_name, field_name, value.span));
                had_error = true;
                continue;
            };

            if !provided.insert(canonical_name.clone()) {
                self.errors.push(errors::duplicate_field_in_call(
                    type_name,
                    canonical_name.as_str(),
                    value.span,
                ));
                had_error = true;
                continue;
            }

            let Some(field_result) = self.eval_const_expr(value, Some(&field_info.ty), stack, decl_span) else {
                had_error = true;
                continue;
            };
            if field_result.kind == ConstKind::Frozen {
                result_kind = ConstKind::Frozen;
            }

            if field_result.ty != field_info.ty {
                match self.const_int_value_checked_against_numeric_expected(&field_result, &field_info.ty, value.span) {
                    Some(true) => {}
                    Some(false) => had_error = true,
                    None => {
                        self.errors.push(errors::field_type_mismatch(
                            field_name,
                            &field_info.ty.to_string(),
                            &field_result.ty.to_string(),
                            value.span,
                        ));
                        had_error = true;
                    }
                }
            }
        }

        for (field_name, field_info) in &model.fields {
            if provided.contains(field_name) {
                continue;
            }
            if field_info.has_default {
                self.errors.push(CompileError::type_error(
                    format!(
                        "Const model constructor '{}' must provide field '{}' explicitly; model defaults are not evaluated in const initializers",
                        type_name, field_name
                    ),
                    call_span,
                ));
            } else {
                self.errors.push(errors::missing_required_constructor_field(
                    type_name, field_name, call_span,
                ));
            }
            had_error = true;
        }

        if had_error {
            return None;
        }

        Some(ConstEvalResult {
            ty: ResolvedType::Named(type_name.to_string()),
            kind: result_kind,
            value: None,
        })
    }

    /// Resolve a model constructor field by canonical source name or model alias.
    fn resolve_const_model_field<'a>(
        fields: &'a std::collections::HashMap<String, crate::frontend::symbols::FieldInfo>,
        field_name: &str,
    ) -> Option<(String, &'a crate::frontend::symbols::FieldInfo)> {
        fields
            .get(field_name)
            .map(|info| (field_name.to_string(), info))
            .or_else(|| {
                fields
                    .iter()
                    .find(|(_, info)| info.alias.as_deref() == Some(field_name))
                    .map(|(name, info)| (name.clone(), info))
            })
    }

    /// Evaluate a literal in a const context, optionally checking it against an expected type.
    fn eval_const_literal(
        &mut self,
        lit: &Literal,
        expected: Option<&ResolvedType>,
        span: Span,
        _decl_span: Span,
    ) -> ConstEvalResult {
        match lit {
            Literal::Int(il) => ConstEvalResult {
                ty: ResolvedType::Int,
                kind: ConstKind::RustNative,
                value: Some(ConstValue::Int(il.value)),
            },
            Literal::Float(f) => ConstEvalResult {
                ty: ResolvedType::Float,
                kind: ConstKind::RustNative,
                value: Some(ConstValue::Float(f.value)),
            },
            Literal::Decimal(_) => ConstEvalResult {
                ty: ResolvedType::Unknown,
                kind: ConstKind::RustNative,
                value: None,
            },
            Literal::Bool(b) => ConstEvalResult {
                ty: ResolvedType::Bool,
                kind: ConstKind::RustNative,
                value: Some(ConstValue::Bool(*b)),
            },
            Literal::String(s) => ConstEvalResult {
                ty: frozen_str_ty(),
                kind: ConstKind::Frozen,
                value: Some(ConstValue::FrozenStr(s.clone())),
            },
            Literal::Bytes(b) => ConstEvalResult {
                ty: frozen_bytes_ty(),
                kind: ConstKind::Frozen,
                value: Some(ConstValue::FrozenBytes(b.clone())),
            },
            Literal::None => {
                // None is ambiguous without annotation.
                let ty = expected.cloned().unwrap_or(ResolvedType::Unknown);
                if matches!(ty, ResolvedType::Unknown) {
                    self.errors.push(errors::const_none_type_inference(span));
                }
                ConstEvalResult {
                    ty,
                    kind: ConstKind::RustNative,
                    value: None,
                }
            }
        }
    }
}

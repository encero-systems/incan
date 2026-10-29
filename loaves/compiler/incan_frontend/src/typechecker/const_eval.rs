//! Const-evaluation / const-validation for RFC 008.
//!
//! This module does not compute runtime values; it validates that an initializer is const-evaluable, determines its
//! type, classifies it (Rust-native vs frozen), and detects const dependency cycles.
//!
//! Numeric semantics follow Python-like rules (via `crate::numeric`):
//! - `/` always yields `Float` (even `int / int`)
//! - `%` supports floats with Python remainder semantics
//! - `**` yields `Int` only for non-negative int literal exponents; otherwise `Float`

use crate::ast::*;
use crate::diagnostics::{CompileError, errors};
use crate::numeric_adapters::{numeric_op_from_ast, numeric_ty_from_resolved, pow_exponent_kind_from_ast};
use crate::partial_projection::{PartialPresetRef, merge_named_partial_args};
use crate::symbols::{ResolvedType, SymbolKind, TypeInfo};
use incan_lang::lang::types::numerics::NumericTypeId;
use incan_lang::numeric_values::{IntegerBounds, integer_bounds};
use incan_lang::strings::{self, StringAccessError};
use incan_lang::{NumericTy, result_numeric_type};

use super::{PartialProjectionTargetKind, TypeChecker};
use crate::typechecker::helpers::{
    freeze_const_type, frozen_bytes_ty, frozen_str_ty, is_frozen_str, is_intlike_for_index,
    is_mutable_collection_const_annotation, is_str_like,
};

/// Const category used by RFC 008.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstKind {
    /// Can be emitted as a Rust `const` directly.
    RustNative,
    /// Needs frozen stdlib wrappers (deep immutability / baked static data).
    Frozen,
}

/// The limited values known during const validation; absence leaves evaluation to native compilation.
#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    /// A known signed integer.
    Int(i64),
    /// A known binary floating-point value.
    Float(f64),
    /// A known Boolean value.
    Bool(bool),
    /// Known immutable UTF-8 text.
    FrozenStr(String),
    /// Known immutable bytes.
    FrozenBytes(Vec<u8>),
}

/// A constant initializer's type, publication category and optional known value.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstEvalResult {
    /// The initializer's resolved type, including frozen collection carriers.
    pub ty: ResolvedType,
    /// Whether native const evaluation suffices or a frozen carrier is required.
    pub kind: ConstKind,
    /// A value retained only when the limited evaluator knows it.
    pub value: Option<ConstValue>,
}

/// Borrow known frozen text without reconstructing or copying the value.
fn const_str(value: &ConstValue) -> Option<&str> {
    match value {
        ConstValue::FrozenStr(s) => Some(s.as_str()),
        _ => None,
    }
}

/// Read a known integer for string indexing or slicing.
fn const_int(value: &ConstValue) -> Option<i64> {
    match value {
        ConstValue::Int(i) => Some(*i),
        _ => None,
    }
}

/// Return the binary floating-point value represented by a const-evaluation result.
fn const_float(value: &ConstValue) -> Option<f64> {
    match value {
        ConstValue::Float(value) => Some(*value),
        _ => None,
    }
}

/// Return whether a non-negative integer literal can initialize a sized numeric type.
fn unsigned_int_literal_fits_numeric_target(lit: &IntLiteral, id: NumericTypeId) -> bool {
    match integer_bounds(id) {
        Some(IntegerBounds::Signed { maximum, .. }) => lit.magnitude <= maximum as u128,
        Some(IntegerBounds::Unsigned { maximum }) => lit.magnitude <= maximum,
        None => false,
    }
}

/// Dependency traversal state used to detect const-initializer cycles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstEvalState {
    /// The declaration has not entered evaluation.
    NotStarted,
    /// The declaration is on the active dependency stack.
    InProgress,
    /// The declaration's evaluation attempt has finished.
    Done,
}

impl TypeChecker {
    /// Resolve a `const` declaration's written annotation to the frozen type the const will carry, rejecting the one
    /// family of annotations that cannot be honored.
    ///
    /// `str`/`bytes` and the frozen wrappers freeze silently: `const X: str = ...` carries `FrozenStr`, which reads
    /// wherever `str` is expected, so the written annotation holds at every use site. A mutable container (`list`,
    /// `dict`, `set`) does not: its frozen wrapper is a different type at every use site, so accepting the annotation
    /// would only move the contradiction to the first call that passes the const along (#1488). That annotation is
    /// rejected here, at its own span, naming the frozen spelling to write instead; the const still resolves as its
    /// frozen type afterwards so the declaration site stays the only place the mismatch is reported.
    fn resolve_const_annotation(&mut self, konst: &ConstDecl, ann: &Spanned<Type>) -> ResolvedType {
        let resolved = self.resolve_type_checked(ann);
        let rejected = is_mutable_collection_const_annotation(&resolved);
        let frozen = freeze_const_type(resolved);
        if rejected {
            self.errors.push(errors::const_mutable_collection_annotation(
                &konst.name,
                &ann.node.to_string(),
                &frozen.to_string(),
                ann.span,
            ));
        }
        frozen
    }

    /// Evaluate, type-check, classify, and publish one `const` declaration for later compiler stages.
    pub fn check_and_resolve_const(&mut self, konst: &ConstDecl, decl_span: Span) {
        // ---- Annotation first: its diagnostic lands whether or not the initializer evaluates ----
        let annotation = konst.ty.as_ref().map(|ann| self.resolve_const_annotation(konst, ann));

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
        if let Some(expected) = annotation {
            match self.const_numeric_value_checked_against_numeric_expected(&result, &expected, konst.value.span) {
                Some(true) => result.ty = expected,
                Some(false) => {}
                None if self.types_compatible(&result.ty, &expected) => result.ty = expected,
                None => {
                    self.errors.push(errors::type_mismatch(
                        &expected.to_string(),
                        &result.ty.to_string(),
                        konst.value.span,
                    ));
                }
            }
        } else if matches!(result.ty, ResolvedType::Unknown) {
            self.errors
                .push(errors::const_missing_type_annotation(&konst.name, decl_span));
        }

        // A value the evaluator does not fold to a scalar (a list of model values, for one) is lowered as computed
        // storage, which reads the checked types and identities of the initializer's own expressions.
        if result.value.is_none() && !matches!(result.ty, ResolvedType::Unknown) {
            let expected = result.ty.clone();
            self.check_expr_with_expected(&konst.value, Some(&expected));
        }

        // Record the root initializer type so lowering/codegen can use it.
        self.record_expr_type(konst.value.span, result.ty.clone());
        self.const_eval_cache.insert(konst.name.clone(), result.clone());

        // Update the symbol table type (so later expressions see the refined type).
        if let Some(var_info) = self.lookup_local_variable_info_mut(&konst.name) {
            var_info.ty = result.ty.clone();
        }
    }

    /// Validate a numeric const literal against an exact-width annotation without widening its declared type.
    fn const_numeric_value_checked_against_numeric_expected(
        &mut self,
        result: &ConstEvalResult,
        expected: &ResolvedType,
        span: Span,
    ) -> Option<bool> {
        let target = super::numeric_type_id_for_compat(expected)?;
        if let Some(value) = result.value.as_ref().and_then(const_int) {
            if matches!(target, NumericTypeId::F32 | NumericTypeId::F64) {
                return Some(true);
            }
            match integer_bounds(target)? {
                IntegerBounds::Signed { minimum, maximum }
                    if i128::from(value) < minimum || i128::from(value) > maximum =>
                {
                    self.errors.push(CompileError::type_error(
                        format!(
                            "Integer literal {value} does not fit in {expected}; valid range is {minimum}..={maximum}"
                        ),
                        span,
                    ));
                    return Some(false);
                }
                IntegerBounds::Unsigned { maximum } if value < 0 || value as u128 > maximum => {
                    self.errors.push(CompileError::type_error(
                        format!("Integer literal {value} does not fit in {expected}; valid range is 0..={maximum}"),
                        span,
                    ));
                    return Some(false);
                }
                _ => {}
            }
            return Some(true);
        }

        let value = result.value.as_ref().and_then(const_float)?;
        let fits = match expected {
            ResolvedType::Numeric(NumericTypeId::F32) => value.is_finite() && value.abs() <= f64::from(f32::MAX),
            _ => return None,
        };
        if !fits {
            self.errors.push(CompileError::type_error(
                format!("Constant value {value} does not fit in {expected}"),
                span,
            ));
        }
        Some(fits)
    }

    /// Evaluate the named module-level const, memoizing the result and detecting dependency cycles.
    ///
    /// `stack` is the chain of consts currently being evaluated: a const whose initializer references another const
    /// recurses through here, and a name found `InProgress` is a cycle reported at that const's declaration span. A
    /// const that fails to evaluate returns `None` after its diagnostic has been pushed, so a dependent const's own
    /// evaluation stops rather than reporting the same failure again. The written annotation supplies only the
    /// frozen expected type here; whether it may be written at all is `check_and_resolve_const`'s question.
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

        // The evaluator only needs the frozen expectation; the declaration check (`check_and_resolve_const`) is
        // the one place an unhonorable annotation is reported.
        let expected = decl.ty.as_ref().map(|t| self.resolve_type_checked(t));
        let expected = expected.map(freeze_const_type);
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
            Expr::Ident(name) => self.eval_const_ident(name, expected, stack, expr.span),
            Expr::Tuple(items) => self.eval_const_tuple(items, stack, decl_span),
            Expr::Unary(op, inner) => self.eval_const_unary(op, inner, expected, stack, decl_span, expr.span),
            Expr::Binary(left, op, right) => self.eval_const_binary(left, op, right, stack, decl_span, expr.span),
            Expr::List(items) => self.eval_const_list(items, expected, stack, decl_span, expr.span),
            Expr::Set(items) => self.eval_const_set(items, expected, stack, decl_span, expr.span),
            Expr::Dict(pairs) => self.eval_const_dict(pairs, expected, stack, decl_span, expr.span),

            Expr::Index(base, index) => self.eval_const_index(base, index, stack, decl_span, expr.span),
            Expr::Slice(base, slice) => self.eval_const_slice(base, slice, stack, decl_span, expr.span),
            Expr::Call(callee, type_args, args) if type_args.is_empty() => {
                self.eval_const_call(callee, args, expected, stack, decl_span, expr.span)
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
            | Expr::VocabBlock(_)
            | Expr::Embedded(_)
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

    /// Resolve an Incan constant or retain an imported Rust constant as an opaque native value.
    ///
    /// An enclosing annotation supplies an opaque value's type when available; rustc remains responsible for its
    /// actual value and type compatibility. Incan declarations keep the existing dependency-cycle stack.
    fn eval_const_ident(
        &mut self,
        name: &str,
        expected: Option<&ResolvedType>,
        stack: &mut Vec<String>,
        span: Span,
    ) -> Option<ConstEvalResult> {
        if self.const_decls.contains_key(name) {
            return self.eval_const_by_name(name, stack);
        }
        if let Some(symbol) = self.lookup_symbol(name)
            && match &symbol.kind {
                SymbolKind::RustItem(_) => true,
                SymbolKind::Module(info) => info.path.first().is_some_and(|segment| segment == "rust"),
                _ => false,
            }
        {
            return Some(ConstEvalResult {
                ty: expected.cloned().unwrap_or(ResolvedType::Unknown),
                kind: ConstKind::RustNative,
                value: None,
            });
        }
        self.errors.push(errors::const_non_const_name(name, span));
        None
    }

    /// Validate tuple members in source order and retain a frozen classification if any member requires it.
    fn eval_const_tuple(
        &mut self,
        items: &[Spanned<Expr>],
        stack: &mut Vec<String>,
        decl_span: Span,
    ) -> Option<ConstEvalResult> {
        let mut types = Vec::with_capacity(items.len());
        let mut kind = ConstKind::RustNative;
        for item in items {
            let result = self.eval_const_expr(item, None, stack, decl_span)?;
            types.push(result.ty);
            if result.kind == ConstKind::Frozen {
                kind = ConstKind::Frozen;
            }
        }
        Some(ConstEvalResult {
            ty: ResolvedType::Tuple(types),
            kind,
            value: None,
        })
    }

    /// Convert a negative integer literal by its exact signed magnitude before ordinary unary constant evaluation.
    fn eval_const_unary(
        &mut self,
        op: &UnaryOp,
        inner: &Spanned<Expr>,
        expected: Option<&ResolvedType>,
        stack: &mut Vec<String>,
        decl_span: Span,
        span: Span,
    ) -> Option<ConstEvalResult> {
        if *op == UnaryOp::Neg
            && let Expr::Literal(Literal::Int(literal)) = &inner.node
        {
            return Some(Self::eval_const_integer_literal(literal, true, expected));
        }
        let result = self.eval_const_expr(inner, None, stack, decl_span)?;
        match op {
            UnaryOp::Neg => {
                if matches!(result.ty, ResolvedType::Int | ResolvedType::Float) {
                    let value = match result.value.as_ref() {
                        Some(ConstValue::Int(number)) => number.checked_neg().map(ConstValue::Int),
                        Some(ConstValue::Float(number)) => Some(ConstValue::Float(-number)),
                        _ => None,
                    };
                    Some(ConstEvalResult {
                        ty: result.ty,
                        kind: result.kind,
                        value,
                    })
                } else {
                    self.errors
                        .push(errors::const_unary_op_not_supported("-", &result.ty.to_string(), span));
                    None
                }
            }
            UnaryOp::Not => {
                if matches!(result.ty, ResolvedType::Bool) {
                    let value = match result.value.as_ref() {
                        Some(ConstValue::Bool(value)) => Some(ConstValue::Bool(!value)),
                        _ => None,
                    };
                    Some(ConstEvalResult {
                        ty: ResolvedType::Bool,
                        kind: result.kind,
                        value,
                    })
                } else {
                    self.errors.push(errors::const_unary_op_not_supported(
                        "not",
                        &result.ty.to_string(),
                        span,
                    ));
                    None
                }
            }
            UnaryOp::Invert => {
                if matches!(result.ty, ResolvedType::Int) {
                    let value = match result.value.as_ref() {
                        Some(ConstValue::Int(number)) => Some(ConstValue::Int(!number)),
                        _ => None,
                    };
                    Some(ConstEvalResult {
                        ty: ResolvedType::Int,
                        kind: result.kind,
                        value,
                    })
                } else {
                    self.errors
                        .push(errors::const_unary_op_not_supported("~", &result.ty.to_string(), span));
                    None
                }
            }
        }
    }

    /// Evaluate binary operands in source order and preserve string policy before general numeric validation.
    ///
    /// An opaque Rust constant may borrow the other operand's numeric type; rustc still checks the actual native
    /// value. String operations retain the same limited known values as the existing const domain.
    fn eval_const_binary(
        &mut self,
        left_expr: &Spanned<Expr>,
        op: &BinaryOp,
        right_expr: &Spanned<Expr>,
        stack: &mut Vec<String>,
        decl_span: Span,
        span: Span,
    ) -> Option<ConstEvalResult> {
        let mut left = self.eval_const_expr(left_expr, None, stack, decl_span)?;
        let mut right = self.eval_const_expr(right_expr, None, stack, decl_span)?;
        if left.kind == ConstKind::RustNative
            && left.ty == ResolvedType::Unknown
            && numeric_ty_from_resolved(&right.ty).is_some()
        {
            left.ty = right.ty.clone();
        } else if right.kind == ConstKind::RustNative
            && right.ty == ResolvedType::Unknown
            && numeric_ty_from_resolved(&left.ty).is_some()
        {
            right.ty = left.ty.clone();
        }

        if matches!(op, BinaryOp::Add) && is_str_like(&left.ty) && is_str_like(&right.ty) {
            let value = match (
                left.value.as_ref().and_then(const_str),
                right.value.as_ref().and_then(const_str),
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
        if matches!(
            op,
            BinaryOp::Eq | BinaryOp::NotEq | BinaryOp::Lt | BinaryOp::Gt | BinaryOp::LtEq | BinaryOp::GtEq
        ) && is_str_like(&left.ty)
            && is_str_like(&right.ty)
        {
            return Some(ConstEvalResult {
                ty: ResolvedType::Bool,
                kind: ConstKind::RustNative,
                value: None,
            });
        }
        if matches!(op, BinaryOp::In | BinaryOp::NotIn) && is_str_like(&left.ty) && is_str_like(&right.ty) {
            let value = match (
                left.value.as_ref().and_then(const_str),
                right.value.as_ref().and_then(const_str),
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
        self.eval_const_binary_operands(op, &left, &right, right_expr, span)
    }

    /// Validate a frozen list, rejecting spreads before evaluating any element and using the first element's type.
    fn eval_const_list(
        &mut self,
        items: &[ListEntry],
        expected: Option<&ResolvedType>,
        stack: &mut Vec<String>,
        decl_span: Span,
        span: Span,
    ) -> Option<ConstEvalResult> {
        if items.iter().any(|item| matches!(item, ListEntry::Spread(_))) {
            self.errors.push(errors::const_expression_not_allowed(span));
            return None;
        }
        let elem_expected = expected.and_then(|ty| match ty {
            ResolvedType::FrozenList(elem) => Some(elem.as_ref()),
            ResolvedType::Generic(name, args)
                if crate::typechecker::helpers::collection_type_id(name.as_str())
                    == Some(incan_lang::lang::types::collections::CollectionTypeId::FrozenList)
                    && !args.is_empty() =>
            {
                Some(&args[0])
            }
            _ => None,
        });
        let elem_ty = if items.is_empty() {
            elem_expected.cloned().unwrap_or(ResolvedType::Unknown)
        } else {
            let ListEntry::Element(first) = &items[0] else {
                unreachable!("list spreads were rejected before const list evaluation");
            };
            let first = self.eval_const_expr(first, elem_expected, stack, decl_span)?;
            for item in items.iter().skip(1) {
                let ListEntry::Element(value) = item else {
                    unreachable!("list spreads were rejected before const list evaluation");
                };
                self.eval_const_expr(value, elem_expected, stack, decl_span)?;
            }
            first.ty
        };
        if items.is_empty() && matches!(elem_ty, ResolvedType::Unknown) {
            self.errors.push(errors::const_empty_list_type_inference(span));
        }
        Some(ConstEvalResult {
            ty: ResolvedType::FrozenList(Box::new(elem_ty)),
            kind: ConstKind::Frozen,
            value: None,
        })
    }

    /// Validate frozen set members against an available annotation, retaining the first member's type.
    fn eval_const_set(
        &mut self,
        items: &[Spanned<Expr>],
        expected: Option<&ResolvedType>,
        stack: &mut Vec<String>,
        decl_span: Span,
        span: Span,
    ) -> Option<ConstEvalResult> {
        let elem_expected = expected.and_then(|ty| match ty {
            ResolvedType::FrozenSet(elem) => Some(elem.as_ref()),
            ResolvedType::Generic(name, args)
                if crate::typechecker::helpers::collection_type_id(name.as_str())
                    == Some(incan_lang::lang::types::collections::CollectionTypeId::FrozenSet)
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
            for item in items.iter().skip(1) {
                self.eval_const_expr(item, elem_expected, stack, decl_span)?;
            }
            first.ty
        };
        if items.is_empty() && matches!(elem_ty, ResolvedType::Unknown) {
            self.errors.push(errors::const_empty_set_type_inference(span));
        }
        Some(ConstEvalResult {
            ty: ResolvedType::FrozenSet(Box::new(elem_ty)),
            kind: ConstKind::Frozen,
            value: None,
        })
    }

    /// Validate frozen dictionary pairs in source order, inferring key and value types from the first pair.
    fn eval_const_dict(
        &mut self,
        pairs: &[DictEntry],
        expected: Option<&ResolvedType>,
        stack: &mut Vec<String>,
        decl_span: Span,
        span: Span,
    ) -> Option<ConstEvalResult> {
        let (key_expected, value_expected) = match expected {
            Some(ResolvedType::FrozenDict(key, value)) => (Some(key.as_ref()), Some(value.as_ref())),
            Some(ResolvedType::Generic(name, args))
                if crate::typechecker::helpers::collection_type_id(name.as_str())
                    == Some(incan_lang::lang::types::collections::CollectionTypeId::FrozenDict)
                    && args.len() >= 2 =>
            {
                (Some(&args[0]), Some(&args[1]))
            }
            _ => (None, None),
        };
        let (key_ty, value_ty) = if pairs.is_empty() {
            (
                key_expected.cloned().unwrap_or(ResolvedType::Unknown),
                value_expected.cloned().unwrap_or(ResolvedType::Unknown),
            )
        } else {
            let DictEntry::Pair(first_key, first_value) = &pairs[0] else {
                self.errors.push(errors::const_expression_not_allowed(span));
                return None;
            };
            let key = self.eval_const_expr(first_key, key_expected, stack, decl_span)?;
            let value = self.eval_const_expr(first_value, value_expected, stack, decl_span)?;
            for entry in pairs.iter().skip(1) {
                let DictEntry::Pair(key, value) = entry else {
                    self.errors.push(errors::const_expression_not_allowed(span));
                    return None;
                };
                self.eval_const_expr(key, key_expected, stack, decl_span)?;
                self.eval_const_expr(value, value_expected, stack, decl_span)?;
            }
            (key.ty, value.ty)
        };
        if pairs.is_empty() && (matches!(key_ty, ResolvedType::Unknown) || matches!(value_ty, ResolvedType::Unknown)) {
            self.errors.push(errors::const_empty_dict_type_inference(span));
        }
        Some(ConstEvalResult {
            ty: ResolvedType::FrozenDict(Box::new(key_ty), Box::new(value_ty)),
            kind: ConstKind::Frozen,
            value: None,
        })
    }

    /// Apply the non-string const policy to already evaluated binary operands.
    fn eval_const_binary_operands(
        &mut self,
        op: &BinaryOp,
        left: &ConstEvalResult,
        right: &ConstEvalResult,
        right_expr: &Spanned<Expr>,
        span: Span,
    ) -> Option<ConstEvalResult> {
        match op {
            BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::Mul
            | BinaryOp::Div
            | BinaryOp::FloorDiv
            | BinaryOp::Mod
            | BinaryOp::Pow => self.eval_const_numeric_binary(op, left, right, right_expr, span),
            BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor | BinaryOp::Shl | BinaryOp::Shr => {
                self.eval_const_bitwise_binary(op, left, right, span)
            }
            BinaryOp::Eq | BinaryOp::NotEq | BinaryOp::Lt | BinaryOp::Gt | BinaryOp::LtEq | BinaryOp::GtEq => {
                self.eval_const_comparison(left, right, span)
            }
            BinaryOp::And | BinaryOp::Or => self.eval_const_logical_binary(op, left, right, span),
            BinaryOp::MatMul
            | BinaryOp::PipeForward
            | BinaryOp::PipeBackward
            | BinaryOp::In
            | BinaryOp::NotIn
            | BinaryOp::Is
            | BinaryOp::IsNot => {
                self.errors
                    .push(errors::const_operator_not_allowed(&op.to_string(), span));
                None
            }
        }
    }

    /// Select an arithmetic constant's numeric type without taking evaluation away from rustc.
    fn eval_const_numeric_binary(
        &mut self,
        op: &BinaryOp,
        left: &ConstEvalResult,
        right: &ConstEvalResult,
        right_expr: &Spanned<Expr>,
        span: Span,
    ) -> Option<ConstEvalResult> {
        let left_numeric = numeric_ty_from_resolved(&left.ty);
        let right_numeric = numeric_ty_from_resolved(&right.ty);
        match (left_numeric, right_numeric) {
            (Some(lhs), Some(rhs)) => {
                let Some(numeric_op) = numeric_op_from_ast(op) else {
                    self.errors.push(errors::const_binary_op_not_supported(
                        &op.to_string(),
                        &left.ty.to_string(),
                        &right.ty.to_string(),
                        span,
                    ));
                    return None;
                };
                let exponent = if matches!(op, BinaryOp::Pow) {
                    Some(pow_exponent_kind_from_ast(right_expr, &right.ty))
                } else {
                    None
                };
                let ty = match result_numeric_type(numeric_op, lhs, rhs, exponent) {
                    NumericTy::Int => ResolvedType::Int,
                    NumericTy::Float => ResolvedType::Float,
                };
                Some(ConstEvalResult {
                    ty,
                    kind: ConstKind::RustNative,
                    value: None,
                })
            }
            _ => {
                self.errors.push(errors::const_binary_op_not_supported(
                    &op.to_string(),
                    &left.ty.to_string(),
                    &right.ty.to_string(),
                    span,
                ));
                None
            }
        }
    }

    /// Validate integer bitwise operands and retain only values supported by the existing checked-shift policy.
    fn eval_const_bitwise_binary(
        &mut self,
        op: &BinaryOp,
        left: &ConstEvalResult,
        right: &ConstEvalResult,
        span: Span,
    ) -> Option<ConstEvalResult> {
        if !matches!((&left.ty, &right.ty), (ResolvedType::Int, ResolvedType::Int)) {
            self.errors.push(errors::const_binary_op_not_supported(
                &op.to_string(),
                &left.ty.to_string(),
                &right.ty.to_string(),
                span,
            ));
            return None;
        }
        let value = match (left.value.as_ref(), right.value.as_ref(), op) {
            (Some(ConstValue::Int(lhs)), Some(ConstValue::Int(rhs)), BinaryOp::BitAnd) => {
                Some(ConstValue::Int(lhs & rhs))
            }
            (Some(ConstValue::Int(lhs)), Some(ConstValue::Int(rhs)), BinaryOp::BitOr) => {
                Some(ConstValue::Int(lhs | rhs))
            }
            (Some(ConstValue::Int(lhs)), Some(ConstValue::Int(rhs)), BinaryOp::BitXor) => {
                Some(ConstValue::Int(lhs ^ rhs))
            }
            (Some(ConstValue::Int(lhs)), Some(ConstValue::Int(rhs)), BinaryOp::Shl) if *rhs >= 0 => u32::try_from(*rhs)
                .ok()
                .and_then(|rhs| lhs.checked_shl(rhs))
                .map(ConstValue::Int),
            (Some(ConstValue::Int(lhs)), Some(ConstValue::Int(rhs)), BinaryOp::Shr) if *rhs >= 0 => u32::try_from(*rhs)
                .ok()
                .and_then(|rhs| lhs.checked_shr(rhs))
                .map(ConstValue::Int),
            _ => None,
        };
        Some(ConstEvalResult {
            ty: ResolvedType::Int,
            kind: ConstKind::RustNative,
            value,
        })
    }

    /// Validate comparison compatibility, admitting mixed numeric types and leaving the value to rustc.
    fn eval_const_comparison(
        &mut self,
        left: &ConstEvalResult,
        right: &ConstEvalResult,
        span: Span,
    ) -> Option<ConstEvalResult> {
        let left_numeric = numeric_ty_from_resolved(&left.ty);
        let right_numeric = numeric_ty_from_resolved(&right.ty);
        if !(left_numeric.is_some() && right_numeric.is_some()) && !self.types_compatible(&left.ty, &right.ty) {
            self.errors.push(errors::const_compare_incompatible(
                &left.ty.to_string(),
                &right.ty.to_string(),
                span,
            ));
            return None;
        }
        Some(ConstEvalResult {
            ty: ResolvedType::Bool,
            kind: ConstKind::RustNative,
            value: None,
        })
    }

    /// Validate Boolean logical operands and preserve a known result only when both operand values are known.
    fn eval_const_logical_binary(
        &mut self,
        op: &BinaryOp,
        left: &ConstEvalResult,
        right: &ConstEvalResult,
        span: Span,
    ) -> Option<ConstEvalResult> {
        if !matches!(left.ty, ResolvedType::Bool) || !matches!(right.ty, ResolvedType::Bool) {
            self.errors.push(errors::const_logical_op_requires_bool(
                &op.to_string(),
                &left.ty.to_string(),
                &right.ty.to_string(),
                span,
            ));
            return None;
        }
        let value = match (left.value.as_ref(), right.value.as_ref()) {
            (Some(ConstValue::Bool(lhs)), Some(ConstValue::Bool(rhs))) => {
                Some(ConstValue::Bool(if matches!(op, BinaryOp::And) {
                    *lhs && *rhs
                } else {
                    *lhs || *rhs
                }))
            }
            _ => None,
        };
        Some(ConstEvalResult {
            ty: ResolvedType::Bool,
            kind: ConstKind::RustNative,
            value,
        })
    }

    /// Validate constant string indexing and retain its character when the base and index values are known.
    fn eval_const_index(
        &mut self,
        base: &Spanned<Expr>,
        index: &Spanned<Expr>,
        stack: &mut Vec<String>,
        decl_span: Span,
        span: Span,
    ) -> Option<ConstEvalResult> {
        let base_result = self.eval_const_expr(base, None, stack, decl_span)?;
        let index_result = self.eval_const_expr(index, None, stack, decl_span)?;
        if !is_frozen_str(&base_result.ty) && !matches!(base_result.ty, ResolvedType::Str) {
            self.errors.push(errors::const_indexing_requires_string(span));
            return None;
        }
        if !is_intlike_for_index(&index_result.ty) {
            self.errors.push(errors::const_string_index_requires_int(
                &index_result.ty.to_string(),
                index.span,
            ));
            return None;
        }
        let mut value = None;
        if let (Some(base_str), Some(index_value)) = (
            base_result.value.as_ref().and_then(const_str),
            index_result.value.as_ref().and_then(const_int),
        ) {
            match strings::str_char_at(base_str, index_value) {
                Ok(character) => value = Some(ConstValue::FrozenStr(character)),
                Err(StringAccessError::IndexOutOfRange) => {
                    self.errors.push(errors::const_string_index_out_of_range(span));
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

    /// Validate constant string slice components in source order and apply the existing clamping and step policy.
    fn eval_const_slice(
        &mut self,
        base: &Spanned<Expr>,
        slice: &SliceExpr,
        stack: &mut Vec<String>,
        decl_span: Span,
        span: Span,
    ) -> Option<ConstEvalResult> {
        let base_result = self.eval_const_expr(base, None, stack, decl_span)?;
        if !is_frozen_str(&base_result.ty) && !matches!(base_result.ty, ResolvedType::Str) {
            self.errors.push(errors::const_slicing_requires_string(base.span));
            return None;
        }
        let mut start_value = None;
        if let Some(start) = &slice.start {
            let result = self.eval_const_expr(start, None, stack, decl_span)?;
            if !is_intlike_for_index(&result.ty) {
                self.errors.push(errors::const_slice_component_requires_int(
                    "start",
                    &result.ty.to_string(),
                    start.span,
                ));
                return None;
            }
            start_value = result.value.as_ref().and_then(const_int);
        }
        let mut end_value = None;
        if let Some(end) = &slice.end {
            let result = self.eval_const_expr(end, None, stack, decl_span)?;
            if !is_intlike_for_index(&result.ty) {
                self.errors.push(errors::const_slice_component_requires_int(
                    "end",
                    &result.ty.to_string(),
                    end.span,
                ));
                return None;
            }
            end_value = result.value.as_ref().and_then(const_int);
        }
        let mut step_value = None;
        if let Some(step) = &slice.step {
            let result = self.eval_const_expr(step, None, stack, decl_span)?;
            if !is_intlike_for_index(&result.ty) {
                self.errors.push(errors::const_slice_component_requires_int(
                    "step",
                    &result.ty.to_string(),
                    step.span,
                ));
                return None;
            }
            step_value = result.value.as_ref().and_then(const_int);
        }
        let mut value = None;
        if let Some(base_str) = base_result.value.as_ref().and_then(const_str) {
            match strings::str_slice(base_str, start_value, end_value, step_value) {
                Ok(sliced) => value = Some(ConstValue::FrozenStr(sliced)),
                Err(StringAccessError::SliceStepZero) => {
                    let span = slice.step.as_ref().map(|step| step.span).unwrap_or(span);
                    self.errors.push(errors::const_slice_step_zero(span));
                    return None;
                }
                Err(StringAccessError::IndexOutOfRange) => {
                    self.errors.push(errors::const_string_index_out_of_range(span));
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

    /// Validate a const model, partial constructor or expected numeric newtype call without explicit type arguments.
    fn eval_const_call(
        &mut self,
        callee: &Spanned<Expr>,
        args: &[CallArg],
        expected: Option<&ResolvedType>,
        stack: &mut Vec<String>,
        decl_span: Span,
        span: Span,
    ) -> Option<ConstEvalResult> {
        if let Expr::Ident(name) = &callee.node
            && self.is_const_model_constructor_name(name)
        {
            return self.eval_const_model_constructor(name, args, expected, stack, decl_span, span);
        }
        if let Expr::Ident(name) = &callee.node
            && let Some(result) = self.eval_const_partial_constructor_call(name, args, expected, stack, decl_span, span)
        {
            return Some(result);
        }
        let Some(ResolvedType::Named(expected_name)) = expected else {
            self.errors.push(errors::const_expression_not_allowed(span));
            return None;
        };
        let Expr::Ident(name) = &callee.node else {
            self.errors.push(errors::const_expression_not_allowed(span));
            return None;
        };
        let Some(TypeInfo::Newtype(newtype)) = self.lookup_type_info(name) else {
            self.errors.push(errors::const_expression_not_allowed(span));
            return None;
        };
        let underlying = newtype.underlying.clone();
        if name != expected_name {
            self.errors.push(errors::const_expression_not_allowed(span));
            return None;
        }
        let [CallArg::Positional(arg)] = args else {
            self.errors.push(errors::const_expression_not_allowed(span));
            return None;
        };
        let Some(target) = super::numeric_type_id_for_compat(&underlying) else {
            self.errors.push(errors::const_expression_not_allowed(span));
            return None;
        };
        let Expr::Literal(Literal::Int(literal)) = &arg.node else {
            self.errors.push(errors::const_expression_not_allowed(span));
            return None;
        };
        if !unsigned_int_literal_fits_numeric_target(literal, target) {
            self.errors.push(CompileError::type_error(
                format!("Integer literal {} does not fit in {}", literal.repr, underlying),
                arg.span,
            ));
            return None;
        }
        if literal.suffix.is_none() {
            self.retain_body_ir_const_newtype(name, &underlying, literal.magnitude, span);
        }
        Some(ConstEvalResult {
            ty: ResolvedType::Named(expected_name.clone()),
            kind: ConstKind::RustNative,
            value: None,
        })
    }

    /// Retain only an already range-checked unsuffixed sized integer constructor, without altering legacy evaluation.
    fn retain_body_ir_const_newtype(&mut self, name: &str, underlying: &ResolvedType, magnitude: u128, span: Span) {
        use incan_lang::lang::types::numerics::{NumericFamily, info_for};
        use incan_semantics_core::body_ir::TypedNumericConstant;
        let ResolvedType::Numeric(kind) = underlying else {
            return;
        };
        let Some(owner) = self.named_type_reference_identity(name) else {
            return;
        };
        if owner.kind != incan_semantics_core::SemanticSourceTargetKind::Newtype {
            return;
        }
        let value = match info_for(*kind).family {
            NumericFamily::SignedInteger => match i128::try_from(magnitude) {
                Ok(value) => TypedNumericConstant::Signed { kind: *kind, value },
                Err(_) => return,
            },
            NumericFamily::UnsignedInteger => TypedNumericConstant::Unsigned {
                kind: *kind,
                value: magnitude,
            },
            _ => return,
        };
        self.type_info
            .consts
            .body_ir_newtype_values
            .insert(span, super::type_info::BodyIrNewtypeConstantValue { owner, value });
    }

    /// Return whether a name resolves to a model constructor that can be considered for const literal validation.
    fn is_const_model_constructor_name(&self, name: &str) -> bool {
        self.lookup_type_info(name)
            .is_some_and(|info| matches!(info, TypeInfo::Model(_)))
    }

    /// Evaluate a model-constructor partial call inside a const initializer.
    fn eval_const_partial_constructor_call(
        &mut self,
        callee_name: &str,
        args: &[CallArg],
        expected: Option<&ResolvedType>,
        stack: &mut Vec<String>,
        decl_span: Span,
        call_span: Span,
    ) -> Option<ConstEvalResult> {
        let projection = self.type_info.partial_projection(callee_name)?.clone();
        if projection.target_kind != PartialProjectionTargetKind::ModelConstructor {
            return None;
        }
        let target_name = projection.target_path.last()?;
        let merged_args = merge_named_partial_args(
            projection.presets.iter().map(|preset| PartialPresetRef {
                name: preset.name.as_str(),
                value: &preset.value,
            }),
            args,
        )?;
        self.eval_const_model_constructor(target_name, &merged_args, expected, stack, decl_span, call_span)
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

            let Some((canonical_name, field_info)) = Self::resolve_const_model_field(&model.fields, &field_name.node)
            else {
                self.eval_const_expr(value, None, stack, decl_span);
                self.errors
                    .push(errors::missing_field(type_name, &field_name.node, field_name.span));
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
                match self.const_numeric_value_checked_against_numeric_expected(
                    &field_result,
                    &field_info.ty,
                    value.span,
                ) {
                    Some(true) => {}
                    Some(false) => had_error = true,
                    None => {
                        self.errors.push(errors::field_type_mismatch(
                            &field_name.node,
                            &field_info.ty.to_string(),
                            &field_result.ty.to_string(),
                            field_name.span,
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
        fields: &'a std::collections::HashMap<String, crate::symbols::FieldInfo>,
        field_name: &str,
    ) -> Option<(String, &'a crate::symbols::FieldInfo)> {
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

    /// Retain an integer literal only when its exact signed magnitude fits the default constant carrier. An
    /// unretained value keeps its declared numeric carrier and proceeds through ordinary expected-type checking,
    /// which validates the full magnitude instead of publishing a saturated integer to Body IR.
    fn eval_const_integer_literal(
        literal: &IntLiteral,
        negative: bool,
        expected: Option<&ResolvedType>,
    ) -> ConstEvalResult {
        let value = literal
            .signed_value(negative)
            .and_then(|value| i64::try_from(value).ok())
            .map(ConstValue::Int);
        let ty = if value.is_some() {
            ResolvedType::Int
        } else {
            literal
                .suffix
                .map(ResolvedType::from_numeric_id)
                .or_else(|| {
                    expected
                        .filter(|ty| super::numeric_type_id_for_compat(ty).is_some())
                        .cloned()
                })
                .unwrap_or(ResolvedType::Int)
        };
        ConstEvalResult {
            ty,
            kind: ConstKind::RustNative,
            value,
        }
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
            Literal::Int(il) => Self::eval_const_integer_literal(il, false, expected),
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

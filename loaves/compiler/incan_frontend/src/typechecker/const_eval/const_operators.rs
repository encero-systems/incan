//! Const unary, binary, numeric, bitwise, comparison and logical operators, collections, indexing and slicing.

use super::*;

impl TypeChecker {
    /// Convert a negative integer literal by its exact signed magnitude before ordinary unary constant evaluation.
    pub(super) fn eval_const_unary(
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
    pub(super) fn eval_const_binary(
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
    pub(super) fn eval_const_list(
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
    pub(super) fn eval_const_set(
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
    pub(super) fn eval_const_dict(
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
    pub(super) fn eval_const_binary_operands(
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
    pub(super) fn eval_const_numeric_binary(
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
    pub(super) fn eval_const_bitwise_binary(
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
    pub(super) fn eval_const_comparison(
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
    pub(super) fn eval_const_logical_binary(
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
    pub(super) fn eval_const_index(
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
    pub(super) fn eval_const_slice(
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
}

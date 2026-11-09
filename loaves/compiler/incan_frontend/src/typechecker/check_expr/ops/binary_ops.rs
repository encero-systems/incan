//! Binary operator checking: integer and numeric results, concatenation, overloads, comparison and membership.

use super::*;

impl TypeChecker {
    /// Give an unsuffixed integer literal operand the exact-width integer type of the other operand.
    ///
    /// RFC 009 types an unsuffixed integer literal from its context, and for `+`, `-`, `*`, `//`, `%`, `&`, `|`, `^`
    /// and the comparisons the other operand is that context: `n + 1` adds two `i8` values when `n` is an `i8`, and
    /// `n == 0` compares two. The literal is checked again against that type as it is at a destination of that type,
    /// which records the type for lowering; in arithmetic that refuses a value outside the type's range (`n + 300`, or
    /// `b - -1` for a `u8` `b`), while a comparison keeps such a literal an `int` and compares in a type holding both
    /// (`b == -1` is `false`). `/` is left alone because it divides as floats whatever its operands are, and so are the
    /// exponent of `**` and the right operand of `<<` and `>>`, which count rather than being values of the other
    /// operand's type. Returns the operand types after the literal has taken its partner's type.
    pub(super) fn integer_literal_operand_takes_partner_type(
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
                | BinaryOp::BitAnd
                | BinaryOp::BitOr
                | BinaryOp::BitXor
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
    pub(super) fn integer_arithmetic_result_type(
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

    /// Return the type of a bitwise operator over two integer operands, by the operator result table.
    ///
    /// RFC 009: same-type integer arithmetic yields that type, and bit arithmetic is integer arithmetic. `&`, `|` and
    /// `^` keep their operands' one integer type, after an unsuffixed integer literal operand has taken the
    /// exact-width type of the other operand, and operands of two different integer types are refused as they are for
    /// `+`. `<<` and `>>` keep their left operand's type: the right operand counts bit positions, so it may be of any
    /// integer type. The table's one home for `a op b` and for `x op= y`; a refusal is recorded and yields `Unknown`.
    pub(in crate::typechecker) fn integer_bitwise_result_type(
        &mut self,
        (left, left_ty): (&Spanned<Expr>, &ResolvedType),
        op: BinaryOp,
        (right, right_ty): (&Spanned<Expr>, &ResolvedType),
        span: Span,
    ) -> ResolvedType {
        if matches!(op, BinaryOp::Shl | BinaryOp::Shr) {
            return left_ty.clone();
        }
        let (left_ty, right_ty) =
            self.integer_literal_operand_takes_partner_type((left, left_ty), op, (right, right_ty));
        self.integer_arithmetic_result_type(&left_ty, op, &right_ty, span)
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
    pub(super) fn adopt_partner_type_for_empty_list_operand(
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

    /// Give an open `Ok` or `Err` operand the concrete `Result` type of its comparison partner.
    ///
    /// A standalone constructor deliberately leaves the opposite `Result` side open. In a comparison such as
    /// `reader.contains(key) == Ok(True)`, the concrete left operand is the type context for that side, just as a
    /// concrete list is the context for an empty-list operand. Re-checking only a recognized, unshadowed constructor
    /// records the closed type for Body IR; compatibility and comparison semantics remain unchanged.
    pub(super) fn adopt_partner_type_for_open_result_constructor_operand(
        &mut self,
        operand: &Spanned<Expr>,
        operand_ty: ResolvedType,
        partner_ty: &ResolvedType,
    ) -> ResolvedType {
        let Some(constructor) = self.returned_result_constructor(operand) else {
            return operand_ty;
        };
        let (ResolvedType::Generic(operand_name, operand_sides), ResolvedType::Generic(partner_name, partner_sides)) =
            (&operand_ty, partner_ty)
        else {
            return operand_ty;
        };
        if collection_type_id(operand_name.as_str()) != Some(CollectionTypeId::Result)
            || collection_type_id(partner_name.as_str()) != Some(CollectionTypeId::Result)
            || operand_sides.len() != 2
            || partner_sides.len() != 2
        {
            return operand_ty;
        }
        let open_side = match constructor {
            ConstructorId::Ok => 1,
            ConstructorId::Err => 0,
            ConstructorId::Some | ConstructorId::None => return operand_ty,
        };
        let is_open = |ty: &ResolvedType| {
            matches!(
                ty,
                ResolvedType::Unknown | ResolvedType::TypeVar(_) | ResolvedType::CallSiteInfer
            )
        };
        if is_open(&operand_sides[open_side]) && !is_open(&partner_sides[open_side]) {
            self.check_expr_with_expected(operand, Some(partner_ty))
        } else {
            operand_ty
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
                self.check_arithmetic_binary((left, left_ty), op, (right, right_ty), span, expected_return_ty)
            }
            BinaryOp::MatMul
            | BinaryOp::PipeForward
            | BinaryOp::PipeBackward
            | BinaryOp::BitAnd
            | BinaryOp::BitOr
            | BinaryOp::BitXor
            | BinaryOp::Shl
            | BinaryOp::Shr => {
                self.check_overload_binary((left, left_ty), op, (right, right_ty), span, expected_return_ty)
            }
            BinaryOp::Eq | BinaryOp::NotEq | BinaryOp::Lt | BinaryOp::Gt | BinaryOp::LtEq | BinaryOp::GtEq => {
                self.check_comparison_binary((left, left_ty), op, (right, right_ty), span)
            }
            BinaryOp::In | BinaryOp::NotIn => self.check_membership_binary((left, left_ty), op, right_ty, span),
            BinaryOp::And | BinaryOp::Or | BinaryOp::Is | BinaryOp::IsNot => ResolvedType::Bool,
        }
    }

    /// Check arithmetic after both operands, preserving concatenation, generic dispatch and numeric precedence.
    pub(super) fn check_arithmetic_binary(
        &mut self,
        left: (&Spanned<Expr>, ResolvedType),
        op: BinaryOp,
        right: (&Spanned<Expr>, ResolvedType),
        span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        let (left, left_ty) = left;
        let (right, right_ty) = right;
        if let Some(result) =
            self.check_string_concatenation(&left_ty, op, (right, &right_ty), span, expected_return_ty)
        {
            return result;
        }
        if let Some(result) = self.check_list_concatenation(&left_ty, op, &right_ty, span) {
            return result;
        }
        if let Some(result) = self.check_generic_arithmetic(&left_ty, op, (right, &right_ty), span, expected_return_ty)
        {
            return result;
        }

        // Check both operands are numeric
        let lhs_num = numeric_ty_from_resolved(&left_ty);
        let rhs_num = numeric_ty_from_resolved(&right_ty);

        match (lhs_num, rhs_num) {
            (Some(_), Some(_)) => self.numeric_arithmetic_result_type((left, &left_ty), op, (right, &right_ty), span),
            // Allow Unknown with numeric partner (treat as that numeric type): an integer partner keeps its
            // own integer type, as same-type integer arithmetic does.
            (Some(n), None) if matches!(right_ty, ResolvedType::Unknown | ResolvedType::RustPath(_)) => match n {
                NumericTy::Int => left_ty.clone(),
                NumericTy::Float => ResolvedType::Float,
            },
            (None, Some(n)) if matches!(left_ty, ResolvedType::Unknown | ResolvedType::RustPath(_)) => match n {
                NumericTy::Int => right_ty.clone(),
                NumericTy::Float => ResolvedType::Float,
            },
            _ => {
                if let Some(method) = binary_operator_dunder(op) {
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

    /// Check text addition before numeric fallback, giving a source receiver's declared operator priority on a mix.
    pub(super) fn check_string_concatenation(
        &mut self,
        left_ty: &ResolvedType,
        op: BinaryOp,
        right: (&Spanned<Expr>, &ResolvedType),
        span: Span,
        expected: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        if !matches!(op, BinaryOp::Add) {
            return None;
        }
        let (right, right_ty) = right;
        let lhs_is_str = is_str_like(left_ty);
        let rhs_is_str = is_str_like(right_ty);
        if lhs_is_str && rhs_is_str {
            return Some(ResolvedType::Str);
        }
        if !lhs_is_str && !rhs_is_str {
            return None;
        }
        if self.is_user_operator_receiver(left_ty)
            && let Some(method) = binary_operator_dunder(op)
        {
            let args = vec![CallArg::Positional(right.clone())];
            let arg_types = vec![right_ty.clone()];
            if let Some(result) = self.resolve_operator_dunder(left_ty, method, &args, &arg_types, span, expected) {
                self.type_info
                    .record_resolved_operator_call(span, method, ResolvedOperatorKind::Binary);
                return Some(result);
            }
        }
        self.errors.push(errors::type_mismatch(
            "str",
            &format!("{} {} {}", left_ty, op, right_ty),
            span,
        ));
        Some(ResolvedType::Unknown)
    }

    /// Admit same-type runtime list addition only when its elements can be copied or cloned.
    pub(super) fn check_list_concatenation(
        &mut self,
        left_ty: &ResolvedType,
        op: BinaryOp,
        right_ty: &ResolvedType,
        span: Span,
    ) -> Option<ResolvedType> {
        if !matches!(op, BinaryOp::Add)
            || !is_runtime_list(left_ty)
            || !is_runtime_list(right_ty)
            || !self.types_compatible(left_ty, right_ty)
        {
            return None;
        }
        if let Some(element) = runtime_list_elem_type(left_ty)
            && !self.is_copy_type(element)
            && !self.is_clone_type(element)
        {
            self.errors
                .push(errors::list_concat_requires_clone(&element.to_string(), span));
            return Some(ResolvedType::Unknown);
        }
        Some(left_ty.clone())
    }

    /// Consult a declared generic operator before the existing same-placeholder arithmetic policy.
    ///
    /// Matching placeholders retain the existing arithmetic policy, which refuses unresolved numeric-only operators.
    /// Unknown operands keep the known generic partner's type; other shapes reach numeric or nominal resolution.
    pub(super) fn check_generic_arithmetic(
        &mut self,
        left_ty: &ResolvedType,
        op: BinaryOp,
        right: (&Spanned<Expr>, &ResolvedType),
        span: Span,
        expected: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        let (right, right_ty) = right;
        if self.is_generic_placeholder_type(left_ty)
            && let Some(method) = binary_operator_dunder(op)
        {
            let args = vec![CallArg::Positional(right.clone())];
            let arg_types = vec![right_ty.clone()];
            if let Some(result) = self.resolve_operator_dunder(left_ty, method, &args, &arg_types, span, expected) {
                self.type_info
                    .record_resolved_operator_call(span, method, ResolvedOperatorKind::Binary);
                return Some(result);
            }
        }
        match (
            self.generic_placeholder_name(left_ty),
            self.generic_placeholder_name(right_ty),
        ) {
            (Some(left_name), Some(right_name)) if left_name == right_name => {
                if let Some(dunder) = numeric_only_operator_dunder(op) {
                    self.errors.push(errors::operator_has_no_type_parameter_bound(
                        &op.to_string(),
                        left_name,
                        dunder,
                        span,
                    ));
                    return Some(ResolvedType::Unknown);
                }
                Some(left_ty.clone())
            }
            (Some(_), None) if matches!(right_ty, ResolvedType::Unknown) => Some(left_ty.clone()),
            (None, Some(_)) if matches!(left_ty, ResolvedType::Unknown) => Some(right_ty.clone()),
            _ => None,
        }
    }

    /// Prefer builtin integer bitwise behavior before resolving the selected overloadable operator.
    pub(super) fn check_overload_binary(
        &mut self,
        left: (&Spanned<Expr>, ResolvedType),
        op: BinaryOp,
        right: (&Spanned<Expr>, ResolvedType),
        span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        let (left, left_ty) = left;
        let (right, right_ty) = right;
        if matches!(
            op,
            BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor | BinaryOp::Shl | BinaryOp::Shr
        ) {
            // An unknown operand beside an integer one is treated as that integer's type, as it is for
            // arithmetic; a shift keeps its left operand's type whatever it counts with.
            match (numeric_ty_from_resolved(&left_ty), numeric_ty_from_resolved(&right_ty)) {
                (Some(NumericTy::Int), Some(NumericTy::Int)) => {
                    return self.integer_bitwise_result_type((left, &left_ty), op, (right, &right_ty), span);
                }
                (Some(NumericTy::Int), None) if matches!(right_ty, ResolvedType::Unknown) => {
                    return left_ty.clone();
                }
                (None, Some(NumericTy::Int)) if matches!(left_ty, ResolvedType::Unknown) => {
                    return if matches!(op, BinaryOp::Shl | BinaryOp::Shr) {
                        ResolvedType::Unknown
                    } else {
                        right_ty.clone()
                    };
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
        if let Some(ret) = self.resolve_operator_dunder(&left_ty, method, &args, &arg_types, span, expected_return_ty) {
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

    /// Adopt literal and constructor context before checking numeric, derived and explicit comparison contracts.
    pub(super) fn check_comparison_binary(
        &mut self,
        left: (&Spanned<Expr>, ResolvedType),
        op: BinaryOp,
        right: (&Spanned<Expr>, ResolvedType),
        span: Span,
    ) -> ResolvedType {
        let (left, left_ty) = left;
        let (right, right_ty) = right;
        let left_ty = self.adopt_partner_type_for_empty_list_operand(left, left_ty, &right_ty);
        let right_ty = self.adopt_partner_type_for_empty_list_operand(right, right_ty, &left_ty);
        let left_ty = self.adopt_partner_type_for_open_result_constructor_operand(left, left_ty, &right_ty);
        let right_ty = self.adopt_partner_type_for_open_result_constructor_operand(right, right_ty, &left_ty);
        if matches!(left_ty, ResolvedType::TypeToken(_)) || matches!(right_ty, ResolvedType::TypeToken(_)) {
            self.errors
                .push(errors::type_token_operation_not_supported("comparison", span));
            return ResolvedType::Unknown;
        }
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
        } else if matches!(op, BinaryOp::Eq | BinaryOp::NotEq)
            && left_ty.option_inner_type().is_some_and(is_str_like)
            && right_ty.option_inner_type().is_some_and(is_str_like)
        {
            // Equality observes text contents symmetrically; it does not assign or freeze either optional payload.
            ResolvedType::Bool
        } else if matches!(op, BinaryOp::Eq | BinaryOp::NotEq) && frozen_list_contents_comparable(&left_ty, &right_ty) {
            // Shared content equality does not require either list's element type to accept the other's storage.
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
            if let Some(ret) =
                self.resolve_operator_dunder(&left_ty, method, &args, &arg_types, span, Some(&ResolvedType::Bool))
            {
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

    /// Check container membership and retain any numeric widening destination for the written probe.
    pub(super) fn check_membership_binary(
        &mut self,
        left: (&Spanned<Expr>, ResolvedType),
        op: BinaryOp,
        right_ty: ResolvedType,
        span: Span,
    ) -> ResolvedType {
        let (left, left_ty) = left;
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
}

/// Admit equal-depth lists of frozen/owned text or bytes symmetrically, without broadening assignment or ordering.
fn frozen_list_contents_comparable(left: &ResolvedType, right: &ResolvedType) -> bool {
    let (ResolvedType::Generic(left_name, left_args), ResolvedType::Generic(right_name, right_args)) = (left, right)
    else {
        return false;
    };
    if collection_type_id(left_name.as_str()) != Some(CollectionTypeId::List)
        || collection_type_id(right_name.as_str()) != Some(CollectionTypeId::List)
        || left_args.len() != 1
        || right_args.len() != 1
    {
        return false;
    }
    let first = &left_args[0];
    let second = &right_args[0];
    (is_str_like(first) && is_str_like(second))
        || matches!(
            (first, second),
            (ResolvedType::Bytes, ResolvedType::FrozenBytes) | (ResolvedType::FrozenBytes, ResolvedType::Bytes)
        )
        || frozen_list_contents_comparable(first, second)
}

#[cfg(test)]
mod optional_text_tests {
    /// Equality accepts both directions of optional owned/frozen text without changing either payload type.
    #[test]
    fn optional_text_equality_is_symmetric() -> Result<(), String> {
        let source = "def same(left: Option[FrozenStr], right: Option[str]) -> bool:\n    return left == right and right == left and not (left != right) and not (right != left)\n";
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = crate::typechecker::TypeChecker::new();
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        Ok(())
    }

    /// Lists compare frozen and ordinary contents in both directions, including nested list shapes.
    #[test]
    fn frozen_list_equality_is_symmetric() -> Result<(), String> {
        for (left, right) in [
            ("list[FrozenStr]", "list[str]"),
            ("list[FrozenBytes]", "list[bytes]"),
            ("list[list[FrozenBytes]]", "list[list[bytes]]"),
        ] {
            let source = format!(
                "def same(left: {left}, right: {right}) -> bool:\n    return left == right and right == left and not (left != right) and not (right != left)\n"
            );
            let tokens = crate::lexer::lex(&source).map_err(|errors| format!("{errors:?}"))?;
            let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
            let mut checker = crate::typechecker::TypeChecker::new();
            checker
                .check_program(&program)
                .map_err(|errors| format!("{errors:?}"))?;
        }
        Ok(())
    }

    /// Text equality does not admit a different optional payload family or optional text ordering.
    #[test]
    fn optional_text_comparison_keeps_other_refusals() -> Result<(), String> {
        for source in [
            "def mixed(left: list[FrozenStr], right: list[int]) -> bool:\n    return left == right\n",
            "def mixed(left: list[FrozenBytes], right: list[str]) -> bool:\n    return left == right\n",
            "def ordered(left: list[str], right: list[FrozenStr]) -> bool:\n    return left < right\n",
            "def mixed(left: Option[FrozenStr], right: Option[int]) -> bool:\n    return left == right\n",
            "def ordered(left: Option[str], right: Option[FrozenStr]) -> bool:\n    return left < right\n",
        ] {
            let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
            let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
            let mut checker = crate::typechecker::TypeChecker::new();
            assert!(checker.check_program(&program).is_err());
        }
        Ok(())
    }
}

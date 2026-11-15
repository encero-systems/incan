//! Suffixed, contextual and exact numeric literal checking.

use super::*;

impl TypeChecker {
    /// Typecheck an explicitly suffixed integer token, preserving its named type and validating its signed value.
    pub(in crate::typechecker::check_expr) fn check_suffixed_int_literal(
        &mut self,
        value: &IntLiteral,
        negative: bool,
        span: Span,
    ) -> ResolvedType {
        let Some(target) = value.suffix else {
            return ResolvedType::Int;
        };
        let target_ty = ResolvedType::from_numeric_id(target);
        let fits = match integer_bounds(target) {
            Some(IntegerBounds::Signed { minimum, .. }) if negative => value.magnitude <= minimum.unsigned_abs(),
            Some(IntegerBounds::Signed { maximum, .. }) => value.magnitude <= maximum as u128,
            Some(IntegerBounds::Unsigned { .. }) if negative => false,
            Some(IntegerBounds::Unsigned { maximum }) => value.magnitude <= maximum,
            None if numerics::info_for(target).family == NumericFamily::BinaryFloat => match target {
                NumericTypeId::F32 => (value.magnitude as f32).is_finite(),
                NumericTypeId::F64 => (value.magnitude as f64).is_finite(),
                _ => false,
            },
            None => false,
        };
        if !fits {
            let spelling = if negative {
                format!("-{}", value.repr)
            } else {
                value.repr.clone()
            };
            self.errors.push(CompileError::type_error(
                format!("Numeric literal {spelling} does not fit in {target_ty}"),
                span,
            ));
        }
        target_ty
    }

    /// Typecheck an explicitly suffixed float token in the suffix's binary-float type.
    ///
    /// An `f32` literal must be finite in `f32`. An `f64` literal is a `float`, which holds IEEE infinity, so it takes
    /// every value an unsuffixed float literal takes.
    pub(super) fn check_suffixed_float_literal(&mut self, value: &FloatLiteral, span: Span) -> ResolvedType {
        let Some(target) = value.suffix else {
            return ResolvedType::Float;
        };
        let target_ty = ResolvedType::from_numeric_id(target);
        let fits = match target {
            NumericTypeId::F32 => value.value.is_finite() && value.value.abs() <= f64::from(f32::MAX),
            NumericTypeId::F64 => true,
            _ => false,
        };
        if !fits {
            self.errors.push(CompileError::type_error(
                format!("Float literal {} does not fit in {target_ty}", value.repr),
                span,
            ));
        }
        target_ty
    }

    /// Typecheck an integer literal in a known numeric target context.
    pub(super) fn check_int_literal_with_expected(
        &mut self,
        expr: &Spanned<Expr>,
        expected_ty: &ResolvedType,
    ) -> ResolvedType {
        let Some(target) = super::numeric_type_id_for_compat(expected_ty) else {
            return self.check_expr(expr);
        };
        if matches!(target, incan_lang::lang::types::numerics::NumericTypeId::U128) {
            if unsigned_int_literal_magnitude(expr).is_some() {
                return expected_ty.clone();
            }
            self.errors.push(CompileError::type_error(
                format!(
                    "Integer literal does not fit in {expected_ty}; valid range is 0..={}",
                    u128::MAX
                ),
                expr.span,
            ));
            return expected_ty.clone();
        }
        if matches!(
            target,
            incan_lang::lang::types::numerics::NumericTypeId::F32
                | incan_lang::lang::types::numerics::NumericTypeId::F64
        ) {
            let finite = match signed_int_literal_value(expr) {
                Some(value) => match target {
                    incan_lang::lang::types::numerics::NumericTypeId::F32 => (value as f32).is_finite(),
                    incan_lang::lang::types::numerics::NumericTypeId::F64 => (value as f64).is_finite(),
                    _ => false,
                },
                None => unsigned_int_literal_magnitude(expr).is_some_and(|value| match target {
                    incan_lang::lang::types::numerics::NumericTypeId::F32 => (value as f32).is_finite(),
                    incan_lang::lang::types::numerics::NumericTypeId::F64 => (value as f64).is_finite(),
                    _ => false,
                }),
            };
            if !finite {
                self.errors.push(CompileError::type_error(
                    format!("Integer literal does not fit in {expected_ty}"),
                    expr.span,
                ));
            }
            return expected_ty.clone();
        }
        let Some(value) = signed_int_literal_value(expr) else {
            return self.check_expr(expr);
        };
        match integer_bounds(target) {
            Some(IntegerBounds::Signed { minimum, maximum }) if value < minimum || value > maximum => {
                self.errors.push(CompileError::type_error(
                    format!(
                        "Integer literal {value} does not fit in {expected_ty}; valid range is {minimum}..={maximum}"
                    ),
                    expr.span,
                ));
            }
            Some(IntegerBounds::Unsigned { maximum }) if value < 0 || (value as u128) > maximum => {
                self.errors.push(CompileError::type_error(
                    format!("Integer literal {value} does not fit in {expected_ty}; valid range is 0..={maximum}"),
                    expr.span,
                ));
            }
            _ => {}
        }
        expected_ty.clone()
    }

    /// Return the numeric type an integer literal takes at an `Option` or union destination: the one numeric type the
    /// destination holds, as `float` for `Option[float]`, `Option[Option[float]]` or `float | str` (#1859).
    ///
    /// A destination that is itself numeric is checked against directly, so it gives nothing here. Neither does one
    /// holding `int`, where the literal is an `int` already, nor one holding two other numeric types (`f32 | f64`),
    /// which does not say which of them the literal is.
    pub(super) fn integer_literal_numeric_destination(&self, expected: &ResolvedType) -> Option<ResolvedType> {
        if super::numeric_type_id_for_compat(expected).is_some() {
            return None;
        }
        let mut held = Vec::new();
        Self::collect_held_numeric_types(&self.expand_type_aliases(expected.clone()), &mut held);
        match held.as_slice() {
            [only] if !matches!(only, ResolvedType::Int) => Some(only.clone()),
            _ => None,
        }
    }

    /// Return `FrozenStr` when it is the only string storage a union or `Option` destination offers a string literal.
    pub(super) fn string_literal_frozen_destination(&self, expected: &ResolvedType) -> Option<ResolvedType> {
        let mut held = Vec::new();
        Self::collect_held_string_types(&self.expand_type_aliases(expected.clone()), &mut held);
        matches!(held.as_slice(), [ResolvedType::FrozenStr]).then_some(ResolvedType::FrozenStr)
    }

    /// Collect the distinct owned and frozen string types held inside `Option` layers and union members.
    pub(super) fn collect_held_string_types(ty: &ResolvedType, held: &mut Vec<ResolvedType>) {
        if matches!(ty, ResolvedType::Str | ResolvedType::FrozenStr) {
            if !held.contains(ty) {
                held.push(ty.clone());
            }
        } else if let Some(inner) = ty.option_inner_type() {
            Self::collect_held_string_types(inner, held);
        } else if let Some(members) = ty.union_members() {
            for member in members {
                Self::collect_held_string_types(member, held);
            }
        }
    }

    /// Collect, without duplicates, the numeric types `ty` is or holds inside `Option` layers and union members.
    pub(super) fn collect_held_numeric_types(ty: &ResolvedType, held: &mut Vec<ResolvedType>) {
        if super::numeric_type_id_for_compat(ty).is_some() {
            if !held.contains(ty) {
                held.push(ty.clone());
            }
        } else if let Some(inner) = ty.option_inner_type() {
            Self::collect_held_numeric_types(inner, held);
        } else if let Some(members) = ty.union_members() {
            for member in members {
                Self::collect_held_numeric_types(member, held);
            }
        }
    }

    /// Typecheck a binary-float literal in a known `f32` target context, where the literal must be finite in `f32`.
    pub(super) fn check_float_literal_with_expected(
        &mut self,
        expr: &Spanned<Expr>,
        expected_ty: &ResolvedType,
    ) -> ResolvedType {
        let Expr::Literal(Literal::Float(value)) = &expr.node else {
            return self.check_expr(expr);
        };
        let fits = match expected_ty {
            ResolvedType::Numeric(incan_lang::lang::types::numerics::NumericTypeId::F32) => {
                value.value.is_finite() && value.value.abs() <= f64::from(f32::MAX)
            }
            _ => true,
        };
        if !fits {
            self.errors.push(CompileError::type_error(
                format!("Float literal {} does not fit in {expected_ty}", value.repr),
                expr.span,
            ));
        }
        expected_ty.clone()
    }

    /// Reject an out-of-domain float literal nested beneath an `f32` arithmetic destination.
    ///
    /// Binary operands are otherwise checked independently, so the enclosing destination does not reach a literal
    /// such as `1e9999` in `1e9999 + 0.0`. This validation deliberately records no inferred type and does not make
    /// `float` (`f64`) finite-only; it only preserves the `f32` destination's literal-domain check and literal span.
    pub(in crate::typechecker::check_expr) fn validate_exact_float_literals_in_arithmetic(
        &mut self,
        expr: &Spanned<Expr>,
        expected_ty: &ResolvedType,
    ) {
        match &expr.node {
            Expr::Literal(Literal::Float(value)) => {
                let fits = match expected_ty {
                    ResolvedType::Numeric(incan_lang::lang::types::numerics::NumericTypeId::F32) => {
                        value.value.is_finite() && value.value.abs() <= f64::from(f32::MAX)
                    }
                    _ => return,
                };
                if !fits {
                    self.errors.push(CompileError::type_error(
                        format!("Float literal {} does not fit in {expected_ty}", value.repr),
                        expr.span,
                    ));
                }
            }
            Expr::Unary(UnaryOp::Neg, inner) | Expr::Paren(inner) => {
                self.validate_exact_float_literals_in_arithmetic(inner, expected_ty);
            }
            Expr::Binary(
                left,
                BinaryOp::Add
                | BinaryOp::Sub
                | BinaryOp::Mul
                | BinaryOp::Div
                | BinaryOp::FloorDiv
                | BinaryOp::Mod
                | BinaryOp::Pow,
                right,
            ) => {
                self.validate_exact_float_literals_in_arithmetic(left, expected_ty);
                self.validate_exact_float_literals_in_arithmetic(right, expected_ty);
            }
            _ => {}
        }
    }

    /// Validate a decimal literal against a known decimal precision and scale.
    pub(super) fn validate_decimal_literal_with_expected(&mut self, expr: &Spanned<Expr>, expected_ty: &ResolvedType) {
        let Expr::Literal(Literal::Decimal(value)) = &expr.node else {
            return;
        };
        let Some((precision, scale)) = decimal_precision_scale(expected_ty) else {
            return;
        };
        let Some((integer_digits, fractional_digits, total_digits)) = decimal_literal_digit_counts(value.body.as_str())
        else {
            self.errors.push(CompileError::type_error(
                format!("Decimal literal {} is not a plain decimal literal", value.repr),
                expr.span,
            ));
            return;
        };
        let max_integer_digits = precision - scale;
        if integer_digits > max_integer_digits {
            self.errors.push(CompileError::type_error(
                format!(
                    "Decimal literal {} has {integer_digits} integer digit(s), but {expected_ty} allows at most {max_integer_digits}"
                    , value.repr
                ),
                expr.span,
            ));
        }
        if fractional_digits > scale {
            self.errors.push(CompileError::type_error(
                format!(
                    "Decimal literal {} has {fractional_digits} fractional digit(s), but {expected_ty} allows at most {scale}"
                    , value.repr
                ),
                expr.span,
            ));
        }
        if total_digits > precision {
            self.errors.push(CompileError::type_error(
                format!(
                    "Decimal literal {} has {total_digits} total digit(s), but {expected_ty} allows at most {precision}",
                    value.repr
                ),
                expr.span,
            ));
        }
    }
}

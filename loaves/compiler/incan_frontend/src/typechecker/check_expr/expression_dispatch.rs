//! Expression checking entry points, type receivers, contextual calls, and surface and embedded expressions.

use super::*;

impl TypeChecker {
    // ========================================================================
    // Expressions
    // ========================================================================

    /// Validate an expression and return its resolved type.
    ///
    /// Dispatches to specialized helpers (`check_call`, `check_binary`, `check_match`, etc.) and accumulates errors.
    /// Returns [`ResolvedType::Unknown`] when the expression is invalid so checking can continue. Once a call has
    /// resolved its callee, its arguments for `mut` parameters whose changes reach the caller are recorded for the
    /// module-level `INCAN-T0117` decision.
    pub fn check_expr(&mut self, expr: &Spanned<Expr>) -> ResolvedType {
        self.refuse_mut_params_held_in(expr);
        let ty = match &expr.node {
            Expr::Ident(name) => self.check_ident(name, expr.span),
            Expr::Literal(lit) => self.check_literal(lit, expr.span),
            Expr::SelfExpr => self.check_self(expr.span),
            Expr::Binary(left, op, right) => self.check_binary(left, *op, right, expr.span),
            Expr::Unary(op, operand) => self.check_unary(*op, operand, expr.span),
            Expr::Call(callee, type_args, args) => {
                let errors_before = self.errors.len();
                let ty = self.check_call(callee, type_args, args, expr.span);
                self.retain_body_ir_reexport_call(expr, errors_before, &ty);
                self.record_mut_arguments(MutArgumentCallee::Function(callee), expr.span, args);
                self.retain_body_ir_empty_list_use(expr, errors_before);
                ty
            }
            Expr::Index(base, index) => self.check_index(base, index, expr.span),
            Expr::Slice(base, slice) => self.check_slice(base, slice, expr.span),
            Expr::Field(base, field) => self.check_field(base, field, expr.span),
            Expr::MethodCall(base, method, type_args, args) => {
                let errors_before = self.errors.len();
                let ty = self.check_method_call(base, method, type_args, args, expr.span);
                self.record_mut_arguments(MutArgumentCallee::Method { receiver: base, method }, expr.span, args);
                self.retain_body_ir_empty_list_use(expr, errors_before);
                self.retain_body_ir_string_slice_call(expr, errors_before);
                ty
            }
            Expr::Partial(partial) => self.check_partial_expr(partial, expr.span),
            Expr::Surface(surface_expr) => self.check_surface_expr(surface_expr, expr.span),
            Expr::Try(inner) => self.check_try(inner, expr.span),
            Expr::Match(subject, arms) => self.check_match(subject, arms, expr.span, None),
            Expr::If(if_expr) => self.check_if_expr(if_expr, expr.span),
            Expr::Loop(loop_expr) => self.check_loop_expr(loop_expr, None, expr.span),
            Expr::Generator(generator) => self.check_generator_expr(generator, expr.span),
            Expr::ListComp(comp) => self.check_list_comp(comp, expr.span),
            Expr::DictComp(comp) => self.check_dict_comp(comp, expr.span),
            Expr::Closure(params, body) => self.check_closure(params, body, expr.span),
            Expr::Tuple(elems) => self.check_tuple(elems),
            Expr::List(elems) => self.check_list(elems),
            Expr::Dict(entries) => self.check_dict(entries),
            Expr::Set(elems) => self.check_set(elems),
            Expr::Paren(inner) => self.check_expr(inner),
            Expr::Constructor(name, args) => self.check_constructor(name, args, expr.span),
            Expr::FString(parts) => {
                self.check_fstring_parts(parts);
                ResolvedType::Str
            }
            Expr::Yield(inner) => {
                let context = self.current_yield_context.clone();
                match context {
                    super::YieldContext::Disallowed => {
                        let yield_ty = inner
                            .as_ref()
                            .map(|inner| self.check_expr(inner))
                            .unwrap_or(ResolvedType::Unit);
                        self.errors.push(errors::yield_outside_generator(expr.span));
                        yield_ty
                    }
                    super::YieldContext::Fixture => inner
                        .as_ref()
                        .map(|inner| self.check_expr(inner))
                        .unwrap_or(ResolvedType::Unit),
                    super::YieldContext::Generator { element_ty } => {
                        if let Some(inner) = inner {
                            let yield_ty = self.check_expr_with_expected(inner, Some(&element_ty));
                            if self.types_compatible(&yield_ty, &element_ty) {
                                // A yielded item is written to the generator's element type as a returned value is
                                // to the return type: a narrower numeric is widened to it (RFC 009).
                                self.record_value_destination_if_compatible(inner.span, &yield_ty, &element_ty);
                            } else {
                                self.errors.push(errors::type_mismatch(
                                    &element_ty.to_string(),
                                    &yield_ty.to_string(),
                                    inner.span,
                                ));
                            }
                            yield_ty
                        } else {
                            self.errors.push(errors::generator_yield_requires_value(expr.span));
                            ResolvedType::Unknown
                        }
                    }
                }
            }
            Expr::Range {
                start,
                end,
                inclusive: _,
            } => self.check_range_expr(start, end),
            Expr::VocabBlock(block) => {
                self.errors.push(CompileError::type_error(
                    format!(
                        "Vocab expression declaration `{}` reached typechecking before desugaring",
                        block.keyword
                    ),
                    expr.span,
                ));
                ResolvedType::Unknown
            }
            Expr::Embedded(fragment) => self.check_embedded_fragment_expr(fragment),
        };

        // Record for downstream stages (lowering/codegen).
        self.record_expr_type(expr.span, ty.clone());
        self.retain_body_ir_result_value(expr, &ty);
        self.refuse_capturing_callables_held_in(expr);
        ty
    }

    /// Type-check an expression used as a type-owned receiver, such as `Type.method()` or `Enum.Variant`.
    pub fn check_type_receiver_expr(&mut self, expr: &Spanned<Expr>) -> ResolvedType {
        self.type_receiver_spans.push(expr.span);
        let ty = self.check_expr(expr);
        self.type_receiver_spans.pop();
        ty
    }

    /// Type-check a type-owned receiver with the destination type inherited from its enclosing method call.
    ///
    /// The caller must establish that the enclosing method returns `Self`; only that contract makes the enclosing
    /// destination a sound expectation for the receiver expression. Keeping the receiver-span marker active preserves
    /// static type and enum-member resolution while contextual generic inference walks an instance method chain.
    pub fn check_type_receiver_expr_with_expected(
        &mut self,
        expr: &Spanned<Expr>,
        expected: &ResolvedType,
    ) -> ResolvedType {
        self.type_receiver_spans.push(expr.span);
        let ty = self.check_expr_with_expected(expr, Some(expected));
        self.type_receiver_spans.pop();
        ty
    }

    /// Return whether a span identifies a type receiver.
    pub fn is_type_receiver_span(&self, span: Span) -> bool {
        self.type_receiver_spans
            .iter()
            .rev()
            .any(|&candidate| candidate == span)
    }

    /// Return whether a span identifies a type name being checked against an explicit `Type[T]` destination.
    pub fn is_type_token_value_span(&self, span: Span) -> bool {
        self.type_token_value_spans
            .iter()
            .rev()
            .any(|&candidate| candidate == span)
    }

    /// Type-check an expression with an expected destination type when one is already known.
    ///
    /// This is intentionally narrow: only expression forms that benefit from contextual typing without broad inference
    /// changes should use the hint. `None` denotes unit when its destination is unit, and otherwise retains its
    /// Option literal meaning. Calls record their `mut` arguments as [`Self::check_expr`] does.
    pub fn check_expr_with_expected(&mut self, expr: &Spanned<Expr>, expected: Option<&ResolvedType>) -> ResolvedType {
        let errors_before = self.errors.len();
        self.refuse_mut_params_held_in(expr);
        let literal_destination = self.contextual_literal_destination(expr, expected);
        let expected = literal_destination.as_ref().or(expected);
        let ty = match (&expr.node, expected) {
            (_, Some(ResolvedType::TypeVar(_))) => return self.check_expr(expr),
            (Expr::Paren(inner), Some(expected_ty)) => self.check_expr_with_expected(inner, Some(expected_ty)),
            (Expr::Ident(_), Some(ResolvedType::TypeToken(_))) => {
                self.type_token_value_spans.push(expr.span);
                let ty = self.check_expr(expr);
                self.type_token_value_spans.pop();
                ty
            }
            (Expr::Literal(Literal::None), Some(ResolvedType::Unit)) => ResolvedType::Unit,
            (Expr::Literal(Literal::None), Some(expected_ty)) if expected_ty.option_inner_type().is_some() => {
                expected_ty.clone()
            }
            (Expr::Literal(literal @ Literal::Int(value)), _) if value.suffix.is_some() => {
                self.check_literal(literal, expr.span)
            }
            (Expr::Literal(literal @ Literal::Float(value)), _) if value.suffix.is_some() => {
                self.check_literal(literal, expr.span)
            }
            (Expr::Unary(UnaryOp::Neg, inner), _) if matches!(&inner.node, Expr::Literal(Literal::Int(value)) if value.suffix.is_some()) => {
                self.check_unary_with_expected(UnaryOp::Neg, inner, expr.span, expected)
            }
            (Expr::Literal(Literal::Int(_)), Some(expected_ty))
                if super::numeric_type_id_for_compat(expected_ty).is_some() =>
            {
                self.check_int_literal_with_expected(expr, expected_ty)
            }
            (Expr::Unary(UnaryOp::Neg, inner), Some(expected_ty))
                if super::numeric_type_id_for_compat(expected_ty).is_some()
                    && matches!(inner.node, Expr::Literal(Literal::Int(_))) =>
            {
                self.check_int_literal_with_expected(expr, expected_ty)
            }
            (Expr::Literal(Literal::Float(_)), Some(expected_ty))
                if matches!(
                    expected_ty,
                    ResolvedType::Numeric(incan_lang::lang::types::numerics::NumericTypeId::F32)
                ) =>
            {
                self.check_float_literal_with_expected(expr, expected_ty)
            }
            (Expr::Literal(Literal::Decimal(_)), Some(expected_ty)) if is_decimal_type(expected_ty) => {
                self.validate_decimal_literal_with_expected(expr, expected_ty);
                expected_ty.clone()
            }
            (Expr::Literal(Literal::String(_)), Some(expected_ty)) if is_frozen_str(expected_ty) => expected_ty.clone(),
            (Expr::Literal(Literal::Bytes(_)), Some(expected_ty)) if is_frozen_bytes(expected_ty) => {
                expected_ty.clone()
            }
            (Expr::Binary(left, op, right), Some(expected_ty)) => {
                self.check_binary_with_expected(left, *op, right, expr.span, Some(expected_ty))
            }
            (Expr::Unary(UnaryOp::Neg, operand), Some(expected_ty))
                if matches!(
                    expected_ty,
                    ResolvedType::Numeric(incan_lang::lang::types::numerics::NumericTypeId::F32)
                ) && matches!(operand.node, Expr::Literal(Literal::Float(_))) =>
            {
                self.check_expr_with_expected(operand, Some(expected_ty));
                expected_ty.clone()
            }
            (Expr::Unary(op, operand), Some(expected_ty)) => {
                self.check_unary_with_expected(*op, operand, expr.span, Some(expected_ty))
            }
            (Expr::Try(inner), Some(expected_ty)) => self.check_try_with_expected(inner, expr.span, Some(expected_ty)),
            (Expr::Call(callee, type_args, args), Some(expected_ty)) => {
                self.check_call_with_contextual_facts(expr, callee, type_args, args, expected_ty)
            }
            (Expr::MethodCall(base, method, type_args, args), Some(expected_ty)) => {
                let errors_before = self.errors.len();
                let ty =
                    self.check_method_call_with_expected(base, method, type_args, args, expr.span, Some(expected_ty));
                self.record_mut_arguments(MutArgumentCallee::Method { receiver: base, method }, expr.span, args);
                self.retain_body_ir_empty_list_use(expr, errors_before);
                self.retain_body_ir_string_slice_call(expr, errors_before);
                ty
            }
            (Expr::Tuple(items), Some(ResolvedType::Unit)) if items.is_empty() => ResolvedType::Unit,
            (Expr::Tuple(items), Some(expected_ty)) => self.check_tuple_with_expected(items, expected_ty),
            (Expr::Closure(params, body), Some(ResolvedType::Function(expected_params, expected_ret))) => {
                self.check_closure_with_expected(params, body, expected_params, expected_ret, expr.span)
            }
            (Expr::Closure(params, body), Some(ResolvedType::Generic(base, members))) if base == "Union" => {
                self.check_closure_with_union_expected(params, body, members, expr.span)
            }
            (Expr::List(elems), expected_ty) => self.check_list_with_expected(elems, expected_ty),
            (Expr::Dict(entries), expected_ty) => self.check_dict_with_expected(entries, expected_ty),
            (Expr::Set(elems), expected_ty) => self.check_set_with_expected(elems, expected_ty),
            (Expr::Loop(loop_expr), expected_ty) => self.check_loop_expr(loop_expr, expected_ty, expr.span),
            (Expr::Match(subject, arms), Some(expected_ty)) => {
                self.check_match(subject, arms, expr.span, Some(expected_ty))
            }
            _ => {
                let mut ty = self.check_expr(expr);
                if self.errors.len() == errors_before
                    && let Some(expected) = expected
                {
                    self.constrain_open_value_destination(expr.span, expected);
                    // A typed destination can fix a watched local's open owner arguments (#2481).
                    if let Some(fixed) = self.fix_open_rust_binding_by_destination(expr, expected) {
                        ty = fixed;
                    }
                }
                return self.retain_declared_conversion(expr, &ty, expected);
            }
        };

        // A literal whose elements already failed to check is not refused a second time for its open type.
        if matches!(expr.node, Expr::List(_) | Expr::Dict(_) | Expr::Tuple(_)) && self.errors.len() == errors_before {
            self.refuse_collection_literal_without_one_member(&ty, expected, expr.span);
        }
        self.record_expr_type(expr.span, ty.clone());
        self.retain_body_ir_result_value(expr, &ty);
        self.retain_body_ir_value_destination(expr, &ty, expected);
        self.refuse_capturing_callables_held_in(expr);
        self.retain_declared_conversion(expr, &ty, expected)
    }

    /// Check a contextually typed function call and retain its re-export, mutable-argument and empty-list facts
    /// against the call's own diagnostic checkpoint, before the surrounding expression publishes its result.
    pub(super) fn check_call_with_contextual_facts(
        &mut self,
        expr: &Spanned<Expr>,
        callee: &Spanned<Expr>,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        expected: &ResolvedType,
    ) -> ResolvedType {
        let errors_before = self.errors.len();
        let ty = self.check_call_with_expected(callee, type_args, args, expr.span, Some(expected));
        self.retain_body_ir_reexport_call(expr, errors_before, &ty);
        self.record_mut_arguments(MutArgumentCallee::Function(callee), expr.span, args);
        self.retain_body_ir_empty_list_use(expr, errors_before);
        ty
    }

    /// Select the one numeric or frozen string member a literal can take from an Option or union destination.
    /// Other expressions keep the original expected type; this hint does not broaden inference to non-literals.
    pub(super) fn contextual_literal_destination(
        &self,
        expr: &Spanned<Expr>,
        expected: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        match (&expr.node, expected) {
            (Expr::Literal(Literal::Int(_)), Some(expected_ty)) => {
                self.integer_literal_numeric_destination(expected_ty)
            }
            (Expr::Unary(UnaryOp::Neg, inner), Some(expected_ty))
                if matches!(inner.node, Expr::Literal(Literal::Int(_))) =>
            {
                self.integer_literal_numeric_destination(expected_ty)
            }
            (Expr::Literal(Literal::String(_)), Some(expected_ty)) => {
                self.string_literal_frozen_destination(expected_ty)
            }
            _ => None,
        }
    }

    /// Typecheck a surface expression via the semantics registry.
    pub(super) fn check_surface_expr(&mut self, expr: &SurfaceExpr, span: Span) -> ResolvedType {
        use crate::semantics_registry::semantics_registry;

        let Some(action) = semantics_registry().typecheck_surface_expr_action(&expr.key) else {
            // No pack claimed this surface expression — report as unknown.
            let label = match &expr.key {
                incan_semantics_core::SurfaceFeatureKey::SoftKeyword(id) => keywords::as_str(*id).to_string(),
                incan_semantics_core::SurfaceFeatureKey::Decorator(_) => "decorator-surface-feature".to_string(),
                incan_semantics_core::SurfaceFeatureKey::ScopedDslSurface {
                    dependency_key,
                    descriptor_key,
                } => {
                    format!("{dependency_key}:{descriptor_key}")
                }
            };
            self.errors.push(errors::unknown_symbol(&label, span));
            return ResolvedType::Unknown;
        };

        match (action, &expr.payload) {
            (SurfaceExprTypeCheck::AwaitCheck, SurfaceExprPayload::PrefixUnary(inner)) => self.check_await(inner, span),
            (SurfaceExprTypeCheck::RaceForCheck, SurfaceExprPayload::RaceFor(race)) => self.check_race_for(race, span),
            _ => ResolvedType::Unknown,
        }
    }

    /// Typecheck a descriptor-gated embedded-fragment expression (RFC 081, `#1023`).
    ///
    /// Unlike [`TypeChecker::check_surface_expr`], this node is never eliminated by the pre-typecheck vocab
    /// desugar pass (`loaves/compiler/incan_frontend/src/vocab_desugar_pass/rewrite.rs`) — it must reach `check_expr`
    /// as itself, because its [`EmbeddedNode::Hole`] sub-expressions are genuine Incan expressions that need real
    /// types, exactly as if they appeared in ordinary expression position. The surrounding structural content
    /// (tags, selectors, declarations, regex/type shapes, ...) is DSL-owned syntax with no ordinary Incan type of
    /// its own — its runtime meaning is supplied by the owning DSL's desugarer or lowering hook (RFC 081
    /// §Semantics), which is downstream of `#1023`. The fragment as a whole therefore resolves to
    /// `ResolvedType::Unknown`, deliberately reusing the existing "no further ordinary-Incan meaning to check here"
    /// sentinel rather than introducing a new `ResolvedType` variant that every exhaustive match over
    /// `ResolvedType` across the compiler would need to handle for a type with no ordinary-Incan operations anyway.
    pub(super) fn check_embedded_fragment_expr(&mut self, fragment: &EmbeddedFragmentExpr) -> ResolvedType {
        for hole in fragment.holes() {
            self.check_expr(hole);
        }
        ResolvedType::Unknown
    }
}

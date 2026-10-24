//! Lower binary operators and checked truthiness, length and operator hooks without rediscovering their dispatch.

use incan_lang::lang::types::numerics::{self, NumericTypeId};
use incan_lang::numeric_values::common_lossless_integer_type;

use super::args::*;
use super::primitives::*;
use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Lower an operator value and capture a scalar storage read before evaluating the next operand.
    fn lower_binary_operand(
        &mut self,
        expression: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let value = self.lower_expr_to_operand(expression, scope, out);
        self.retain_evaluated_value_read(expression, value, scope, out)
    }

    /// Capture scalar projections and owned text at their evaluation point before a later operand can borrow their
    /// owner or consume its final read. Whole scalar locals and already captured temporaries need no snapshot.
    /// Mutable arguments and live collection handles retain their separate borrowing contracts. Global enum reads
    /// still require the admitted declaration identity; numeric, boolean and text carriers come from the checked
    /// expression.
    pub(super) fn retain_evaluated_value_read(
        &mut self,
        expression: &ast::Spanned<ast::Expr>,
        value: bir::Operand,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let bir::Operand::Place(read) = &value else {
            return value;
        };
        let ty = self.resolve_ty(expression.span);
        let global = read.place.global().is_some();
        if !global && read.place.projection.is_empty() {
            let temporary = read
                .place
                .local_id()
                .and_then(|local| self.locals.get(local.index()))
                .is_some_and(|local| matches!(local.origin, bir::LocalOrigin::Temporary));
            if temporary || !matches!(ty, IncanType::Primitive(IncanPrimitiveType::Str)) {
                return value;
            }
        }
        if numeric_carrier_id(&ty).is_some()
            || matches!(
                ty,
                IncanType::Primitive(IncanPrimitiveType::Bool | IncanPrimitiveType::Str)
            )
        {
            return self.push_assign_temp(bir::Rvalue::Use(value), ty, scope, hir_span(expression.span), out);
        }
        if !global {
            return value;
        }
        let IncanType::Named(name) = &ty else {
            return value;
        };
        let Some(identity) = self.type_info.declarations.named_type_identities.get(name).or_else(|| {
            self.type_info
                .expressions
                .expression_type_identities
                .get(&expression.span)?
                .get(&Vec::new())
        }) else {
            return value;
        };
        let local = self
            .local_enum_declarations
            .values()
            .any(|declaration| declaration.canonical == *identity);
        let loaf = super::is_loaf_identity(identity) && identity.kind == SemanticSourceTargetKind::Enum;
        if !local && !loaf {
            return value;
        }
        self.push_assign_temp(bir::Rvalue::Use(value), ty, scope, hir_span(expression.span), out)
    }

    /// Lower a binary-operator expression. Bails out to an explicit unsupported placeholder *before* evaluating
    /// either operand when `op` has no Body IR v0 handling at all (see [`Self::binary_op_is_supported`]), so an
    /// unsupported operator's sub-expressions are never partially lowered. Source boolean operators retain conditional
    /// evaluation through [`Self::lower_boolean_short_circuit`]. Other operators defer to
    /// [`Self::lower_binary_from_operands`] for string-helper-or-plain-binop emission, shared with
    /// [`Self::lower_compound_assignment`]. Builtin collection equality retains shared reads, matching legacy's
    /// `PartialEq` receivers rather than introducing observable element clones.
    pub(super) fn lower_binary(
        &mut self,
        lhs: &ast::Spanned<ast::Expr>,
        op: ast::BinaryOp,
        rhs: &ast::Spanned<ast::Expr>,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let lhs_ty = self.resolve_ty(lhs.span);
        let rhs_ty = self.resolve_ty(rhs.span);
        let result_ty = self.resolve_ty(span);

        // Legacy represents membership as a receiver-first method call, negating it for `not in`.
        if let Some(dispatch) = self.type_info.resolved_operator_call(span)
            && dispatch.kind == ResolvedOperatorKind::Contains
        {
            let method = dispatch.method.clone();
            let value = self.lower_operator_dispatch(&method, rhs, &[lhs], result_ty.clone(), span, scope, out);
            return if op == ast::BinaryOp::NotIn {
                self.push_assign_temp(
                    bir::Rvalue::UnaryOp(bir::UnOp::Not, value),
                    result_ty,
                    scope,
                    hir_span_value,
                    out,
                )
            } else {
                value
            };
        }

        // A user-defined operator is a method call, not a primitive operation. The typechecker already resolved
        // which dunder this spelling dispatches to, so lowering follows that decision rather than falling through
        // to the primitive operator set -- which would represent `a + b` on two `Vec2` values as machine addition.
        if let Some(dispatch) = self.type_info.resolved_operator_call(span)
            && dispatch.kind == ResolvedOperatorKind::Binary
        {
            let method = dispatch.method.clone();
            return self.lower_operator_dispatch(&method, lhs, &[rhs], result_ty, span, scope, out);
        }

        if !Self::binary_op_is_supported(op, &lhs_ty, &rhs_ty) {
            return self.unsupported_operand(format!("binary operator {op:?}"), scope, hir_span_value, out);
        }
        if matches!(op, ast::BinaryOp::And | ast::BinaryOp::Or) {
            return self.lower_boolean_short_circuit(lhs, op, rhs, scope, hir_span_value, out);
        }
        let contextual_types = if matches!(op, ast::BinaryOp::Eq | ast::BinaryOp::NotEq) {
            self.retain_comparison_field_carriers(lhs, rhs)
        } else {
            Default::default()
        };
        let lhs_ty = self.resolve_ty(lhs.span);
        let rhs_ty = self.resolve_ty(rhs.span);
        let (lhs_operand, rhs_operand) = if collection_equality_borrows(op, &lhs_ty, &rhs_ty) {
            (
                self.lower_collection_equality_operand(lhs, scope, out),
                self.lower_collection_equality_operand(rhs, scope, out),
            )
        } else if is_string_like(&lhs_ty)
            && is_string_like(&rhs_ty)
            && matches!(string_helper_for_binop(op), Some(helper) if !matches!(helper, bir::HelperOp::StrConcat))
        {
            (
                self.lower_text_comparison_operand(lhs, scope, out),
                self.lower_text_comparison_operand(rhs, scope, out),
            )
        } else {
            (
                self.lower_binary_operand(lhs, scope, out),
                self.lower_binary_operand(rhs, scope, out),
            )
        };
        self.restore_constructor_argument_carriers(contextual_types);
        self.lower_binary_from_operands(
            op,
            &lhs_ty,
            lhs_operand,
            &rhs_ty,
            rhs_operand,
            result_ty,
            scope,
            hir_span_value,
            out,
        )
    }

    /// Borrow local text for a non-consuming comparison while capturing global and projected values before later
    /// effects. The read still decrements its ordinary countdown, but cannot mark the compared owner as transferred
    /// along a short-circuit branch that falls through to another comparison.
    fn lower_text_comparison_operand(
        &mut self,
        expression: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let mut value = self.lower_expr_to_operand(expression, scope, out);
        if let bir::Operand::Place(read) = &mut value
            && read.place.global().is_none()
            && read.place.projection.is_empty()
        {
            self.retain_argument_borrow(read, false);
            return value;
        }
        self.retain_evaluated_value_read(expression, value, scope, out)
    }

    /// Materialize one comparison place and retain its shared read without marking source storage as moved.
    ///
    /// A bare binding's comparison still counts as a read, so a subsequent final value use can move instead of
    /// acquiring an extra clone. Projected reads retain the existing conservative partial-move policy.
    fn lower_collection_equality_operand(
        &mut self,
        expr: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let place = self.lower_expr_to_place(expr, scope, out);
        if place.projection.is_empty()
            && let Some(local) = place.local_id()
            && let Some(remaining) = self.remaining_reads.get_mut(&local)
        {
            *remaining = remaining.saturating_sub(1);
        }
        bir::Operand::place(place, bir::OwnershipFact::Borrow, false)
    }

    /// Retain boolean evaluation order as control flow before flattening the right operand.
    ///
    /// Both successors initialize one boolean temporary. Calls, panics, and mutations in the right operand stay
    /// inside the selected branch; downstream consumers need no source-expression reconstruction.
    fn lower_boolean_short_circuit(
        &mut self,
        lhs: &ast::Spanned<ast::Expr>,
        op: ast::BinaryOp,
        rhs: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let cond = self.lower_expr_to_operand(lhs, scope, out);
        let local = self.new_temp(IncanType::Primitive(IncanPrimitiveType::Bool), scope, span);
        let branch_scope = self.new_scope(Some(scope), span);
        let enclosing_bindings = self.bindings.clone();
        let range_layouts_before = self.materialized_range_locals.clone();
        let mut evaluated = Vec::new();
        let value = self.lower_expr_to_operand(rhs, branch_scope, &mut evaluated);
        evaluated.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(local),
                rvalue: bir::Rvalue::Use(value),
            },
            span,
        });
        self.insert_scope_drops(&mut evaluated, branch_scope);
        self.bindings = enclosing_bindings;
        self.materialized_range_locals
            .retain(|local| range_layouts_before.contains(local));
        let skipped = bir::Block {
            scope,
            stmts: vec![bir::Statement {
                kind: bir::StatementKind::Assign {
                    place: bir::Place::from_local(local),
                    rvalue: bir::Rvalue::Use(bir::Operand::Constant(bir::Constant::Bool(op == ast::BinaryOp::Or))),
                },
                span,
            }],
        };
        let evaluated = bir::Block {
            scope: branch_scope,
            stmts: evaluated,
        };
        let (then_block, else_block) = if op == ast::BinaryOp::And {
            (evaluated, skipped)
        } else {
            (skipped, evaluated)
        };
        out.push(bir::Statement {
            kind: bir::StatementKind::If {
                cond,
                then_block,
                else_block: Some(else_block),
            },
            span,
        });
        self.temp_operand(local, &IncanType::Primitive(IncanPrimitiveType::Bool))
    }

    /// Lower a user-defined operator to the dunder method call the typechecker resolved for it.
    ///
    /// RFC 028 lets a type define `__add__`, `__getitem__`, `__invert__` and friends, and the typechecker records
    /// which method one operator spelling dispatches to, with that method's identity at the operator's span. Body IR
    /// must follow that decision: representing `a + b` on two `Vec2` values as [`bir::BinOp::Add`] would claim a
    /// primitive machine operation where the source calls a method, which is a wrong representation rather than an
    /// honest refusal — no `Unsupported` marker, nothing for a consumer to notice.
    ///
    /// The operand the operator reads first becomes the receiver, borrowed and evaluated before the arguments,
    /// matching how [`Self::lower_method_call`] arranges an ordinary method call: `args[0]` is the receiver.
    /// Checking resolved the dunder against exactly these positional arguments, so they fill its declared slots in
    /// order. Checked by-value parameter slots capture scalar/text global reads before later argument effects; mutable
    /// or unknown slots keep their place behavior. An operator whose identity checking did not record keeps no
    /// canonical target and stays refused.
    ///
    /// Hooks on generic owners keep their checked identities; frame specialization closes their receivers, and
    /// f-string lowering retains the original type-parameter display contract before that specialization.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_operator_dispatch(
        &mut self,
        method: &str,
        receiver: &ast::Spanned<ast::Expr>,
        arguments: &[&ast::Spanned<ast::Expr>],
        result_ty: IncanType,
        operator_span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        // Source evaluation observes the receiver before the arguments, exactly as for a written method call.
        let receiver_place = self.lower_expr_to_place(receiver, scope, out);
        let mut elements = vec![bir::ArgumentElement::One(bir::Operand::place(
            receiver_place,
            bir::OwnershipFact::Borrow,
            false,
        ))];
        for (index, argument) in arguments.iter().enumerate() {
            let by_value = self
                .type_info
                .body_ir_newtypes
                .callable_params(operator_span, self.type_info)
                .or_else(|| self.type_info.body_ir_call_site_callable_params(operator_span))
                .and_then(|parameters| parameters.get(index))
                .is_some_and(|parameter| !parameter.is_mut);
            let mut operand = self.lower_expr_to_operand(argument, scope, out);
            if by_value {
                operand = self.owned_parameter_operand(argument, operand);
                operand = self.retain_evaluated_value_read(argument, operand, scope, out);
            }
            elements.push(bir::ArgumentElement::One(operand));
        }
        let target = self.checked_operator_target(method, arguments.len(), operator_span);
        self.push_call_temp(
            bir::Callee::Method(target),
            elements,
            result_ty,
            scope,
            hir_span(operator_span),
            false,
            out,
        )
    }

    /// Retain the checked operator's method identity and positional slots; an absent identity remains unproven.
    fn checked_operator_target(&self, method: &str, arguments: usize, span: ast::Span) -> bir::MethodTarget {
        bir::MethodTarget {
            native_trait_delegation: None,
            name: method.to_string(),
            intrinsic_carrier: None,
            reflection: None,
            loaf_trait: None,
            trait_arguments: self
                .checked_trait_arguments(self.type_info.resolved_method_call(span).map(|call| &call.dispatch)),
            canonical: self
                .type_info
                .resolved_identity(span)
                .filter(|identity| identity.kind == SemanticSourceTargetKind::Method)
                .cloned(),
            type_args: Vec::new(),
            binding: bir::ArgumentBinding::resolved_positional(arguments),
        }
    }

    /// Lower a checked slice hook as a receiver-first call with three closed Option[int] parts.
    ///
    /// The checker has already validated the slice contract and recorded its selected method. Written parts are
    /// evaluated once in start/end/step order, after the receiver; omitted parts retain typed None storage. Builtin
    /// slices never enter this path and keep their separate projection and evaluation contract.
    #[allow(clippy::too_many_arguments)]
    fn lower_slice_hook_dispatch(
        &mut self,
        method: &str,
        receiver: &ast::Spanned<ast::Expr>,
        slice: &ast::SliceExpr,
        result_ty: IncanType,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        // ---- Receiver: evaluate before the written slice parts ----
        let receiver = self.lower_expr_to_place(receiver, scope, out);
        let mut arguments = vec![bir::ArgumentElement::One(bir::Operand::place(
            receiver,
            bir::OwnershipFact::Borrow,
            false,
        ))];

        // ---- Parts: retain three typed Option slots ----
        let option_int = IncanType::Generic {
            base: "Option".to_string(),
            args: vec![IncanType::Primitive(IncanPrimitiveType::Int)],
        };
        for part in [&slice.start, &slice.end, &slice.step] {
            let value = match part {
                Some(expression) => {
                    let payload = self.lower_expr_to_operand(expression, scope, out);
                    bir::Rvalue::Aggregate(bir::AggregateKind::OptionSome, fixed_elements(vec![payload]))
                }
                None => bir::Rvalue::Use(bir::Operand::Constant(bir::Constant::None)),
            };
            let part = self.push_assign_temp(value, option_int.clone(), scope, hir_span(span), out);
            arguments.push(bir::ArgumentElement::One(part));
        }

        // ---- Dispatch: honor the checked source method ----
        let target = self.checked_operator_target(method, 3, span);
        self.push_call_temp(
            bir::Callee::Method(target),
            arguments,
            result_ty,
            scope,
            hir_span(span),
            false,
            out,
        )
    }

    /// Lower a unary, index, slice, length or identifier truthiness expression through its checked source method.
    ///
    /// `~value` and `-value` call the operand's hook with no arguments; `base[index]` calls `__getitem__` on the base
    /// with the index. A slice supplies three checked Option[int] parts. Length uses its single checked receiver.
    /// Truthiness needs the separately retained receiver identity and returns bool; other truthiness receiver
    /// shapes remain named unsupported. Every other expression returns `None` and keeps its ordinary lowering.
    pub(super) fn lower_operator_hook_expr(
        &mut self,
        expr: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let dispatch = self.type_info.resolved_operator_call(expr.span)?;
        let (method, kind) = (dispatch.method.clone(), dispatch.kind);
        let ty = self.resolve_ty(expr.span);
        match (&expr.node, kind) {
            (ast::Expr::Ident(_), ResolvedOperatorKind::Truthiness)
                if self
                    .type_info
                    .protocols
                    .body_ir_truthiness_receivers
                    .contains_key(&expr.span) =>
            {
                let bool_ty = IncanType::Primitive(IncanPrimitiveType::Bool);
                Some(self.lower_operator_dispatch(&method, expr, &[], bool_ty, expr.span, scope, out))
            }
            (_, ResolvedOperatorKind::Truthiness) => Some(self.unsupported_operand(
                "truthiness receiver without a retained identifier value".to_string(),
                scope,
                hir_span(expr.span),
                out,
            )),
            (ast::Expr::Call(_, type_args, args), ResolvedOperatorKind::Len) if type_args.is_empty() => {
                let [ast::CallArg::Positional(receiver)] = args.as_slice() else {
                    return Some(self.unsupported_operand(
                        "length protocol call without one retained positional receiver".to_string(),
                        scope,
                        hir_span(expr.span),
                        out,
                    ));
                };
                Some(self.lower_operator_dispatch(&method, receiver, &[], ty, expr.span, scope, out))
            }
            (ast::Expr::Unary(_, operand), ResolvedOperatorKind::Unary) => {
                Some(self.lower_operator_dispatch(&method, operand, &[], ty, expr.span, scope, out))
            }
            (ast::Expr::Index(base, index), ResolvedOperatorKind::Index) => {
                Some(self.lower_operator_dispatch(&method, base, &[&**index], ty, expr.span, scope, out))
            }
            (ast::Expr::Slice(base, slice), ResolvedOperatorKind::Slice) => {
                Some(self.lower_slice_hook_dispatch(&method, base, slice, ty, expr.span, scope, out))
            }
            _ => None,
        }
    }

    /// Whether `op` between operands of `lhs_ty`/`rhs_ty` has any Body IR v0 handling (either the string-helper
    /// path or a direct [`bir::BinOp`] mapping). Both operand facts must be resolved: a primitive Body IR operation
    /// may carry a typechecker's decision forward, but it may not guess one around `Unknown` or `Infer`. Checked
    /// *before* evaluating operand sub-expressions in both [`Self::lower_binary`] and
    /// [`Self::lower_compound_assignment`], so an operator that v0 does not model never causes its operands' side
    /// effects (calls, reads) to be lowered on the way to an unsupported placeholder.
    pub(super) fn binary_op_is_supported(op: ast::BinaryOp, lhs_ty: &IncanType, rhs_ty: &IncanType) -> bool {
        if !binary_operand_types_are_resolved(lhs_ty, rhs_ty) {
            return false;
        }
        if is_string_like(lhs_ty) && is_string_like(rhs_ty) && string_helper_for_binop(op).is_some() {
            return true;
        }
        if collection_helper_for_binop(op, lhs_ty, rhs_ty).is_some() {
            return true;
        }
        // A builtin collection with no collection helper is *not* refused here. `==` on two lists is the case that
        // settles it: the Rust-emission backend routes list `+` to `incan_std_core::collections::list_concat`, which
        // is why representing it as `BinOp::Add` contradicted that backend -- but it emits comparisons as an infix
        // operator, landing on Rust's `PartialEq` for `Vec`, which is also what Python's element-wise `==` means.
        // A primitive is therefore the faithful representation there, and refusing it would invent a divergence
        // between the two backends rather than close one. An operator that genuinely needs a runtime call earns a
        // helper in `collection_helper_for_binop`, and the check above admits it.
        lower_binary_op(op).is_some()
    }

    /// Emit the result of a binary operator given already-lowered operands, preferring an explicit
    /// [`bir::Callee::Helper`] call (with runtime requirements recorded) over a primitive whenever the operand types
    /// make the operator a runtime operation -- Body IR's compiler-owned-runtime-operation requirement (#653
    /// criterion 3). Two helper families qualify: string operands with a [`string_helper_for_binop`] mapping, and
    /// builtin collections with a [`collection_helper_for_binop`] mapping. Everything else becomes a plain
    /// [`bir::Rvalue::BinaryOp`], with a division/modulo panic fact recorded when [`bir::BinOp::may_panic`] holds. A
    /// primitive whose checked result is a float carrier first assigns each mixed operand to a temporary of that
    /// carrier (see [`promotes_operands_to_float_result`]), so its operands and result share one checked type; a mixed
    /// numeric comparison reads both operands at their common carrier the same way (see [`comparison_carrier`]).
    ///
    /// Helper families are checked before the primitive path rather than after, because several operators map to
    /// both: `+` is a primitive on two ints and a concatenation on two lists, and only the operand types separate
    /// them. Callers are expected to have already checked [`Self::binary_op_is_supported`]; an operator with no
    /// handling at all still falls back to an explicit unsupported placeholder defensively rather than panicking.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_binary_from_operands(
        &mut self,
        op: ast::BinaryOp,
        lhs_ty: &IncanType,
        lhs_operand: bir::Operand,
        rhs_ty: &IncanType,
        rhs_operand: bir::Operand,
        result_ty: IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        if is_string_like(lhs_ty)
            && is_string_like(rhs_ty)
            && let Some(helper) = string_helper_for_binop(op)
        {
            self.record_runtime_requirement(AbiV0RuntimeRequirement::RuntimeHelper(helper.as_str().to_string()));
            self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
            // Membership is the one string operator whose surface order is the reverse of its helper's signature:
            // `needle in haystack` reads needle-first, but `str_contains` takes the haystack first, as every
            // `contains` in Rust does. Emit the helper's own order here rather than documenting a reversal, because a
            // backend binding these positionally has no way to know one helper disagrees with the other eight.
            let arguments = match helper {
                bir::HelperOp::StrContains | bir::HelperOp::StrNotContains => {
                    vec![rhs_operand, lhs_operand]
                }
                _ => vec![lhs_operand, rhs_operand],
            };
            return self.push_call_temp(
                bir::Callee::Helper(helper),
                fixed_elements(arguments),
                result_ty,
                scope,
                span,
                false,
                out,
            );
        }

        if let Some(helper) = collection_helper_for_binop(op, lhs_ty, rhs_ty) {
            let lhs_operand = if matches!(op, ast::BinaryOp::In | ast::BinaryOp::NotIn) {
                match numeric_destinations::collection_numeric_destination(lhs_ty, rhs_ty) {
                    Some(carrier) => self.promote_operand(lhs_ty, lhs_operand, &carrier, scope, span, out),
                    None => lhs_operand,
                }
            } else {
                lhs_operand
            };
            self.record_runtime_requirement(AbiV0RuntimeRequirement::RuntimeHelper(helper.as_str().to_string()));
            self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
            // Membership helpers take `(haystack, needle)` while the source reads needle-first, matching the
            // string membership pair and every `contains` in Rust. Concatenation keeps source order, since it is
            // not commutative and has no receiver convention pulling the operands apart.
            let arguments = match op {
                ast::BinaryOp::In | ast::BinaryOp::NotIn => vec![rhs_operand, lhs_operand],
                _ => vec![lhs_operand, rhs_operand],
            };
            return self.push_call_temp(
                bir::Callee::Helper(helper),
                fixed_elements(arguments),
                result_ty,
                scope,
                span,
                false,
                out,
            );
        }

        let Some(bin_op) = lower_binary_op(op) else {
            return self.unsupported_operand(format!("binary operator {op:?}"), scope, span, out);
        };
        if bin_op.may_panic() {
            self.panic_facts.push(bir::PanicFact {
                span,
                reason: bir::PanicReason::DivisionOrModulo,
            });
            self.record_runtime_requirement(AbiV0RuntimeRequirement::PanicStrategy);
        }
        let operand_carrier = if promotes_operands_to_float_result(bin_op, &result_ty) {
            Some(result_ty.clone())
        } else {
            comparison_carrier(bin_op, lhs_ty, rhs_ty)
        };
        let (lhs_operand, rhs_operand) = match operand_carrier {
            Some(carrier) => (
                self.promote_operand(lhs_ty, lhs_operand, &carrier, scope, span, out),
                self.promote_operand(rhs_ty, rhs_operand, &carrier, scope, span, out),
            ),
            None => (lhs_operand, rhs_operand),
        };
        self.push_assign_temp(
            bir::Rvalue::BinaryOp(bin_op, lhs_operand, rhs_operand),
            result_ty,
            scope,
            span,
            out,
        )
    }

    /// Assign a numeric operand to a temporary of the checked result carrier when its own type differs.
    ///
    /// The temporary's declared type is the checker's answer, so a backend reads the conversion off a typed assignment
    /// exactly as it reads any other checked widening, and never decides which operand a binary promotion applies to.
    fn promote_operand(
        &mut self,
        operand_ty: &IncanType,
        operand: bir::Operand,
        result_ty: &IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        if operand_ty == result_ty || !is_numeric_carrier(operand_ty) {
            return operand;
        }
        self.push_assign_temp(bir::Rvalue::Use(operand), result_ty.clone(), scope, span, out)
    }
}

/// Whether a primitive operation reads both operands at its checked float result carrier (RFC 009).
///
/// Mixed arithmetic whose checked result is a float carrier converts each operand to that carrier first: `int / int`
/// is `float`, `int` with `float` is `float`, and `f32` with `int` or `float` widens to `float`. `**` is excluded
/// because a dynamic `int ** int` resolves `float` from its exponent rather than by promoting its operands, and the
/// power lowering keeps that rule. Integer results are excluded because mixed exact-width `//` and `%` keep their
/// unsigned operand through a dedicated helper rather than converting the `int` side.
fn promotes_operands_to_float_result(op: bir::BinOp, result_ty: &IncanType) -> bool {
    matches!(
        op,
        bir::BinOp::Add | bir::BinOp::Sub | bir::BinOp::Mul | bir::BinOp::Div | bir::BinOp::FloorDiv | bir::BinOp::Mod
    ) && matches!(
        result_ty,
        IncanType::Primitive(
            IncanPrimitiveType::Float | IncanPrimitiveType::Numeric(NumericTypeId::F32 | NumericTypeId::F64)
        )
    )
}

/// The carrier both operands of a mixed numeric comparison are read at, chosen as legacy's comparison plan does.
///
/// A comparison has a `bool` result, so the checker records no operand carrier for it. The rule is the language's:
/// two different integer carriers compare at [`common_lossless_integer_type`], the narrowest integer type holding both,
/// and a pair involving `float`, `f64` or `f32` compares as `float`, with an `int` side or a lone `f32` side widened.
/// Same-carrier pairs, non-numeric operands and integer pairs no single type holds keep their operands.
fn comparison_carrier(op: bir::BinOp, lhs_ty: &IncanType, rhs_ty: &IncanType) -> Option<IncanType> {
    if !matches!(
        op,
        bir::BinOp::Eq | bir::BinOp::Ne | bir::BinOp::Lt | bir::BinOp::Le | bir::BinOp::Gt | bir::BinOp::Ge
    ) || lhs_ty == rhs_ty
    {
        return None;
    }
    let (left, right) = (numeric_carrier_id(lhs_ty)?, numeric_carrier_id(rhs_ty)?);
    if numerics::is_binary_float(left) || numerics::is_binary_float(right) {
        return Some(IncanType::Primitive(IncanPrimitiveType::Float));
    }
    common_lossless_integer_type(left, right).map(|common| match common {
        NumericTypeId::I64 => IncanType::Primitive(IncanPrimitiveType::Int),
        other => IncanType::Primitive(IncanPrimitiveType::Numeric(other)),
    })
}

/// The numeric identity of a checked carrier: `int` is `i64` and `float` is `f64`; `bool` and non-numerics have none.
pub(super) fn numeric_carrier_id(ty: &IncanType) -> Option<NumericTypeId> {
    match ty {
        IncanType::Primitive(IncanPrimitiveType::Int) => Some(NumericTypeId::I64),
        IncanType::Primitive(IncanPrimitiveType::Float) => Some(NumericTypeId::F64),
        IncanType::Primitive(IncanPrimitiveType::Numeric(id)) if *id != NumericTypeId::Bool => Some(*id),
        _ => None,
    }
}

/// Whether a checked type is a numeric carrier that a typed assignment may convert; `bool` is never promoted.
fn is_numeric_carrier(ty: &IncanType) -> bool {
    match ty {
        IncanType::Primitive(IncanPrimitiveType::Int | IncanPrimitiveType::Float) => true,
        IncanType::Primitive(IncanPrimitiveType::Numeric(id)) => *id != NumericTypeId::Bool,
        _ => false,
    }
}

/// Whether both inputs carry a typechecker fact sufficiently resolved to select a concrete Body IR operation.
///
/// `Unknown` and `Infer` are recovery/inference states, not operand categories a backend may reinterpret as a
/// primitive. Other semantic types remain facts the typechecker did resolve; whether a source spelling is primitive
/// or a protocol hook is still decided by the recorded operator dispatch checked before this admission gate.
fn binary_operand_types_are_resolved(lhs_ty: &IncanType, rhs_ty: &IncanType) -> bool {
    !matches!(lhs_ty, IncanType::Unknown | IncanType::Infer) && !matches!(rhs_ty, IncanType::Unknown | IncanType::Infer)
}

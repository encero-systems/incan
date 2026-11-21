/// Call arguments: each argument is evaluated by its selected slot, mutable and fresh ordinary arguments are
/// recognized, and a closed enum parameter's argument is classified. Included by `calls.rs` so the methods stay in
/// its module.
impl Builder<'_> {
    /// Evaluate one argument according to its selected slot. A borrowed place is evaluated once before taking its
    /// loan; an explicit ownership request constrains even an otherwise ordinary source binding. Unresolved
    /// duckborrow or ordinary mutable adapters refuse rather than cloning or guessing an access mode.
    fn call_argument(
        &mut self,
        expression: &Spanned<Expr>,
        parameter: &CallableParam,
        mode: Option<OwnershipMode>,
        ordinary_callee: bool,
        closed_enum_parameter_observer: bool,
        selected_result_map_err: bool,
        accepts_fresh_inferred: bool,
        invocation: Span,
        incoming: Vec<usize>,
    ) -> Result<(Vec<usize>, Option<LoanId>), Unavailable> {
        match mode {
            Some(OwnershipMode::Borrowed | OwnershipMode::BorrowedMut) => {
                let (source, incoming) = self.borrowed_input_source(expression, invocation, incoming)?;
                let loan = self.call_loan(
                    source,
                    mode == Some(OwnershipMode::BorrowedMut),
                    parameter.is_mut,
                    expression.span,
                );
                Ok((self.event(incoming, Event::Borrow(loan), expression.span), Some(loan)))
            }
            Some(OwnershipMode::Owned) => {
                if let Some(source) = self.optional_place(expression)? {
                    self.constrained.insert(source.place.root);
                }
                self.expression(expression, incoming, true)
                    .map(|outgoing| (outgoing, None))
            }
            None if self.copy_value(&parameter.ty) && !parameter.is_mut => self
                .expression(expression, incoming, false)
                .map(|outgoing| (outgoing, None)),
            // `Result.map_err` invokes its one checked callback without retaining it. A capture-free closure literal
            // therefore evaluates no caller-owned storage; capturing callbacks still need the ordinary local-closure
            // route so their loans remain live through invocation.
            None if selected_result_map_err
                && !parameter.is_mut
                && matches!(expression.node, Expr::Closure(..))
                && self.checker.type_info.expr_type(expression.span) == Some(&parameter.ty)
                && self
                    .checker
                    .closure_captures(expression.span)
                    .is_some_and(|captures| captures.is_empty()) =>
            {
                Ok((self.event(incoming, Event::Nothing, expression.span), None))
            }
            // One exact canonical field may be taken from a selected dependency-free direct call result. This must
            // precede every fallible optional-place guard because its authenticated source shape is Field(Call), not
            // stored caller storage. Persist the outer value while evaluating only the inner invocation.
            None if ordinary_callee
                && !parameter.is_mut
                && let Some(plan) = self.classify_call_result_field_projection(expression, &parameter.ty) =>
            {
                self.apply_value_use(expression, plan, incoming)
                    .map(|outgoing| (outgoing, None))
            }
            // A fresh closed value has no caller-owned source to conflict with. Explicit boundaries and an ordinary
            // model constructor argument retain that checked independence without inventing a copy or loan.
            None if accepts_fresh_inferred
                && !parameter.is_mut
                && self.optional_place(expression)?.is_none()
                && (self
                    .checker
                    .type_info
                    .expr_type(expression.span)
                    .is_some_and(plain_result)
                    || (ordinary_callee
                        && self
                            .checker
                            .type_info
                            .constructor_field_binding(expression.span)
                            .is_some())) =>
            {
                self.expression(expression, incoming, true)
                    .map(|outgoing| (outgoing, None))
            }
            // A private helper's ordinary slot may receive a fresh value whose checked type cannot hold a borrow.
            // Its expression still proves every nested element; stored and reference-carrying values stay refused.
            None if ordinary_callee
                && !parameter.is_mut
                && self.optional_place(expression)?.is_none()
                && matches!(
                    expression.node,
                    Expr::List(_) | Expr::Tuple(_) | Expr::Set(_) | Expr::Dict(_)
                )
                && self.checker.type_info.expr_type(expression.span).is_some_and(|ty| {
                    !self.checker.type_holds_borrow(ty) && !self.checker.type_holds_native_loan(ty)
                }) =>
            {
                self.expression(expression, incoming, true)
                    .map(|outgoing| (outgoing, None))
            }
            // One exact whole mutable Local may be read into an ordinary mutable slot when its cloneable checked
            // value has no alias, loan, dependency or prior constraint. Mutation settlement remains authoritative;
            // this route invents neither an ownership transfer nor an input loan.
            None if self.mutable_ordinary_local_argument(expression, parameter, ordinary_callee)? => self
                .expression(expression, incoming, false)
                .map(|outgoing| (outgoing, None)),
            // A checked string, bytes, empty option or fieldless enum literal creates fresh storage for an immutable
            // ordinary slot. An authenticated payload-variant call is equally fresh, while its normal
            // construction evaluates every payload. None has a caller-owned source place, so no stored
            // ValueUse authority is invented.
            None if self.fresh_ordinary_argument(expression, parameter, ordinary_callee)? => self
                .expression(expression, incoming, false)
                .map(|outgoing| (outgoing, None)),
            // A checked whole enum Parameter is an observation, not a generic non-Copy transfer. This exact route
            // must precede current-source ordinary_move so a later constructor field may still consume the value.
            None if closed_enum_parameter_observer => self
                .expression(expression, incoming, false)
                .map(|outgoing| (outgoing, None)),
            // Current-source classification is the one authority for exact independent stored reads. Restored
            // records carry no classification and therefore retain the existing ordinary-argument refusal.
            None if ordinary_callee
                && !parameter.is_mut
                && let Some(plan) = self.classify_value_use(
                    expression,
                    super::value_uses::ValueUseContext::OrdinaryArgument(parameter),
                )? =>
            {
                self.apply_value_use(expression, plan, incoming)
                    .map(|outgoing| (outgoing, None))
            }
            // A checked local or parameter, including a field-only path, may be observed by an immutable ordinary
            // helper while its body retains storage. Indexes and borrowed aliases still need separate access facts.
            None if ordinary_callee
                && !parameter.is_mut
                && self.optional_place(expression)?.is_some_and(|path| {
                    path.through.is_none()
                        && path
                            .place
                            .projection
                            .iter()
                            .all(|projection| matches!(projection, Projection::Field(_)))
                        && matches!(&path.place.root, Root::Binding(identity)
                            if identity.kind == incan_semantics_core::SemanticSourceTargetKind::Local
                                || (identity.kind == incan_semantics_core::SemanticSourceTargetKind::Parameter
                                    && !path.place.projection.is_empty()))
                }) =>
            {
                self.expression(expression, incoming, false)
                    .map(|outgoing| (outgoing, None))
            }
            _ => Err(Unavailable {
                span: expression.span,
                reason: "ordinary argument requires inferred access or mutable binding facts".into(),
            }),
        }
    }

    /// Whether one ordinary mutable slot reads an exact whole cloneable mutable Local without taking ownership.
    fn mutable_ordinary_local_argument(
        &mut self,
        expression: &Spanned<Expr>,
        parameter: &CallableParam,
        ordinary_callee: bool,
    ) -> Result<bool, Unavailable> {
        if !ordinary_callee
            || !parameter.is_mut
            || !self.checker.is_clone_type(&parameter.ty)
            || self.checker.type_holds_borrow(&parameter.ty)
            || self.checker.type_holds_native_loan(&parameter.ty)
            || !self.checker.type_info.expr_type(expression.span).is_some_and(|actual| {
                super::super::projection::same_checked_value_type_in(&self.nominal_scope, actual, &parameter.ty)
            })
        {
            return Ok(false);
        }
        let Some(path) = self.optional_place(expression)? else {
            return Ok(false);
        };
        if path.through.is_some() || !path.place.projection.is_empty() {
            return Ok(false);
        }
        let Root::Binding(identity) = &path.place.root else {
            return Ok(false);
        };
        Ok(identity.kind == incan_semantics_core::SemanticSourceTargetKind::Local
            && !self.aliases.contains_key(identity)
            && !self.aliases.values().any(|alias| alias.place.root == path.place.root)
            && !self.borrowed_inputs.contains(&path.place.root)
            && !self.graph.local_loans.contains_key(identity)
            && !self.graph.dependent_locals.contains_key(identity)
            && !self.graph.owned_locals.contains_key(identity)
            && !self.constrained.contains(&path.place.root)
            && self
                .graph
                .permissions
                .get(&path.place.root)
                .is_some_and(|permission| permission.exclusive && permission.writable && permission.transferable))
    }

    /// Whether one immutable ordinary slot receives exact fresh scalar storage rather than observing a stored place.
    fn fresh_ordinary_argument(
        &mut self,
        expression: &Spanned<Expr>,
        parameter: &CallableParam,
        ordinary_callee: bool,
    ) -> Result<bool, Unavailable> {
        let literal = matches!(
            expression.node,
            Expr::Literal(Literal::Bytes(_) | Literal::String(_) | Literal::None)
        );
        let empty_option = matches!(expression.node, Expr::Literal(Literal::None)) && parameter.ty.is_option();
        let checked_variant = matches!(expression.node, Expr::Field(..) | Expr::MethodCall(..))
            && self
                .checker
                .type_info
                .resolved_identity(expression.span)
                .is_some_and(|identity| identity.kind == incan_semantics_core::SemanticSourceTargetKind::Variant)
            && matches!(&parameter.ty,
            ResolvedType::Named(owner) | ResolvedType::Generic(owner, _)
                if matches!(self.checker.lookup_type_info(owner), Some(TypeInfo::Enum(_)))
                    || super::super::projection::checked_nominal_identity_in(&self.nominal_scope, owner)
                        .is_some_and(|identity| {
                            identity.kind == incan_semantics_core::SemanticSourceTargetKind::Enum
                        }));
        if !literal && !checked_variant {
            return Ok(false);
        }
        if !ordinary_callee
            || parameter.is_mut
            || !self.checker.type_info.expr_type(expression.span).is_some_and(|actual| {
                super::super::projection::same_checked_value_type_in(&self.nominal_scope, actual, &parameter.ty)
                    || (empty_option && actual.is_option())
            })
            || self.checker.type_holds_borrow(&parameter.ty)
            || self.checker.type_holds_native_loan(&parameter.ty)
        {
            return Ok(false);
        }
        Ok(self.optional_place(expression)?.is_none())
    }

    /// Authenticate the only whole non-Copy Parameter observation currently retained by an ordinary inferred slot.
    fn closed_enum_parameter_argument(
        &mut self,
        expression: &Spanned<Expr>,
        parameter: &CallableParam,
        ordinary_callee: bool,
    ) -> Result<bool, Unavailable> {
        if !ordinary_callee || parameter.is_mut {
            return Ok(false);
        }
        let Some(path) = self.optional_place(expression)? else {
            return Ok(false);
        };
        if path.through.is_some()
            || !path.place.projection.is_empty()
            || !matches!(&path.place.root, Root::Binding(identity)
                if identity.kind == incan_semantics_core::SemanticSourceTargetKind::Parameter)
        {
            return Ok(false);
        }
        Ok(self.checker.type_info.expr_type(expression.span).is_some_and(|ty| {
            super::super::projection::same_checked_value_type_in(&self.nominal_scope, ty, &parameter.ty)
                && match &parameter.ty {
                    ResolvedType::Named(owner) | ResolvedType::Generic(owner, _) => {
                        matches!(self.checker.lookup_type_info(owner), Some(TypeInfo::Enum(_)))
                            || super::super::projection::checked_nominal_identity_in(&self.nominal_scope, owner)
                                .is_some_and(|identity| {
                                    identity.kind == incan_semantics_core::SemanticSourceTargetKind::Enum
                                })
                    }
                    _ => false,
                }
                && !self.checker.type_holds_borrow(ty)
                && !self.checker.type_holds_native_loan(ty)
        }))
    }
}

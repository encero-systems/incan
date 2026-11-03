//! Match lowering, scrutinee temporaries, owned payload moves and statement pattern arms.

use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Lower a match expression into one structured [`bir::Rvalue::Match`] with arm-scoped bindings and results.
    ///
    /// The scrutinee is borrowed; each payload read retains its checked ownership fact. The temporaries made while
    /// evaluating it end before the arms when its value holds no loan ([`Self::end_scrutinee_temporaries`]). Guards
    /// and bodies see only their arm's bindings. Range-layout facts after the match are the intersection of the arm
    /// outcomes, since textual arm order does not establish which arm executes. Source reads in an arm preserve
    /// enclosing owners across alternative paths; arm-local payload transfers remain governed by their checked
    /// pattern facts.
    ///
    /// A byte-string literal pattern is outside the supported pattern vocabulary. Refuse the whole expression
    /// before lowering its scrutinee so an unsupported pattern cannot leave partially lowered effects.
    pub(in crate::body_ir) fn lower_match(
        &mut self,
        subject: &ast::Spanned<ast::Expr>,
        arms: &[ast::Spanned<ast::MatchArm>],
        expr_span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(expr_span);
        if arms
            .iter()
            .any(|arm| !match_pattern_is_supported(&arm.node.pattern.node))
        {
            return self.unsupported_operand(
                "match arm with a byte-string literal pattern".to_string(),
                scope,
                hir_span_value,
                out,
            );
        }

        let first_temporary = self.locals.len();
        let scrutinee_place = self.lower_expr_to_place(subject, scope, out);
        let scrutinee_ty = if scrutinee_place.projection.is_empty() {
            scrutinee_place
                .local_id()
                .and_then(|local| self.locals.get(local.index()))
                .map_or_else(|| self.resolve_ty(subject.span), |local| local.ty.clone())
        } else {
            self.resolve_ty(subject.span)
        };
        let in_place = self.matches_in_place(subject.span, &scrutinee_ty);
        self.end_scrutinee_temporaries(subject.span, &scrutinee_place, in_place, first_temporary, scope, out);
        // Always read as `Borrow` -- see `bir::Rvalue::Match::scrutinee`'s own docs for why the overall scrutinee
        // read must not risk an unconditional move while individual pattern bindings below compute their own,
        // more precise facts against projected places rooted at this same scrutinee.
        let scrutinee_operand = bir::Operand::place(scrutinee_place.clone(), bir::OwnershipFact::Borrow, false);
        let carried_view = self.carried_view_of(subject);

        let range_layouts_before = self.materialized_range_locals.clone();
        let mut range_layouts_after_arms = Vec::with_capacity(arms.len());
        let mut lowered_arms = Vec::with_capacity(arms.len());
        // Explicit transfer subjects join like statement-if branches: every arm starts from the scrutinee's state,
        // and the state after the match is what every continuing arm leaves.
        let subjects = self.input_subject_locals();
        let entry_moves = self.input_move_state(&subjects);
        let mut reaching_moves = Vec::new();
        for arm in arms {
            // At runtime exactly one arm executes. Lower every arm from the predecessor fact set so a range
            // constructed only in an earlier textual arm cannot authorize field projections in a later one.
            self.materialized_range_locals = range_layouts_before.clone();
            self.restore_input_move_state(&subjects, &entry_moves);
            let arm_span = hir_span(arm.span);
            let arm_scope = self.new_scope(Some(scope), arm_span);

            // ---- Lower the pattern, declaring one fresh arm-scoped local per distinct bound name ----
            // The arm of the variant holding a carried view names the call's substituted place, as `?` does; the
            // carrier's own reference is never read, so that pattern binds no payload.
            let arm_view = carried_view
                .as_ref()
                .and_then(|view| self.bind_carried_arm_view(&arm.node.pattern, view));
            let unbound_pattern = arm_view.as_ref().map(|_| without_payload_binding(&arm.node.pattern));
            let mut seen: HashMap<String, bir::LocalId> = HashMap::new();
            let mut saved_bindings: Vec<(String, Option<bir::LocalId>)> = Vec::new();
            let enclosing_in_place = std::mem::replace(&mut self.in_place_pattern, in_place);
            let mut pattern = self.lower_match_pattern(
                unbound_pattern.as_ref().unwrap_or(&arm.node.pattern),
                &scrutinee_ty,
                &scrutinee_place,
                arm_scope,
                &PatternReadScope::MatchArm(&arm.node),
                &mut seen,
                &mut saved_bindings,
            );
            self.in_place_pattern = enclosing_in_place;
            if arm.node.guard.is_none() {
                self.retain_owned_carrier_payload_move(&scrutinee_place, &mut pattern);
            }

            // ---- Guard and body see this arm's own pattern bindings, shadowing any outer binding of the same name
            // ----
            let mut guard_stmts = Vec::new();
            self.conditional_scopes.push(arm_scope);
            // A guard is a further condition inside the arm; only unguarded arms have an explicit join.
            let joined = arm.node.guard.is_none();
            if joined {
                self.input_if_scopes.push(arm_scope);
            }
            let guard = arm
                .node
                .guard
                .as_ref()
                .map(|g| self.lower_expr_to_operand(g, arm_scope, &mut guard_stmts));

            let (body_stmts, result) = match &arm.node.body {
                ast::MatchBody::Expr(e) => {
                    let mut stmts = Vec::new();
                    let result = self.lower_boundary_value(e, None, arm_scope, &mut stmts);
                    (stmts, result)
                }
                ast::MatchBody::Block(block_stmts) => {
                    let mut stmts = Vec::new();
                    self.lower_block_into(block_stmts, arm_scope, &mut stmts);
                    self.insert_scope_drops(&mut stmts, arm_scope);
                    (stmts, bir::Operand::Constant(bir::Constant::Unit))
                }
            };
            if joined {
                self.input_if_scopes.pop();
            }
            self.conditional_scopes.pop();
            match &arm.node.body {
                ast::MatchBody::Expr(_) => reaching_moves.push(self.input_move_state(&subjects)),
                ast::MatchBody::Block(block_stmts) => {
                    self.retain_reaching_input_moves(block_stmts, &subjects, &mut reaching_moves)
                }
            }

            // ---- Restore the enclosing scope's bindings before moving on to the next (mutually exclusive) arm ----
            self.restore_local_bindings(saved_bindings);
            if let Some(identity) = arm_view {
                self.input_aliases.remove(&identity);
            }
            range_layouts_after_arms.push(self.materialized_range_locals.clone());

            lowered_arms.push(bir::MatchArm {
                pattern,
                guard_stmts,
                guard,
                body_stmts,
                result,
            });
        }
        // Code after a match must be valid for whichever arm matched. Preserve a layout fact only when every
        // arm leaves it true; no-arm matches conservatively retain no constructed-layout fact.
        self.materialized_range_locals = intersect_local_states(range_layouts_after_arms);
        self.restore_input_move_state(&subjects, &intersect_local_states(reaching_moves));

        let ty = self.resolve_ty(expr_span);
        self.push_assign_temp(
            bir::Rvalue::Match {
                scrutinee: scrutinee_operand,
                arms: lowered_arms,
            },
            ty,
            scope,
            hir_span_value,
            out,
        )
    }

    /// End the temporaries that evaluating a scrutinee made, except the scrutinee's own value, before its arms run.
    ///
    /// They would otherwise end with the enclosing statement, keeping their loans across the arms: in
    /// `match connected.stream.borrow_mut().write_all(sent):` the `RefMut` guard would still borrow `connected` when
    /// an arm returns, and so moves, `connected`. Only a whole local whose value the checker proved holds no loan
    /// ([`TypeCheckInfo::scrutinee_holds_no_loan`]), and whose storage names no reference, is independent of them. A
    /// projected or in-place scrutinee keeps its temporaries until the statement ends.
    ///
    /// [`TypeCheckInfo::scrutinee_holds_no_loan`]: crate::typechecker::TypeCheckInfo::scrutinee_holds_no_loan
    pub(in crate::body_ir) fn end_scrutinee_temporaries(
        &mut self,
        subject: ast::Span,
        scrutinee: &bir::Place,
        in_place: bool,
        first_temporary: usize,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) {
        if in_place || !scrutinee.projection.is_empty() || !self.type_info.scrutinee_holds_no_loan(subject) {
            return;
        }
        let Some(kept) = scrutinee.local_id() else {
            return;
        };
        if self
            .locals
            .get(kept.index())
            .is_none_or(|local| storage_names_reference(&local.ty))
        {
            return;
        }
        self.insert_statement_temporary_drops(out, scope, first_temporary, Some(kept), hir_span(subject));
    }

    /// Transfer a single owned payload from an unguarded match on a carrier no later read can reuse: a fresh enum
    /// temporary, or the last read of an owned local binding. A successful arm has no other payload to preserve, so
    /// the binding takes the payload and is its only owner: cloning it would leave the clone and the carrier to be
    /// dropped, running the payload's drop twice. A native payload, which cannot be cloned, also transfers from the
    /// last read of an owned native iterator item or owned parameter. Borrowed parameters, loop-reused carriers, nested
    /// patterns and guards retain their facts. A final owned local read authorizes transfer independently of whether
    /// its payload's declaration is available in this consumer. Checked non-Clone tuple newtypes retain the same
    /// transfer boundary as their native payload; reference payloads retain their existing facts.
    pub(super) fn retain_owned_carrier_payload_move(&mut self, source: &bir::Place, pattern: &mut bir::Pattern) {
        if !source.projection.is_empty() {
            return;
        }
        let Some(id) = source.local_id() else {
            return;
        };
        let Some(local) = self.locals.get(id.index()) else {
            return;
        };
        if matches!(local.ty, IncanType::Ref(_) | IncanType::RefMut(_)) {
            return;
        }
        let owned_last_read = (self.native_enum_payload_bindings.contains(&id)
            || matches!(
                local.origin,
                bir::LocalOrigin::Parameter | bir::LocalOrigin::UserBinding
            ))
            && !self.is_borrowed_parameter(id)
            && !self.loop_reuses_local(id)
            && !self.pending_argument_reads.contains(&id)
            && self
                .remaining_reads
                .get(&id)
                .is_some_and(|remaining| remaining.remaining == 1);
        if local.origin != bir::LocalOrigin::Temporary && !owned_last_read {
            return;
        }
        let fields = match pattern {
            bir::Pattern::Result { fields, .. } | bir::Pattern::Enum { fields, .. } => fields,
            _ => return,
        };
        let [bir::Pattern::Var(binding)] = fields.as_mut_slice() else {
            return;
        };
        let Some(payload) = self.locals.get(binding.local.index()) else {
            return;
        };
        let unread_carrier = (local.origin == bir::LocalOrigin::Temporary
            || (local.origin == bir::LocalOrigin::UserBinding && owned_last_read))
            && !matches!(payload.ty, IncanType::Ref(_) | IncanType::RefMut(_));
        if (unread_carrier || self.payload_needs_owned_transfer(&payload.ty))
            && matches!(
                binding.fact,
                bir::OwnershipFact::Borrow | bir::OwnershipFact::Clone | bir::OwnershipFact::Move
            )
        {
            binding.fact = bir::OwnershipFact::Move;
            self.native_enum_payload_bindings.insert(binding.local);
        }
    }

    /// Identify native owners and checked non-Clone tuple newtypes through owned carrier arguments.
    /// References never grant an owned transfer, and source declarations are selected by retained canonical identity.
    pub(super) fn payload_needs_owned_transfer(&self, ty: &IncanType) -> bool {
        let name = match ty {
            IncanType::Generic { base, args } => {
                if args.iter().any(|argument| self.payload_needs_owned_transfer(argument)) {
                    return true;
                }
                base
            }
            IncanType::Tuple(fields) => {
                return fields.iter().any(|field| self.payload_needs_owned_transfer(field));
            }
            IncanType::Named(name) => name,
            _ => return false,
        };
        if bir::rust_item_identity(name).is_some() {
            return true;
        }
        parameter_types::retained_nominal_identity(name, self.type_info).is_some_and(|identity| {
            self.local_nominal_declarations
                .values()
                .find(|declaration| &declaration.canonical == identity)
                .is_some_and(|declaration| {
                    declaration.fields == ["0"]
                        && !declaration
                            .derives
                            .iter()
                            .any(|derive| derives::from_str(derive) == Some(DeriveId::Clone))
                })
        })
    }

    /// Return whether the `match`, `if let` or `while let` over the scrutinee at `span` matches it in place.
    ///
    /// The checker records a scrutinee when an arm changes a part of a writable place through a name the pattern
    /// binds (#1561); legacy then matches through `&mut` so the change lands in the place. A union-typed scrutinee
    /// keeps the ordinary match, because its arms bind narrowed members rather than parts of the place.
    pub(in crate::body_ir) fn matches_in_place(&self, span: ast::Span, scrutinee_ty: &IncanType) -> bool {
        scrutinee_ty.union_members().is_none() && self.type_info.match_scrutinee_is_changed_in_place(span)
    }

    /// Lower one pattern arm whose body is a statement block, restoring the enclosing bindings afterwards.
    ///
    /// Shared by the `if let` and `while let` desugarings (#1161), which both need the same three steps: test a
    /// pattern, run a block with its bindings in scope, then put the enclosing bindings back so they cannot leak
    /// past the construct. A `match` arm deliberately does not use this -- its body may be a single expression and
    /// it may carry a guard, both of which change what the arm produces.
    ///
    /// Restoring is what makes `if let`'s scoping honest: the pattern's names belong to the `then` branch only, and
    /// an `else` branch lowered afterwards must not see them. `in_place` is [`Self::matches_in_place`] for the
    /// construct's scrutinee; it applies to the pattern only, never to patterns nested in the body.
    /// Unguarded statement arms use the same checked native payload transfer as expression matches; a fresh owned
    /// Option/Result temporary must not require cloning its payload merely because the source spells `if let`.
    /// Source reads in the conditional body retain enclosing owners for the other path and its scope-exit drops.
    #[allow(clippy::too_many_arguments)] // The scrutinee's type, place and in-place fact travel together into one arm.
    pub(in crate::body_ir) fn lower_statement_pattern_arm(
        &mut self,
        pattern: &ast::Spanned<ast::Pattern>,
        scrutinee_ty: &IncanType,
        scrutinee_place: &bir::Place,
        in_place: bool,
        scope: bir::ScopeId,
        body: &[ast::Spanned<ast::Statement>],
        span: HirSourceSpan,
    ) -> bir::MatchArm {
        let arm_scope = self.new_scope(Some(scope), span);
        let mut seen: HashMap<String, bir::LocalId> = HashMap::new();
        let mut saved_bindings: Vec<(String, Option<bir::LocalId>)> = Vec::new();
        let enclosing_in_place = std::mem::replace(&mut self.in_place_pattern, in_place);
        let mut lowered_pattern = self.lower_match_pattern(
            pattern,
            scrutinee_ty,
            scrutinee_place,
            arm_scope,
            &PatternReadScope::FollowingStatements(body),
            &mut seen,
            &mut saved_bindings,
        );
        self.in_place_pattern = enclosing_in_place;
        self.retain_owned_carrier_payload_move(scrutinee_place, &mut lowered_pattern);

        let mut body_stmts = Vec::new();
        self.conditional_scopes.push(arm_scope);
        self.lower_block_into(body, arm_scope, &mut body_stmts);
        self.conditional_scopes.pop();
        self.insert_scope_drops(&mut body_stmts, arm_scope);

        self.restore_local_bindings(saved_bindings);

        bir::MatchArm {
            pattern: lowered_pattern,
            guard_stmts: Vec::new(),
            guard: None,
            body_stmts,
            // A statement-block arm produces no value, exactly as a `match` arm with a block body does.
            result: bir::Operand::Constant(bir::Constant::Unit),
        }
    }
}

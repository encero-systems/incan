//! For-pattern bindings, general and non-range iteration, dictionary values, and in-place item changes.

use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Declare the local each produced item of a `for` loop is written into.
    ///
    /// A plain `for x in ...` binds the item directly: the item local *is* `x`'s local, so the produced value is
    /// never copied and the loop shape #1103/#1101 established is preserved byte-for-byte. Every other supported
    /// pattern shape has no single name to write into, so the item goes into a temporary that
    /// [`Self::bind_for_pattern`] projects the real bindings out of -- the same "materialize once, then bind each
    /// element off a projection" shape [`Self::lower_tuple_unpack`] already uses for `a, b = value`.
    pub(in crate::body_ir) fn declare_for_item_local(
        &mut self,
        pattern: &ast::Spanned<ast::Pattern>,
        item_ty: &IncanType,
        loop_scope: bir::ScopeId,
        span: HirSourceSpan,
        reads: &dyn Fn(&str) -> usize,
    ) -> bir::LocalId {
        match &pattern.node {
            ast::Pattern::Binding(name) => {
                let total_reads = reads(name);
                self.declare_new_local_with_reads(name.clone(), item_ty.clone(), loop_scope, span, total_reads)
            }
            _ => self.new_temp(item_ty.clone(), loop_scope, span),
        }
    }

    /// Emit the binding statements a `for` loop's pattern needs against the item local, immediately after the
    /// per-iteration `IterNext` (or, on the range path, after the index copy) has written it.
    ///
    /// A bare [`ast::Pattern::Binding`] emits nothing: [`Self::declare_for_item_local`] already declared the item
    /// local *as* that binding, so there is nothing left to project. Every other shape delegates to
    /// [`Self::bind_for_pattern_fields`], which means every binding that walk reaches is nested under at least one
    /// tuple field and therefore always reads through a projection.
    pub(in crate::body_ir) fn bind_for_pattern(
        &mut self,
        pattern: &ast::Spanned<ast::Pattern>,
        item_ty: &IncanType,
        item_local: bir::LocalId,
        loop_scope: bir::ScopeId,
        reads: &dyn Fn(&str) -> usize,
        out: &mut Vec<bir::Statement>,
    ) {
        if matches!(pattern.node, ast::Pattern::Binding(_)) {
            return;
        }
        let item_place = bir::Place::from_local(item_local);
        self.bind_for_pattern_fields(pattern, item_ty, &item_place, loop_scope, reads, out);
    }

    /// Recursively bind one `for`-pattern node against `place`, the (already projected) part of the produced item
    /// it corresponds to, emitting one `Assign` per bound name in source order.
    ///
    /// Iteration binding is *irrefutable*: unlike [`Self::lower_match_pattern`], which builds a [`bir::Pattern`]
    /// for match-arm dispatch, there is nothing here to test or branch on, so this walk emits plain assignments and
    /// deliberately does not reuse that machinery (#1125 names conflating the two as a non-goal). What it does
    /// share is that walk's projection convention -- the zero-based tuple-element index spelled as a
    /// [`bir::PlaceElem::Field`], matching [`Self::lower_tuple_unpack`]'s `.0`/`.1` spelling -- and its
    /// [`tuple_element_types`] source for per-element types, so a nested tuple keeps resolved element types all the
    /// way down and falls back to [`IncanType::Unknown`] per slot only where the resolved type is not a tuple of
    /// the right arity.
    ///
    /// Each element is read through [`Self::ownership_fact_for_place`], exactly as [`Self::lower_tuple_unpack`] reads
    /// its own elements, so a non-Copy element borrows rather than moving out of a place v0 does not track
    /// partial-move state for. Each bound name becomes a real [`bir::LocalOrigin::UserBinding`] local in `loop_scope`,
    /// seeded with its own last-use countdown over the loop body, so [`Self::insert_scope_drops`] gives every non-Copy
    /// binding an explicit per-iteration drop.
    ///
    /// [`unsupported_for_pattern`] has already rejected every shape outside the accepted subset -- and every item
    /// type that is not a tuple of matching arity -- before [`Self::lower_for`] reaches this walk, so the remaining
    /// arms are unreachable in practice; they emit nothing rather than panicking if that invariant is ever violated
    /// by a hand-built AST.
    pub(in crate::body_ir) fn bind_for_pattern_fields(
        &mut self,
        pattern: &ast::Spanned<ast::Pattern>,
        expected_ty: &IncanType,
        place: &bir::Place,
        loop_scope: bir::ScopeId,
        reads: &dyn Fn(&str) -> usize,
        out: &mut Vec<bir::Statement>,
    ) {
        let span = hir_span(pattern.span);
        match &pattern.node {
            ast::Pattern::Wildcard => {}
            ast::Pattern::Binding(name) => {
                let (fact, last_use) = self.ownership_fact_for_place(place, expected_ty);
                let element = bir::Operand::place(place.clone(), fact, last_use);
                let total_reads = reads(name);
                let local =
                    self.declare_new_local_with_reads(name.clone(), expected_ty.clone(), loop_scope, span, total_reads);
                out.push(bir::Statement {
                    kind: bir::StatementKind::Assign {
                        place: bir::Place::from_local(local),
                        rvalue: bir::Rvalue::Use(element),
                    },
                    span,
                });
            }
            ast::Pattern::Tuple(items) => {
                let element_types = tuple_element_types(expected_ty, items.len());
                for (index, (item, element_ty)) in items.iter().zip(&element_types).enumerate() {
                    let mut field_place = place.clone();
                    field_place
                        .projection
                        .push(bir::PlaceElem::structural_field(index.to_string()));
                    self.bind_for_pattern_fields(item, element_ty, &field_place, loop_scope, reads, out);
                }
            }
            ast::Pattern::Literal(_) | ast::Pattern::Constructor(..) | ast::Pattern::Group(_) | ast::Pattern::Or(_) => {
            }
        }
    }

    /// Lower a general iteration through the same checked path for statement loops and comprehension clauses.
    ///
    /// The iterable is evaluated once before the loop. Builtin collections retain their borrowed-value initializer and
    /// [`bir::StatementKind::IterNext`]; a custom protocol uses separately retained canonical hook calls, intrinsic
    /// Option dispatch and, for a fallible poll, checked [`bir::StatementKind::TryPropagate`] routing. No backend
    /// resolves a user hook from its spelling.
    ///
    /// Only the checker's structural protocol record selects hook calls. Builtin iterator values keep builtin polling,
    /// including when standard trait declarations are loaded.
    ///
    /// A fallible loop's header `?` marks polling, so its inner expression is materialized without a second Result
    /// unwrap. An ordinary Result-valued iterable still goes through normal expression propagation. Hook receivers
    /// use their checked modes; a builtin loop that changes an item in place borrows the actual initializer
    /// exclusively, preserving legacy's `iter_mut` behavior even when evaluating the iterable emitted other statements.
    /// Checker-selected item-taking loops and whole temporary lists move their builtin iterable, as legacy's
    /// `into_iter` does. Other builtin sources remain borrowed, and consuming iterators are polled exclusively without
    /// changing their initialization into an in-place item borrow.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::body_ir) fn lower_general_iteration(
        &mut self,
        iter_expr: &ast::Spanned<ast::Expr>,
        pattern_local: bir::LocalId,
        outer_scope: bir::ScopeId,
        loop_scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
        body_fn: impl FnOnce(&mut Self, bir::ScopeId, &mut Vec<bir::Statement>),
    ) {
        match self.retained_range_loop_source(iter_expr, outer_scope, out) {
            Ok(Some(source)) => {
                self.lower_counting_iteration(&source, pattern_local, outer_scope, loop_scope, span, out, body_fn);
                return;
            }
            Err(reason) => {
                self.push_unsupported_stmt(reason, span, out);
                return;
            }
            Ok(None) => {}
        }
        self.lower_nonrange_iteration(iter_expr, pattern_local, outer_scope, loop_scope, span, out, body_fn);
    }

    /// Poll a nonrange source using its checked native, source, or builtin collection iteration protocol.
    /// Special collection adapters retain the same pattern callback and exclusive receiver adjustments.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_nonrange_iteration(
        &mut self,
        iter_expr: &ast::Spanned<ast::Expr>,
        pattern_local: bir::LocalId,
        outer_scope: bir::ScopeId,
        loop_scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
        body_fn: impl FnOnce(&mut Self, bir::ScopeId, &mut Vec<bir::Statement>),
    ) {
        let protocol = self.type_info.protocol_iteration(iter_expr.span).cloned();
        let fallible = protocol.as_ref().is_some_and(|p| p.fallible_error_type.is_some());
        // The private hook table also covers opaque SDK Iterator returns whose legacy view stays builtin.
        let custom = self.type_info.rust.iteration_hooks.contains_key(&iter_expr.span)
            || protocol.is_some()
            || self
                .type_info
                .protocols
                .body_ir_iterations
                .contains_key(&iter_expr.span);
        let effective_iter_expr: &ast::Spanned<ast::Expr> = match (&iter_expr.node, fallible) {
            (ast::Expr::Try(inner), true) => inner,
            _ => iter_expr,
        };

        if !custom
            && self.enumerated_pattern_is_normalizable(pattern_local)
            && let Some(source) = self.enumerated_loop_source(effective_iter_expr)
        {
            self.lower_enumerated_iteration(source, pattern_local, outer_scope, loop_scope, span, out, body_fn);
            return;
        }

        if !custom
            && self.pair_pattern_is_normalizable(pattern_local)
            && let Some(sources) = self.zipped_loop_sources(effective_iter_expr)
        {
            if let Err(reason) =
                self.lower_zipped_iteration(sources, pattern_local, outer_scope, loop_scope, span, out, body_fn)
            {
                self.push_unsupported_stmt(reason, span, out);
            }
            return;
        }
        if !custom && let Some(source) = self.dictionary_values_source(effective_iter_expr) {
            self.lower_dictionary_values_iteration(source, pattern_local, outer_scope, loop_scope, span, out, body_fn);
            return;
        }

        // ---- Materialize the iterable exactly once ----
        let (iterator_local, iterator_init, taking) =
            match self.initialize_general_iterator(effective_iter_expr, iter_expr.span, custom, outer_scope, span, out)
            {
                Ok(iterator) => iterator,
                Err(description) => {
                    self.push_unsupported_stmt(description, span, out);
                    return;
                }
            };

        // ---- Poll before each iteration body ----
        self.loop_break_targets.push(None);
        self.loop_scopes.push(loop_scope);
        self.loop_continue_actions.push(Vec::new());
        let mut body_stmts = Vec::new();

        if custom {
            if let Err(description) = self.lower_checked_iteration_poll(
                iter_expr.span,
                iterator_local,
                pattern_local,
                fallible,
                loop_scope,
                span,
                &mut body_stmts,
            ) {
                self.push_unsupported_stmt(description, span, &mut body_stmts);
            }
        } else {
            body_stmts.push(bir::Statement {
                kind: bir::StatementKind::IterNext {
                    destination: bir::Place::from_local(pattern_local),
                    iterator: bir::Operand::place(
                        bir::Place::from_local(iterator_local),
                        bir::OwnershipFact::MutBorrow,
                        false,
                    ),
                    protocol: bir::IterProtocol::Builtin,
                },
                span,
            });
        }

        // ---- Body control flow and scope cleanup ----
        body_fn(self, loop_scope, &mut body_stmts);
        self.retain_in_place_iteration_bindings(&mut body_stmts, pattern_local);
        self.loop_break_targets.pop();
        self.loop_scopes.pop();
        self.loop_continue_actions.pop();
        // A builtin loop whose body changes its item in place iterates the list exclusively, as legacy's `iter_mut`.
        if !custom && !taking && self.body_changes_item_in_place(&body_stmts, pattern_local) {
            borrow_iterable_exclusively(&mut out[iterator_init]);
        }

        out.push(bir::Statement {
            kind: bir::StatementKind::Loop {
                body: bir::Block {
                    scope: loop_scope,
                    stmts: body_stmts,
                },
            },
            span,
        });
    }

    /// A materialized list expression owns its items, as legacy iterates a call result, literal, or comprehension
    /// directly by value. Named places and projections require the checker's explicit consumption decision.
    pub(in crate::body_ir) fn temporary_list_iterable(&self, place: &bir::Place, ty: &IncanType) -> bool {
        ty.collection_arguments(CollectionTypeId::List).is_some()
            && place.projection.is_empty()
            && place
                .local_id()
                .and_then(|local| self.locals.get(local.index()))
                .is_some_and(|local| local.name.is_none())
    }

    /// Recognize only the checked compiler-owned dictionary value view; a user method named values keeps its call.
    pub(super) fn dictionary_values_source<'expr>(
        &self,
        expr: &'expr ast::Spanned<ast::Expr>,
    ) -> Option<&'expr ast::Spanned<ast::Expr>> {
        if let ast::Expr::Paren(inner) = &expr.node {
            return self.dictionary_values_source(inner);
        }
        let ast::Expr::MethodCall(source, _, type_args, args) = &expr.node else {
            return None;
        };
        if !type_args.is_empty() || !args.is_empty() {
            return None;
        }
        let identity = self.type_info.resolved_identity(expr.span)?;
        if identity.origin != incan_semantics_core::SymbolOrigin::Builtin || identity.declaration_name != "Dict.values"
        {
            return None;
        }
        self.resolve_ty(source.span)
            .collection_arguments(CollectionTypeId::Dict)?;
        Some(source)
    }

    /// Poll original dictionary keys and retain the corresponding value place, rather than iterating copied values.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_dictionary_values_iteration(
        &mut self,
        source: &ast::Spanned<ast::Expr>,
        item: bir::LocalId,
        outer_scope: bir::ScopeId,
        loop_scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
        body_fn: impl FnOnce(&mut Self, bir::ScopeId, &mut Vec<bir::Statement>),
    ) {
        // ---- Original map storage and its polled key ----
        let source_ty = self.resolve_ty(source.span);
        let key_ty = source_ty
            .collection_arguments(CollectionTypeId::Dict)
            .map(|types| types[0].clone())
            .unwrap_or(IncanType::Unknown);
        let (iterator, initializer) = self.initialize_iteration_source(source, outer_scope, span, out);
        let key = self.new_temp(key_ty.clone(), loop_scope, span);
        let mut value = bir::Place::from_local(iterator);
        value
            .projection
            .push(bir::PlaceElem::Index(Box::new(self.temp_operand(key, &key_ty))));
        let mut stmts = vec![
            self.iteration_poll(iterator, key, span),
            bir::Statement {
                kind: bir::StatementKind::Assign {
                    place: bir::Place::from_local(item),
                    rvalue: bir::Rvalue::Use(bir::Operand::place(value, bir::OwnershipFact::Borrow, false)),
                },
                span,
            },
        ];
        self.loop_break_targets.push(None);
        self.loop_scopes.push(loop_scope);
        self.loop_continue_actions.push(Vec::new());

        // ---- Checked value bindings and mutable value storage ----
        body_fn(self, loop_scope, &mut stmts);
        self.retain_in_place_iteration_bindings(&mut stmts, item);
        if self.body_changes_item_in_place(&stmts, item) {
            borrow_iterable_exclusively(&mut out[initializer]);
            borrow_iterable_exclusively(&mut stmts[1]);
        }
        self.loop_break_targets.pop();
        self.loop_scopes.pop();
        self.loop_continue_actions.pop();
        out.push(bir::Statement {
            kind: bir::StatementKind::Loop {
                body: bir::Block {
                    scope: loop_scope,
                    stmts,
                },
            },
            span,
        });
    }

    /// Retain exclusive reads for changed non-Copy pattern bindings only. Ordinary aliases declared in the body
    /// keep their existing value-copy semantics; the pattern's own source spans identify its generated bindings.
    /// Projected writes retain their own operand facts: changing a field does not reinitialize its owner binding.
    pub(super) fn retain_in_place_iteration_bindings(&self, stmts: &mut [bir::Statement], item: bir::LocalId) {
        let pattern_span = self.locals[item.index()].span;
        let changed: Vec<_> = stmts
            .iter()
            .filter_map(|statement| {
                let bir::StatementKind::Assign {
                    place,
                    rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
                } = &statement.kind
                else {
                    return None;
                };
                let local = place.local_id()?;
                (read.place.local_id() == Some(item)
                    && statement.span.start >= pattern_span.start
                    && statement.span.end <= pattern_span.end
                    && !self.locals[local.index()]
                        .ty
                        .abi_v0_facts()
                        .ownership
                        .is_trivially_copy()
                    && self.body_changes_item_in_place(stmts, local))
                .then_some(local)
            })
            .collect();
        for statement in stmts {
            if let bir::StatementKind::Assign {
                place,
                rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
            } = &mut statement.kind
                && place.projection.is_empty()
                && place.local_id().is_some_and(|local| changed.contains(&local))
            {
                read.fact = bir::OwnershipFact::MutBorrow;
                read.last_use = false;
            }
        }
    }

    /// Return whether a loop body changes its item in place: through a method that changes it, a pattern matched in
    /// place over it, a write to a part of it, or a nested loop that changes its elements in place.
    ///
    /// Legacy iterates such a loop with `iter_mut`, so each change lands in its element. An item passed to a `mut`
    /// parameter does not count: legacy passes the callee a copy of it (#1561).
    pub(super) fn body_changes_item_in_place(&self, stmts: &[bir::Statement], item: bir::LocalId) -> bool {
        stmts
            .iter()
            .any(|stmt| self.statement_changes_item_in_place(stmt, item))
    }

    /// Report whether one statement, or a block nested in it, changes `item` in place.
    pub(super) fn statement_changes_item_in_place(&self, statement: &bir::Statement, item: bir::LocalId) -> bool {
        match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(_),
                args,
                ..
            } => args
                .first()
                .and_then(bir::ArgumentElement::as_one)
                .is_some_and(|receiver| self.method_changes_item(receiver, statement.span, item)),
            bir::StatementKind::Assign { place, rvalue } => {
                (!place.projection.is_empty() && place.local_id() == Some(item))
                    || self.rvalue_changes_item_in_place(rvalue, item)
            }
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                self.body_changes_item_in_place(&then_block.stmts, item)
                    || else_block
                        .as_ref()
                        .is_some_and(|block| self.body_changes_item_in_place(&block.stmts, item))
            }
            bir::StatementKind::Loop { body } | bir::StatementKind::UnsafeRegion { body, .. } => {
                self.body_changes_item_in_place(&body.stmts, item)
            }
            _ => false,
        }
    }

    /// Report whether a method call at `span` changes its receiver `item`, or a part of it: a collection method that
    /// borrows it exclusively, or a source method the checker resolved to a `mut self` receiver.
    pub(super) fn method_changes_item(&self, receiver: &bir::Operand, span: HirSourceSpan, item: bir::LocalId) -> bool {
        let bir::Operand::Place(read) = receiver else {
            return false;
        };
        read.place.local_id() == Some(item)
            && (read.fact == bir::OwnershipFact::MutBorrow || self.type_info.method_call_takes_mutable_receiver(span))
    }

    /// Report whether an assigned value borrows `item` exclusively (a nested loop iterating it in place), or is a
    /// `match` that binds part of `item` in place or changes it in one of its arms.
    pub(super) fn rvalue_changes_item_in_place(&self, rvalue: &bir::Rvalue, item: bir::LocalId) -> bool {
        match rvalue {
            bir::Rvalue::Use(operand) => borrows_item_exclusively(operand, item),
            bir::Rvalue::Match { scrutinee, arms } => {
                let over_item = matches!(scrutinee, bir::Operand::Place(read) if read.place.local_id() == Some(item));
                arms.iter().any(|arm| {
                    (over_item && pattern_binds_in_place(&arm.pattern))
                        || self.body_changes_item_in_place(&arm.guard_stmts, item)
                        || self.body_changes_item_in_place(&arm.body_stmts, item)
                })
            }
            _ => false,
        }
    }
}

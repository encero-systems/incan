//! Body checking of functions, declaration owners, callables and properties, settlement, and checked body reuse.

use super::*;

impl TypeChecker {
    /// Coordinate a completed function declaration and its independent statement producer. Defaults see generic
    /// binders but no callable parameters; statements consume the exact prepared frame without rechecking it.
    pub(in crate::typechecker) fn check_function(&mut self, function: &FunctionDecl, span: Span) {
        let index = self.body_checks.len();
        let prepared =
            self.check_declaration_owner(span, |checker| checker.prepare_function_declaration(function, span));
        self.check_prepared_function(function, span, prepared);
        self.complete_function_declaration(function, span, index);
        #[cfg(test)]
        self.observe_body_local(span);
    }

    /// Consume a selected declaration value at its ordinary source-order position, then either reuse its exact Body
    /// IR product or check the current statements in a frame reconstructed from the already collected signature.
    /// A refused record falls back before any declaration check is skipped.
    pub(in crate::typechecker) fn check_reused_function(
        &mut self,
        program: &Program,
        function: &FunctionDecl,
        span: Span,
    ) {
        let Some(binding) = self.declaration_view().function(span).cloned() else {
            self.check_function(function, span);
            return;
        };
        if !matches!(self.reuse_declaration_record(program, span), Ok(true)) {
            self.check_function(function, span);
            return;
        }
        if self.callable_body_matches_current_scope(span) {
            self.symbols.enter_scope(crate::symbols::ScopeKind::Function);
            self.symbols.exit_scope();
            return;
        }
        let prepared = self.prepare_reused_function_body(function, span, &binding);
        self.check_prepared_function(function, span, prepared);
        #[cfg(test)]
        self.observe_body_local(span);
    }

    /// Run a declaration's actual semantic rules in the same scoped writer used for callable bodies. Constants keep
    /// their const-evaluation owner, statics retain registry initialization rules, and defaults retain declaration
    /// visibility. Reserve the publication position before nested bodies so traversal order remains source order.
    pub(in crate::typechecker) fn check_declaration_owner<T>(
        &mut self,
        span: Span,
        check: impl FnOnce(&mut Self) -> T,
    ) -> T {
        #[cfg(test)]
        if self.legacy_body_checking {
            return check(self);
        }
        self.check_work.declaration_local_checks += 1;
        let index = self.body_checks.len();
        let scope = BodyCheckScope::enter(self);
        #[cfg(test)]
        self.active_declaration_checks.push(span);
        let prepared = check(self);
        #[cfg(test)]
        let _ = self.active_declaration_checks.pop();
        let mut local = scope.finish(self);
        local.owner = span;
        local.kind = LocalOwnerKind::Declaration;
        self.body_checks.insert(index, BodyCheckInput::Local(local));
        prepared
    }

    /// Append post-body declaration work to its original output without checking the declaration again. The writer
    /// resumes only that owner's facts; child bodies have already returned and retain their own output ownership.
    pub(in crate::typechecker) fn continue_declaration_owner(&mut self, index: usize, check: impl FnOnce(&mut Self)) {
        #[cfg(test)]
        if self.legacy_body_checking {
            check(self);
            return;
        }

        // ---- Retain the declaration's prior output ----
        let BodyCheckInput::Local(previous) = &mut self.body_checks[index] else {
            unreachable!("a declaration continuation requires its fresh local producer output");
        };
        let mut previous = std::mem::take(previous);
        let owner = previous.owner;
        let order = previous.order;
        let nested = std::mem::take(&mut previous.nested);
        let mut diagnostics = std::mem::take(&mut previous.diagnostic_slots);
        let mut warnings = std::mem::take(&mut previous.warning_slots);

        // ---- Resume post-body declaration work ----
        let scope = BodyCheckScope::enter(self);
        previous.append(self);
        #[cfg(test)]
        self.active_declaration_checks.push(owner);
        check(self);
        #[cfg(test)]
        let _ = self.active_declaration_checks.pop();

        // ---- Replace the invocation-owned output ----
        let mut completed = scope.finish(self);
        completed.owner = owner;
        completed.kind = LocalOwnerKind::Declaration;
        completed.order = order;
        completed.nested.extend(nested);
        diagnostics.append(&mut completed.diagnostic_slots);
        warnings.append(&mut completed.warning_slots);
        completed.diagnostic_slots = diagnostics;
        completed.warning_slots = warnings;
        self.body_checks[index] = BodyCheckInput::Local(completed);
    }

    /// Check one concrete callable's statements with prepared parameters, receiver, bounds and return context.
    pub(in crate::typechecker) fn check_callable_body(
        &mut self,
        body: &[Spanned<Statement>],
        is_async: bool,
        span: Span,
        context: YieldContext,
        ownership_source: CallableSource<'_>,
    ) {
        #[cfg(test)]
        if !self.legacy_body_checking {
            assert!(
                self.active_declaration_checks.is_empty(),
                "statement checking must consume completed declaration inputs"
            );
        }
        self.check_work.body_local_checks += 1;
        self.record_check_work("body", "callable", span);
        #[cfg(test)]
        if self.legacy_body_checking {
            self.retain_callable_input_bindings(ownership_source);
            self.check_callable_body_in_frame(body, is_async, context);
            self.record_explicit_input_flow(ownership_source, span);
            self.record_mutation_flow(span);
            return;
        }
        let scope = BodyCheckScope::enter(self);
        self.retain_callable_input_bindings(ownership_source);
        self.check_callable_body_in_frame(body, is_async, context);
        self.record_explicit_input_flow(ownership_source, span);
        self.record_mutation_flow(span);
        let mut local = scope.finish(self);
        local.owner = span;
        self.body_checks.push(BodyCheckInput::Local(local));
    }

    /// Check one computed property's statements in its own writer, preserving its original synchronous block rules.
    /// Property checking does not acquire callable-only iterator or resource finalization semantics at this boundary.
    pub(in crate::typechecker) fn check_property_body(&mut self, body: &[Spanned<Statement>], span: Span) {
        #[cfg(test)]
        if !self.legacy_body_checking {
            assert!(
                self.active_declaration_checks.is_empty(),
                "property checking must consume completed declaration inputs"
            );
        }
        self.check_work.body_local_checks += 1;
        self.check_work.property_bodies += 1;
        self.record_check_work("body", "property", span);
        #[cfg(test)]
        if self.legacy_body_checking {
            self.check_property_body_in_frame(body);
            return;
        }
        let scope = BodyCheckScope::enter(self);
        self.check_property_body_in_frame(body);
        let mut local = scope.finish(self);
        local.owner = span;
        self.body_checks.push(BodyCheckInput::Local(local));
    }
    /// Apply the property's existing synchronous statement semantics under its prepared receiver and return scope.
    /// Callable-only flow finalization stays outside this property boundary, matching the original checker.
    pub(super) fn check_property_body_in_frame(&mut self, body: &[Spanned<Statement>]) {
        let previous_async = std::mem::replace(&mut self.in_async_body, false);
        self.check_statement_block(body);
        self.in_async_body = previous_async;
    }
}

impl TypeChecker {
    /// Close effects from local projections once, finalize each lexical owner, and join its final facts. Recorded
    /// traversal ordinals restore legacy mutation and dictionary diagnostic order independently of completion order.
    /// `program` is the checked syntax; it binds stored diagnostics to this revision and is never checked again.
    pub(in crate::typechecker) fn settle_body_checks(&mut self, program: &Program) {
        let mut positions = DiagnosticPositions::new(program, self.symbols.declaration_origin());
        // ---- Final callee effects ----
        let locals = std::mem::take(&mut self.body_checks);
        let graphs = locals.iter().flat_map(|input| {
            let local = input.local();
            local
                .protocols
                .explicit_input_flow
                .values()
                .filter_map(|flow| match flow {
                    super::super::ownership_contracts::InputFlow::Checked { graph, .. }
                    | super::super::ownership_contracts::InputFlow::Invalid { graph, .. } => Some(graph),
                    super::super::ownership_contracts::InputFlow::Unavailable(_) => None,
                })
                .chain(local.protocols.mutation_flows.values())
        });
        let effects = super::super::ownership_contracts::flow::analyze_mutation(graphs);
        #[cfg(test)]
        {
            let mut inputs = self.mutation.clone();
            for local in &locals {
                inputs.append_body_local(local.local().mutation.clone());
            }
            let original = std::mem::replace(&mut self.mutation, inputs);
            assert_eq!(effects.changed, self.legacy_mut_param_change_closure());
            self.mutation = original;
        }
        self.check_work.mutation_settlements += 1;
        self.check_work.dictionary_settlements += 1;
        self.check_work.mutation_components += effects.components;
        self.check_work.mutation_member_visits += effects.members;
        let mut mutation = Vec::new();

        // ---- Declaration-expression owners ----
        let mut module = self.mutation.take_body_local();
        module.finalize_arguments(&effects, &mut mutation);
        module.clear_pending_arguments();
        self.mutation.install_body_local(module);
        let mut dictionary = std::mem::take(&mut self.pending_uncopyable_dict_lookups)
            .into_iter()
            .filter_map(|(order, span, value)| {
                (!self.type_info.is_read_only_dict_lookup(span)).then(|| {
                    (
                        order,
                        crate::diagnostics::errors::kept_dict_lookup_value_cannot_be_copied(&value, span),
                    )
                })
            })
            .collect::<Vec<_>>();

        // ---- Completed body publication ----
        for local in locals {
            let order = local.order();
            let checked = local.finalize(self, &effects, &mut positions);
            append_ordered_decisions(
                &mut mutation,
                &checked.mutation,
                (checked.local.order.mutation, order.mutation),
                &mut positions,
            );
            append_ordered_decisions(
                &mut dictionary,
                &checked.dictionary,
                (checked.local.order.dictionary, order.dictionary),
                &mut positions,
            );
            for value in &checked.open {
                self.body_open_diagnostics.push(positions.publish(value));
            }
            // Local diagnostics already occupy their original traversal positions.
            let mut published = checked.local.as_ref().clone();
            published.mutation.clear_pending_arguments();
            published.dictionary.clear();
            published.append(self);
            if checked.local.kind == LocalOwnerKind::Declaration {
                let owner = checked.owner();
                self.checked_declarations.push(checked);
                if let Ok(fields) = records::produce_declaration(self, program, owner) {
                    self.produced_declaration_records.insert(owner, fields);
                }
            } else {
                self.checked_bodies.push(checked);
            }
        }
        render_decisions(mutation, &mut self.errors, true);
        render_decisions(dictionary, &mut self.errors, false);
        let counts = positions.capture_counts();
        self.check_work.stored_diagnostics += counts.stored;
        self.check_work.node_relative_anchors += counts.within;
        self.check_work.detached_anchors += counts.detached;
    }
}

/// Render deferred refusals in their original stage/traversal order. Mutation arguments retain their legacy
/// span/message deduplication; dictionary refusals retain each recorded lookup and the catalog's exact wording.
pub(super) fn render_decisions(
    mut decisions: Vec<(usize, CompileError)>,
    errors: &mut Vec<CompileError>,
    deduplicate: bool,
) {
    decisions.sort_by_key(|(order, _)| *order);
    for (_, error) in decisions {
        if !deduplicate
            || !errors
                .iter()
                .any(|existing| existing.span == error.span && existing.message == error.message)
        {
            errors.push(error);
        }
    }
}

impl TypeChecker {
    /// Accept an unchanged declaration/default result whose declaration inputs the caller has already validated.
    /// Its stored diagnostics are bound to the current revision by `positions`. Reusing a statement body alone never
    /// supplies its declaration's checked expressions.
    pub(in crate::typechecker) fn reuse_checked_declaration(
        &mut self,
        checked: Arc<CheckedBodyView>,
        positions: &mut DiagnosticPositions<'_>,
    ) -> Result<(), AnchorFault> {
        debug_assert_eq!(checked.local.kind, LocalOwnerKind::Declaration);
        self.reuse_checked_local(checked, positions)
    }

    /// Accept a previous body only after its source, declaration/default/name inputs have been proved applicable by
    /// the caller. A stored diagnostic that does not bind to the current revision refuses the reuse before anything
    /// is published. This is a bootstrap adapter, never an implicit cache lookup by body name.
    pub(in crate::typechecker) fn reuse_checked_body(
        &mut self,
        checked: Arc<CheckedBodyView>,
        positions: &mut DiagnosticPositions<'_>,
    ) -> Result<(), AnchorFault> {
        debug_assert_eq!(checked.local.kind, LocalOwnerKind::Body);
        self.reuse_checked_local(checked, positions)
    }

    /// Bind one validated prior output to this activation's traversal order without replaying semantic checks.
    pub(super) fn reuse_checked_local(
        &mut self,
        checked: Arc<CheckedBodyView>,
        positions: &mut DiagnosticPositions<'_>,
    ) -> Result<(), AnchorFault> {
        let errors = checked
            .diagnostics
            .iter()
            .map(|value| positions.render(value))
            .collect::<Result<Vec<_>, _>>()?;
        let warnings = checked
            .warnings
            .iter()
            .map(|value| positions.render(value))
            .collect::<Result<Vec<_>, _>>()?;
        self.errors.extend(errors);
        self.warnings.extend(warnings);
        let order = BodyDiagnosticOrder {
            mutation: self.check_work.mutation_arguments,
            dictionary: self.check_work.dictionary_lookups,
        };
        self.check_work.mutation_arguments += checked.local.mutation.argument_count();
        self.check_work.dictionary_lookups += checked.local.dictionary.len();
        self.body_checks.push(BodyCheckInput::Reuse { checked, order });
        Ok(())
    }
}

/// Rebind a body's stored decision ordinals from their recorded start to this invocation's start, and bind each
/// stored value to the current revision. Construction guarantees that each stored ordinal is at least its recorded
/// start; source coordinates and producer completion order are irrelevant.
pub(super) fn append_ordered_decisions(
    target: &mut Vec<(usize, CompileError)>,
    source: &[(usize, AnchoredDiagnostic)],
    (old, current): (usize, usize),
    positions: &mut DiagnosticPositions<'_>,
) {
    target.extend(
        source
            .iter()
            .map(|(order, value)| (current + (order - old), positions.publish(value))),
    );
}

impl TypeChecker {
    /// Read a callable's settled statement result by its revision-local declaration span. A missing result means
    /// this activation has not settled that body or the declaration has no body; this accessor never checks it.
    pub(crate) fn checked_body(&self, declaration: Span) -> Option<&CheckedBodyView> {
        self.checked_bodies
            .iter()
            .find(|body| body.local.owner == declaration)
            .map(AsRef::as_ref)
    }

    /// Read settled declaration/default/initializer facts independently of statement facts. The span belongs to this
    /// checked source revision; callers must use the record adapter's identity and position binding across edits.
    pub(crate) fn checked_declaration(&self, declaration: Span) -> Option<&CheckedBodyView> {
        self.checked_declarations
            .iter()
            .find(|body| body.local.owner == declaration)
            .map(AsRef::as_ref)
    }
}

impl TypeChecker {
    /// Retain indexed references to completed native owners for the Body IR migration baseline, without checking or
    /// settling them again.
    pub(crate) fn body_ir_baseline_inputs(&self) -> BodyIrBaselineInputs {
        BodyIrBaselineInputs {
            declarations: self
                .checked_declarations
                .iter()
                .map(|body| (body.owner(), Arc::clone(body)))
                .collect(),
            bodies: self
                .checked_bodies
                .iter()
                .map(|body| (body.owner(), Arc::clone(body)))
                .collect(),
        }
    }
}

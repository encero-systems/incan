//! Dictionary lookup loans projected into the ordinary ownership graph.

use incan_lang::lang::builtins::BuiltinMethodId;
use incan_lang::lang::surface::dict_methods::DictMethodId;
use incan_semantics_core::CanonicalSymbolId;

use super::{Builder, Event, Expr, LoanId, Path, Spanned, Unavailable};
use crate::ast::{CallArg, MatchArm, MatchBody, Pattern, PatternArg};
use crate::typechecker::ownership_contracts::flow::{DictionaryLookup, Projection};

impl Builder<'_> {
    /// Project an `if let` or `while let` lookup condition, returning its matched path, unmatched path and temporary
    /// aliases. The condition's branch split is explicit in the two frontiers; only the matched path creates loans.
    pub(super) fn dictionary_condition(
        &mut self,
        pattern: &Spanned<Pattern>,
        lookup: &Spanned<Expr>,
        incoming: Vec<usize>,
    ) -> Result<Option<(Vec<usize>, Vec<usize>, Vec<CanonicalSymbolId>)>, Unavailable> {
        let Some((receiver, key)) = self.dictionary_lookup(lookup) else {
            return Ok(None);
        };
        let Some(mut source) = self.optional_place(receiver)? else {
            return Ok(None);
        };
        if self.checker.expr_reads_static_storage(receiver) {
            return Ok(None);
        }
        let incoming = self.evaluate_place(receiver, incoming)?;
        let evaluated = self.expression(key, incoming, false)?;
        source.place.projection.push(Projection::Element(key.span));
        let bindings = self.dictionary_arm_bindings(pattern)?;
        let mut matched = evaluated.clone();
        let loans = self.bind_dictionary_arm(&bindings, &source, lookup.span, &mut matched)?;
        self.record_dictionary_lookup(lookup.span, source.place, loans.iter().map(|(_, loan)| *loan).collect())?;
        Ok(Some((matched, evaluated, bindings)))
    }

    /// Remove pattern aliases after the branch in which their declarations are in scope.
    pub(super) fn leave_dictionary_bindings(&mut self, bindings: &[CanonicalSymbolId]) {
        for identity in bindings {
            self.aliases.remove(identity);
        }
    }

    /// Project a matched optional `Dict.get` as shared loans of the selected entry. The caller uses the ordinary
    /// graph analysis to decide whether those loans remain observations or require an owned copy.
    pub(super) fn dictionary_match(
        &mut self,
        lookup: &Spanned<Expr>,
        arms: &[Spanned<MatchArm>],
        incoming: Vec<usize>,
    ) -> Result<Option<Vec<usize>>, Unavailable> {
        let Some((receiver, key)) = self.dictionary_lookup(lookup) else {
            return Ok(None);
        };
        let Some(mut source) = self.optional_place(receiver)? else {
            return Ok(None);
        };
        if self.checker.expr_reads_static_storage(receiver) {
            return Ok(None);
        }
        let incoming = self.evaluate_place(receiver, incoming)?;
        let evaluated = self.expression(key, incoming, false)?;
        source.place.projection.push(Projection::Element(key.span));

        let mut outgoing = Vec::new();
        let mut loans = Vec::new();
        let mut remaining = evaluated;
        let entry = self.carrier_state();
        let mut ends = Vec::new();
        for arm in arms {
            self.restore_carrier_state(&entry);
            let bindings = self.dictionary_arm_bindings(&arm.node.pattern)?;
            let arm_loans = self.bind_dictionary_arm(&bindings, &source, lookup.span, &mut remaining)?;
            loans.extend(arm_loans.iter().map(|(_, loan)| *loan));
            let arm_input = if let Some(guard) = &arm.node.guard {
                let guarded = self.expression(guard, remaining.clone(), false)?;
                remaining = guarded.clone();
                guarded
            } else {
                remaining.clone()
            };
            let continuing = match &arm.node.body {
                MatchBody::Expr(value) => self.expression(value, arm_input, false)?,
                MatchBody::Block(body) => self.block(body, arm_input)?,
            };
            for (identity, _) in arm_loans {
                self.aliases.remove(&identity);
            }
            if !continuing.is_empty() {
                ends.push(self.carrier_state());
            }
            outgoing.extend(continuing);
        }
        self.join_carrier_states(&entry, &ends);
        self.record_dictionary_lookup(lookup.span, source.place, loans)?;
        Ok(Some(outgoing))
    }

    /// Retain one lookup's exact source and binding loans once.
    fn record_dictionary_lookup(
        &mut self,
        span: crate::ast::Span,
        source: super::Place,
        loans: Vec<LoanId>,
    ) -> Result<(), Unavailable> {
        if self
            .graph
            .dictionary_lookups
            .insert(span, DictionaryLookup { source, loans })
            .is_some()
        {
            return Err(Unavailable {
                span,
                reason: "dictionary lookup has more than one ownership projection".into(),
            });
        }
        Ok(())
    }

    /// Select only the checker-authenticated optional dictionary lookup and its one evaluated key.
    fn dictionary_lookup<'a>(&self, expression: &'a Spanned<Expr>) -> Option<(&'a Spanned<Expr>, &'a Spanned<Expr>)> {
        let Expr::MethodCall(receiver, _, _, arguments) = &expression.node else {
            return None;
        };
        if self.checker.type_info.resolved_builtin_method_call(expression.span)
            != Some(BuiltinMethodId::Dict(DictMethodId::Get))
        {
            return None;
        }
        let [CallArg::Positional(key)] = arguments.as_slice() else {
            return None;
        };
        Some((receiver, key))
    }

    /// Resolve every binding introduced by one accepted match pattern to its canonical declaration identity.
    fn dictionary_arm_bindings(&self, pattern: &Spanned<Pattern>) -> Result<Vec<CanonicalSymbolId>, Unavailable> {
        let mut bindings = Vec::new();
        self.collect_dictionary_bindings(pattern, &mut bindings)?;
        Ok(bindings)
    }

    /// Recursively retain binding identities without deriving ownership from pattern spelling.
    fn collect_dictionary_bindings(
        &self,
        pattern: &Spanned<Pattern>,
        bindings: &mut Vec<CanonicalSymbolId>,
    ) -> Result<(), Unavailable> {
        match &pattern.node {
            Pattern::Binding(name) => {
                let identity = self
                    .checker
                    .type_info
                    .resolved_write_identity(pattern.span, name)
                    .cloned()
                    .ok_or(Unavailable {
                        span: pattern.span,
                        reason: "missing checked dictionary match binding".into(),
                    })?;
                bindings.push(identity);
            }
            Pattern::Constructor(_, arguments) => {
                for argument in arguments {
                    match argument {
                        PatternArg::Positional(inner) | PatternArg::Named(_, inner) => {
                            self.collect_dictionary_bindings(inner, bindings)?;
                        }
                    }
                }
            }
            Pattern::Tuple(items) | Pattern::Or(items) => {
                for item in items {
                    self.collect_dictionary_bindings(item, bindings)?;
                }
            }
            Pattern::Group(inner) => self.collect_dictionary_bindings(inner, bindings)?,
            Pattern::Wildcard | Pattern::Literal(_) => {}
        }
        Ok(())
    }

    /// Bind each pattern local as a shared alias of the selected entry and emit the loan origin on this arm.
    fn bind_dictionary_arm(
        &mut self,
        bindings: &[CanonicalSymbolId],
        source: &Path,
        span: crate::ast::Span,
        incoming: &mut Vec<usize>,
    ) -> Result<Vec<(CanonicalSymbolId, LoanId)>, Unavailable> {
        let mut loans = Vec::new();
        for identity in bindings {
            let loan = self.call_loan(source.clone(), false, false, span);
            if self.graph.local_loans.insert(identity.clone(), loan).is_some() {
                return Err(Unavailable {
                    span: identity.declaration_span,
                    reason: "dictionary match binding has more than one ownership destination".into(),
                });
            }
            self.aliases.insert(
                identity.clone(),
                Path {
                    place: source.place.clone(),
                    through: Some(loan),
                },
            );
            *incoming = self.event(incoming.clone(), Event::Borrow(loan), span);
            loans.push((identity.clone(), loan));
        }
        Ok(loans)
    }
}

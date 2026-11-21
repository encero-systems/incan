/// Loops and comprehensions: the iterable evaluated once, fresh loop pattern bindings, and each poll's read through
/// the loop's own shared loan. Included by `accesses.rs` so the methods stay in its module.
impl SourceAccesses<'_, '_> {
    /// A loop evaluates its iterable once; a loop that takes the list's items consumes it. Each poll of a stored
    /// iterable reads it through the loop's own shared loan, created over that exact place at the iterable.
    fn for_loop(&mut self, loop_: &ForStmt) -> Result<(), &'static str> {
        self.for_iterable(&loop_.iter)?;
        let locals = self.enter_fresh_loop_pattern(&loop_.pattern)?;
        let result = self.block(&loop_.body);
        for local in locals {
            self.fresh_loop_locals.remove(&local);
        }
        result?;
        Ok(())
    }

    /// Claim the producer-retained replacement that initializes each canonical loop-pattern local on every poll.
    /// Statement loops and eager comprehensions share this exact boundary so neither can leave a valid access event
    /// unclaimed or silently infer authority from the authored pattern alone.
    fn enter_fresh_loop_pattern(
        &mut self,
        pattern: &Spanned<crate::ast::Pattern>,
    ) -> Result<Vec<incan_semantics_core::CanonicalSymbolId>, &'static str> {
        let locals = self.fresh_loop_pattern_bindings(pattern);
        self.fresh_loop_locals.extend(locals.iter().cloned());
        for local in &locals {
            let place = flow::Place {
                root: flow::Root::Binding(local.clone()),
                projection: Vec::new(),
            };
            self.known.insert(place.root.clone());
            self.access(pattern.span, &place, None, flow::Access::Replace)?;
        }
        Ok(locals)
    }

    fn fresh_loop_pattern_bindings(
        &self,
        pattern: &Spanned<crate::ast::Pattern>,
    ) -> Vec<incan_semantics_core::CanonicalSymbolId> {
        let mut bindings = Vec::new();
        self.collect_fresh_loop_pattern_bindings(pattern, &mut bindings);
        bindings
    }

    fn collect_fresh_loop_pattern_bindings(
        &self,
        pattern: &Spanned<crate::ast::Pattern>,
        bindings: &mut Vec<incan_semantics_core::CanonicalSymbolId>,
    ) {
        match &pattern.node {
            crate::ast::Pattern::Binding(name) | crate::ast::Pattern::MutBinding(name) => {
                if let Some(identity) = self
                    .proof
                    .facts
                    .resolved_write_identity(pattern.span, name)
                    .filter(|identity| identity.kind == SemanticSourceTargetKind::Local)
                {
                    bindings.push(identity.clone());
                }
            }
            crate::ast::Pattern::Tuple(items) => {
                for item in items {
                    self.collect_fresh_loop_pattern_bindings(item, bindings);
                }
            }
            crate::ast::Pattern::Group(inner) => self.collect_fresh_loop_pattern_bindings(inner, bindings),
            _ => {}
        }
    }

    /// Validate an eager comprehension's nested source loops, filters and produced values in evaluation order.
    fn comprehension(
        &mut self,
        clauses: &[crate::ast::ComprehensionClause],
        output: &[&Spanned<Expr>],
    ) -> Result<(), &'static str> {
        let Some((clause, remaining)) = clauses.split_first() else {
            for value in output {
                self.expression(value, false)?;
            }
            return Ok(());
        };
        match clause {
            crate::ast::ComprehensionClause::For { pattern, iter } => {
                self.for_iterable(iter)?;
                let locals = self.enter_fresh_loop_pattern(pattern)?;
                let result = self.comprehension(remaining, output);
                for local in locals {
                    self.fresh_loop_locals.remove(&local);
                }
                result
            }
            crate::ast::ComprehensionClause::If(condition) => {
                self.expression(condition, false)?;
                self.comprehension(remaining, output)
            }
        }
    }

    /// Validate one eager-loop iterable evaluation and its retained poll without inferring either access.
    fn for_iterable(&mut self, iter: &Spanned<Expr>) -> Result<(), &'static str> {
        let takes_items = self.proof.facts.for_loop_takes_items(iter.span);
        let iterable = if takes_items { None } else { self.place(iter)? };
        self.expression(iter, takes_items)?;
        if let Some((place, through)) = iterable {
            let loan = self
                .proof
                .graph
                .loans
                .iter()
                .position(|loan| {
                    loan.span == iter.span
                        && loan.source == place
                        && loan.parent == through
                        && loan.kind == flow::LoanKind::Shared
                })
                .ok_or("loop poll has no iteration loan")?;
            self.access(iter.span, &place, Some(flow::LoanId(loan)), flow::Access::Read)?;
        }
        Ok(())
    }
}

//! Eager comprehension projection. Each `for` clause is a real control-flow loop, so a retained reader used after a
//! source change on one iteration remains live across the back edge into the next iteration.

use super::*;

/// Values an eager comprehension materializes after all clauses accept the current bindings.
enum Output<'a> {
    List(&'a Spanned<Expr>),
    Dict(&'a Spanned<Expr>, &'a Spanned<Expr>),
}

impl Builder<'_> {
    /// Project a list comprehension as nested loops whose produced element transfers into fresh list storage.
    pub(super) fn list_comprehension(
        &mut self,
        comprehension: &crate::ast::ListComp,
        span: Span,
        incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        self.comprehension(
            &comprehension.clauses,
            Output::List(&comprehension.expr),
            span,
            incoming,
        )
    }

    /// Project a dictionary comprehension as nested loops whose key is read before its value is transferred.
    pub(super) fn dict_comprehension(
        &mut self,
        comprehension: &crate::ast::DictComp,
        span: Span,
        incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        self.comprehension(
            &comprehension.clauses,
            Output::Dict(&comprehension.key, &comprehension.value),
            span,
            incoming,
        )
    }

    /// Enter the checked clause sequence; an eager comprehension must contain at least one source loop.
    fn comprehension(
        &mut self,
        clauses: &[crate::ast::ComprehensionClause],
        output: Output<'_>,
        span: Span,
        incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        if !clauses
            .iter()
            .any(|clause| matches!(clause, crate::ast::ComprehensionClause::For { .. }))
        {
            return Err(Unavailable {
                span,
                reason: "comprehension has no checked source loop".into(),
            });
        }
        self.comprehension_clauses(clauses, &output, span, incoming)
    }

    /// Build nested `for` back edges and filter skip paths in source order, then evaluate the materialized value.
    fn comprehension_clauses(
        &mut self,
        clauses: &[crate::ast::ComprehensionClause],
        output: &Output<'_>,
        span: Span,
        incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        let Some((clause, remaining)) = clauses.split_first() else {
            return match output {
                Output::List(value) => self.expression(value, incoming, true),
                Output::Dict(key, value) => {
                    let incoming = self.expression(key, incoming, false)?;
                    self.expression(value, incoming, true)
                }
            };
        };
        match clause {
            crate::ast::ComprehensionClause::For { pattern, iter } => {
                let (header, poll) = self.for_header(pattern, iter, span, incoming)?;
                let body = self.comprehension_clauses(remaining, output, span, poll.clone())?;
                self.connect(&body, header);
                Ok(poll)
            }
            crate::ast::ComprehensionClause::If(condition) => {
                let checked = self.expression(condition, incoming, false)?;
                let mut accepted = self.comprehension_clauses(remaining, output, span, checked.clone())?;
                accepted.extend(checked);
                Ok(accepted)
            }
        }
    }
}

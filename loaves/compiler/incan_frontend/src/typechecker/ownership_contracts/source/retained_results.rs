//! Lazy builtin results retain ordinary ownership-graph loans to the storage they read.
//!
//! The typechecker has already selected every method through a canonical builtin identity. This module only turns
//! those typed selections and generator-expression syntax into the same dependency facts returned views and native
//! borrowed results use; it never recognizes a dependency from a method's source spelling.

use super::*;
use crate::ast::CallArg;
use incan_lang::lang::surface::iterator_methods::{self, IteratorMethodId};
use incan_lang::lang::types::collections::{self, CollectionTypeId};

/// One evaluated lazy value and the graph loans stored in that value.
pub(super) struct RetainedResult {
    pub incoming: Vec<usize>,
    pub loans: Vec<LoanId>,
}

impl Builder<'_> {
    /// Evaluate a compiler-identified lazy result. `escaping` allows a whole transferable list source to move into
    /// the returned cursor; every other stored source retains a checked shared dependency.
    pub(super) fn retained_result(
        &mut self,
        expression: &Spanned<Expr>,
        incoming: Vec<usize>,
        escaping: bool,
    ) -> Result<Option<RetainedResult>, Unavailable> {
        match &expression.node {
            Expr::Paren(inner) => self.retained_result(inner, incoming, escaping),
            Expr::Generator(generator) => self
                .generator_result(generator, expression.span, incoming, escaping)
                .map(Some),
            Expr::MethodCall(receiver, _, _, arguments) => {
                let Some(method) = self.retained_iterator_method(expression.span) else {
                    return Ok(None);
                };
                self.iterator_result(receiver, arguments, method, expression.span, incoming, escaping)
                    .map(Some)
            }
            _ => Ok(None),
        }
    }

    /// Return dependencies stored by an existing place after recording its value use.
    pub(super) fn stored_result(
        &mut self,
        expression: &Spanned<Expr>,
        incoming: Vec<usize>,
    ) -> Result<Option<RetainedResult>, Unavailable> {
        let Some(path) = self.optional_place(expression)? else {
            return Ok(None);
        };
        let Root::Binding(root) = &path.place.root else {
            return Ok(None);
        };
        let Some(loans) = self.graph.dependent_locals.get(root).cloned() else {
            return Ok(None);
        };
        if !loans
            .iter()
            .any(|loan| super::super::builtins::lazy_result_loan(&self.graph, &self.checker.type_info, *loan))
        {
            return Ok(None);
        }
        let incoming = self.expression(expression, incoming, false)?;
        Ok(Some(RetainedResult { incoming, loans }))
    }

    /// Evaluate a builtin `Some`, `Ok` or `Err` whose payload retains a lazy dependency. The carrier owns its payload
    /// without erasing that payload's source loans; an owned payload with no loans remains independent.
    pub(super) fn retained_carrier(
        &mut self,
        expression: &Spanned<Expr>,
        incoming: Vec<usize>,
        escaping: bool,
    ) -> Result<Option<RetainedResult>, Unavailable> {
        let Some((_variant, arguments)) =
            super::super::builtins::carrier_construction(&self.checker.type_info, expression)
        else {
            return Ok(None);
        };
        let [CallArg::Positional(payload)] = arguments else {
            return Err(Unavailable {
                span: expression.span,
                reason: "lazy carrier needs one positional payload".into(),
            });
        };
        let result = match self.retained_result(payload, incoming.clone(), escaping)? {
            Some(result) => result,
            None => match self.stored_result(payload, incoming)? {
                Some(result) => result,
                None => return Ok(None),
            },
        };
        if result.loans.is_empty() {
            return Ok(Some(result));
        }
        self.carry_result(result.loans, expression.span, result.incoming)
            .map(Some)
    }

    /// Evaluate a source that may itself be a lazy result, a stored dependent value or a list place.
    fn retained_source(
        &mut self,
        expression: &Spanned<Expr>,
        incoming: Vec<usize>,
        result_span: Span,
        escaping: bool,
    ) -> Result<RetainedResult, Unavailable> {
        if let Some(result) = self.retained_result(expression, incoming.clone(), escaping)? {
            return Ok(result);
        }
        if let Some(result) = self.stored_result(expression, incoming.clone())? {
            return Ok(result);
        }
        let list = matches!(self.checker.type_info.expr_type(expression.span),
            Some(ResolvedType::Generic(name, arguments))
                if collections::from_str(name) == Some(CollectionTypeId::List) && arguments.len() == 1);
        if list && let Some(path) = self.optional_place(expression)? {
            let incoming = self.evaluate_place(expression, incoming)?;
            let transferable = path.place.projection.is_empty()
                && self
                    .graph
                    .permissions
                    .get(&path.place.root)
                    .is_some_and(|permission| permission.transferable)
                && path.through.is_none();
            if escaping && transferable {
                let incoming = self.event(
                    incoming,
                    Event::Access {
                        place: path.place,
                        access: Access::Move,
                        through: None,
                    },
                    expression.span,
                );
                return Ok(RetainedResult {
                    incoming,
                    loans: Vec::new(),
                });
            }
            return self.borrow_result(path, result_span, incoming);
        }
        let incoming = self.expression(expression, incoming, true)?;
        Ok(RetainedResult {
            incoming,
            loans: Vec::new(),
        })
    }

    /// Create one shared result loan at the checked lazy expression that retains it.
    fn borrow_result(&mut self, path: Path, span: Span, incoming: Vec<usize>) -> Result<RetainedResult, Unavailable> {
        let loan = LoanId(self.graph.loans.len());
        self.graph.loans.push(Loan {
            source: path.place,
            kind: LoanKind::Shared,
            writable: false,
            parent: path.through,
            span,
        });
        Ok(RetainedResult {
            incoming: self.event(incoming, Event::Borrow(loan), span),
            loans: vec![loan],
        })
    }

    /// Reborrow dependencies into a new adapter or generator value, preserving every source through the result.
    fn carry_result(
        &mut self,
        dependencies: Vec<LoanId>,
        span: Span,
        mut incoming: Vec<usize>,
    ) -> Result<RetainedResult, Unavailable> {
        let mut loans = Vec::with_capacity(dependencies.len());
        for parent in dependencies {
            let source = self
                .graph
                .loans
                .get(parent.0)
                .map(|loan| loan.source.clone())
                .ok_or(Unavailable {
                    span,
                    reason: "lazy result has no retained source loan".into(),
                })?;
            let loan = LoanId(self.graph.loans.len());
            self.graph.loans.push(Loan {
                source,
                kind: LoanKind::Shared,
                writable: false,
                parent: Some(parent),
                span,
            });
            incoming = self.event(incoming, Event::Borrow(loan), span);
            loans.push(loan);
        }
        Ok(RetainedResult { incoming, loans })
    }

    /// Reborrow one lazy dependency into another carrier while preserving its public diagnostic identity.
    pub(super) fn carry_dependency(
        &mut self,
        parent: LoanId,
        span: Span,
        incoming: Vec<usize>,
    ) -> Result<(Vec<usize>, Path, LoanId), Unavailable> {
        let source = self
            .graph
            .loans
            .get(parent.0)
            .map(|loan| loan.source.clone())
            .ok_or(Unavailable {
                span,
                reason: "lazy carrier has no retained source loan".into(),
            })?;
        let loan = LoanId(self.graph.loans.len());
        self.graph.loans.push(Loan {
            source: source.clone(),
            kind: LoanKind::Shared,
            writable: false,
            parent: Some(parent),
            span,
        });
        let incoming = self.event(incoming, Event::Borrow(loan), span);
        Ok((
            incoming,
            Path {
                place: source,
                through: Some(loan),
            },
            loan,
        ))
    }

    /// Evaluate the first eager source of a generator expression and retain it through the generator value.
    fn generator_result(
        &mut self,
        generator: &crate::ast::GeneratorExpr,
        span: Span,
        incoming: Vec<usize>,
        escaping: bool,
    ) -> Result<RetainedResult, Unavailable> {
        let Some(crate::ast::ComprehensionClause::For { iter, .. }) = generator.clauses.first() else {
            return Err(Unavailable {
                span,
                reason: "generator expression has no first source".into(),
            });
        };
        let result = self.retained_source(iter, incoming, span, escaping)?;
        if result.loans.is_empty() || result.loans.iter().all(|loan| self.graph.loans[loan.0].span == span) {
            return Ok(result);
        }
        self.carry_result(result.loans, span, result.incoming)
    }

    /// Evaluate an iterator method, carrying dependencies through adapters and ending them at terminal calls.
    fn iterator_result(
        &mut self,
        receiver: &Spanned<Expr>,
        arguments: &[CallArg],
        method: IteratorMethodId,
        span: Span,
        incoming: Vec<usize>,
        escaping: bool,
    ) -> Result<RetainedResult, Unavailable> {
        if method == IteratorMethodId::Iter {
            return self.retained_source(receiver, incoming, span, escaping);
        }
        let mut result = self.retained_source(receiver, incoming, span, escaping)?;
        let mut dependencies = result.loans;
        for (index, argument) in arguments.iter().enumerate() {
            let value = match argument {
                CallArg::Positional(value) | CallArg::Named(_, value) => value,
                _ => {
                    return Err(Unavailable {
                        span,
                        reason: "lazy iterator call needs fixed arguments".into(),
                    });
                }
            };
            let carries_iterable = index == 0 && matches!(method, IteratorMethodId::Zip | IteratorMethodId::Chain);
            if carries_iterable {
                let carried = self.retained_source(value, result.incoming, span, false)?;
                result.incoming = carried.incoming;
                dependencies.extend(carried.loans);
            } else if !matches!(
                self.checker.type_info.expr_type(value.span),
                Some(ResolvedType::Function(..))
            ) {
                result.incoming = self.expression(value, result.incoming, false)?;
            }
        }
        if iterator_methods::is_terminal(method) {
            for loan in dependencies {
                let source = self
                    .graph
                    .loans
                    .get(loan.0)
                    .map(|loan| loan.source.clone())
                    .ok_or(Unavailable {
                        span,
                        reason: "iterator terminal has no retained source loan".into(),
                    })?;
                result.incoming = self.event(
                    result.incoming,
                    Event::Access {
                        place: source,
                        access: Access::Read,
                        through: Some(loan),
                    },
                    span,
                );
            }
            return Ok(RetainedResult {
                incoming: result.incoming,
                loans: Vec::new(),
            });
        }
        if dependencies.is_empty() {
            return Ok(RetainedResult {
                incoming: result.incoming,
                loans: Vec::new(),
            });
        }
        self.carry_result(dependencies, span, result.incoming)
    }

    /// Select the exact builtin iterator member recorded for this call.
    fn retained_iterator_method(&self, span: Span) -> Option<IteratorMethodId> {
        super::super::builtins::retained_iterator_method(&self.checker.type_info, span)
    }

    /// Make an escaping stored dependency conflict with destruction of its source, then retain its returned use.
    pub(super) fn escape_retained_result(
        &mut self,
        result: RetainedResult,
        span: Span,
    ) -> Result<Vec<usize>, Unavailable> {
        let mut incoming = result.incoming;
        for loan in result.loans {
            let source = self
                .graph
                .loans
                .get(loan.0)
                .map(|loan| loan.source.clone())
                .ok_or(Unavailable {
                    span,
                    reason: "escaping lazy result has no retained source loan".into(),
                })?;
            incoming = self.event(
                incoming,
                Event::Access {
                    place: source.clone(),
                    access: Access::Move,
                    through: None,
                },
                span,
            );
            incoming = self.event(
                incoming,
                Event::Access {
                    place: source,
                    access: Access::Read,
                    through: Some(loan),
                },
                span,
            );
        }
        Ok(incoming)
    }
}

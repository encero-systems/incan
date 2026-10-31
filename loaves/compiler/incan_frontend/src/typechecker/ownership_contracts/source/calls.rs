//! Selected source-call access projection. Declaration modes remain canonical; argument evaluation order is
//! preserved independently of parameter order. Returned/native dependencies and unresolved adaptations refuse
//! this profile until their own facts are available.

use super::{Access, Builder, Event, Loan, LoanId, LoanKind, OwnershipMode, Path, ResolvedType, Unavailable};
use crate::ast::{CallArg, Expr, ParamKind, Span, Spanned};
use crate::symbols::CallableParam;
use crate::typechecker::TypeChecker;

impl Builder<'_> {
    /// Apply a selected free-function contract without inspecting its body. Borrowed arguments remain live until
    /// invocation, and owned arguments transfer at their source evaluation position even for a read-only callee.
    /// Missing default/rest/capture/returned-dependency facts never become a successful partial call graph.
    pub(super) fn call(
        &mut self,
        callee: &Spanned<Expr>,
        arguments: &[CallArg],
        span: Span,
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        if !matches!(callee.node, Expr::Ident(_)) {
            return Err(Unavailable {
                span,
                reason: "callee evaluation needs checked access facts",
            });
        }
        let call = self
            .checker
            .type_info
            .protocols
            .explicit_input_calls
            .get(&span)
            .ok_or(Unavailable {
                span,
                reason: "call has no selected explicit input access facts",
            })?;
        let contract = self
            .checker
            .type_info
            .declarations
            .ownership_contracts
            .get(&call.callee)
            .ok_or(Unavailable {
                span,
                reason: "selected callable contract is missing",
            })?;
        if call.owner.as_ref() != Some(&self.owner) {
            return Err(Unavailable {
                span,
                reason: "call facts belong to a different callable",
            });
        }
        if contract.result_source.is_some()
            || contract.modes.iter().any(|mode| {
                mode.component.parameter.is_none()
                    || !mode.component.children.is_empty()
                    || mode.mode == OwnershipMode::Reference
            })
            || call
                .parameters
                .iter()
                .any(|parameter| parameter.kind != ParamKind::Normal || parameter.is_partial_preset)
            || arguments.len() != call.parameters.len()
        {
            return Err(Unavailable {
                span,
                reason: "call needs returned, default or packed-argument dependency facts",
            });
        }
        let result = self.checker.type_info.expr_type(span).ok_or(Unavailable {
            span,
            reason: "missing call result type",
        })?;
        if !plain_result(result) {
            return Err(Unavailable {
                span,
                reason: "call result needs retained dependency facts",
            });
        }
        let paired = TypeChecker::arguments_with_parameter_slots(&call.parameters, arguments);
        if paired.len() != arguments.len() {
            return Err(Unavailable {
                span,
                reason: "unpacked call requires checked argument expansion",
            });
        }
        let mut loans = Vec::new();
        for (expression, slot) in paired {
            let slot = slot.ok_or(Unavailable {
                span: expression.span,
                reason: "missing selected argument slot",
            })?;
            let mode = contract
                .modes
                .iter()
                .find(|mode| mode.component.parameter == Some(slot))
                .map(|mode| mode.mode);
            let (outgoing, loan) = self.call_argument(expression, &call.parameters[slot], mode, incoming)?;
            incoming = outgoing;
            if let Some(loan) = loan {
                loans.push(loan);
            }
        }
        // Invocation observes every borrowed argument, even when the callee implementation does not use it.
        for id in loans {
            let loan = &self.graph.loans[id.0];
            incoming = self.event(
                incoming,
                Event::Access {
                    place: loan.source.clone(),
                    access: Access::Read,
                    through: Some(id),
                },
                span,
            );
        }
        Ok(incoming)
    }

    /// Evaluate one argument according to its selected slot. A borrowed place is evaluated once before taking its
    /// loan; an explicit ownership request constrains even an otherwise ordinary source binding. Unresolved
    /// duckborrow or ordinary mutable adapters refuse rather than cloning or guessing an access mode.
    fn call_argument(
        &mut self,
        expression: &Spanned<Expr>,
        parameter: &CallableParam,
        mode: Option<OwnershipMode>,
        incoming: Vec<usize>,
    ) -> Result<(Vec<usize>, Option<LoanId>), Unavailable> {
        match mode {
            Some(OwnershipMode::Borrowed | OwnershipMode::BorrowedMut) => {
                let source = self.place(expression)?;
                let incoming = self.evaluate_place(expression, incoming)?;
                let loan = self.call_loan(
                    source,
                    mode == Some(OwnershipMode::BorrowedMut),
                    parameter,
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
            _ => Err(Unavailable {
                span: expression.span,
                reason: "ordinary argument requires inferred access or mutable binding facts",
            }),
        }
    }

    /// A borrowed argument derives its permissions from the evaluated source. Exclusive access and permission to
    /// write remain separate, and reborrowing preserves the parent's dependency rather than upgrading the owner.
    fn call_loan(&mut self, source: Path, exclusive: bool, parameter: &CallableParam, span: Span) -> LoanId {
        let id = LoanId(self.graph.loans.len());
        self.graph.loans.push(Loan {
            source: source.place,
            kind: if exclusive {
                LoanKind::Exclusive
            } else {
                LoanKind::Shared
            },
            writable: exclusive && parameter.is_mut,
            parent: source.through,
            span,
        });
        id
    }
}

/// This initial call profile admits only closed scalar/owned byte-string results, with no possible retained source
/// dependency. Aggregates, references, native and generic results await their checked dependency representation.
fn plain_result(ty: &ResolvedType) -> bool {
    matches!(
        ty,
        ResolvedType::Unit
            | ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Numeric(_)
            | ResolvedType::Bool
            | ResolvedType::Str
            | ResolvedType::Bytes
            | ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes
    )
}

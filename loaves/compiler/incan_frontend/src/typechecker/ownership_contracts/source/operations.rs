//! Operation-local shared loans prevent hidden snapshots while later operands evaluate.

use super::{
    Access, Builder, Event, Expr, Loan, LoanId, LoanKind, OperandDependency, OperationInputs, Place, Span, Spanned,
    Unavailable,
};

/// An evaluated operand owns a shared read dependency, not the storage itself.
pub(super) struct PendingOperand {
    operand: Span,
    source: Place,
    loan: LoanId,
}

impl Builder<'_> {
    /// Take a shared read loan immediately after evaluating a non-Copy borrowed operand. Even an exclusive alias
    /// needs a shared child: retaining only its exclusive parent would allow later arguments to mutate through it.
    pub(super) fn retain_operation_operand(
        &mut self,
        expression: &Spanned<Expr>,
        incoming: Vec<usize>,
    ) -> Result<(Vec<usize>, Option<PendingOperand>), Unavailable> {
        let ty = self.checker.type_info.expr_type(expression.span).ok_or(Unavailable {
            span: expression.span,
            reason: "missing checked operand type".into(),
        })?;
        if self.copy_value(ty) {
            return Ok((incoming, None));
        }
        let Some(path) = self.optional_place(expression)? else {
            return Ok((incoming, None));
        };
        if path.through.is_none() && !self.borrowed_inputs.contains(&path.place.root) {
            return Ok((incoming, None));
        }
        let id = LoanId(self.graph.loans.len());
        self.graph.loans.push(Loan {
            source: path.place.clone(),
            kind: LoanKind::Shared,
            writable: false,
            parent: path.through,
            span: expression.span,
        });
        let outgoing = self.event(incoming, Event::Borrow(id), expression.span);
        Ok((
            outgoing,
            Some(PendingOperand {
                operand: expression.span,
                source: path.place,
                loan: id,
            }),
        ))
    }

    /// Consume the retained operands only after evaluating the complete operation. Fresh results and Copy values
    /// have no pending loan; completion records link existing evaluated storage to exact final read events.
    pub(super) fn finish_operation(
        &mut self,
        operation: Span,
        retained: Vec<PendingOperand>,
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        if retained.is_empty() {
            return Ok(incoming);
        }
        let completion_node = self.graph.nodes.len();
        incoming = self.event(incoming, Event::Nothing, operation);
        let mut operands = Vec::new();
        for PendingOperand { operand, source, loan } in retained {
            let final_access = self.graph.nodes.len();
            incoming = self.event(
                incoming,
                Event::Access {
                    place: source.clone(),
                    access: Access::Read,
                    through: Some(loan),
                },
                operand,
            );
            operands.push(OperandDependency {
                operand,
                source,
                loan,
                final_access,
            });
        }
        if self
            .graph
            .operations
            .insert(
                operation,
                OperationInputs {
                    completion_node,
                    operands,
                },
            )
            .is_some()
        {
            return Err(Unavailable {
                span: operation,
                reason: "operation has more than one dependency binding".into(),
            });
        }
        Ok(incoming)
    }
}

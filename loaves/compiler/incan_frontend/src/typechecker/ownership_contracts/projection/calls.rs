//! Project an authenticated invocation into the facts required by native argument materialization.

use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, IncanInputAccess};

use super::{CheckedInputBody, TypeCheckInfo, flow};
use crate::ast::{CallArg, Expr, Spanned};
use crate::symbols::CallableParam;

/// A checked input source names existing storage, a retained view-carrying local or a real invocation-bounded
/// temporary.
pub(crate) enum CheckedInputSource {
    /// Field projections retain canonical declaration identities rather than matching display names.
    Place {
        root: CanonicalSymbolId,
        fields: Vec<CanonicalSymbolId>,
        /// Zero-based projection positions that select an element. The checked expression span owns evaluation;
        /// Body IR authenticates that each position remains an index instead of a field.
        elements: Vec<usize>,
    },
    /// A stored aggregate owns its carrier while retaining a checked loan to one borrowed component.
    CarriedLocal,
    /// The source graph owns the temporary's expression and invocation ancestry.
    Temporary,
}

/// One argument in actual evaluation order, with its original written position kept separately.
pub(crate) struct CheckedInputArgument {
    pub span: HirSourceSpan,
    pub slot: usize,
    pub written_position: usize,
    pub access: IncanInputAccess,
    pub source: Option<CheckedInputSource>,
    /// A native result reference is evaluated and reborrowed; its retained owner loan is not its physical place.
    pub reference_result: bool,
}

/// A method receiver borrowed for one invocation: shared for `self`, exclusive for `mut self`.
pub(crate) struct CheckedInputReceiver {
    pub source: Option<CheckedInputSource>,
    pub exclusive: bool,
    pub owned: bool,
}

/// Instantiated value parameters and authenticated source access belong to one selected invocation.
pub(crate) struct CheckedInputInvocation<'a> {
    pub parameters: &'a [CallableParam],
    pub arguments: Vec<CheckedInputArgument>,
    pub receiver: Option<CheckedInputReceiver>,
    /// Direct functions evaluate selected slots in declaration order; callable values and methods keep written order.
    pub declaration_order: bool,
}

impl<'a> CheckedInputBody<'a> {
    /// Separate a checked native reference result from the owner whose lifetime protects it. A result loan and
    /// its invocation must agree exactly; an ordinary owner or a reference-shaped annotation grants no shortcut.
    fn native_reference_argument(
        &self,
        argument: &flow::InvocationArgument,
        mut expression: &Spanned<Expr>,
    ) -> Result<bool, &'static str> {
        if expression.span != argument.span {
            return Err("native input expression differs from its checked argument");
        }
        while let Expr::Paren(inner) = &expression.node {
            expression = inner;
        }
        let Some(call) = self.facts.rust.checked_calls.get(&expression.span) else {
            return Ok(false);
        };
        let mutable = match call.return_type {
            crate::symbols::ResolvedType::Ref(_) => false,
            crate::symbols::ResolvedType::RefMut(_) => true,
            _ => return Ok(false),
        };
        let Some(loan) = argument.loan.and_then(|id| self.graph.loans.get(id.0)) else {
            return Ok(false);
        };
        let Some(parent) = loan.parent.and_then(|id| self.graph.loans.get(id.0)) else {
            return Ok(false);
        };
        if parent.span != expression.span {
            return Ok(false);
        }
        if argument.source.as_ref() != Some(&loan.source)
            || parent.source != loan.source
            || !self.graph.invocations.contains_key(&expression.span)
            || !self
                .facts
                .protocols
                .explicit_input_calls
                .get(&expression.span)
                .is_some_and(|call| call.kind == super::super::InputCallKind::Native)
            || (argument.access == IncanInputAccess::Exclusive && !mutable)
            || !matches!(argument.access, IncanInputAccess::Shared | IncanInputAccess::Exclusive)
        {
            return Err("native reference argument differs from its checked result loan");
        }
        Ok(true)
    }

    /// Select exactly the call already authenticated in this body's flow. Missing or unsupported source facts
    /// refuse rather than recovering access from the native signature or the spelling of the function.
    pub(crate) fn invocation(
        &self,
        span: HirSourceSpan,
        callee: &CanonicalSymbolId,
        written_arguments: &[CallArg],
    ) -> Result<CheckedInputInvocation<'a>, &'static str> {
        let invocation = self
            .graph
            .invocations
            .get(&span)
            .ok_or("explicit invocation has no source flow binding")?;
        if &invocation.callee != callee {
            return Err("explicit invocation selected a different canonical callee");
        }
        let call = self
            .facts
            .protocols
            .explicit_input_calls
            .get(&span)
            .ok_or("explicit invocation has no checked parameters")?;
        let mut arguments = Vec::with_capacity(invocation.arguments.len());
        for argument in &invocation.arguments {
            let expression = match written_arguments.get(argument.written_position) {
                Some(CallArg::Positional(value) | CallArg::Named(_, value)) => value,
                _ => return Err("explicit invocation has no matching written argument"),
            };
            let source = argument.source.as_ref().map(project_source).transpose()?;
            arguments.push(CheckedInputArgument {
                span: argument.span,
                slot: argument.slot,
                written_position: argument.written_position,
                access: argument.access,
                reference_result: self.native_reference_argument(argument, expression)?,
                source,
            });
        }
        let receiver = if invocation.receiver_owned {
            Some(CheckedInputReceiver {
                source: invocation.receiver_source.as_ref().map(project_source).transpose()?,
                exclusive: false,
                owned: true,
            })
        } else {
            invocation
                .receiver
                .map(|loan| {
                    let loan = self
                        .graph
                        .loans
                        .get(loan.0)
                        .ok_or("explicit method receiver has no checked loan")?;
                    Ok::<_, &'static str>(CheckedInputReceiver {
                        source: Some(project_source(&loan.source)?),
                        exclusive: loan.kind == flow::LoanKind::Exclusive,
                        owned: false,
                    })
                })
                .transpose()?
        };
        Ok(CheckedInputInvocation {
            parameters: &call.parameters,
            arguments,
            receiver,
            declaration_order: call.kind != super::super::InputCallKind::Indirect && call.receiver.is_none(),
        })
    }
}

impl TypeCheckInfo {
    /// The direct returned view a selected call produces, read from its callee's contract: the access, the source
    /// slot and the declared stored-field path. Calls without a direct returned view, including calls whose view is
    /// carried in a `Result` or `Option`, or without selected facts, give `None`.
    pub(crate) fn selected_call_view(
        &self,
        span: HirSourceSpan,
    ) -> Option<(
        IncanInputAccess,
        crate::ownership_metadata::SourceSlot,
        Vec<CanonicalSymbolId>,
    )> {
        let view = self
            .selected_returned_view(span)
            .filter(|view| view.component.is_empty())?;
        Some((view.access, view.source.slot.clone(), view.source.fields.clone()))
    }

    /// The returned view a selected call carries in one payload of its result, with the same access, source slot
    /// and stored-field path as a direct view, and the payload carrying it: `0` for `Ok` or `Some`, `1` for `Err`,
    /// or the borrowed element's position in a tuple.
    pub(crate) fn selected_carried_view(
        &self,
        span: HirSourceSpan,
    ) -> Option<(
        IncanInputAccess,
        crate::ownership_metadata::SourceSlot,
        Vec<CanonicalSymbolId>,
        usize,
    )> {
        let view = self.selected_returned_view(span)?;
        Some((
            view.access,
            view.source.slot.clone(),
            view.source.fields.clone(),
            view.carried?,
        ))
    }

    /// The returned view stored inside a user-defined or nested aggregate, including its complete result component.
    pub(crate) fn selected_aggregate_view(
        &self,
        span: HirSourceSpan,
    ) -> Option<(
        IncanInputAccess,
        crate::ownership_metadata::SourceSlot,
        Vec<CanonicalSymbolId>,
        Vec<usize>,
    )> {
        let view = self
            .selected_returned_view(span)
            .filter(|view| !view.component.is_empty() && view.carried.is_none())?;
        Some((
            view.access,
            view.source.slot.clone(),
            view.source.fields.clone(),
            view.component.to_vec(),
        ))
    }

    fn selected_returned_view(&self, span: HirSourceSpan) -> Option<crate::ownership_metadata::ReturnedView<'_>> {
        let call = self.protocols.explicit_input_calls.get(&span)?;
        let contract = self.selected_input_contract(call).ok()?;
        match contract {
            Some(std::borrow::Cow::Borrowed(contract)) => {
                crate::ownership_metadata::returned_view(Some(contract)).ok()?
            }
            Some(std::borrow::Cow::Owned(_)) | None => None,
        }
    }

    /// Select the explicit route even when its owning body proof is absent, so defaults and indirect evaluations
    /// cannot fall back to ordinary inferred passing merely because the supported source flow was unavailable.
    pub(crate) fn has_explicit_input_invocation(
        &self,
        span: HirSourceSpan,
        callee: Option<&CanonicalSymbolId>,
    ) -> bool {
        self.protocols.explicit_input_calls.contains_key(&span)
            || callee
                .and_then(|callee| self.declarations.ownership_contracts.get(callee))
                .is_some_and(|contract| {
                    contract.result_source.is_some()
                        || contract.modes.iter().any(|mode| mode.component.parameter.is_some())
                })
    }
}

/// Static stored fields can be authenticated against Body IR places. Indexed adaptation needs its own evaluated
/// index identity and native no-copy proof; this first consumer deliberately keeps that shape refused.
pub(super) fn project_source(source: &flow::Place) -> Result<CheckedInputSource, &'static str> {
    match &source.root {
        flow::Root::Binding(root) => {
            let mut fields = Vec::with_capacity(source.projection.len());
            let mut elements = Vec::new();
            for (position, projection) in source.projection.iter().enumerate() {
                match projection {
                    flow::Projection::Field(field) => fields.push(field.clone()),
                    flow::Projection::Element(_) => elements.push(position),
                }
            }
            Ok(CheckedInputSource::Place {
                root: root.clone(),
                fields,
                elements,
            })
        }
        flow::Root::Temporary { .. } if source.projection.is_empty() => Ok(CheckedInputSource::Temporary),
        _ => Err("explicit temporary input has an unsupported projection"),
    }
}

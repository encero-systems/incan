//! Project an authenticated invocation into the facts required by native argument materialization.

use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, IncanInputAccess};

use super::{CheckedInputBody, TypeCheckInfo, flow};
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
    /// Select exactly the call already authenticated in this body's flow. Missing or unsupported source facts
    /// refuse rather than recovering access from the native signature or the spelling of the function.
    pub(crate) fn invocation(
        &self,
        span: HirSourceSpan,
        callee: &CanonicalSymbolId,
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
            let source = argument.source.as_ref().map(project_source).transpose()?;
            arguments.push(CheckedInputArgument {
                span: argument.span,
                slot: argument.slot,
                written_position: argument.written_position,
                access: argument.access,
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
            .filter(|view| view.carried.is_none())?;
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

    fn selected_returned_view(&self, span: HirSourceSpan) -> Option<crate::ownership_metadata::ReturnedView<'_>> {
        let call = self.protocols.explicit_input_calls.get(&span)?;
        let contract = self.selected_input_contract(call).ok()?;
        crate::ownership_metadata::returned_view(contract).ok()?
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

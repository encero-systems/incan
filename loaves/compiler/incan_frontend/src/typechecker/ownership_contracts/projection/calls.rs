//! Project an authenticated invocation into the facts required by native argument materialization.

use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, IncanInputAccess};

use super::{CheckedInputBody, TypeCheckInfo, flow};
use crate::symbols::CallableParam;

/// A checked source argument names existing storage or a real invocation-bounded temporary.
pub(crate) enum CheckedInputSource {
    /// Field projections retain canonical declaration identities rather than matching display names.
    Place {
        root: CanonicalSymbolId,
        fields: Vec<CanonicalSymbolId>,
    },
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

/// Instantiated value parameters and authenticated source access belong to one selected invocation.
pub(crate) struct CheckedInputInvocation<'a> {
    pub parameters: &'a [CallableParam],
    pub arguments: Vec<CheckedInputArgument>,
    pub has_receiver: bool,
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
        Ok(CheckedInputInvocation {
            parameters: &call.parameters,
            arguments,
            has_receiver: invocation.receiver.is_some(),
        })
    }
}

impl TypeCheckInfo {
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
                .is_some_and(|contract| contract.modes.iter().any(|mode| mode.component.parameter.is_some()))
    }
}

/// Static stored fields can be authenticated against Body IR places. Indexed adaptation needs its own evaluated
/// index identity and native no-copy proof; this first consumer deliberately keeps that shape refused.
pub(super) fn project_source(source: &flow::Place) -> Result<CheckedInputSource, &'static str> {
    match &source.root {
        flow::Root::Binding(root) => {
            let mut fields = Vec::with_capacity(source.projection.len());
            for projection in &source.projection {
                match projection {
                    flow::Projection::Field(field) => fields.push(field.clone()),
                    flow::Projection::Element(_) => {
                        return Err("explicit indexed input requires evaluated index storage proof");
                    }
                }
            }
            Ok(CheckedInputSource::Place {
                root: root.clone(),
                fields,
            })
        }
        flow::Root::Temporary { .. } if source.projection.is_empty() => Ok(CheckedInputSource::Temporary),
        _ => Err("explicit temporary input has an unsupported projection"),
    }
}

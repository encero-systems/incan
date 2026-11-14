//! Loss-check the C facade tooling's redundant ordinary-call view against canonical checked call fields.

use super::{BodyLocal, NodeBindingError, Positions};
use crate::typechecker::type_info::CBindingFunctionCall;

/// Admit the derived view only when the complete native sequence is recoverable from already recorded facts.
pub(super) fn validate(positions: &Positions, local: &BodyLocal) -> Result<(), NodeBindingError> {
    let reconstructed = calls(positions, local)?;
    if reconstructed != local.c_abi.function_calls {
        return Err(NodeBindingError::InvalidField(format!(
            "ordinary function-call provenance: expected {:?}; reconstructed {:?}",
            local.c_abi.function_calls, reconstructed
        )));
    }
    Ok(())
}

/// Restore the tooling view without resolving a source name or retaining a second call-target authority.
pub(super) fn restore(positions: &Positions, local: &mut BodyLocal) -> Result<(), NodeBindingError> {
    local.c_abi.function_calls = calls(positions, local)?;
    Ok(())
}

/// Direct checking records the same target at both the callee and the full call. Their common start and distinct
/// ends recover the full-call view without requiring a parameter row, which is absent for zero-argument calls.
/// The current declaration supplies caller provenance. Direct checking records the outer target before visiting
/// its arguments, so source preorder retains the complete native call sequence.
fn calls(positions: &Positions, local: &BodyLocal) -> Result<Vec<CBindingFunctionCall>, NodeBindingError> {
    let Some(caller) = &positions.caller else {
        if local.c_abi.function_calls.is_empty() {
            return Ok(Vec::new());
        }
        return Err(NodeBindingError::MissingOwner);
    };
    let mut calls = local
        .expressions
        .source_targets
        .iter()
        .filter(|(span, target)| {
            target.is_function()
                && local.expressions.source_targets.iter().any(|(callee_span, callee)| {
                    callee == *target && callee_span.start == span.start && callee_span.end < span.end
                })
        })
        .map(|(span, target)| CBindingFunctionCall {
            span: *span,
            caller: caller.clone(),
            target: target.clone(),
        })
        .collect::<Vec<_>>();
    calls.sort_by_key(|call| (call.span.start, std::cmp::Reverse(call.span.end)));
    Ok(calls)
}

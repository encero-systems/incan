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

/// Direct checking records the same source target on the typed call and its callee. Use syntax roles for
/// widened interpolation spans. The corresponding checked target on both nodes is required in either case.
fn calls(positions: &Positions, local: &BodyLocal) -> Result<Vec<CBindingFunctionCall>, NodeBindingError> {
    let Some(caller) = &positions.caller else {
        if local.c_abi.function_calls.is_empty() {
            return Ok(Vec::new());
        }
        return Err(NodeBindingError::MissingOwner);
    };
    let targets = &local.expressions.source_targets;
    let widened = positions
        .nodes
        .direct_call_sites()
        .filter_map(|(call, callee)| {
            (call.start != callee.start
                && targets
                    .get(&callee)
                    .is_some_and(|target| targets.get(&call) == Some(target)))
            .then_some(call)
        })
        .collect::<std::collections::HashSet<_>>();
    let mut calls = local
        .expressions
        .source_targets
        .iter()
        .filter(|(span, target)| {
            target.is_function()
                && (widened.contains(span)
                    || local.expressions.source_targets.iter().any(|(callee_span, callee)| {
                        callee == *target && callee_span.start == span.start && callee_span.end < span.end
                    }))
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

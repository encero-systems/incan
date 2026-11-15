//! The base's read-only rule for published callables. A published callable's parameter passing is part of the
//! signature that Rust callers and other packages are compiled against, so it must not follow the private
//! "never consumed" rule. An input of a published callable borrows only when its whole body reads it, settled under
//! the same hypothesis the base settled every callable with, so the published signature is the base's.

use crate::body_ir::ContextModules;
use std::collections::{HashMap, HashSet, VecDeque};

use super::{Candidate, FieldTypes, ReadContext, bir, closure_reads, collect_signatures, readonly};

/// Settle the base's decreasing fixed point over `candidates`: an input stays shared only when its body reads it, and
/// a callee input that loses shared access refuses every candidate that forwards to it.
pub(super) fn read_only_parameters(
    modules: &[bir::BodyIrModule],
    context: ContextModules<'_>,
    field_types: &FieldTypes,
    mut candidates: HashSet<Candidate>,
) -> HashSet<Candidate> {
    let signatures = collect_signatures(modules, context, &candidates);
    let mut dependents: HashMap<Candidate, HashSet<Candidate>> = HashMap::new();
    let mut refused = VecDeque::new();
    for &candidate in &candidates {
        let Some((body, local)) = candidate_body(modules, candidate) else {
            refused.push_back(candidate);
            continue;
        };
        let read = ReadContext {
            locals: &body.locals,
            signatures: &signatures,
            field_types,
            candidates: &candidates,
            local,
            cursor: false,
            derived: false,
        };
        if !readonly(&body.block.stmts, &read) {
            refused.push_back(candidate);
            continue;
        }
        for dependency in closure_reads::dependencies(&body.block.stmts, &read) {
            dependents.entry(dependency).or_default().insert(candidate);
        }
    }
    while let Some(refused_candidate) = refused.pop_front() {
        if !candidates.remove(&refused_candidate) {
            continue;
        }
        if let Some(affected) = dependents.get(&refused_candidate) {
            for &candidate in affected {
                if candidates.contains(&candidate) {
                    refused.push_back(candidate);
                }
            }
        }
    }
    candidates
}

/// Resolve one candidate's checked body and its parameter local.
fn candidate_body(modules: &[bir::BodyIrModule], candidate: Candidate) -> Option<(&bir::Body, bir::LocalId)> {
    let (module_index, body_index, parameter_index) = candidate;
    let body = modules.get(module_index)?.bodies.get(body_index)?;
    Some((body, body.params.get(parameter_index)?.local))
}

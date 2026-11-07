//! Interprocedural mutation edges and their canonical ownership-flow projection.
//!
//! Caller-visible parameter changes are effects on ownership places, not a second checker. Local checking records
//! direct changes and forwarding edges here; the ownership flow engine closes the graph once after all bodies are
//! known. Call diagnostics consume only the closed projection.

use incan_semantics_core::CanonicalSymbolId;
use std::collections::{HashMap, HashSet, VecDeque};

use super::Graph;
use crate::ast::Span;

/// A caller-visible parameter passed on to another callee's caller-visible parameter.
#[derive(Debug, Clone)]
pub(in crate::typechecker) struct MutationForward {
    /// The callable whose parameter is passed on, and that parameter.
    pub(in crate::typechecker) owner: CanonicalSymbolId,
    pub(in crate::typechecker) param: String,
    /// The callee it is passed to, and the callee's parameter.
    pub(in crate::typechecker) callee: CanonicalSymbolId,
    pub(in crate::typechecker) callee_param: String,
    /// The declaration span of the local the call went through, when it did.
    pub(in crate::typechecker) through_local: Option<Span>,
}

/// Caller-visible mutation facts carried by one callable's canonical ownership graph.
///
/// This is source-graph data, not an analyzer. Local checking records direct changes and forwarding edges while the
/// callable is in scope; the ownership flow engine closes those edges with the other callable graphs at module
/// settlement. Known bodies distinguish an observed empty effect from an imported or indirect call.
#[derive(Debug, Clone, Default)]
pub(in crate::typechecker) struct MutationBody {
    pub(in crate::typechecker) known: bool,
    pub(in crate::typechecker) changed: HashSet<String>,
    pub(in crate::typechecker) forwards: Vec<MutationForward>,
    pub(in crate::typechecker) reassigned_locals: HashSet<Span>,
}

/// Final caller-visible effects. Known empty bodies differ from unknown/imported bodies; absence from `changed`
/// is a negative effect projection only when the callable belongs to `bodies_checked`.
#[derive(Debug, Clone, Default)]
pub(in crate::typechecker) struct MutationEffects {
    /// Exact known source bodies; every other callable remains conservative.
    pub(in crate::typechecker) bodies_checked: HashSet<CanonicalSymbolId>,
    /// Closed changed-parameter member projections.
    pub(in crate::typechecker) changed: HashSet<(CanonicalSymbolId, String)>,
    /// Callable-valued locals reassigned before a later call through the binding.
    pub(in crate::typechecker) reassigned_locals: HashSet<Span>,
    /// Forwarding components closed by this evaluation.
    pub(in crate::typechecker) components: usize,
    /// Parameter members visited, separately from local checking.
    pub(in crate::typechecker) members: usize,
}

/// Close parameter-forwarding edges from the same canonical callable graphs that own places, accesses and loans.
/// A conservative edge seeds its owner directly; known edges form SCCs, and changed projections flow over their
/// acyclic reverse relation. This is the module phase of ownership-flow analysis, not a second checker traversal.
pub(in crate::typechecker) fn analyze_mutation<'a>(graphs: impl IntoIterator<Item = &'a Graph>) -> MutationEffects {
    let graphs = graphs.into_iter().collect::<Vec<_>>();
    let bodies_checked = graphs
        .iter()
        .filter(|graph| graph.mutation.known)
        .map(|graph| graph.owner.clone())
        .collect::<HashSet<_>>();
    let reassigned_locals = graphs
        .iter()
        .flat_map(|graph| graph.mutation.reassigned_locals.iter().copied())
        .collect::<HashSet<_>>();
    let forwards = graphs
        .iter()
        .flat_map(|graph| graph.mutation.forwards.iter())
        .collect::<Vec<_>>();

    // ---- Recorded parameter graph ----
    let mut keys = HashMap::new();
    let mut nodes = Vec::new();
    for graph in &graphs {
        for parameter in &graph.mutation.changed {
            intern(&mut keys, &mut nodes, (graph.owner.clone(), parameter.clone()));
        }
    }
    for forward in &forwards {
        intern(&mut keys, &mut nodes, (forward.owner.clone(), forward.param.clone()));
        intern(
            &mut keys,
            &mut nodes,
            (forward.callee.clone(), forward.callee_param.clone()),
        );
    }
    let mut edges = vec![Vec::new(); nodes.len()];
    let mut reverse = edges.clone();
    let mut seeds = vec![false; nodes.len()];
    for (index, (owner, parameter)) in nodes.iter().enumerate() {
        seeds[index] = graphs
            .iter()
            .find(|graph| &graph.owner == owner)
            .is_some_and(|graph| graph.mutation.changed.contains(parameter));
    }
    for forward in forwards {
        let owner = keys[&(forward.owner.clone(), forward.param.clone())];
        let callee = keys[&(forward.callee.clone(), forward.callee_param.clone())];
        if !bodies_checked.contains(&forward.callee)
            || forward
                .through_local
                .is_some_and(|local| reassigned_locals.contains(&local))
        {
            seeds[owner] = true;
        } else {
            edges[owner].push(callee);
            reverse[callee].push(owner);
        }
    }

    // ---- Joint forwarding components ----
    let (membership, components) = components(&edges, &reverse);
    let mut changed = vec![false; components];
    let mut readers = vec![Vec::new(); components];
    for (owner, callees) in edges.iter().enumerate() {
        changed[membership[owner]] |= seeds[owner];
        for callee in callees {
            if membership[owner] != membership[*callee] {
                readers[membership[*callee]].push(membership[owner]);
            }
        }
    }

    // ---- Changed effects over the component DAG ----
    let mut ready = changed
        .iter()
        .enumerate()
        .filter_map(|(index, value)| value.then_some(index))
        .collect::<VecDeque<_>>();
    while let Some(callee) = ready.pop_front() {
        for owner in &readers[callee] {
            if !changed[*owner] {
                changed[*owner] = true;
                ready.push_back(*owner);
            }
        }
    }
    MutationEffects {
        bodies_checked,
        reassigned_locals,
        members: nodes.len(),
        components,
        changed: nodes
            .into_iter()
            .enumerate()
            .filter_map(|(index, key)| changed[membership[index]].then_some(key))
            .collect(),
    }
}

/// Assign a compact invocation-local graph index to an exact callable/parameter key.
fn intern(
    keys: &mut HashMap<(CanonicalSymbolId, String), usize>,
    nodes: &mut Vec<(CanonicalSymbolId, String)>,
    key: (CanonicalSymbolId, String),
) {
    if !keys.contains_key(&key) {
        keys.insert(key.clone(), nodes.len());
        nodes.push(key);
    }
}

/// Find forwarding SCCs with iterative depth-first traversals so legal recursion never consumes the host stack.
/// Indices are private to this evaluation; only member effect values escape it.
fn components(edges: &[Vec<usize>], reverse: &[Vec<usize>]) -> (Vec<usize>, usize) {
    let mut seen = vec![false; edges.len()];
    let mut order = Vec::new();
    for root in 0..edges.len() {
        if seen[root] {
            continue;
        }
        seen[root] = true;
        let mut stack = vec![(root, 0)];
        while let Some((node, next)) = stack.last_mut() {
            if let Some(child) = edges[*node].get(*next) {
                *next += 1;
                if !seen[*child] {
                    seen[*child] = true;
                    stack.push((*child, 0));
                }
            } else if let Some((node, _)) = stack.pop() {
                order.push(node);
            }
        }
    }
    let mut membership = vec![usize::MAX; edges.len()];
    let mut count = 0;
    for root in order.into_iter().rev() {
        if membership[root] != usize::MAX {
            continue;
        }
        membership[root] = count;
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            for child in &reverse[node] {
                if membership[*child] == usize::MAX {
                    membership[*child] = count;
                    stack.push(*child);
                }
            }
        }
        count += 1;
    }
    (membership, count)
}

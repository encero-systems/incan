//! Mutation-effect projections from recorded local facts, with one closure of each forwarding component.

use super::{CanonicalSymbolId, HashMap, HashSet, MutParamFacts};
use std::collections::VecDeque;

/// Final caller-visible effects. Known empty bodies differ from unknown/imported bodies; absence from `changed`
/// is a negative effect projection only when the callable belongs to `bodies_checked`.
#[derive(Debug, Clone, Default)]
pub(in crate::typechecker) struct MutEffects {
    /// Exact known source bodies; every other callable remains conservative.
    pub(in crate::typechecker) bodies_checked: HashSet<CanonicalSymbolId>,
    /// Closed changed-parameter member projections.
    pub(in crate::typechecker) changed: HashSet<(CanonicalSymbolId, String)>,
    /// Forwarding components closed by this evaluation.
    pub(in crate::typechecker) components: usize,
    /// Parameter members visited, separately from local checking.
    pub(in crate::typechecker) members: usize,
}

impl MutParamFacts {
    /// Close only recorded parameter-forwarding edges. A conservative edge seeds its owner directly; known edges
    /// form a graph whose SCCs are evaluated once, and changed projections flow over its acyclic reverse relation.
    pub(in crate::typechecker) fn close_effects(&self) -> MutEffects {
        // ---- Recorded parameter graph ----
        let mut keys = HashMap::new();
        let mut nodes = Vec::new();
        for (owner, parameters) in &self.changed {
            for parameter in parameters {
                intern(&mut keys, &mut nodes, (owner.clone(), parameter.clone()));
            }
        }
        for forward in &self.forwards {
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
            seeds[index] = self
                .changed
                .get(owner)
                .is_some_and(|changed| changed.contains(parameter));
        }
        for forward in &self.forwards {
            let owner = keys[&(forward.owner.clone(), forward.param.clone())];
            let callee = keys[&(forward.callee.clone(), forward.callee_param.clone())];
            if !self.bodies_checked.contains(&forward.callee)
                || forward
                    .through_local
                    .is_some_and(|local| self.reassigned_locals.contains(&local))
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
        MutEffects {
            bodies_checked: self.bodies_checked.clone(),
            members: nodes.len(),
            components,
            changed: nodes
                .into_iter()
                .enumerate()
                .filter_map(|(index, key)| changed[membership[index]].then_some(key))
                .collect(),
        }
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

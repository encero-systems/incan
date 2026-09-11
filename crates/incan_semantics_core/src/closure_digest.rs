//! Folding per-declaration digests along dependency edges.
//!
//! A declaration's own [`crate::semantic_digest`] answers a narrow question. Consumers ask a wider one: has
//! anything this declaration depends on changed. RFC 106 defines a **closure digest** for that — a digest over a
//! declaration's own digest followed by the closure digests of its dependencies — and RFC 124 folds the same shape
//! one level up, where a compiled unit's identity includes the identities of the units it links.
//!
//! Two properties are load-bearing and are what the tests here pin.
//!
//! **The fold must be order-free.** Dependencies are folded in a canonical order derived from their identities,
//! never in traversal order. A fold sensitive to traversal reintroduces exactly the positional dependency that
//! [`crate::stable_identity`] exists to remove: the graph would report a change because a declaration moved.
//!
//! **Cycles must fold deterministically.** Mutually recursive declarations have no well-founded order, so a naive
//! recursive fold either loops forever or produces a different answer per entry point. Each strongly connected
//! component is therefore folded as a single unit, over the sorted set of its members' own digests, so every member
//! of a cycle shares one component contribution and the result does not depend on where the walk began.
//!
//! This module deliberately knows nothing about where edges come from. Body IR cannot supply them — it records the
//! source-level callee spelling and defers full call-target resolution past v0 — so the resolved edges live in the
//! RFC 106 graph, whose `target_id` and `canonical_identity` are `Option` precisely because resolution is partial.
//! Keeping the fold independent of its edge source is what lets it be proven here rather than end-to-end.

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// One node's contribution before folding: its own semantic digest and the nodes it depends on.
#[derive(Debug, Clone, Default)]
pub struct DependencyNode {
    /// The declaration's own semantic digest.
    pub digest: String,
    /// Identities this declaration depends on, in any order.
    pub dependencies: BTreeSet<String>,
}

/// Fold every node's digest with its dependencies', keyed by identity.
///
/// Returns a closure digest per input node. A dependency naming an identity absent from `nodes` is folded as an
/// explicit unresolved marker rather than skipped: RFC 106 requires that an edge the graph could not resolve be
/// treated as affected, and silently dropping it would let a closure claim to cover a dependency it never saw.
pub fn closure_digests(nodes: &BTreeMap<String, DependencyNode>) -> BTreeMap<String, String> {
    let components = strongly_connected_components(nodes);

    // Component index per node, so a dependency edge can be classified as internal or external to the cycle.
    let mut component_of: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, component) in components.iter().enumerate() {
        for member in component {
            component_of.insert(member.as_str(), index);
        }
    }

    let mut component_digests: Vec<Option<String>> = vec![None; components.len()];
    let mut resolved: BTreeMap<String, String> = BTreeMap::new();

    // `strongly_connected_components` yields components in dependency order, so every external dependency of a
    // component is already folded by the time the component is reached.
    for (index, component) in components.iter().enumerate() {
        let mut hasher = Sha256::new();
        hasher.update(b"closure-v1");

        // Members' own digests, sorted, so the component's contribution does not depend on traversal order.
        for member in component {
            let node = nodes.get(member.as_str());
            hasher.update(b"\x01member\0");
            update_delimited(&mut hasher, member.as_bytes());
            update_delimited(
                &mut hasher,
                node.map(|node| node.digest.as_bytes()).unwrap_or(b"unresolved"),
            );
        }

        // External dependencies, deduplicated and sorted by the identity they name.
        let mut external: BTreeSet<(&str, String)> = BTreeSet::new();
        for member in component {
            let Some(node) = nodes.get(member.as_str()) else {
                continue;
            };
            for dependency in &node.dependencies {
                match component_of.get(dependency.as_str()) {
                    Some(other) if *other == index => continue,
                    Some(other) => {
                        let digest = component_digests[*other]
                            .clone()
                            .unwrap_or_else(|| "pending".to_string());
                        external.insert((dependency.as_str(), digest));
                    }
                    // An edge to an identity the graph does not contain. Recorded, never dropped.
                    None => {
                        external.insert((dependency.as_str(), "unresolved".to_string()));
                    }
                }
            }
        }
        for (identity, digest) in external {
            hasher.update(b"\x02dependency\0");
            update_delimited(&mut hasher, identity.as_bytes());
            update_delimited(&mut hasher, digest.as_bytes());
        }

        let digest = format!("sha256:{}", hex::encode(hasher.finalize()));
        component_digests[index] = Some(digest.clone());
        for member in component {
            resolved.insert(member.clone(), digest.clone());
        }
    }
    resolved
}

/// Write a length-delimited run, so adjacent values cannot run together.
///
/// Without a length, two identities differing only in where their boundary falls encode identically — the same
/// collision [`crate::semantic_digest`] guards against.
fn update_delimited(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value);
}

/// Group nodes into strongly connected components, returned in dependency order.
///
/// Iterative Tarjan: a recursive walk would blow the stack on a deep dependency chain, and a standard library's
/// call graph is deep enough to care. Components come out with dependencies before dependents, which is the order
/// the fold needs.
fn strongly_connected_components(nodes: &BTreeMap<String, DependencyNode>) -> Vec<Vec<String>> {
    #[derive(Default)]
    struct State {
        index: usize,
        indices: BTreeMap<String, usize>,
        lowlink: BTreeMap<String, usize>,
        on_stack: BTreeSet<String>,
        stack: Vec<String>,
        components: Vec<Vec<String>>,
    }

    let mut state = State::default();

    for root in nodes.keys() {
        if state.indices.contains_key(root) {
            continue;
        }
        // (node, index of the next dependency to visit)
        let mut work: Vec<(String, usize)> = vec![(root.clone(), 0)];
        while let Some((node, child)) = work.pop() {
            if child == 0 {
                state.indices.insert(node.clone(), state.index);
                state.lowlink.insert(node.clone(), state.index);
                state.index += 1;
                state.stack.push(node.clone());
                state.on_stack.insert(node.clone());
            }

            let dependencies: Vec<String> = nodes
                .get(&node)
                .map(|entry| entry.dependencies.iter().cloned().collect())
                .unwrap_or_default();

            if child < dependencies.len() {
                let dependency = dependencies[child].clone();
                work.push((node.clone(), child + 1));
                if !nodes.contains_key(&dependency) {
                    continue;
                }
                if !state.indices.contains_key(&dependency) {
                    work.push((dependency, 0));
                } else if state.on_stack.contains(&dependency) {
                    let candidate = state.indices.get(&dependency).copied().unwrap_or_default();
                    let current = state.lowlink.get(&node).copied().unwrap_or_default();
                    state.lowlink.insert(node.clone(), current.min(candidate));
                }
                continue;
            }

            // Every dependency visited: propagate this node's lowlink into its parent, then close a root.
            if let Some((parent, _)) = work.last() {
                let child_low = state.lowlink.get(&node).copied().unwrap_or_default();
                let parent_low = state.lowlink.get(parent.as_str()).copied().unwrap_or_default();
                state.lowlink.insert(parent.clone(), parent_low.min(child_low));
            }
            if state.lowlink.get(&node) == state.indices.get(&node) {
                let mut component = Vec::new();
                while let Some(member) = state.stack.pop() {
                    state.on_stack.remove(&member);
                    let done = member == node;
                    component.push(member);
                    if done {
                        break;
                    }
                }
                // Defensive, and deliberately not test-covered: component discovery is already deterministic
                // because `nodes` is a `BTreeMap`, so the same graph always yields the same stack order and no
                // test can observe this sort. It stays because that determinism is a property of the container,
                // not of this algorithm — swap the map and the sort is what keeps a cycle's digest stable.
                component.sort();
                state.components.push(component);
            }
        }
    }
    state.components
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(entries: &[(&str, &str, &[&str])]) -> BTreeMap<String, DependencyNode> {
        entries
            .iter()
            .map(|(name, digest, dependencies)| {
                (
                    (*name).to_string(),
                    DependencyNode {
                        digest: (*digest).to_string(),
                        dependencies: dependencies.iter().map(|d| (*d).to_string()).collect(),
                    },
                )
            })
            .collect()
    }

    /// A change to a leaf must reach everything that depends on it, transitively, and nothing else.
    #[test]
    fn a_change_propagates_to_every_dependent_and_no_further() {
        let before = graph(&[
            ("leaf", "d1", &[]),
            ("mid", "d2", &["leaf"]),
            ("top", "d3", &["mid"]),
            ("apart", "d4", &[]),
        ]);
        let after = graph(&[
            ("leaf", "CHANGED", &[]),
            ("mid", "d2", &["leaf"]),
            ("top", "d3", &["mid"]),
            ("apart", "d4", &[]),
        ]);

        let before = closure_digests(&before);
        let after = closure_digests(&after);

        assert_ne!(before["leaf"], after["leaf"], "the changed leaf");
        assert_ne!(before["mid"], after["mid"], "its direct dependent");
        assert_ne!(before["top"], after["top"], "its transitive dependent");
        assert_eq!(
            before["apart"], after["apart"],
            "an unrelated declaration must not move"
        );
    }

    /// A change must not propagate backwards. Depending on something does not make you part of it.
    #[test]
    fn a_change_does_not_reach_a_dependency() {
        let before = graph(&[("leaf", "d1", &[]), ("top", "d2", &["leaf"])]);
        let after = graph(&[("leaf", "d1", &[]), ("top", "CHANGED", &["leaf"])]);
        assert_eq!(
            closure_digests(&before)["leaf"],
            closure_digests(&after)["leaf"],
            "editing a dependent must not move its dependency"
        );
    }

    /// The fold is over a set, not a sequence: the order dependencies were discovered in must not matter.
    ///
    /// This is the property that keeps traversal order out of the result. A fold sensitive to it would reintroduce
    /// exactly the positional dependency the stable identity exists to remove.
    #[test]
    fn the_fold_is_order_free() {
        let one = graph(&[("a", "d1", &[]), ("b", "d2", &[]), ("top", "d3", &["a", "b"])]);
        let other = graph(&[("b", "d2", &[]), ("a", "d1", &[]), ("top", "d3", &["b", "a"])]);
        assert_eq!(closure_digests(&one)["top"], closure_digests(&other)["top"]);
    }

    /// A cycle must fold to one answer regardless of which member the walk starts from.
    #[test]
    fn a_cycle_folds_deterministically() {
        let forward = graph(&[("a", "d1", &["b"]), ("b", "d2", &["a"]), ("outside", "d3", &["a"])]);
        let reversed = graph(&[("b", "d2", &["a"]), ("a", "d1", &["b"]), ("outside", "d3", &["a"])]);

        let forward = closure_digests(&forward);
        let reversed = closure_digests(&reversed);

        assert_eq!(forward["a"], reversed["a"]);
        assert_eq!(forward["b"], reversed["b"]);
        assert_eq!(forward["outside"], reversed["outside"]);
        assert_eq!(
            forward["a"], forward["b"],
            "members of one cycle share their component's digest"
        );
    }

    /// A change inside a cycle must move every member of it, and its dependents.
    #[test]
    fn a_change_inside_a_cycle_moves_the_whole_cycle() {
        let before = graph(&[("a", "d1", &["b"]), ("b", "d2", &["a"]), ("outside", "d3", &["a"])]);
        let after = graph(&[("a", "CHANGED", &["b"]), ("b", "d2", &["a"]), ("outside", "d3", &["a"])]);

        let before = closure_digests(&before);
        let after = closure_digests(&after);

        assert_ne!(before["a"], after["a"]);
        assert_ne!(before["b"], after["b"], "the other member of the cycle moves too");
        assert_ne!(before["outside"], after["outside"]);
    }

    /// An edge naming an identity the graph does not contain must be recorded, never silently dropped.
    ///
    /// RFC 106 requires an unresolved edge be treated as affected. Dropping it would let a closure claim to cover a
    /// dependency it never saw — under-invalidation, and a wrong build.
    #[test]
    fn an_unresolved_edge_is_recorded_rather_than_ignored() {
        let with_edge = graph(&[("top", "d1", &["missing"])]);
        let without_edge = graph(&[("top", "d1", &[])]);
        assert_ne!(
            closure_digests(&with_edge)["top"],
            closure_digests(&without_edge)["top"],
            "a dependency on something unresolved is not the same as no dependency"
        );
    }

    /// A deep chain must not overflow the stack; a recursive walk would.
    #[test]
    fn a_deep_chain_folds_without_recursion() {
        let mut nodes = BTreeMap::new();
        for index in 0..5_000 {
            let dependencies = if index == 0 {
                BTreeSet::new()
            } else {
                [format!("n{:05}", index - 1)].into_iter().collect()
            };
            nodes.insert(
                format!("n{index:05}"),
                DependencyNode {
                    digest: format!("d{index}"),
                    dependencies,
                },
            );
        }
        let digests = closure_digests(&nodes);
        assert_eq!(digests.len(), 5_000);
    }

    /// An identity and the digest beside it must not be able to run together.
    ///
    /// Each member contributes its identity followed by its own digest. Without a length between them, a node
    /// named `ab` with digest `cd` encodes byte-for-byte as one named `abc` with digest `d` — the boundary moves
    /// and the encoding does not. Two genuinely different declarations would then share a closure digest, which
    /// under-invalidates and yields a wrong build.
    #[test]
    fn identity_and_digest_are_length_delimited() {
        let one = graph(&[("ab", "cd", &[])]);
        let other = graph(&[("abc", "d", &[])]);
        assert_ne!(
            closure_digests(&one)["ab"],
            closure_digests(&other)["abc"],
            "the identity/digest boundary must be encoded, not implied"
        );
    }
}

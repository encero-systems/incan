//! Folding exported graph facts into per-declaration closure digests.
//!
//! [`crate::inspect::codegraph`] answers what each declaration *is* and whether it changed.
//! [`incan_semantics_core::closure_digest`] knows how to fold a digest with its dependencies'. This module is the
//! join: it reads exported records and produces the dependency graph the fold consumes.
//!
//! The edges have to come from here rather than from lowering. Body IR records the source-level spelling a call
//! site used and defers resolving which declaration it binds past v0, so folding over it would fold over spellings.
//! The graph resolves, which is what makes an edge foldable at all — and its resolution is partial by design, which
//! is why an unresolved edge is treated as affected rather than dropped.

use std::collections::{BTreeMap, BTreeSet};

use incan_codegraph::{CodegraphRecord, CodegraphStableDeclarationId};
use incan_semantics_core::closure_digest::{DependencyNode, closure_digests};

/// Render one exported identity into the key the fold uses.
///
/// Deliberately not `Hash` on the struct: the key has to be stable across processes and across a serialisation
/// round trip, and a derived hash is neither.
fn identity_key(identity: &CodegraphStableDeclarationId) -> String {
    let origin = format!("{:?}", identity.origin);
    let nested = if identity.nested { "#nested" } else { "" };
    let signature = identity
        .signature
        .as_deref()
        .map(|signature| format!("|{signature}"))
        .unwrap_or_default();
    format!(
        "{}:{}:{}:{}{nested}{signature}",
        identity.namespace, origin, identity.declaration_kind, identity.declaration_name
    )
}

/// Build the dependency graph one export describes.
///
/// Declarations become nodes carrying their own semantic digest. Calls and references become edges from the
/// declaration that owns the site to the declaration it resolved to.
///
/// Three cases are deliberately *not* dropped, because each would make a closure claim to cover something it never
/// saw, and under-invalidation is the failure direction that produces a wrong build:
///
/// - a declaration whose digest the producer could not compute keeps its node, with an explicit unresolved marker, so
///   anything depending on it inherits the uncertainty;
/// - an edge whose target the graph could not resolve is recorded against a synthetic unresolved identity rather than
///   omitted, since an edge that could not be resolved is not an edge that does not exist;
/// - an edge whose *owner* is unknown is skipped, because it cannot be attributed to a dependent at all — and this is
///   the one case that loses information, so it is counted rather than ignored.
pub fn dependency_graph(records: &[CodegraphRecord]) -> (BTreeMap<String, DependencyNode>, usize) {
    let mut declaration_key_by_record_id: BTreeMap<&str, String> = BTreeMap::new();
    let mut nodes: BTreeMap<String, DependencyNode> = BTreeMap::new();

    for record in records {
        if let CodegraphRecord::Declaration(declaration) = record {
            let Some(identity) = declaration.stable_identity.as_ref() else {
                continue;
            };
            let key = identity_key(identity);
            declaration_key_by_record_id.insert(declaration.id.as_str(), key.clone());
            nodes.entry(key).or_insert_with(|| DependencyNode {
                digest: declaration
                    .semantic_digest
                    .clone()
                    .unwrap_or_else(|| "unresolved".to_string()),
                dependencies: BTreeSet::new(),
            });
        }
    }

    let mut unattributable = 0usize;
    let mut edge = |owner: Option<&String>,
                    target: Option<&CodegraphStableDeclarationId>,
                    nodes: &mut BTreeMap<String, DependencyNode>| {
        let Some(owner_id) = owner else {
            unattributable += 1;
            return;
        };
        let Some(owner_key) = declaration_key_by_record_id.get(owner_id.as_str()) else {
            unattributable += 1;
            return;
        };
        let target_key = target
            .map(identity_key)
            .unwrap_or_else(|| "unresolved-target".to_string());
        if let Some(node) = nodes.get_mut(owner_key.as_str()) {
            node.dependencies.insert(target_key);
        }
    };

    for record in records {
        match record {
            CodegraphRecord::Call(call) => edge(call.owner_id.as_ref(), call.stable_identity.as_ref(), &mut nodes),
            CodegraphRecord::Reference(reference) => edge(
                reference.owner_id.as_ref(),
                reference.stable_identity.as_ref(),
                &mut nodes,
            ),
            _ => {}
        }
    }
    (nodes, unattributable)
}

/// Fold one export into a closure digest per declaration.
///
/// The returned map answers the question a build system actually asks: has anything this declaration depends on,
/// transitively, changed since last time.
pub fn closure_digests_for_export(records: &[CodegraphRecord]) -> BTreeMap<String, String> {
    let (nodes, _unattributable) = dependency_graph(records);
    closure_digests(&nodes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use incan_codegraph::{
        CodegraphCallRecord, CodegraphDeclarationRecord, CodegraphLanguage, CodegraphProvenance, CodegraphSymbolOrigin,
    };

    fn identity(name: &str) -> CodegraphStableDeclarationId {
        CodegraphStableDeclarationId {
            namespace: "ordinary_lexical".to_string(),
            origin: CodegraphSymbolOrigin::Module {
                path: vec!["m".to_string()],
            },
            declaration_name: name.to_string(),
            declaration_kind: "function".to_string(),
            nested: false,
            signature: Some("()->Unit".to_string()),
        }
    }

    fn declaration(id: &str, name: &str, digest: Option<&str>) -> CodegraphRecord {
        CodegraphRecord::Declaration(CodegraphDeclarationRecord {
            id: id.to_string(),
            language: CodegraphLanguage::Incan,
            module_id: "m".to_string(),
            kind: "function".to_string(),
            name: name.to_string(),
            visibility: "public".to_string(),
            type_params: Vec::new(),
            signature: None,
            canonical_identity: None,
            stable_identity: Some(identity(name)),
            semantic_digest: digest.map(str::to_string),
            doc_digest: None,
            span: None,
            provenance: CodegraphProvenance::Checked,
            degraded: false,
        })
    }

    fn call(owner: &str, target: Option<&str>) -> CodegraphRecord {
        CodegraphRecord::Call(CodegraphCallRecord {
            id: format!("c:{owner}"),
            language: CodegraphLanguage::Incan,
            module_id: "m".to_string(),
            owner_id: Some(owner.to_string()),
            callee: target.unwrap_or("?").to_string(),
            kind: "function".to_string(),
            argument_count: 0,
            type_argument_count: 0,
            target_id: None,
            canonical_identity: None,
            stable_identity: target.map(identity),
            span: None,
            provenance: CodegraphProvenance::Checked,
            degraded: false,
        })
    }

    /// A change to a leaf must reach every declaration that depends on it, and nothing else.
    ///
    /// This is the property the whole invalidation argument rests on, and until the graph supplied both node
    /// digests and resolved edges there was nothing to run it against.
    #[test]
    fn a_changed_leaf_reaches_its_dependents_and_no_others() {
        let build = |leaf_digest: &str| {
            closure_digests_for_export(&[
                declaration("d:leaf", "leaf", Some(leaf_digest)),
                declaration("d:mid", "mid", Some("d2")),
                declaration("d:apart", "apart", Some("d3")),
                call("d:mid", Some("leaf")),
            ])
        };

        let before = build("d1");
        let after = build("CHANGED");

        let leaf = identity_key(&identity("leaf"));
        let mid = identity_key(&identity("mid"));
        let apart = identity_key(&identity("apart"));

        assert_ne!(before[&leaf], after[&leaf], "the changed leaf");
        assert_ne!(before[&mid], after[&mid], "its dependent must follow");
        assert_eq!(before[&apart], after[&apart], "an unrelated declaration must not move");
    }

    /// An edge the graph could not resolve must still reach the closure.
    #[test]
    fn an_unresolved_edge_still_affects_its_owner() {
        let with_edge =
            closure_digests_for_export(&[declaration("d:owner", "owner", Some("d1")), call("d:owner", None)]);
        let without_edge = closure_digests_for_export(&[declaration("d:owner", "owner", Some("d1"))]);
        let owner = identity_key(&identity("owner"));
        assert_ne!(
            with_edge[&owner], without_edge[&owner],
            "a call to something unresolved is not the same as no call"
        );
    }

    /// A declaration whose digest could not be computed must not read as unchanged.
    #[test]
    fn a_missing_digest_does_not_read_as_stable() {
        let (nodes, _) = dependency_graph(&[declaration("d:x", "x", None)]);
        let key = identity_key(&identity("x"));
        assert_eq!(nodes[&key].digest, "unresolved");
    }
}

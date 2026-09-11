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
use incan_semantics_core::closure_digest::{DependencyNode, closure_digests, update_delimited};
use sha2::{Digest, Sha256};

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

/// The set of declarations a consumer of this unit can observe.
///
/// RFC 106 roots the external closure at public declarations and extends it by reachability: visibility marks the
/// roots, reachability decides membership. A private declaration a public one depends on is part of the external
/// surface — not an exception to the rule, an instance of it — because a consumer does not merely link against
/// public signatures, it instantiates parts of what it depends on. Generic bodies are monomorphised in the
/// consumer's crate, inlinable bodies are code-generated there, and compile-time-evaluated bodies are evaluated
/// there.
///
/// This follows *every* edge out of a public declaration, which is the loose variant RFC 106 describes as sound but
/// not tight: for an ordinary non-generic, non-inline public function a consumer only ever observes the signature,
/// so its private callees are not in fact externally visible, and including them over-reports. Over-reporting costs
/// a rebuild; under-reporting ships a stale artifact. The tighter variant needs to know which bodies a consumer can
/// instantiate, which is not exported yet, so the safe version goes first and the improvement can then be measured
/// against it rather than guessed at.
pub fn external_closure(records: &[CodegraphRecord]) -> BTreeSet<String> {
    let (nodes, _) = dependency_graph(records);

    let mut frontier: Vec<String> = records
        .iter()
        .filter_map(|record| match record {
            CodegraphRecord::Declaration(declaration) if declaration.visibility == "public" => {
                declaration.stable_identity.as_ref().map(identity_key)
            }
            _ => None,
        })
        .collect();

    let mut reached: BTreeSet<String> = frontier.iter().cloned().collect();
    while let Some(current) = frontier.pop() {
        let Some(node) = nodes.get(&current) else {
            continue;
        };
        for dependency in &node.dependencies {
            if reached.insert(dependency.clone()) {
                frontier.push(dependency.clone());
            }
        }
    }
    reached
}

/// Digest what a consumer of this unit can observe.
///
/// Paired with [`closure_digests_for_export`], this is the internal/external split RFC 124 folds into unit identity:
/// the internal digest decides whether *this* unit is rebaked, the external one whether its *dependents* are. A
/// change confined to a unit's internals moves the first and not the second.
///
/// Folding the external closure's members in identity order keeps the result independent of traversal, for the same
/// reason the per-declaration fold does.
pub fn external_digest(records: &[CodegraphRecord]) -> String {
    let digests = closure_digests_for_export(records);
    let external = external_closure(records);

    let mut hasher = Sha256::new();
    hasher.update(b"external-v1");
    for member in &external {
        // A member with no closure digest is recorded as unresolved rather than skipped: an external surface that
        // silently omitted part of itself would under-report, which is the direction that ships a stale artifact.
        let digest = digests.get(member).map(String::as_str).unwrap_or("unresolved");
        update_delimited(&mut hasher, member.as_bytes());
        update_delimited(&mut hasher, digest.as_bytes());
    }
    format!("sha256:{}", hex::encode(hasher.finalize()))
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
        declaration_with(id, name, digest, "public")
    }

    fn declaration_with(id: &str, name: &str, digest: Option<&str>, visibility: &str) -> CodegraphRecord {
        CodegraphRecord::Declaration(CodegraphDeclarationRecord {
            id: id.to_string(),
            language: CodegraphLanguage::Incan,
            module_id: "m".to_string(),
            kind: "function".to_string(),
            name: name.to_string(),
            visibility: visibility.to_string(),
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

    /// A private declaration a public one reaches is part of the external surface.
    ///
    /// This is the rule that makes the split correct rather than convenient: a consumer instantiates parts of what
    /// it depends on, so "private" does not mean "invisible".
    #[test]
    fn a_private_declaration_reached_from_a_public_one_is_external() {
        let records = [
            declaration_with("d:api", "api", Some("d1"), "public"),
            declaration_with("d:helper", "helper", Some("d2"), "private"),
            declaration_with("d:alone", "alone", Some("d3"), "private"),
            call("d:api", Some("helper")),
        ];
        let external = external_closure(&records);
        assert!(
            external.contains(&identity_key(&identity("api"))),
            "a public declaration is a root"
        );
        assert!(
            external.contains(&identity_key(&identity("helper"))),
            "a private declaration a public one reaches is observable"
        );
        assert!(
            !external.contains(&identity_key(&identity("alone"))),
            "a private declaration nothing public reaches is not: {external:?}"
        );
    }

    /// An internal-only change must move the unit's own digests and not what its dependents see.
    ///
    /// This is the whole point of carrying two identities: without it, every private edit propagates through the
    /// closure and the split buys nothing.
    #[test]
    fn an_internal_only_change_does_not_move_the_external_digest() {
        let build = |private_digest: &str| {
            let records = [
                declaration_with("d:api", "api", Some("d1"), "public"),
                declaration_with("d:alone", "alone", Some(private_digest), "private"),
            ];
            (
                closure_digests_for_export(&records)[&identity_key(&identity("alone"))].clone(),
                external_digest(&records),
            )
        };
        let (before_internal, before_external) = build("d2");
        let (after_internal, after_external) = build("CHANGED");

        assert_ne!(before_internal, after_internal, "the edited declaration itself moves");
        assert_eq!(
            before_external, after_external,
            "nothing a consumer can observe changed, so dependents must not rebuild"
        );
    }

    /// A change a consumer can observe must move the external digest.
    #[test]
    fn a_public_change_moves_the_external_digest() {
        let build =
            |public_digest: &str| external_digest(&[declaration_with("d:api", "api", Some(public_digest), "public")]);
        assert_ne!(build("d1"), build("CHANGED"));
    }

    // ---- Encoding soundness ----
    //
    // The tests above prove the external surface is computed from the right roots and extended along the right
    // edges. These prove the surface is *encoded* soundly, which fails differently: not as a rebuild that did not
    // happen, but as two different units sharing one external digest, so a dependent is not rebaked when it should
    // be. Each was written against a mutation the tests above did not notice.

    /// Two overloads are two declarations, and the surface must keep them apart.
    ///
    /// `identity_key` folds the signature discriminant precisely because a name and kind do not distinguish
    /// overloads. Dropping it merges them into one key, so a change to either would move a digest the other shares
    /// and a change to both would move one entry rather than two.
    #[test]
    fn two_overloads_do_not_share_one_external_entry() {
        let overload = |signature: &str, digest: &str| {
            let mut record = declaration("d:same", "same", Some(digest));
            if let CodegraphRecord::Declaration(declaration) = &mut record
                && let Some(identity) = declaration.stable_identity.as_mut()
            {
                identity.signature = Some(signature.to_string());
            }
            record
        };
        let records = vec![overload("(Int)->Unit", "d1"), overload("(Str)->Unit", "d2")];
        let digests = closure_digests_for_export(&records);
        assert_eq!(
            digests.len(),
            2,
            "two overloads must hold two closure entries, got {digests:?}"
        );
    }

    /// An edge the graph could not resolve still reaches the external digest.
    ///
    /// Named for what it proves. It does *not* isolate the `unwrap_or("unresolved")` fallback in `external_digest`:
    /// replacing that with a skip leaves this passing, because introducing an unresolved edge also moves its
    /// owner's closure digest, and the owner's entry already folds both the dependency identity and its unresolved
    /// state. That fallback is therefore redundant rather than untested — kept because an entry absent from the
    /// surface is the failure this whole model exists to avoid, and the cost of keeping it is one hash update.
    #[test]
    fn an_unresolved_edge_reaches_the_external_digest() {
        let with_unresolved = vec![declaration("d:pub", "pub_fn", Some("d1")), call("d:pub", None)];
        let without = vec![declaration("d:pub", "pub_fn", Some("d1"))];
        assert_ne!(
            external_digest(&with_unresolved),
            external_digest(&without),
            "an edge the graph could not resolve must reach the external digest"
        );
    }
}

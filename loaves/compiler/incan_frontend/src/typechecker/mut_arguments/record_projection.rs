//! Completed mutation records retain exact known bodies, empty owner sets and reassigned source identities.

use super::{
    ArgumentPlace, BTreeMap, BTreeSet, CanonicalSymbolId, HashMap, HashSet, MutBodyLocal, MutParamBody,
    MutParamForward, PendingMutArgument, RecordMutation, SemanticSourceTargetKind, Span,
};
use incan_semantics_core::{SymbolNamespace, SymbolOrigin};

/// Construct distinct source owners without relying on checker-local symbol numbering.
fn owner(name: &str) -> CanonicalSymbolId {
    CanonicalSymbolId {
        namespace: SymbolNamespace::OrdinaryLexical,
        origin: SymbolOrigin::Module(vec!["mutation_records".into()]),
        declaration_name: name.into(),
        kind: SemanticSourceTargetKind::Function,
        scope_discriminant: None,
        declaration_span: Span::new(1, 2),
    }
}

/// Empty changed-owner maps and same-offset generated bindings must survive the canonical projection exactly.
#[test]
fn completed_facts_roundtrip_without_erasing_empty_owners() -> Result<(), Box<dyn std::error::Error>> {
    let empty = owner("a_empty");
    let changing = owner("z_changing");
    let authored = Span::new(30, 40);
    let generated = authored.with_expansion(Some([1; 32]));
    let local = MutBodyLocal {
        bodies_checked: HashSet::from([changing.clone(), empty.clone()]),
        changed: HashMap::from([
            (changing.clone(), HashSet::from(["z".into(), "a".into()])),
            (empty.clone(), HashSet::new()),
        ]),
        reassigned_locals: HashSet::from([generated, authored]),
        ..MutBodyLocal::default()
    };
    let expected = RecordMutation {
        bodies: BTreeSet::from([empty.clone(), changing.clone()]),
        changed: BTreeMap::from([
            (empty.clone(), BTreeSet::new()),
            (changing.clone(), BTreeSet::from(["a".into(), "z".into()])),
        ]),
        reassigned: BTreeSet::from([authored, generated]),
    };
    let record = local
        .record_projection()
        .ok_or("completed mutation projection refused")?;
    assert_eq!(record, expected);
    assert_eq!(record.changed.keys().collect::<Vec<_>>(), vec![&empty, &changing]);
    let restored = MutBodyLocal::from_record_projection(record);
    assert_eq!(restored.bodies_checked, local.bodies_checked);
    assert_eq!(restored.changed, local.changed);
    assert_eq!(restored.reassigned_locals, local.reassigned_locals);
    assert_eq!(restored.record_projection(), Some(expected));
    assert!(local.plain_bodies().is_none());
    assert!(restored.plain_bodies().is_none());
    let empty_record = RecordMutation::default();
    let empty_local = MutBodyLocal::from_record_projection(empty_record.clone());
    assert_eq!(empty_local.record_projection(), Some(empty_record));
    assert!(empty_local.plain_bodies().is_some());
    Ok(())
}

/// Every unrepresented capability independently refuses projection, even when the represented facts are empty.
#[test]
fn unfinished_or_unrepresented_facts_are_never_discarded() {
    let identity = owner("pending");
    let span = Span::new(10, 20);
    let cases = [
        (
            "forwards",
            MutBodyLocal {
                forwards: vec![MutParamForward {
                    owner: identity.clone(),
                    param: "value".into(),
                    callee: identity.clone(),
                    callee_param: "other".into(),
                    through_local: None,
                }],
                ..MutBodyLocal::default()
            },
        ),
        (
            "function_values",
            MutBodyLocal {
                function_values: HashMap::from([(span, identity.clone())]),
                ..MutBodyLocal::default()
            },
        ),
        (
            "loop_variables",
            MutBodyLocal {
                loop_variables: vec![(span, false)],
                ..MutBodyLocal::default()
            },
        ),
        (
            "pending",
            MutBodyLocal {
                pending: vec![PendingMutArgument {
                    order: 0,
                    callee: None,
                    callee_name: "operation".into(),
                    param: "value".into(),
                    position: 1,
                    place: ArgumentPlace::Temporary,
                    uncopyable_type: None,
                    span,
                    through_local: None,
                }],
                ..MutBodyLocal::default()
            },
        ),
        (
            "current",
            MutBodyLocal {
                current: Some(MutParamBody {
                    identity,
                    params: HashMap::new(),
                    closure_depth: 0,
                    views: Vec::new(),
                }),
                ..MutBodyLocal::default()
            },
        ),
    ];
    for (capability, local) in cases {
        assert!(local.record_projection().is_none(), "dropped {capability}");
    }
}

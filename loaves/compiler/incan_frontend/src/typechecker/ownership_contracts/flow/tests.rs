use std::collections::{BTreeMap, BTreeSet};

use super::{Access, Event, Graph, Loan, LoanId, LoanKind, Node, Permission, Place, Projection, Root, analyze};
use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind};

/// Make different lexical declarations even when a test deliberately reuses a name.
fn identity(name: &str, offset: usize) -> CanonicalSymbolId {
    CanonicalSymbolId::module_declaration(
        vec!["ownership_test".into()],
        name,
        SemanticSourceTargetKind::Local,
        HirSourceSpan::new(offset, offset + 1),
    )
}

/// An owned mutable place supplies all three permissions; field checks remain separate checked facts.
fn graph(events: Vec<Event>, edges: Vec<Vec<usize>>, loans: Vec<Loan>) -> Graph {
    Graph {
        owner: CanonicalSymbolId::module_declaration(
            vec!["ownership_test".into()],
            "run",
            SemanticSourceTargetKind::Function,
            HirSourceSpan::new(0, 100),
        ),
        permissions: [
            (
                Root::Binding(identity("value", 1)),
                Permission {
                    exclusive: true,
                    writable: true,
                    transferable: true,
                },
            ),
            (
                Root::Binding(identity("value", 2)),
                Permission {
                    exclusive: true,
                    writable: true,
                    transferable: true,
                },
            ),
        ]
        .into(),
        loans,
        direct_iteration_loans: BTreeSet::new(),
        dictionary_lookups: BTreeMap::new(),
        local_loans: BTreeMap::new(),
        dependent_locals: BTreeMap::new(),
        dependent_values: BTreeSet::new(),
        operations: BTreeMap::new(),
        owned_locals: BTreeMap::new(),
        invocations: BTreeMap::new(),
        closures: BTreeMap::new(),
        nodes: events
            .into_iter()
            .zip(edges)
            .enumerate()
            .map(|(index, (event, successors))| Node {
                event,
                successors,
                span: HirSourceSpan::new(index * 10, index * 10 + 1),
            })
            .collect(),
        entry: 0,
    }
}

/// Borrowing the first binding never aliases a shadowed binding with the same spelling.
fn place(offset: usize) -> Place {
    Place {
        root: Root::Binding(identity("value", offset)),
        projection: Vec::new(),
    }
}

/// A loan declaration's mutable permission is explicit rather than inferred from its uses.
fn loan(source: Place, kind: LoanKind, parent: Option<LoanId>) -> Loan {
    Loan {
        source,
        kind,
        writable: kind == LoanKind::Exclusive,
        parent,
        span: HirSourceSpan::new(0, 1),
    }
}

/// Name both direct and through-loan operations without changing the original storage place.
fn access(place: Place, access: Access, through: Option<LoanId>) -> Event {
    Event::Access { place, access, through }
}

/// Last dependent use is path-sensitive; a later textual loan use on another branch does not extend this path.
#[test]
fn loans_follow_edges_fields_and_reborrows() -> Result<(), String> {
    let owner = place(1);
    let shared = loan(owner.clone(), LoanKind::Shared, None);
    let conflicting = graph(
        vec![
            Event::Borrow(LoanId(0)),
            access(owner.clone(), Access::Write, None),
            access(owner.clone(), Access::Read, Some(LoanId(0))),
        ],
        vec![vec![1], vec![2], vec![]],
        vec![shared.clone()],
    );
    let result = analyze(&conflicting).map_err(|error| error.0)?;
    assert_eq!(result.conflicts.len(), 1, "write while a shared loan has a later use");
    assert_eq!(result.conflicts[0].node, 1);
    let branches = graph(
        vec![
            Event::Borrow(LoanId(0)),
            Event::Nothing,
            access(owner.clone(), Access::Read, Some(LoanId(0))),
            access(owner.clone(), Access::Write, None),
        ],
        vec![vec![1], vec![2, 3], vec![], vec![]],
        vec![shared],
    );
    assert!(
        analyze(&branches).map_err(|error| error.0)?.conflicts.is_empty(),
        "the write branch does not use the loan"
    );

    let mut left = owner.clone();
    left.projection.push(Projection::Field(identity("left", 3)));
    let mut right = owner.clone();
    right.projection.push(Projection::Field(identity("right", 4)));
    let siblings = graph(
        vec![
            Event::Borrow(LoanId(0)),
            access(right, Access::Write, None),
            access(place(2), Access::Move, None),
            access(left.clone(), Access::Write, Some(LoanId(0))),
        ],
        vec![vec![1], vec![2], vec![3], vec![]],
        vec![loan(left, LoanKind::Exclusive, None)],
    );
    assert!(
        analyze(&siblings).map_err(|error| error.0)?.conflicts.is_empty(),
        "disjoint fields and shadowed roots stay independent"
    );

    let reborrow = graph(
        vec![
            Event::Borrow(LoanId(0)),
            Event::Borrow(LoanId(1)),
            access(owner.clone(), Access::Write, Some(LoanId(0))),
            access(owner.clone(), Access::Write, Some(LoanId(1))),
        ],
        vec![vec![1], vec![2], vec![3], vec![]],
        vec![
            loan(owner.clone(), LoanKind::Exclusive, None),
            loan(owner, LoanKind::Exclusive, Some(LoanId(0))),
        ],
    );
    let result = analyze(&reborrow).map_err(|error| error.0)?;
    assert_eq!(
        result.conflicts.len(),
        1,
        "parent cannot be used while its exclusive child remains live"
    );
    assert_eq!(result.conflicts[0].node, 2);

    let mut read_only = conflicting.clone();
    read_only.nodes[1].event = access(place(1), Access::Write, Some(LoanId(0)));
    assert!(
        analyze(&read_only)
            .map_err(|error| error.0)?
            .conflicts
            .iter()
            .any(|conflict| { conflict.node == 1 && conflict.kind == super::ConflictKind::Permission }),
        "a mutable local cannot upgrade a shared loan"
    );

    let mut invalid_edge = branches;
    invalid_edge.nodes[1].successors.push(99);
    assert!(
        analyze(&invalid_edge).is_err(),
        "missing CFG facts cannot authorize execution"
    );
    checked_indices_and_creation()
}

/// Unknown indices cannot prove containment, and a loan must exist on every predecessor reaching a use.
fn checked_indices_and_creation() -> Result<(), String> {
    let mut indexed = place(1);
    indexed.projection.push(Projection::Element(HirSourceSpan::new(80, 81)));
    let mut other_index = place(1);
    other_index
        .projection
        .push(Projection::Element(HirSourceSpan::new(90, 91)));
    let indexing = graph(
        vec![
            Event::Borrow(LoanId(0)),
            access(other_index.clone(), Access::Write, None),
            access(indexed.clone(), Access::Read, Some(LoanId(0))),
        ],
        vec![vec![1], vec![2], vec![]],
        vec![loan(indexed, LoanKind::Shared, None)],
    );
    assert_eq!(
        analyze(&indexing).map_err(|error| error.0)?.conflicts.len(),
        1,
        "unknown indices may overlap"
    );
    let mut invalid_projection = indexing.clone();
    invalid_projection.nodes[2].event = access(other_index, Access::Read, Some(LoanId(0)));
    assert!(
        analyze(&invalid_projection).is_err(),
        "potential overlap is not proof that a loan reaches another index evaluation"
    );

    let conditional_creation = graph(
        vec![
            Event::Nothing,
            Event::Borrow(LoanId(0)),
            Event::Nothing,
            access(place(1), Access::Read, Some(LoanId(0))),
        ],
        vec![vec![1, 2], vec![3], vec![3], vec![]],
        vec![loan(place(1), LoanKind::Shared, None)],
    );
    assert!(
        analyze(&conditional_creation)
            .map_err(|error| error.0)?
            .conflicts
            .iter()
            .any(|conflict| { conflict.node == 3 && conflict.kind == super::ConflictKind::UnavailableLoan(LoanId(0)) }),
        "may-created is insufficient when another predecessor never borrowed"
    );

    Ok(())
}

/// Availability joins every incoming path and loops back to earlier nodes; replacing the whole place restores it.
#[test]
fn transfers_follow_joins_back_edges_and_reinitialization() -> Result<(), String> {
    let owner = place(1);
    let joined = graph(
        vec![
            Event::Nothing,
            access(owner.clone(), Access::Move, None),
            Event::Nothing,
            access(owner.clone(), Access::Read, None),
        ],
        vec![vec![1, 2], vec![3], vec![3], vec![]],
        vec![],
    );
    let result = analyze(&joined).map_err(|error| error.0)?;
    assert_eq!(
        result.conflicts.len(),
        1,
        "a move on either predecessor invalidates the joined read"
    );
    assert_eq!(result.conflicts[0].node, 3);
    let looped = graph(
        vec![
            access(owner.clone(), Access::Read, None),
            access(owner.clone(), Access::Move, None),
        ],
        vec![vec![1], vec![0]],
        vec![],
    );
    assert!(
        !analyze(&looped).map_err(|error| error.0)?.conflicts.is_empty(),
        "a previous iteration consumed the source"
    );
    let restored = graph(
        vec![
            access(owner.clone(), Access::Move, None),
            access(owner.clone(), Access::Replace, None),
            access(owner, Access::Read, None),
        ],
        vec![vec![1], vec![2], vec![]],
        vec![],
    );
    assert!(analyze(&restored).map_err(|error| error.0)?.conflicts.is_empty());

    let mut field = place(1);
    field.projection.push(Projection::Field(identity("field", 2)));
    let incomplete = graph(
        vec![
            access(place(1), Access::Move, None),
            access(field, Access::Replace, None),
        ],
        vec![vec![1], vec![]],
        vec![],
    );
    assert!(
        analyze(&incomplete)
            .map_err(|error| error.0)?
            .conflicts
            .iter()
            .any(|conflict| { conflict.node == 1 && conflict.kind == super::ConflictKind::Moved(0) }),
        "replacing a child cannot access an already consumed parent"
    );

    let mut forbidden_transfer = restored;
    forbidden_transfer
        .permissions
        .get_mut(&Root::Binding(identity("value", 1)))
        .ok_or("missing root")?
        .transferable = false;
    assert!(
        analyze(&forbidden_transfer)
            .map_err(|error| error.0)?
            .conflicts
            .iter()
            .any(|conflict| { conflict.node == 0 && conflict.kind == super::ConflictKind::Permission }),
        "a borrowed input cannot be consumed merely because its body only reads later"
    );
    Ok(())
}

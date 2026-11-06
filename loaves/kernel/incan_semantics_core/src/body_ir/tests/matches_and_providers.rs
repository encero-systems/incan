//! Match rendering, provider operation plans, constants and argument binding wire tags.

use super::*;

/// One `Rvalue::Match` exercising every `Pattern` variant this data model closes over (see #1101's B6): a
/// literal, a tuple nesting a binding and a wildcard behind a guard, a named-field struct constructor, a
/// positional enum constructor, and an alternation. Mirrors `sample_body`'s own style of hand-building a
/// [`Statement`] rather than going through the frontend lowering the
/// `loaves/compiler/incan_frontend/src/body_ir.rs` integration tests exercise instead.
#[test]
fn match_rvalue_renders_scrutinee_and_every_pattern_shape() {
    let mut body = sample_body();
    let match_stmt = Statement {
        kind: StatementKind::Assign {
            place: Place::from_local(LocalId(2)),
            rvalue: Rvalue::Match {
                scrutinee: Operand::place(Place::from_local(LocalId(0)), OwnershipFact::Borrow, false),
                arms: vec![
                    MatchArm {
                        pattern: Pattern::Literal(Constant::Int(0)),
                        guard_stmts: Vec::new(),
                        guard: None,
                        body_stmts: Vec::new(),
                        result: Operand::Constant(Constant::Int(100)),
                    },
                    MatchArm {
                        pattern: Pattern::Tuple(vec![
                            Pattern::Var(PatternBinding {
                                local: LocalId(1),
                                fact: OwnershipFact::Copy,
                                last_use: false,
                            }),
                            Pattern::Wildcard,
                        ]),
                        guard_stmts: Vec::new(),
                        guard: Some(Operand::place(
                            Place::from_local(LocalId(1)),
                            OwnershipFact::Copy,
                            false,
                        )),
                        body_stmts: Vec::new(),
                        result: Operand::place(Place::from_local(LocalId(1)), OwnershipFact::Copy, false),
                    },
                    MatchArm {
                        pattern: Pattern::Struct {
                            canonical: None,
                            name: "Point".to_string(),
                            fields: vec![("x".to_string(), Pattern::Wildcard)],
                        },
                        guard_stmts: Vec::new(),
                        guard: None,
                        body_stmts: Vec::new(),
                        result: Operand::Constant(Constant::Unit),
                    },
                    MatchArm {
                        pattern: Pattern::Enum {
                            canonical: None,
                            name: String::new(),
                            variant: "Some".to_string(),
                            fields: vec![Pattern::Wildcard],
                        },
                        guard_stmts: Vec::new(),
                        guard: None,
                        body_stmts: Vec::new(),
                        result: Operand::Constant(Constant::Unit),
                    },
                    MatchArm {
                        pattern: Pattern::Or(vec![
                            Pattern::Literal(Constant::Int(1)),
                            Pattern::Literal(Constant::Int(2)),
                        ]),
                        guard_stmts: Vec::new(),
                        guard: None,
                        body_stmts: Vec::new(),
                        result: Operand::Constant(Constant::Unit),
                    },
                ],
            },
        },
        span: HirSourceSpan::new(0, 1),
    };
    body.block.stmts.insert(0, match_stmt);
    let snapshot = body.render_snapshot();

    assert!(
        snapshot.contains("match borrow(_0) {"),
        "unexpected match rendering: {snapshot}"
    );
    assert!(
        snapshot.contains("const(0) => const(100)"),
        "literal pattern: {snapshot}"
    );
    assert!(
        snapshot.contains("(bind(_1, copy), _) if copy(_1) => copy(_1)"),
        "tuple pattern with a nested binding, wildcard, and guard: {snapshot}"
    );
    assert!(
        snapshot.contains("Point { x: _ } canonical=<unresolved> => const(())"),
        "named-field struct pattern: {snapshot}"
    );
    assert!(
        snapshot.contains("Some(_) canonical=<unresolved> => const(())"),
        "positional enum pattern: {snapshot}"
    );
    assert!(
        snapshot.contains("const(1) | const(2) => const(())"),
        "alternation pattern: {snapshot}"
    );
}

/// The plan phrases the RFC 104 question without answering any part of it itself.
#[test]
fn a_plan_builds_an_authority_request_naming_its_capability_and_operation() {
    let plan = sample_provider_operation_plan();

    let request = plan.authority_request();

    assert_eq!(request.capability, plan.required_capability);
    assert_eq!(request.operation, plan.operation);
    assert_eq!(request.request_span, plan.call_span);
    assert!(
        request.requested_scope.is_empty(),
        "scope values bind at grant time, not while lowering",
    );
}

/// The suggested grant is a rendering of the capability's declaration location, never an author-chosen string.
#[test]
fn a_capabilitys_grant_spelling_comes_from_where_it_was_declared() {
    let plan = sample_provider_operation_plan();

    assert_eq!(plan.authority_request().suggested_grant, "host.http.request");
}

/// A package-owned operation renders under its library identity, so two libraries cannot collide.
#[test]
fn a_package_owned_operation_renders_under_its_library_identity() {
    let plan = sample_provider_operation_plan();

    assert_eq!(render_symbol_path(&plan.operation), "billing.api.charge");
}

/// A provider operation renders its checked facts, so a snapshot shows what would execute.
#[test]
fn a_provider_operation_call_renders_its_plan() {
    let mut body = sample_body();
    body.block.stmts.insert(
        0,
        Statement {
            kind: StatementKind::Call {
                destination: Some(Place::from_local(LocalId(2))),
                callee: Callee::ProviderOperation(Box::new(sample_provider_operation_plan())),
                args: vec![ArgumentElement::One(Operand::Constant(Constant::Str("x".to_string())))],
                may_panic: false,
            },
            span: HirSourceSpan::new(110, 130),
        },
    );

    let snapshot = body.render_snapshot();

    assert!(
        snapshot.contains("provider_operation:billing.api.charge"),
        "the canonical operation identity must be visible: {snapshot}"
    );
    assert!(
        snapshot.contains("capability=host.http.request"),
        "the required capability must be visible: {snapshot}"
    );
    assert!(
        snapshot.contains("provider=billing@1.2.3#abc[]@billing.api:active"),
        "provider activation must be visible: {snapshot}"
    );
    assert_eq!(body.render_snapshot(), snapshot, "rendering must be deterministic");
}

#[test]
fn match_rvalue_snapshot_is_deterministic() {
    let mut body = sample_body();
    body.block.stmts.insert(
        0,
        Statement {
            kind: StatementKind::Assign {
                place: Place::from_local(LocalId(2)),
                rvalue: Rvalue::Match {
                    scrutinee: Operand::place(Place::from_local(LocalId(0)), OwnershipFact::Borrow, false),
                    arms: vec![MatchArm {
                        pattern: Pattern::Wildcard,
                        guard_stmts: Vec::new(),
                        guard: None,
                        body_stmts: Vec::new(),
                        result: Operand::Constant(Constant::Unit),
                    }],
                },
            },
            span: HirSourceSpan::new(0, 1),
        },
    );
    assert_eq!(body.render_snapshot(), body.render_snapshot());
}

#[test]
fn bytes_constants_and_range_aggregates_render_distinctly() {
    let mut body = sample_body();
    body.block.stmts.insert(
        0,
        Statement {
            kind: StatementKind::Assign {
                place: Place::from_local(LocalId(2)),
                rvalue: Rvalue::Use(Operand::Constant(Constant::Bytes(b"hi".to_vec()))),
            },
            span: HirSourceSpan::new(0, 1),
        },
    );
    body.block.stmts.insert(
        1,
        Statement {
            kind: StatementKind::Assign {
                place: Place::from_local(LocalId(2)),
                rvalue: Rvalue::Aggregate(
                    AggregateKind::Range,
                    vec![
                        ArgumentElement::One(Operand::Constant(Constant::Int(0))),
                        ArgumentElement::One(Operand::Constant(Constant::Int(10))),
                        ArgumentElement::One(Operand::Constant(Constant::Int(1))),
                        ArgumentElement::One(Operand::Constant(Constant::Bool(true))),
                    ],
                ),
            },
            span: HirSourceSpan::new(0, 1),
        },
    );
    let snapshot = body.render_snapshot();

    // A byte string escapes every byte, so it can never be mistaken for the `str` constant with the same
    // contents -- which is exactly the conflation `Constant::Bytes` exists to prevent.
    assert!(
        snapshot.contains("const(b\"\\x68\\x69\")"),
        "bytes constant: {snapshot}"
    );
    assert_ne!(
        Constant::Bytes(b"hi".to_vec()).render_snapshot(),
        Constant::Str("hi".to_string()).render_snapshot(),
        "a bytes constant and a string constant with the same contents must not render alike"
    );
    assert!(
        snapshot.contains("range[const(0), const(10), const(1), const(true)]"),
        "range aggregate in RANGE_FIELDS order: {snapshot}"
    );
    assert_eq!(AggregateKind::RANGE_FIELDS.len(), 4);
}

/// Generalizing the semantic variant preserves old serialized SDK/body metadata in both directions.
#[test]
fn declaration_order_binding_preserves_existing_wire_tag() -> Result<(), Box<dyn std::error::Error>> {
    let original = serde_json::json!({
        "ResolvedFactory": {
            "arguments": [
                { "slot": 0, "written_position": 1 },
                { "slot": 1, "written_position": 0 }
            ],
            "defaulted_slots": [2]
        }
    });
    let binding: ArgumentBinding = serde_json::from_value(original.clone())?;
    assert_eq!(
        binding,
        ArgumentBinding::ResolvedDeclarationOrder {
            arguments: vec![
                BoundArgument {
                    slot: 0,
                    written_position: 1
                },
                BoundArgument {
                    slot: 1,
                    written_position: 0
                },
            ],
            defaulted_slots: vec![2],
        }
    );
    assert_eq!(serde_json::to_value(&binding)?, original);
    Ok(())
}

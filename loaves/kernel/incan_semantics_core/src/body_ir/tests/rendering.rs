//! Body snapshot rendering of locals, rvalues, aggregates, iteration, closures and generators.

use super::*;

#[test]
fn body_snapshot_renders_locals_and_control_flow() {
    let snapshot = sample_body().render_snapshot();
    assert!(snapshot.contains("body add decl:m::add span=0..30"));
    assert!(snapshot.contains("local 0 x : int [param]"));
    assert!(snapshot.contains("local 2 <tmp> : int [temp]"));
    assert!(snapshot.contains("_2 = copy(_0) + copy(_1, last_use)"));
    assert!(snapshot.contains("return move(_2, last_use)"));
}

#[test]
fn generator_rvalue_keeps_construction_inputs_separate_from_its_deferred_body() {
    let source_local = LocalId(2);
    let capture_local = LocalId(1);
    let generator = Rvalue::Generator {
        source: Operand::Constant(Constant::Int(1)),
        captured_operands: vec![Operand::place(
            Place::from_local(capture_local),
            OwnershipFact::Copy,
            false,
        )],
        body: Box::new(GeneratorBody {
            source_local,
            capture_locals: vec![capture_local],
            stmts: vec![Statement {
                kind: StatementKind::Yield {
                    value: Operand::Constant(Constant::Int(2)),
                },
                span: HirSourceSpan::new(10, 15),
            }],
        }),
    };
    let snapshot = generator.render_snapshot();
    assert_eq!(
        snapshot,
        "generator(source=const(1), captures=[copy(_1)]) { deferred: yield const(2) }"
    );

    let mut body = sample_body();
    body.block.stmts.insert(
        0,
        Statement {
            kind: StatementKind::Assign {
                place: Place::from_local(source_local),
                rvalue: generator,
            },
            span: HirSourceSpan::new(0, 15),
        },
    );
    assert!(
        !body.is_generator(),
        "a yielded generator expression is a value; it must not turn its enclosing function into a generator"
    );
}

#[test]
fn a_dict_spread_cannot_be_mispaired_with_the_following_key() {
    // With a flattened key/value list, `{**base, "x": 1}` had to be walked carefully or the spread source
    // would pair with the following key. Structured entries make that state unrepresentable; this pins the
    // rendering that proves the entries stay distinct.
    let entries = vec![
        DictEntry::Spread(SpreadElement {
            source: Operand::Constant(Constant::Str("base".to_string())),
            kind: SpreadKind::Mapping,
        }),
        DictEntry::Pair(
            Operand::Constant(Constant::Str("x".to_string())),
            Box::new(Operand::Constant(Constant::Int(1))),
        ),
    ];

    assert_eq!(
        Rvalue::Dict(entries).render_snapshot(),
        "dict[**const(\"base\"), const(\"x\"): const(1)]"
    );
}

#[test]
fn a_non_spread_element_renders_exactly_as_its_operand_did() {
    // Every pre-existing snapshot assertion depends on this: wrapping operands in `ArgumentElement` must not
    // change how an ordinary aggregate or call renders.
    let operand = Operand::Constant(Constant::Int(7));
    assert_eq!(
        ArgumentElement::One(operand.clone()).render_snapshot(),
        operand.render_snapshot()
    );
    assert_eq!(
        ArgumentElement::Spread(SpreadElement {
            source: operand.clone(),
            kind: SpreadKind::Sequence,
        })
        .render_snapshot(),
        format!("*{}", operand.render_snapshot())
    );
    assert_eq!(
        ArgumentElement::Spread(SpreadElement {
            source: operand.clone(),
            kind: SpreadKind::Mapping,
        })
        .render_snapshot(),
        format!("**{}", operand.render_snapshot())
    );
}

#[test]
fn helper_call_and_runtime_requirements_render() {
    let mut body = sample_body();
    body.block.stmts.insert(
        0,
        Statement {
            kind: StatementKind::Call {
                destination: Some(Place::from_local(LocalId(2))),
                callee: Callee::Helper(HelperOp::StrConcat),
                args: vec![ArgumentElement::One(Operand::Constant(Constant::Str("a".to_string())))],
                may_panic: false,
            },
            span: HirSourceSpan::new(0, 1),
        },
    );
    body.runtime_requirements = vec![
        AbiV0RuntimeRequirement::Allocator,
        AbiV0RuntimeRequirement::RuntimeHelper("str_concat".to_string()),
    ];
    body.panic_facts = vec![PanicFact {
        span: HirSourceSpan::new(20, 25),
        reason: PanicReason::DivisionOrModulo,
    }];

    let snapshot = body.render_snapshot();
    assert!(snapshot.contains("call helper:str_concat(const(\"a\"))"));
    assert!(snapshot.contains("runtime_requirements:"));
    assert!(snapshot.contains("allocator"));
    assert!(snapshot.contains("runtime_helper(str_concat)"));
    assert!(snapshot.contains("panic_facts:"));
    assert!(snapshot.contains("division_or_modulo span=20..25"));
}

#[test]
fn local_callable_target_and_tagged_default_parameter_render() {
    let mut body = sample_body();
    body.block.stmts.insert(
        0,
        Statement {
            kind: StatementKind::Call {
                destination: Some(Place::from_local(LocalId(2))),
                callee: Callee::Function(CallableTarget::Local(LocalCallableTarget {
                    operand: PlaceOperand {
                        place: Place::from_local(LocalId(0)),
                        fact: OwnershipFact::Copy,
                        last_use: false,
                    },
                    binding: ArgumentBinding::resolved_positional(1),
                })),
                args: vec![ArgumentElement::One(Operand::Constant(Constant::Int(1)))],
                may_panic: false,
            },
            span: HirSourceSpan::new(0, 1),
        },
    );

    let snapshot = body.render_snapshot();
    assert!(
        snapshot.contains("call local:copy(_0)(const(1))"),
        "a local call target must retain its operand ownership fact: {snapshot}"
    );
    assert_eq!(
        CallableParam {
            mutable: false,
            input_access: crate::IncanInputAccess::Inferred,
            native_owned: false,
            kind: IncanCallableParamKind::Normal,
            local: LocalId(7),
            name: "suffix".to_string(),
            ty: IncanType::Primitive(IncanPrimitiveType::Str),
            span: HirSourceSpan::new(12, 18),
            default: CallableParamDefault::Source(Box::new(DefaultComputation {
                span: HirSourceSpan::new(21, 27),
                stmts: Vec::new(),
                result: Operand::Constant(Constant::Str("txt".to_string())),
            })),
        }
        .render_snapshot(),
        "suffix: str local=_7 span=12..18 = source_default(span=21..27 result: const(\"txt\"))"
    );
}

#[test]
fn callable_parameter_default_origins_are_distinct_and_deterministic() {
    let source = CallableParam {
        mutable: false,
        input_access: crate::IncanInputAccess::Inferred,
        native_owned: false,
        kind: IncanCallableParamKind::Normal,
        local: LocalId(1),
        name: "limit".to_string(),
        ty: IncanType::Primitive(IncanPrimitiveType::Int),
        span: HirSourceSpan::new(4, 14),
        default: CallableParamDefault::Source(Box::new(DefaultComputation {
            span: HirSourceSpan::new(12, 13),
            stmts: vec![Statement {
                kind: StatementKind::Assign {
                    place: Place::from_local(LocalId(3)),
                    rvalue: Rvalue::UnaryOp(UnOp::Neg, Operand::Constant(Constant::Int(1))),
                },
                span: HirSourceSpan::new(12, 13),
            }],
            result: Operand::place(Place::from_local(LocalId(3)), OwnershipFact::Copy, true),
        })),
    };
    let preset = CallableParam {
        mutable: false,
        input_access: crate::IncanInputAccess::Inferred,
        native_owned: false,
        kind: IncanCallableParamKind::Normal,
        local: LocalId(2),
        name: "method".to_string(),
        ty: IncanType::Primitive(IncanPrimitiveType::Str),
        span: HirSourceSpan::new(16, 27),
        default: CallableParamDefault::PartialPreset { capture: LocalId(6) },
    };
    let unsupported = CallableParam {
        mutable: false,
        input_access: crate::IncanInputAccess::Inferred,
        native_owned: false,
        kind: IncanCallableParamKind::Normal,
        local: LocalId(4),
        name: "payload".to_string(),
        ty: IncanType::Primitive(IncanPrimitiveType::Bytes),
        span: HirSourceSpan::new(29, 43),
        default: CallableParamDefault::Unsupported {
            span: HirSourceSpan::new(40, 43),
            description: "bytes literal".to_string(),
        },
    };

    assert_eq!(
        source.render_snapshot(),
        "limit: int local=_1 span=4..14 = source_default(span=12..13 _3 = -const(1); result: copy(_3, last_use))"
    );
    assert_eq!(
        preset.render_snapshot(),
        "method: str local=_2 span=16..27 = captured(_6)"
    );
    assert_eq!(
        unsupported.render_snapshot(),
        "payload: bytes local=_4 span=29..43 = unsupported_default(bytes literal span=40..43)"
    );
}

#[test]
fn dict_aggregate_renders_as_key_value_pairs() {
    let mut body = sample_body();
    body.block.stmts.insert(
        0,
        Statement {
            kind: StatementKind::Assign {
                place: Place::from_local(LocalId(2)),
                rvalue: Rvalue::Dict(vec![DictEntry::Pair(
                    Operand::Constant(Constant::Str("a".to_string())),
                    Box::new(Operand::Constant(Constant::Int(1))),
                )]),
            },
            span: HirSourceSpan::new(0, 1),
        },
    );
    let snapshot = body.render_snapshot();
    assert!(
        snapshot.contains("dict[const(\"a\"): const(1)]"),
        "dict aggregate should render key/value pairs, not a flat list: {snapshot}"
    );
}

#[test]
fn set_aggregate_renders_as_a_flat_element_list() {
    let mut body = sample_body();
    body.block.stmts.insert(
        0,
        Statement {
            kind: StatementKind::Assign {
                place: Place::from_local(LocalId(2)),
                rvalue: Rvalue::Aggregate(
                    AggregateKind::Set,
                    vec![ArgumentElement::One(Operand::Constant(Constant::Int(1)))],
                ),
            },
            span: HirSourceSpan::new(0, 1),
        },
    );
    let snapshot = body.render_snapshot();
    assert!(snapshot.contains("set[const(1)]"));
}

#[test]
fn slice_projection_renders_optional_components() {
    let full_slice = Place {
        root: PlaceRoot::Local(LocalId(0)),
        projection: vec![PlaceElem::Slice {
            start: Some(Box::new(Operand::Constant(Constant::Int(1)))),
            end: Some(Box::new(Operand::Constant(Constant::Int(3)))),
            step: None,
        }],
    };
    assert_eq!(full_slice.render_snapshot(), "_0[const(1):const(3)]");

    let stepped_slice = Place {
        root: PlaceRoot::Local(LocalId(0)),
        projection: vec![PlaceElem::Slice {
            start: None,
            end: None,
            step: Some(Box::new(Operand::Constant(Constant::Int(2)))),
        }],
    };
    assert_eq!(stepped_slice.render_snapshot(), "_0[::const(2)]");
}

#[test]
fn try_propagate_statement_renders_destination_and_operand() {
    let mut body = sample_body();
    body.block.stmts.insert(
        0,
        Statement {
            kind: StatementKind::TryPropagate {
                destination: Place::from_local(LocalId(2)),
                operand: Operand::place(Place::from_local(LocalId(0)), OwnershipFact::Move, true),
                error_routing: TryErrorRouting::SameType {
                    error_type: IncanType::Named("E".to_string()),
                },
            },
            span: HirSourceSpan::new(0, 1),
        },
    );
    let snapshot = body.render_snapshot();
    assert!(snapshot.contains("_2 = try?(move(_0, last_use), same_error_type=E)"));
}

#[test]
fn iter_next_renders_builtin_protocol() {
    let mut body = sample_body();
    body.block.stmts.insert(
        0,
        Statement {
            kind: StatementKind::IterNext {
                destination: Place::from_local(LocalId(2)),
                iterator: Operand::place(Place::from_local(LocalId(0)), OwnershipFact::MutBorrow, false),
                protocol: IterProtocol::Builtin,
            },
            span: HirSourceSpan::new(0, 1),
        },
    );
    let snapshot = body.render_snapshot();
    assert!(snapshot.contains("_2 = iter_next(mut_borrow(_0), builtin)"));
}

#[test]
fn iter_next_renders_user_defined_protocol_and_fallible_flag() {
    let mut body = sample_body();
    body.block.stmts.insert(
        0,
        Statement {
            kind: StatementKind::IterNext {
                destination: Place::from_local(LocalId(2)),
                iterator: Operand::place(Place::from_local(LocalId(0)), OwnershipFact::MutBorrow, false),
                protocol: IterProtocol::UserDefined {
                    next_method: "__next__".to_string(),
                    fallible: false,
                },
            },
            span: HirSourceSpan::new(0, 1),
        },
    );
    let snapshot = body.render_snapshot();
    assert!(snapshot.contains("_2 = iter_next(mut_borrow(_0), user_defined(__next__))"));

    let mut fallible_body = sample_body();
    fallible_body.block.stmts.insert(
        0,
        Statement {
            kind: StatementKind::IterNext {
                destination: Place::from_local(LocalId(2)),
                iterator: Operand::place(Place::from_local(LocalId(0)), OwnershipFact::MutBorrow, false),
                protocol: IterProtocol::UserDefined {
                    next_method: "__next__".to_string(),
                    fallible: true,
                },
            },
            span: HirSourceSpan::new(0, 1),
        },
    );
    let fallible_snapshot = fallible_body.render_snapshot();
    assert!(fallible_snapshot.contains("_2 = iter_next(mut_borrow(_0), user_defined(__next__, fallible))"));
}

#[test]
fn format_rvalue_renders_literal_and_expr_parts_in_order() {
    let mut body = sample_body();
    body.block.stmts.insert(
        0,
        Statement {
            kind: StatementKind::Assign {
                place: Place::from_local(LocalId(2)),
                rvalue: Rvalue::Format(vec![
                    FormatPart::Literal("x=".to_string()),
                    FormatPart::Expr {
                        operand: Box::new(Operand::place(
                            Place::from_local(LocalId(0)),
                            OwnershipFact::Copy,
                            false,
                        )),
                        style: FormatStyle::Display,
                    },
                    FormatPart::Literal(" y=".to_string()),
                    FormatPart::Expr {
                        operand: Box::new(Operand::place(
                            Place::from_local(LocalId(1)),
                            OwnershipFact::Borrow,
                            false,
                        )),
                        style: FormatStyle::Debug,
                    },
                ]),
            },
            span: HirSourceSpan::new(0, 1),
        },
    );
    let snapshot = body.render_snapshot();
    assert!(
        snapshot.contains("_2 = fstring(lit(\"x=\"), copy(_0):display, lit(\" y=\"), borrow(_1):debug)"),
        "unexpected fstring rendering: {snapshot}"
    );
}

#[test]
fn closure_rvalue_renders_params_captures_and_nested_body() {
    let mut body = sample_body();
    // Simulate `(z: int) => x + z` capturing `_0` (the sample body's `x` param) with a `clone` fact, where the
    // closure's own param `z` gets local `_3` and the capture gets local `_4`.
    let param_local = LocalId(3);
    let capture_local = LocalId(4);
    body.locals.push(LocalDecl {
        id: param_local,
        name: Some("z".to_string()),
        identity: None,
        ty: IncanType::Primitive(IncanPrimitiveType::Int),
        origin: LocalOrigin::Parameter,
        scope: ScopeId(0),
        span: HirSourceSpan::new(0, 1),
    });
    body.locals.push(LocalDecl {
        id: capture_local,
        name: Some("x".to_string()),
        identity: None,
        ty: IncanType::Primitive(IncanPrimitiveType::Int),
        origin: LocalOrigin::Captured,
        scope: ScopeId(0),
        span: HirSourceSpan::new(0, 1),
    });
    body.block.stmts.insert(
        0,
        Statement {
            kind: StatementKind::Assign {
                place: Place::from_local(LocalId(2)),
                rvalue: Rvalue::Closure {
                    params: vec![CallableParam {
                        mutable: false,
                        input_access: crate::IncanInputAccess::Inferred,
                        native_owned: false,
                        kind: IncanCallableParamKind::Normal,
                        local: param_local,
                        name: "z".to_string(),
                        ty: IncanType::Primitive(IncanPrimitiveType::Int),
                        span: HirSourceSpan::new(0, 1),
                        default: CallableParamDefault::Required,
                    }],
                    captured_operands: vec![Operand::place(
                        Place::from_local(LocalId(0)),
                        OwnershipFact::Clone,
                        false,
                    )],
                    body: Box::new(ClosureBody {
                        native_storage: None,
                        original_callable: None,
                        function_item: false,
                        consumes_environment: false,
                        capture_locals: vec![capture_local],
                        stmts: Vec::new(),
                        result: Operand::place(
                            Place {
                                root: PlaceRoot::Local(capture_local),
                                projection: Vec::new(),
                            },
                            OwnershipFact::Copy,
                            false,
                        ),
                    }),
                },
            },
            span: HirSourceSpan::new(0, 1),
        },
    );
    let snapshot = body.render_snapshot();
    assert!(
        snapshot.contains("closure(params=[z: int local=_3 span=0..1], captures=[clone(_0)]) { result: copy(_4) }"),
        "unexpected closure rendering: {snapshot}"
    );
    assert!(snapshot.contains("local 4 x : int [captured]"));
}

#[test]
fn yield_statement_renders_and_marks_the_body_as_a_generator() {
    let mut body = sample_body();
    body.block.stmts.insert(
        0,
        Statement {
            kind: StatementKind::Yield {
                value: Operand::place(Place::from_local(LocalId(0)), OwnershipFact::Copy, false),
            },
            span: HirSourceSpan::new(0, 1),
        },
    );
    let snapshot = body.render_snapshot();
    assert!(
        snapshot.contains("yield copy(_0)"),
        "unexpected yield rendering: {snapshot}"
    );
    assert!(
        body.is_generator(),
        "a top-level yield should mark the body a generator"
    );
}

#[test]
fn is_generator_finds_a_yield_nested_inside_if_and_loop_blocks() {
    let mut body = sample_body();
    let yield_stmt = Statement {
        kind: StatementKind::Yield {
            value: Operand::Constant(Constant::Int(1)),
        },
        span: HirSourceSpan::new(0, 1),
    };
    // Nested under a `Loop` inside an `If`'s `then_block`, mirroring `yield` inside `if cond: while ...`.
    body.block.stmts.insert(
        0,
        Statement {
            kind: StatementKind::If {
                cond: Operand::Constant(Constant::Bool(true)),
                then_block: Block {
                    scope: ScopeId(0),
                    stmts: vec![Statement {
                        kind: StatementKind::Loop {
                            body: Block {
                                scope: ScopeId(0),
                                stmts: vec![yield_stmt],
                            },
                        },
                        span: HirSourceSpan::new(0, 1),
                    }],
                },
                else_block: None,
            },
            span: HirSourceSpan::new(0, 1),
        },
    );
    assert!(
        body.is_generator(),
        "a yield nested under If -> Loop should still be found"
    );
}

#[test]
fn is_generator_is_false_without_any_yield_statement() {
    assert!(
        !sample_body().is_generator(),
        "sample_body contains no yield and must not be reported as a generator"
    );
}

#[test]
fn locals_requiring_unwind_drop_is_conservative_over_non_copy_locals() {
    let mut body = sample_body();
    body.locals.push(LocalDecl {
        id: LocalId(3),
        name: Some("s".to_string()),
        identity: None,
        ty: IncanType::Primitive(IncanPrimitiveType::Str),
        origin: LocalOrigin::UserBinding,
        scope: ScopeId(0),
        span: HirSourceSpan::new(10, 15),
    });

    let drop_relevant = body.locals_requiring_unwind_drop();
    assert_eq!(drop_relevant, vec![LocalId(3)]);
}

#[test]
fn locals_requiring_unwind_drop_excludes_receiver_locals_even_when_non_copy() {
    let mut body = sample_body();
    body.locals.push(LocalDecl {
        id: LocalId(3),
        name: Some("self".to_string()),
        identity: None,
        ty: IncanType::Named("Counter".to_string()),
        origin: LocalOrigin::Receiver { mutable: true },
        scope: ScopeId(0),
        span: HirSourceSpan::new(0, 4),
    });

    let drop_relevant = body.locals_requiring_unwind_drop();
    assert!(
        !drop_relevant.contains(&LocalId(3)),
        "a receiver is a reference, not drop-relevant, even though its type is non-Copy: {drop_relevant:?}"
    );
}

#[test]
fn receiver_origin_renders_mutability_in_the_snapshot() {
    let mut body = sample_body();
    body.locals.push(LocalDecl {
        id: LocalId(3),
        name: Some("self".to_string()),
        identity: None,
        ty: IncanType::Named("Counter".to_string()),
        origin: LocalOrigin::Receiver { mutable: false },
        scope: ScopeId(0),
        span: HirSourceSpan::new(0, 4),
    });
    body.locals.push(LocalDecl {
        id: LocalId(4),
        name: Some("self".to_string()),
        identity: None,
        ty: IncanType::Named("Counter".to_string()),
        origin: LocalOrigin::Receiver { mutable: true },
        scope: ScopeId(0),
        span: HirSourceSpan::new(0, 4),
    });

    let snapshot = body.render_snapshot();
    assert!(snapshot.contains("local 3 self : Counter [receiver]"));
    assert!(snapshot.contains("local 4 self : Counter [receiver_mut]"));
}

#[test]
/// Retain stable module snapshots when consumer-only facade bindings are absent.
fn body_ir_module_snapshot_wraps_bodies() {
    let module = BodyIrModule {
        runtime_callable_bindings: Vec::new(),
        decorated_bindings: Vec::new(),
        entrypoint_effects: Vec::new(),
        program_entrypoints: Vec::new(),
        string_conversion_contract: StringConversionContract::Unselected,
        constant_declarations: Vec::new(),
        type_alias_declarations: Vec::new(),
        module_id: CompilerNodeId::new(CompilerNodeKind::Module, "m"),
        loaf_callables: Vec::new(),
        native_union_declarations: Vec::new(),
        enum_declarations: Vec::new(),
        static_declarations: Vec::new(),
        nominal_declarations: Vec::new(),
        fieldless_enum_declarations: Vec::new(),
        value_enum_declarations: Vec::new(),
        trait_declarations: Vec::new(),
        trait_implementations: Vec::new(),
        bodies: vec![sample_body()],
    };
    let snapshot = module.render_snapshot();
    assert!(snapshot.starts_with("body_ir_module module:m\n"));
    assert!(snapshot.contains("body add decl:m::add"));
}

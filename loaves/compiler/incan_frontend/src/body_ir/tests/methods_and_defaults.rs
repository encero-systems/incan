//! Method bodies and parameter defaults: receiver reads and mutation, abstract, default and static trait methods,
//! chained and tuple-swap assignment, recorded parameter types, deferred default computations for top-level, generic,
//! trait and byte-string defaults, refused defaults that restore ownership state, and newtype and enum method bodies.

use super::*;

#[test]
fn lowers_an_immutable_receiver_read_through_a_field_projection() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Counter:\n  value: int\n\n  def get(self) -> int:\n    return self.value\n";
    let module = build(source, &["m", "receiver_read"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("body get decl:m::receiver_read::Counter::get"));
    assert!(snapshot.contains("local 0 self : Counter [receiver]"));
    // `self.value` is a projected read of an `int` (Copy) field, so it reads `copy`, never `move` or `clone`.
    assert!(snapshot.contains("return copy(_0.value)"));

    Ok(())
}

#[test]
fn mut_self_receiver_origin_is_mutable_and_field_mutation_lowers() -> Result<(), Box<dyn std::error::Error>> {
    // `mut self` must remain a mutable receiver when its field assignment is lowered.
    let source = "model Counter:\n  value: int\n\n  def bump(mut self) -> None:\n    self.value = self.value + 1\n";
    let module = build(source, &["m", "receiver_mut"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("body bump decl:m::receiver_mut::Counter::bump"));
    assert!(snapshot.contains("local 0 self : Counter [receiver_mut]"));
    assert!(
        !snapshot.contains("unsupported("),
        "mutable receiver field assignment should lower without a placeholder: {snapshot}"
    );

    Ok(())
}

#[test]
fn lowers_a_method_call_on_self_with_a_borrowed_receiver_argument() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Counter:\n  value: int\n\n  def get(self) -> int:\n    return self.value\n\n  def get_twice(self) -> int:\n    return self.get() + self.get()\n";
    let module = build(source, &["m", "method_call"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("body get_twice decl:m::method_call::Counter::get_twice"));
    // Method-call receivers borrow, mirroring how any other method call's receiver already lowers.
    assert!(snapshot.contains("call method:get(borrow(_0))"));
    Ok(())
}

#[test]
fn abstract_trait_method_produces_no_body() -> Result<(), Box<dyn std::error::Error>> {
    let source = "trait Greeter:\n  def greet(self) -> str: ...\n";
    let module = build(source, &["m", "abstract_method"])?;

    assert!(
        module.bodies.is_empty(),
        "an abstract method has no body to lower, and must not produce an Unsupported placeholder body either: {:?}",
        module.bodies
    );

    Ok(())
}

#[test]
fn lowers_tuple_assign_swap_with_correct_evaluation_order() -> Result<(), Box<dyn std::error::Error>> {
    // `arr[i], arr[j] = (arr[j], arr[i])` must read both original values before writing either target, or the
    // swap would clobber `arr[i]` before `arr[j]`'s read observes it. A leading plain-identifier target (`a, b
    // = ...`) always parses as `TupleUnpackStmt` instead (new bindings, possibly shadowing) -- lvalue index/
    // field targets are what actually reaches `TupleAssignStmt`, matching the parser's own routing
    // (`loaves/kernel/incan_syntax/src/parser/stmts.rs`'s `assignment_or_expr_stmt`).
    let source =
        "def swap(mut arr: list[int], i: int, j: int) -> int:\n  arr[i], arr[j] = (arr[j], arr[i])\n  return arr[i]\n";
    let module = build(source, &["m", "tuple_assign"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "tuple assign should not fall back: {snapshot}"
    );
    // Both targets should end up written via a plain `Assign` into an `[index]`-projected place, not
    // `Unsupported`.
    assert!(
        snapshot.matches("] = ").count() >= 2,
        "both index-projected targets should be assigned: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_a_default_trait_method_with_a_self_typed_receiver() -> Result<(), Box<dyn std::error::Error>> {
    let source = "trait Identity:\n  def identity(self) -> Self:\n    return self\n";
    let module = build(source, &["m", "trait_default"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("body identity decl:m::trait_default::Identity::identity"));
    assert!(snapshot.contains("local 0 self : Self [receiver]"));
    assert!(snapshot.contains("return clone(_0)"));

    Ok(())
}

#[test]
fn lowers_chained_assignment_right_to_left() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def chain() -> int:\n  x = y = z = 5\n  return x + y + z\n";
    let module = build(source, &["m", "chained"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "chained assignment should not fall back: {snapshot}"
    );
    assert!(
        snapshot.contains("const(5)"),
        "the rightmost target reads the literal value: {snapshot}"
    );
    Ok(())
}

#[test]
fn static_method_lowers_like_a_free_function_with_no_receiver_local() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Counter:\n  value: int\n\n  def zero() -> Counter:\n    return Counter(value=0)\n";
    let module = build(source, &["m", "static_method"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("body zero decl:m::static_method::Counter::zero"));
    assert!(
        !snapshot.contains("[receiver"),
        "a static/associated method (receiver: None) must not declare a receiver local: {snapshot}"
    );

    Ok(())
}

#[test]
fn method_parameter_type_is_recorded_from_the_checked_callable_signature() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "model Counter:\n  value: int\n\n  def add(self, amount: int) -> int:\n    return self.value + amount\n";
    let module = build(source, &["m", "method_param"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("body add decl:m::method_param::Counter::add"));
    assert!(
        snapshot.contains("local 1 amount : int [param]"),
        "an ordinary method parameter must declare with its checked resolved type, not Unknown: {snapshot}"
    );

    Ok(())
}

#[test]
fn top_level_defaults_lower_to_deferred_source_computations() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def fallback() -> int:\n  return 2\n\ndef choose(limit: u8 = 7, value: int = fallback()) -> int:\n  return value\n";
    let module = build(source, &["m", "top_level_default"])?;
    let choose = module
        .bodies
        .iter()
        .find(|body| body.name == "choose")
        .ok_or("expected the choose Body IR")?;
    let limit = choose.params.first().ok_or("expected choose's limit parameter")?;
    let value = choose.params.get(1).ok_or("expected choose's value parameter")?;

    assert_eq!(limit.local, bir::LocalId(0));
    assert_eq!(limit.name, "limit");
    assert_eq!(
        limit.ty,
        IncanType::Primitive(IncanPrimitiveType::Numeric(
            incan_lang::lang::types::numerics::NumericTypeId::U8
        ))
    );
    let bir::CallableParamDefault::Source(limit_default) = &limit.default else {
        return Err("a checked literal default must become a deferred Body-IR computation".into());
    };
    let limit_start = source.find("7,").ok_or("missing literal default source spelling")?;
    assert_eq!(limit_default.span, HirSourceSpan::new(limit_start, limit_start + 1));
    assert!(limit_default.stmts.is_empty());
    assert_eq!(
        limit_default.result,
        bir::Operand::Constant(bir::Constant::TypedNumeric(bir::TypedNumericConstant::Unsigned {
            kind: incan_lang::lang::types::numerics::NumericTypeId::U8,
            value: 7,
        }))
    );

    assert_eq!(value.local, bir::LocalId(1));
    assert_eq!(value.name, "value");
    let bir::CallableParamDefault::Source(value_default) = &value.default else {
        return Err("a checked function default call must become a deferred Body-IR computation".into());
    };
    let call_start = source.rfind("fallback()").ok_or("missing default source spelling")?;
    assert_eq!(
        value_default.span,
        HirSourceSpan::new(call_start, call_start + "fallback()".len()),
        "the direct consumer must receive the default expression's exact source span"
    );
    let [call] = value_default.stmts.as_slice() else {
        return Err("the deferred function default should contain one call statement".into());
    };
    let bir::StatementKind::Call {
        destination: Some(destination),
        callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
        args,
        may_panic,
    } = &call.kind
    else {
        return Err("the deferred default must retain a direct named call".into());
    };
    assert_eq!(target.name, "fallback");
    assert!(target.type_args.is_empty());
    assert!(args.is_empty());
    assert!(!may_panic);
    let bir::Operand::Place(result) = &value_default.result else {
        return Err("the deferred default call must return its computed temporary".into());
    };
    assert_eq!(&result.place, destination);
    assert!(
        !choose.block.stmts.iter().any(|statement| matches!(
            &statement.kind,
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                ..
            } if target.name == "fallback"
        )),
        "the default call must not be appended to the ordinary function body: {choose:?}"
    );
    assert!(
        !choose
            .locals
            .iter()
            .any(|local| matches!(local.origin, bir::LocalOrigin::External)),
        "a refused source default must not retain an implicit frontend lookup: {choose:?}"
    );

    Ok(())
}

#[test]
fn generic_method_defaults_use_the_shared_parameter_contract_after_self() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def fallback() -> str:\n  return \"label\"\n\nmodel Shelf[T]:\n  def label[U](self, owner_items: list[T] = [], method_items: list[U] = [], suffix: str = \"\", fallback_label: str = fallback()) -> str:\n    return suffix\n";
    let module = build(source, &["m", "method_default"])?;
    let label = module
        .bodies
        .iter()
        .find(|body| body.name == "label")
        .ok_or("expected the label method Body IR")?;
    let self_param = label.params.first().ok_or("expected the self parameter")?;
    let owner_items = label.params.get(1).ok_or("expected the owner-generic parameter")?;
    let method_items = label.params.get(2).ok_or("expected the method-generic parameter")?;
    let suffix = label.params.get(3).ok_or("expected the literal default parameter")?;
    let fallback_label = label.params.get(4).ok_or("expected the call default parameter")?;

    assert_eq!(self_param.local, bir::LocalId(0));
    assert!(
        self_param.span.start < self_param.span.end,
        "the synthetic receiver must carry its documented declaration-span fallback"
    );
    assert!(matches!(&self_param.default, bir::CallableParamDefault::Required));
    assert!(matches!(&owner_items.default, bir::CallableParamDefault::Source(_)));
    assert!(matches!(&method_items.default, bir::CallableParamDefault::Source(_)));
    let bir::CallableParamDefault::Source(suffix_default) = &suffix.default else {
        return Err("a checked method literal default must become a deferred computation".into());
    };
    let literal_start = source.find("\"\"").ok_or("missing method literal default spelling")?;
    assert_eq!(
        suffix_default.span,
        HirSourceSpan::new(literal_start, literal_start + "\"\"".len())
    );
    assert!(suffix_default.stmts.is_empty());
    assert_eq!(
        suffix_default.result,
        bir::Operand::Constant(bir::Constant::Str(String::new()))
    );
    let bir::CallableParamDefault::Source(fallback_default) = &fallback_label.default else {
        return Err("a checked method call default must become a deferred computation".into());
    };
    let call_start = source
        .rfind("fallback()")
        .ok_or("missing method call default spelling")?;
    assert_eq!(
        fallback_default.span,
        HirSourceSpan::new(call_start, call_start + "fallback()".len())
    );
    assert_eq!(fallback_default.stmts.len(), 1);
    assert!(
        !label.block.stmts.iter().any(|statement| matches!(
            &statement.kind,
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                ..
            } if target.name == "fallback"
        )),
        "the method default call must not be appended to the ordinary method body: {label:?}"
    );
    assert_eq!(label.param_locals.len(), 5);

    Ok(())
}

#[test]
fn trait_method_default_uses_a_deferred_source_computation() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def fallback() -> str:\n  return \"hello\"\n\ntrait Greeter:\n  def greet(self, greeting: str = fallback()) -> str:\n    return greeting\n";
    let module = build(source, &["m", "trait_method_default"])?;
    let greet = module
        .bodies
        .iter()
        .find(|body| body.name == "greet")
        .ok_or("expected the greet trait-method Body IR")?;
    let self_param = greet.params.first().ok_or("expected the self parameter")?;
    let greeting = greet.params.get(1).ok_or("expected the greeting parameter")?;

    assert!(matches!(&self_param.default, bir::CallableParamDefault::Required));
    let bir::CallableParamDefault::Source(default) = &greeting.default else {
        return Err("a checked trait-method default must become a deferred computation".into());
    };
    let default_start = source
        .rfind("fallback()")
        .ok_or("missing trait-method default source spelling")?;
    assert_eq!(
        default.span,
        HirSourceSpan::new(default_start, default_start + "fallback()".len())
    );
    let [call] = default.stmts.as_slice() else {
        return Err("the deferred trait-method default should contain one call statement".into());
    };
    assert!(matches!(
        &call.kind,
        bir::StatementKind::Call {
            callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
            ..
        } if target.name == "fallback"
    ));
    assert!(
        !greet.block.stmts.iter().any(|statement| matches!(
            &statement.kind,
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                ..
            } if target.name == "fallback"
        )),
        "the trait-method default call must not be appended to the ordinary method body: {greet:?}"
    );

    Ok(())
}

#[test]
fn byte_string_default_is_a_deferred_body_ir_constant_at_its_own_span() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def keep(payload: bytes = b\"x\") -> bytes:\n  return payload\n";
    let module = build(source, &["m", "unsupported_default"])?;
    let keep = module.bodies.first().ok_or("expected the keep function Body IR")?;
    let payload = keep.params.first().ok_or("expected the payload parameter")?;

    let bir::CallableParamDefault::Source(default) = &payload.default else {
        return Err("bytes defaults must retain their representable Body-IR constant".into());
    };
    let default_start = source.find("b\"x\"").ok_or("missing bytes default spelling")?;
    assert_eq!(
        default.span,
        HirSourceSpan::new(default_start, default_start + "b\"x\"".len()),
        "the deferred computation must retain the default expression's exact source span"
    );
    assert!(default.stmts.is_empty());
    assert_eq!(
        default.result,
        bir::Operand::Constant(bir::Constant::Bytes(b"x".to_vec()))
    );
    assert_eq!(
        keep.locals.len(),
        1,
        "a literal default must not allocate a speculative temporary or external local: {keep:?}"
    );
    assert!(
        !keep
            .block
            .stmts
            .iter()
            .any(|statement| matches!(&statement.kind, bir::StatementKind::Unsupported { .. })),
        "the deferred default belongs to parameter metadata, not the normal function body: {keep:?}"
    );

    Ok(())
}

#[test]
fn unsupported_race_arm_in_a_default_is_found_at_its_nested_source_span() {
    // A race remains a structured Body-IR node even when one arm has an unsupported construct. Callable
    // defaults have the stricter contract: the whole deferred computation must be executable, so the default
    // boundary must find that nested refusal and retain the nested construct's span for direct consumers.
    let unsupported_span = HirSourceSpan::new(24, 34);
    let statements = vec![bir::Statement {
        kind: bir::StatementKind::Race {
            destination: None,
            arms: vec![bir::RaceArm {
                awaitable: bir::Operand::Constant(bir::Constant::Int(1)),
                binding: bir::LocalId(0),
                body: bir::Block {
                    scope: bir::ScopeId(1),
                    stmts: vec![bir::Statement {
                        kind: bir::StatementKind::Unsupported {
                            description: "power operator".to_string(),
                        },
                        span: unsupported_span,
                    }],
                },
                result: bir::Operand::Constant(bir::Constant::Int(0)),
            }],
        },
        span: HirSourceSpan::new(10, 40),
    }];

    assert_eq!(
        first_unsupported_default_statement(&statements),
        Some((unsupported_span, "power operator".to_string())),
        "a direct consumer must refuse the nested construct rather than accept a partially unsupported default"
    );
}

#[test]
fn unsupported_rvalue_bodies_in_a_default_are_found_at_their_nested_source_spans() {
    // A source default can construct a closure or generator, or evaluate a match, whose structured Body IR owns
    // more statements than the outer assignment exposes. Those statement sequences are still part of the direct
    // default contract: a consumer must receive their original refusal span instead of a misleading `Source`.
    let unsupported = |span: HirSourceSpan, description: &str| bir::Statement {
        kind: bir::StatementKind::Unsupported {
            description: description.to_string(),
        },
        span,
    };
    let assignment = |rvalue| bir::Statement {
        kind: bir::StatementKind::Assign {
            place: bir::Place::from_local(bir::LocalId(0)),
            rvalue,
        },
        span: HirSourceSpan::new(0, 80),
    };
    let result = bir::Operand::Constant(bir::Constant::Int(0));
    let closure_span = HirSourceSpan::new(10, 20);
    let generator_span = HirSourceSpan::new(21, 31);
    let guard_span = HirSourceSpan::new(32, 42);
    let body_span = HirSourceSpan::new(43, 53);
    let cases = vec![
        (
            vec![assignment(bir::Rvalue::Closure {
                params: Vec::new(),
                captured_operands: Vec::new(),
                body: Box::new(bir::ClosureBody {
                    capture_locals: Vec::new(),
                    stmts: vec![unsupported(closure_span, "closure body")],
                    result: result.clone(),
                }),
            })],
            closure_span,
            "closure body",
        ),
        (
            vec![assignment(bir::Rvalue::Generator {
                source: bir::Operand::Constant(bir::Constant::Int(1)),
                captured_operands: Vec::new(),
                body: Box::new(bir::GeneratorBody {
                    source_local: bir::LocalId(1),
                    capture_locals: Vec::new(),
                    stmts: vec![unsupported(generator_span, "generator body")],
                }),
            })],
            generator_span,
            "generator body",
        ),
        (
            vec![assignment(bir::Rvalue::Match {
                scrutinee: bir::Operand::Constant(bir::Constant::Int(1)),
                arms: vec![bir::MatchArm {
                    pattern: bir::Pattern::Wildcard,
                    guard_stmts: vec![unsupported(guard_span, "match guard")],
                    guard: Some(bir::Operand::Constant(bir::Constant::Bool(true))),
                    body_stmts: Vec::new(),
                    result: result.clone(),
                }],
            })],
            guard_span,
            "match guard",
        ),
        (
            vec![assignment(bir::Rvalue::Match {
                scrutinee: bir::Operand::Constant(bir::Constant::Int(1)),
                arms: vec![bir::MatchArm {
                    pattern: bir::Pattern::Wildcard,
                    guard_stmts: Vec::new(),
                    guard: None,
                    body_stmts: vec![unsupported(body_span, "match body")],
                    result,
                }],
            })],
            body_span,
            "match body",
        ),
    ];

    for (statements, span, description) in cases {
        assert_eq!(
            first_unsupported_default_statement(&statements),
            Some((span, description.to_string())),
            "a nested {description} refusal must prevent an incomplete default computation from becoming Source"
        );
    }
}

#[test]
fn invalid_default_is_rejected_before_body_ir_is_built() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def choose(value: int = \"wrong\") -> int:\n  return value\n";
    let error = build(source, &["m", "invalid_default"])
        .err()
        .ok_or("a mismatched callable default must be rejected before Body IR construction")?;
    assert!(
        error.to_string().contains("Type mismatch: expected 'int', found 'str'"),
        "the source typechecker must reject the mismatched default before a Body-IR consumer sees it: {error}"
    );

    Ok(())
}

#[test]
fn refused_default_restores_ownership_state_before_local_ids_are_reused() -> Result<(), Box<dyn std::error::Error>> {
    // Lowering the partial moves one of its synthesized forwarding locals before the unsupported binary refuses.
    // The transaction must discard that move before `second` reuses the local id in the normal body, or the
    // required root-scope drop would silently disappear.
    let source = "def route(method: str) -> str:\n  return method\n\ndef choose(value: str = (partial route(method=\"GET\")) + missing) -> str:\n  first = \"first\"\n  second = \"second\"\n  return first\n";
    let (module, _diagnostics) = build_after_expected_typecheck_errors(source, &["m", "default_ownership_rollback"])?;
    let choose = module
        .bodies
        .iter()
        .find(|body| body.name == "choose")
        .ok_or("expected the choose Body IR")?;
    let value = choose.params.first().ok_or("expected choose's value parameter")?;
    let bir::CallableParamDefault::Unsupported { description, .. } = &value.default else {
        return Err(format!("the unsupported default must remain a refusal: {:?}", value.default).into());
    };
    assert!(
        description.contains("binary operator Add"),
        "the refusal must name the unsupported default operation: {description}"
    );
    let second = choose
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("second"))
        .ok_or("expected second binding after refused default")?;
    assert!(
        choose.block.stmts.iter().any(|statement| matches!(
            &statement.kind,
            bir::StatementKind::Drop { local } if *local == second.id
        )),
        "a stale speculative move must not suppress second's required drop: {choose:?}"
    );

    Ok(())
}

#[test]
fn invalid_callable_defaults_remain_body_ir_refusals_without_implicit_captures()
-> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (
            "earlier parameter",
            "def choose(first: str, second: str = first) -> str:\n  return second\n",
            "first",
        ),
        (
            "receiver",
            "model Label:\n  text: str\n\n  def choose(self, value: str = self.text) -> str:\n    return value\n",
            "self.text",
        ),
        (
            "bare field",
            "model Label:\n  text: str\n\n  def choose(self, value: str = text) -> str:\n    return value\n",
            "text",
        ),
        (
            "bare property",
            "model Label:\n  text: str\n\n  property display -> str:\n    return self.text\n\n  def choose(self, value: str = display) -> str:\n    return value\n",
            "display",
        ),
    ];

    for (case, source, default_spelling) in cases {
        let (module, diagnostics) = build_after_expected_typecheck_errors(source, &["m", "default_capture"])?;
        let rejected_name = match default_spelling.split('.').next() {
            Some(name) => name,
            None => default_spelling,
        };
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic.contains(rejected_name)),
            "the source checker must reject the {case} default before Body IR: {diagnostics:?}"
        );
        let choose = module
            .bodies
            .iter()
            .find(|body| body.name == "choose")
            .ok_or("expected the choose Body IR")?;
        let parameter = choose.params.last().ok_or("expected the defaulted parameter")?;
        let bir::CallableParamDefault::Unsupported { span, description } = &parameter.default else {
            return Err(format!("a {case} default must not fabricate a callable-frame or instance capture").into());
        };
        let default_start = source
            .rfind(default_spelling)
            .ok_or("missing invalid default source spelling")?;
        assert_eq!(
            *span,
            HirSourceSpan::new(default_start, default_start + default_spelling.len()),
            "the {case} refusal must preserve the whole default expression span"
        );
        assert!(description.contains(rejected_name));
        assert!(
            !choose
                .locals
                .iter()
                .any(|local| matches!(local.origin, bir::LocalOrigin::External)),
            "a direct consumer must not need an implicit lexical lookup for the refused {case} default: {choose:?}"
        );
    }

    Ok(())
}

#[test]
fn validated_newtype_default_remains_a_visible_body_ir_refusal() -> Result<(), Box<dyn std::error::Error>> {
    let source = "type Attempts = newtype int:\n  def from_underlying(n: int) -> Result[Attempts, ValidationError]:\n    return Ok(Attempts(n))\n\ndef choose(value: Attempts = 3) -> Attempts:\n  return value\n";
    let module = build(source, &["m", "newtype_default"])?;
    let choose = module
        .bodies
        .iter()
        .find(|body| body.name == "choose")
        .ok_or("expected the choose Body IR")?;
    let value = choose.params.first().ok_or("expected the newtype default parameter")?;
    let bir::CallableParamDefault::Unsupported { span, description } = &value.default else {
        return Err("a default requiring validated-newtype coercion must not become a raw source computation".into());
    };
    let default_start = source.rfind("3)").ok_or("missing newtype default spelling")?;
    assert_eq!(*span, HirSourceSpan::new(default_start, default_start + 1));
    assert_eq!(
        description,
        "default requires a validated-newtype coercion Body IR does not yet represent"
    );
    assert_eq!(choose.locals.len(), 1);
    assert!(
        !choose
            .locals
            .iter()
            .any(|local| matches!(local.origin, bir::LocalOrigin::External)),
        "the newtype refusal must not leave a hidden source lookup in the callable body: {choose:?}"
    );

    Ok(())
}

#[test]
fn aliased_method_parameter_type_retains_the_checked_callable_type() -> Result<(), Box<dyn std::error::Error>> {
    // `UserId` is a type alias for `int` (RFC-style `type X = Y`). A naive re-parse of the raw `id: UserId`
    // annotation inside Body IR (with no alias table of its own) could only produce `Named("UserId")`; the
    // checked callable type resolves the alias all the way through, so the local must show `int`.
    let source = "type UserId = int\n\nmodel Account:\n  balance: int\n\n  def credit(self, id: UserId, amount: int) -> int:\n    return self.balance + amount\n";
    let module = build(source, &["m", "aliased_param"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("local 1 id : int [param]"),
        "an aliased parameter type must resolve through the alias like any other checked expression, not stay \
             the raw `UserId` annotation spelling: {snapshot}"
    );

    Ok(())
}

#[test]
fn generic_method_parameter_type_retains_the_owner_type_variable() -> Result<(), Box<dyn std::error::Error>> {
    let source = "class Box[T]:\n  value: T\n\n  def replace(mut self, other: T) -> None:\n    self.value = other\n\n  def wrap(mut self, items: list[T]) -> None:\n    pass\n";
    let module = build(source, &["m", "generic_param"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("local 1 other : T [param]"),
        "a bare owner type-variable parameter must retain the checked type variable: {snapshot}"
    );
    assert!(
        snapshot.contains("local 1 items : List[T] [param]"),
        "a generic collection parameter must retain its checked element type variable: {snapshot}"
    );

    Ok(())
}

#[test]
fn static_method_parameter_types_are_recorded_like_ordinary_methods() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "model Counter:\n  value: int\n\n  def from_value(amount: int) -> Counter:\n    return Counter(value=amount)\n";
    let module = build(source, &["m", "static_param"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("body from_value decl:m::static_param::Counter::from_value"));
    assert!(
        !snapshot.contains("[receiver"),
        "a static/associated method (receiver: None) must not declare a receiver local: {snapshot}"
    );
    assert!(
        snapshot.contains("local 0 amount : int [param]"),
        "a static method's ordinary parameters must resolve the same way an instance method's do: {snapshot}"
    );

    Ok(())
}

#[test]
fn overloaded_method_declarations_retain_distinct_parameter_types_by_declaration_span()
-> Result<(), Box<dyn std::error::Error>> {
    // Two `add` methods on the same owner, distinguished only by adopting two instantiations of the same
    // generic trait (RFC 042 multi-instantiation) -- the language surface's one legitimate way to declare
    // same-name, same-owner method overloads with genuinely different parameter types. If the checked binding
    // table were keyed by `(owner, method_name)` alone (like `decorated_method_bindings`), the second
    // declaration would silently overwrite the first and both bodies would report the same parameter type.
    let source = "trait Adder[T]:\n  def add(self, x: T) -> T: ...\n\nmodel Calc with Adder[int], Adder[str]:\n  count: int\n\n  def add(self, x: int) -> int:\n    return x\n\n  def add(self, x: str) -> str:\n    return x\n";
    let module = build(source, &["m", "overload_param"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("local 1 x : int [param]"),
        "the int-instantiated overload must keep its own checked parameter type: {snapshot}"
    );
    assert!(
        snapshot.contains("local 1 x : str [param]"),
        "the str-instantiated overload must keep its own distinct checked parameter type, not collide with the \
             int overload recorded under the same method name: {snapshot}"
    );

    Ok(())
}

#[test]
fn method_parameter_type_falls_back_to_unknown_only_when_the_typechecker_binding_is_absent()
-> Result<(), Box<dyn std::error::Error>> {
    // A successful typecheck always populates `method_bindings_by_span` for every method Body IR actually
    // lowers a body for (see `TypeChecker::check_method_with_self_ty`), so the only way to observe the
    // fallback honestly is to simulate the checked fact genuinely being absent -- exercising the same
    // defence-in-depth path `lower_method_body` falls back to, rather than asserting on a state ordinary
    // typechecking can never produce.
    let source =
        "model Counter:\n  value: int\n\n  def add(self, amount: int) -> int:\n    return self.value + amount\n";
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let module_path: Vec<String> = vec!["m".to_string(), "fallback_param".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

    let mut type_info = checker.type_info().clone();
    type_info.declarations.method_bindings_by_span.clear();

    let module = build_body_ir_module_v0(&program, &module_path, &type_info);
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("local 1 amount : ? [param]"),
        "with no recorded checked binding for this declaration, the parameter must fall back to the explicit \
             Unknown type rather than guessing from the raw annotation: {snapshot}"
    );

    Ok(())
}

/// Newtype and enum methods lower like any other owner's methods.
///
/// The body count is asserted explicitly so a future regression shows up as a mismatch rather than a silent
/// absence, which is how this gap went unnoticed: nothing failed when these bodies were simply never produced.
#[test]
fn newtype_and_enum_methods_contribute_bodies() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
type Meters = newtype int:
    def value(self) -> int:
        return self.0

    def scale(mut self, factor: int) -> None:
        self.0 = self.0 * factor

enum Signal:
    Idle
    Active(int)

    def code(self) -> int:
        return 0

def run() -> int:
    return 1
"#;
    let module = build(source, &["app"])?;

    let names: Vec<&str> = module.bodies.iter().map(|body| body.name.as_str()).collect();
    assert!(
        names.contains(&"value"),
        "newtype method `value` must lower, got {names:?}"
    );
    assert!(
        names.contains(&"scale"),
        "newtype `mut self` method must lower, got {names:?}"
    );
    assert!(names.contains(&"code"), "enum method `code` must lower, got {names:?}");
    assert!(names.contains(&"run"));
    assert_eq!(
        module.bodies.len(),
        4,
        "one body per method plus the free function, and nothing else: {names:?}"
    );

    // Each method's identity is scoped under its owning declaration, so two owners may share a method name.
    for (owner, method) in [("Meters", "value"), ("Meters", "scale"), ("Signal", "code")] {
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == method)
            .ok_or_else(|| Box::<dyn std::error::Error>::from(format!("`{method}` body missing")))?;
        assert!(
            body.decl_id.path().ends_with(&format!("{owner}::{method}")),
            "`{method}` must be scoped under `{owner}`, got {}",
            body.decl_id.path()
        );
    }
    Ok(())
}

/// An enum method that dispatches on `self`, including a payload variant, lowers a real body rather than an empty or
/// placeholder one.
#[test]
fn an_enum_method_dispatching_on_self_lowers_its_match() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
enum Signal:
    Idle
    Active(int)

    def level(self) -> int:
        match self:
            case Signal.Idle:
                return 0
            case Signal.Active(amount):
                return amount
"#;
    let module = build(source, &["app"])?;

    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "level")
        .ok_or_else(|| Box::<dyn std::error::Error>::from("enum method `level` must lower"))?;

    assert!(!body.block.stmts.is_empty(), "the method must lower a real body");
    // A refused construct also produces statements, so an empty check proves nothing on its own. The snapshot
    // carrying no `unsupported(` node is what distinguishes a lowered match from a placeholder.
    let rendered = module.render_snapshot();
    assert!(
        !rendered.contains("unsupported("),
        "the match and its payload binding must lower, not refuse:\n{rendered}"
    );
    Ok(())
}

/// A newtype method reading its wrapped value lowers through the nominal receiver.
#[test]
fn a_newtype_method_reads_its_wrapped_value() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
type Meters = newtype int:
    def doubled(self) -> int:
        return self.0 * 2
"#;
    let module = build(source, &["app"])?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "doubled")
        .ok_or_else(|| Box::<dyn std::error::Error>::from("newtype method `doubled` must lower"))?;
    assert!(!body.block.stmts.is_empty());
    assert!(
        !module.render_snapshot().contains("unsupported("),
        "reading the wrapped value must lower, not refuse"
    );
    assert!(
        body.decl_id.path().ends_with("Meters::doubled"),
        "identity is scoped under the newtype, got {}",
        body.decl_id.path()
    );
    Ok(())
}

/// A newtype method's receiver is the newtype itself, and `mut self` stays a mutable receiver.
#[test]
fn newtype_method_receivers_are_receiver_locals() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
type Meters = newtype int:
    def value(self) -> int:
        return self.0

    def scale(mut self, factor: int) -> None:
        self.0 = self.0 * factor
"#;
    let module = build(source, &["m", "newtype_receiver"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("local 0 self : Meters [receiver]"), "{snapshot}");
    assert!(snapshot.contains("local 0 self : Meters [receiver_mut]"), "{snapshot}");
    assert!(
        !snapshot.contains("unsupported("),
        "newtype receiver reads and mutation must lower without a placeholder: {snapshot}"
    );
    Ok(())
}

/// An enum method's receiver is the enum value it dispatches on.
#[test]
fn enum_method_receiver_is_a_receiver_local() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
enum Signal:
    Idle
    Active(int)

    def code(self) -> int:
        return 0
"#;
    let module = build(source, &["m", "enum_receiver"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("local 0 self : Signal [receiver]"), "{snapshot}");
    Ok(())
}

/// A bodyless newtype or enum method never reaches lowering: the source checker rejects it first.
///
/// Only a trait declaration admits a bodyless method, so the "abstract method contributes nothing" boundary sits
/// upstream for these two kinds rather than in the lowering walk. Lowering still has to stay total for the rejected
/// program, which is what this pins — no body, and no `Unsupported` placeholder standing in for one.
#[test]
fn a_bodyless_newtype_or_enum_method_is_a_source_error_and_lowers_nothing() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
enum Signal:
    Idle

    def label(self) -> str: ...
"#;
    let (module, errors) = build_after_expected_typecheck_errors(source, &["m", "bodyless"])?;

    assert!(
        errors
            .iter()
            .any(|error| error.contains("must have a body outside trait declarations")),
        "the source checker owns this refusal: {errors:?}"
    );
    assert!(
        module.bodies.is_empty(),
        "a rejected bodyless method must produce neither a body nor a placeholder: {:?}",
        module.bodies.iter().map(|body| body.name.as_str()).collect::<Vec<_>>()
    );
    Ok(())
}

/// Every declaration kind that carries a `methods` field lowers its bodies.
///
/// A skipped kind is the one coverage failure this module cannot make visible — it produces no `Body` at all rather
/// than an `Unsupported` marker — so the exhaustive set is pinned here rather than left to the walk's `_ => ` arm.
#[test]
fn every_declaration_kind_that_carries_methods_lowers_its_bodies() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
trait Describable:
    def describe(self) -> int:
        return 0

model Point:
    x: int

    def get_x(self) -> int:
        return self.x

class Counter:
    total: int

    def total_now(self) -> int:
        return self.total

type Meters = newtype int:
    def raw(self) -> int:
        return self.0

enum Signal:
    Idle

    def code(self) -> int:
        return 1
"#;
    let module = build(source, &["m", "all_owners"])?;
    let names: Vec<&str> = module.bodies.iter().map(|body| body.name.as_str()).collect();

    for method in ["describe", "get_x", "total_now", "raw", "code"] {
        assert!(names.contains(&method), "`{method}` must lower, got {names:?}");
    }
    assert_eq!(
        module.bodies.len(),
        5,
        "one body per methods-carrying declaration kind and nothing else: {names:?}"
    );
    Ok(())
}

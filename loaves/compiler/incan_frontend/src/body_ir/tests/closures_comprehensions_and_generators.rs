//! Expression forms that carry their own scope: list and dict comprehensions, generator expressions and their captures,
//! f-strings and last-use tracking of embedded expressions, closures and their capture facts, partial callables lowered
//! to forwarding closures, and generator bodies (`yield`).

use super::*;

#[test]
fn lowers_a_list_comprehension_into_a_push_loop() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def doubled(items: list[int]) -> list[int]:\n  return [x * 2 for x in items]\n";
    let module = build(source, &["m", "list_comp"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("list[]"),
        "should start from an empty list aggregate: {snapshot}"
    );
    assert!(
        snapshot.contains("call method:push unbound(mut_borrow("),
        "should grow the list via a synthesized push call: {snapshot}"
    );
    assert!(
        snapshot.contains("iter_next("),
        "should desugar into the shared iteration primitive: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_a_filtered_list_comprehension_with_a_guarding_if() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def evens(items: list[int]) -> list[int]:\n  return [x for x in items if x % 2 == 0]\n";
    let module = build(source, &["m", "list_comp_filter"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("call method:push unbound("),
        "filtered comprehension should still push accepted elements: {snapshot}"
    );
    assert!(
        snapshot.contains("if "),
        "the filter clause should lower to a guarding If: {snapshot}"
    );
    Ok(())
}

#[test]
fn comprehension_bindings_do_not_escape_the_expression_scope() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def keep_outer(x: int, items: list[int]) -> int:\n  doubled = [x * 2 for x in items]\n  return x\n";
    let module = build(source, &["m", "comprehension_scope"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("return copy(_0)"),
        "the trailing read must resolve the enclosing parameter, not the comprehension binding: {snapshot}"
    );
    let body = body_named(&module, "keep_outer")?;
    let identities: Vec<_> = body
        .locals
        .iter()
        .filter(|local| local.name.as_deref() == Some("x"))
        .map(|local| local.identity.as_ref())
        .collect();
    let [Some(parameter), Some(comprehension_binding)] = identities.as_slice() else {
        return Err(
            format!("expected canonical identities for the parameter and comprehension binding: {body:?}").into(),
        );
    };
    assert_ne!(
        parameter, comprehension_binding,
        "scoped bindings need distinct identities"
    );
    Ok(())
}

#[test]
fn lowers_a_dict_comprehension_into_an_insert_loop() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def doubled(items: list[int]) -> dict[int, int]:\n  return {x: x * 2 for x in items}\n";
    let module = build(source, &["m", "dict_comp"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("dict[]"),
        "should start from an empty dict aggregate: {snapshot}"
    );
    assert!(
        snapshot.contains("call method:insert unbound(mut_borrow("),
        "should grow the dict via a synthesized insert call: {snapshot}"
    );
    Ok(())
}

#[test]
fn generator_expression_keeps_its_multi_clause_body_lazy_and_captures_its_environment()
-> Result<(), Box<dyn std::error::Error>> {
    // Mirrors the multi-clause fixture from `test_rfc006_generator_expression_infers_element_type` in
    // `loaves/compiler/incan_frontend/src/typechecker/tests/async_and_iteration.rs`, but also reads `offset` from both
    // the filter and element. The Body IR value must capture that enclosing local once at construction; it must not
    // materialize the chain or run either filter/element in the enclosing body.
    let source = "def positives(offset: int, xs: list[int], ys: list[int]) -> Generator[int]:\n  return (x * offset for x in xs if x > offset for y in ys if y > x)\n";
    let module = build(source, &["m", "generator_expr"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("generator(source="),
        "generator construction must be represented as a distinct lazy rvalue: {snapshot}"
    );
    assert!(
        snapshot.contains("captures=["),
        "the deferred body must receive explicit construction-time captures: {snapshot}"
    );
    assert!(
        !snapshot.contains("list[]"),
        "a generator expression must not materialize an eager list while claiming Generator[T]: {snapshot}"
    );
    assert!(
        snapshot.contains("yield "),
        "the element must be suspended in the generator body: {snapshot}"
    );
    assert!(
        snapshot.contains("iter_next("),
        "for clauses must remain deferred iteration operations: {snapshot}"
    );
    assert!(
        snapshot.contains("if "),
        "filters must remain deferred guard operations: {snapshot}"
    );
    assert!(
        !snapshot.contains("unsupported("),
        "a valid generator expression must not leave an unsupported placeholder: {snapshot}"
    );
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "positives")
        .ok_or("generator fixture must lower its function body")?;
    assert!(
        body.block.stmts.iter().all(|statement| !matches!(
            statement.kind,
            bir::StatementKind::IterNext { .. } | bir::StatementKind::Yield { .. }
        )),
        "polling and yield must stay inside the generator rvalue, not the enclosing body: {snapshot}"
    );
    let (source, captured_operands, generator_body) = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue:
                    bir::Rvalue::Generator {
                        source,
                        captured_operands,
                        body,
                    },
                ..
            } => Some((source, captured_operands, body)),
            _ => None,
        })
        .ok_or("generator fixture must assign a Generator rvalue")?;
    assert!(
        matches!(source, bir::Operand::Place(_)),
        "the first for source must be captured as a construction-time operand: {source:?}"
    );
    assert_eq!(
        captured_operands.len(),
        2,
        "offset and ys are the deferred free captures"
    );
    let capture_names: Vec<_> = generator_body
        .capture_locals
        .iter()
        .map(|local| body.locals[local.index()].name.as_deref())
        .collect();
    assert_eq!(capture_names, vec![Some("offset"), Some("ys")]);
    assert!(
        matches!(
            body.locals[generator_body.source_local.index()].origin,
            bir::LocalOrigin::Captured
        ),
        "the construction-time source needs a generator-owned local"
    );
    assert!(
        generator_body
            .capture_locals
            .iter()
            .all(|local| matches!(body.locals[local.index()].origin, bir::LocalOrigin::Captured)),
        "each deferred free value must bind through an explicit captured local"
    );
    Ok(())
}

#[test]
fn generator_expression_evaluates_only_its_outer_source_before_construction() -> Result<(), Box<dyn std::error::Error>>
{
    let source = concat!(
        "def source() -> list[int]:\n",
        "  return [1, 2]\n\n",
        "def lazy() -> Generator[int]:\n",
        "  return (item for item in source())\n"
    );
    let module = build(source, &["m", "generator_source_timing"])?;
    let snapshot = module.render_snapshot();
    let source_call = snapshot
        .find("call fn:source()")
        .ok_or("outer generator source call must lower at construction")?;
    let generator = snapshot
        .find("generator(source=")
        .ok_or("generator construction must have a distinct rvalue")?;
    assert!(
        source_call < generator,
        "the first for source must be evaluated before generator construction: {snapshot}"
    );
    assert!(
        !snapshot.contains("unsupported("),
        "a supported outer source must not leave an unsupported marker: {snapshot}"
    );
    Ok(())
}

#[test]
fn generator_expression_captures_an_outer_value_without_leaking_its_clause_binding()
-> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "def preserve(prefix: str, values: list[str]) -> str:\n",
        "  generated = (prefix + value for value in values)\n",
        "  return prefix\n"
    );
    let module = build(source, &["m", "generator_capture_scope"])?;
    let snapshot = module.render_snapshot();
    assert!(
        snapshot.contains("captures=[clone(_0)"),
        "the generator must own a construction-time clone while the enclosing binding remains live: {snapshot}"
    );
    assert!(
        snapshot.contains("return move(_0, last_use)"),
        "the trailing source read must resolve the outer prefix, not a generator-local capture: {snapshot}"
    );
    assert!(
        !snapshot.contains("unsupported("),
        "captured generator values must lower without an unsupported placeholder: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_an_fstring_into_a_format_rvalue_with_literal_and_display_parts() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def greet(name: str) -> str:\n  return f\"hello {name}\"\n";
    let module = build(source, &["m", "fstring_display"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("fstring(lit(\"hello \"), move(_0, last_use):display"),
        "f-string should lower to an explicit Format rvalue with literal and display parts: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_an_fstring_debug_interpolation_using_the_debug_style() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def show(n: int) -> str:\n  return f\"n={n:?}\"\n";
    let module = build(source, &["m", "fstring_debug"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains(":debug"),
        "`{{n:?}}` should lower to a Debug-styled format part: {snapshot}"
    );
    assert!(
        !snapshot.contains(":display"),
        "a debug interpolation should not also render as display: {snapshot}"
    );
    Ok(())
}

#[test]
fn fstring_records_the_fstring_runtime_helper_and_allocator_requirements() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def label(x: int) -> str:\n  return f\"x={x}\"\n";
    let module = build(source, &["m", "fstring_reqs"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("runtime_requirements:"));
    assert!(snapshot.contains("runtime_helper(fstring)"));
    assert!(snapshot.contains("allocator"));
    Ok(())
}

#[test]
fn fstring_embedded_expression_participates_in_last_use_tracking() -> Result<(), Box<dyn std::error::Error>> {
    // `s` is read twice: once as a plain binding RHS and once inside the f-string. The f-string's embedded read
    // must still count toward `s`'s last-use countdown (see `count_reads_in_expr`'s `ast::Expr::FString` arm),
    // so the first (non-last) read clones and only the f-string's read -- the true last use -- moves.
    let source = "def dup(s: str) -> str:\n  first = s\n  return f\"value={s}\"\n";
    let module = build(source, &["m", "fstring_last_use"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("clone(_0)"),
        "the first, non-last read of `s` should clone: {snapshot}"
    );
    assert!(
        snapshot.contains("fstring(lit(\"value=\"), move(_0, last_use):display"),
        "the f-string's embedded read is the true last use and should move: {snapshot}"
    );
    Ok(())
}

#[test]
fn comprehension_embedded_expression_participates_in_last_use_tracking() -> Result<(), Box<dyn std::error::Error>> {
    // Mirrors `fstring_embedded_expression_participates_in_last_use_tracking`'s regression shape for the same
    // class of bug: `count_reads_in_expr` must recurse into `ast::Expr::ListComp`'s element expression, or the
    // earlier, non-comprehension read of `s` on the first line would be miscounted as the last use (`Move`)
    // even though the list comprehension on the next line reads `s` again -- an unsound move, not merely an
    // imprecise clone. `s` is read twice: once as a plain binding RHS, once inside the comprehension's element.
    let source = "def dup(s: str, items: list[int]) -> list[str]:\n  first = s\n  return [s for n in items]\n";
    let module = build(source, &["m", "comp_last_use"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("clone(_0)"),
        "the first, non-last read of `s` should clone because the comprehension reads it again: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_a_closure_capturing_nothing_with_an_empty_capture_list() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def make(step: int) -> int:\n  add: (int) -> int = (x) => x + 1\n  return add(step)\n";
    let module = build(source, &["m", "closure_no_capture"])?;
    let snapshot = module.render_snapshot();
    let make = module.bodies.first().ok_or("expected the make function Body IR")?;
    let closure_params = make
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Closure { params, .. },
                ..
            } => Some(params),
            _ => None,
        })
        .ok_or("expected the closure literal")?;
    let x = closure_params.first().ok_or("expected the closure parameter")?;

    assert!(
        !snapshot.contains("unsupported("),
        "a closure literal should lower fully, not fall back: {snapshot}"
    );
    assert!(
        snapshot.contains("captures=[]"),
        "a closure that reads no outer variable should capture nothing: {snapshot}"
    );
    assert!(
        snapshot.contains("closure(params=[x: int local=_"),
        "the closure's own parameter should be recorded: {snapshot}"
    );
    assert_eq!(x.name, "x");
    assert_eq!(x.local, bir::LocalId(1));
    assert_eq!(
        x.span,
        HirSourceSpan::new(
            source.find("(x)").ok_or("missing closure parameter spelling")? + 1,
            source.find("(x)").ok_or("missing closure parameter spelling")? + 2,
        )
    );
    assert!(matches!(&x.default, bir::CallableParamDefault::Required));
    Ok(())
}

#[test]
fn source_closure_default_syntax_is_refused_before_body_ir_exists() -> Result<(), Box<dyn std::error::Error>> {
    // Closure parameter parsing deliberately accepts identifiers only. Keeping this source-level failure explicit
    // means #1172 does not invent an executable local-closure default from parser-unrepresentable syntax.
    let source = "def make() -> int:\n  value: (int) -> int = (x = 1) => x\n  return value(2)\n";
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let errors = match parser::parse(&tokens) {
        Ok(_) => return Err("closure-default source syntax must not parse into a Body-IR input".into()),
        Err(errors) => errors,
    };
    let parameter_start = source
        .find("x = 1")
        .ok_or("missing closure default parameter spelling")?;
    let parameter_end = parameter_start + "x = 1".len();

    assert!(
        errors
            .iter()
            .any(|error| { error.span.start >= parameter_start && error.span.end <= parameter_end }),
        "the parser must refuse the closure-default spelling at its own source parameter range: {errors:?}"
    );

    Ok(())
}

#[test]
fn lowers_a_closure_capturing_an_outer_variable_with_a_real_clone_fact() -> Result<(), Box<dyn std::error::Error>> {
    // `name` is read once inside the closure (a capture) and again afterward by `return name`, so the capture
    // is not the last use: it must clone, not move -- a real Duckborrower fact, not a placeholder.
    let source = "def greet(name: str) -> str:\n  make_msg: () -> str = () => name\n  return name\n";
    let module = build(source, &["m", "closure_capture_clone"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("captures=[clone(_0)]"),
        "capturing `name` before its last use should clone: {snapshot}"
    );
    assert!(snapshot.contains("local 1 name : str [captured]"));
    Ok(())
}

#[test]
fn lowers_a_closure_capturing_an_outer_variable_at_its_last_use() -> Result<(), Box<dyn std::error::Error>> {
    // `name` is read once, inside the closure, and never again -- the capture itself is `name`'s last use, so
    // it should move rather than clone.
    let source = "def greet(name: str) -> str:\n  make_msg: () -> str = () => name\n  return make_msg()\n";
    let module = build(source, &["m", "closure_capture_move"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("captures=[move(_0, last_use)]"),
        "capturing `name` at its only/last use should move: {snapshot}"
    );
    Ok(())
}

#[test]
fn invokes_a_stored_closure_through_its_local_operand_and_preserves_its_capture_ownership()
-> Result<(), Box<dyn std::error::Error>> {
    // The local `decorate` is a value with a lexical environment, not a declaration named `decorate`.
    // Its call target must therefore retain the closure-local read (including its ownership fact) rather than
    // being approximated as a direct function call and losing the relationship to the captured `prefix`.
    let source = "def greet(prefix: str) -> str:\n  decorate: (str) -> str = (suffix) => prefix + suffix\n  return decorate(\"!\")\n";
    let module = build(source, &["m", "stored_closure_call"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("captures=[move(_0, last_use)]"),
        "the closure must own its last-use capture explicitly: {snapshot}"
    );
    assert!(
        snapshot.contains("call local:move(_"),
        "the stored closure must be invoked through its local operand: {snapshot}"
    );
    assert!(
        !snapshot.contains("call fn:decorate("),
        "a stored closure must never be misrepresented as a named function: {snapshot}"
    );
    Ok(())
}

#[test]
fn closure_body_can_still_read_its_capture_after_lowering_restores_outer_bindings()
-> Result<(), Box<dyn std::error::Error>> {
    // The closure's own capture-binding local must resolve inside the closure body (via `result:`), and the
    // enclosing function's own read of `step` afterward must resolve back to the *outer* local, not the
    // closure's capture -- i.e. `Self::lower_closure`'s save/restore of `self.bindings` must round-trip.
    let source = "def make(step: int) -> int:\n  add: () -> int = () => step\n  return step\n";
    let module = build(source, &["m", "closure_capture_restore"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("result: copy(_1)"),
        "the closure body should read its own capture-binding local for `step` (an `int`, so `copy`): {snapshot}"
    );
    assert!(
        snapshot.contains("return copy(_0)"),
        "the function's own trailing `return step` must resolve back to the *outer* local `_0`, not the \
             closure's capture-binding local `_1`, proving the save/restore round-trips: {snapshot}"
    );
    assert!(
        !snapshot.contains("unsupported("),
        "nothing here should fall back: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_a_partial_callable_into_a_forwarding_closure() -> Result<(), Box<dyn std::error::Error>> {
    // A local partial retains every target parameter in its callable surface. The captured `method` is a
    // defaulted, overrideable slot, while `path` remains required and `content_type` keeps the target default.
    // `method` is read again after construction, so the non-Copy preset capture must be a real clone fact.
    let source = "def route(method: str, path: str, content_type: str = \"text\") -> str:\n  return method + path + content_type\n\ndef make(method: str) -> str:\n  get = partial route(method=method)\n  named = get(method=\"POST\", path=\"/named\")\n  return method + get(\"/health\")\n";
    let module = build(source, &["m", "partial_callable"])?;
    let snapshot = module.render_snapshot();
    let make = module
        .bodies
        .iter()
        .find(|body| body.name == "make")
        .ok_or("expected the make function Body IR")?;
    let (partial_params, captured_operands, closure_body) = make
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue:
                    bir::Rvalue::Closure {
                        params,
                        captured_operands,
                        body,
                    },
                ..
            } => Some((params, captured_operands, body)),
            _ => None,
        })
        .ok_or("expected the synthesized partial closure")?;
    let method = partial_params
        .iter()
        .find(|param| param.name == "method")
        .ok_or("expected the captured method parameter")?;
    let content_type = partial_params
        .iter()
        .find(|param| param.name == "content_type")
        .ok_or("expected the target default parameter")?;

    assert!(matches!(
        &method.default,
        bir::CallableParamDefault::PartialPreset { .. }
    ));
    let bir::CallableParamDefault::PartialPreset { capture } = &method.default else {
        return Err("the partial preset must retain its capture local".into());
    };
    assert_eq!(closure_body.capture_locals, vec![*capture]);
    assert!(matches!(
        captured_operands.first(),
        Some(bir::Operand::Place(bir::PlaceOperand {
            fact: bir::OwnershipFact::Clone,
            ..
        }))
    ));
    let bir::CallableParamDefault::Source(content_type_default) = &content_type.default else {
        return Err("the unpresetted checked default must remain a deferred source computation".into());
    };
    let content_type_start = source.find("\"text\"").ok_or("missing content_type default spelling")?;
    assert_eq!(
        content_type_default.span,
        HirSourceSpan::new(content_type_start, content_type_start + "\"text\"".len())
    );
    assert!(content_type_default.stmts.is_empty());
    assert_eq!(
        content_type_default.result,
        bir::Operand::Constant(bir::Constant::Str("text".to_string()))
    );
    let supplied_slots: Vec<Vec<usize>> = make
        .block
        .stmts
        .iter()
        .filter_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Local(target)),
                ..
            } => match &target.binding {
                bir::ArgumentBinding::Resolved { arguments, .. } => {
                    Some(arguments.iter().map(|argument| argument.slot).collect())
                }
                bir::ArgumentBinding::UnresolvedPositional => None,
            },
            _ => None,
        })
        .collect();
    assert!(
        supplied_slots.iter().any(|slots| slots.as_slice() == [0, 1]),
        "a named argument must override the captured preset in its declaration slot: {make:?}"
    );
    assert!(
        supplied_slots.iter().any(|slots| slots.as_slice() == [1]),
        "a positional residual argument must omit the preset and trailing source default by declaration slot: {make:?}"
    );
    assert!(
        snapshot.contains("call local:move(_"),
        "a stored partial must be invoked through its local operand: {snapshot}"
    );
    assert!(
        !snapshot.contains("call fn:get("),
        "a stored partial must never be misrepresented as a named function: {snapshot}"
    );
    assert!(
        snapshot.contains("call fn:route("),
        "the synthesized closure body should forward into a call to the target function: {snapshot}"
    );
    Ok(())
}

#[test]
fn stored_partial_refuses_too_few_or_too_many_residual_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let too_few = "def add3(a: int, b: int, c: int) -> int:\n  return a + b + c\n\ndef make() -> int:\n  add_with_one = partial add3(a=1)\n  return add_with_one(9)\n";
    let (too_few_module, diagnostics) = build_after_expected_typecheck_errors(too_few, &["m", "partial_too_few"])?;
    let too_few_snapshot = too_few_module.render_snapshot();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("Missing required argument 'c'")),
        "the source checker must diagnose the missing residual parameter: {diagnostics:?}"
    );
    assert!(
            too_few_snapshot
            .contains("unsupported(local callable `add_with_one` expects at least 2 required arguments, got 1; missing required parameter `c`)"),
            "a partial invocation may not omit a required residual argument: {too_few_snapshot}"
        );

    let too_many = "def add3(a: int, b: int, c: int) -> int:\n  return a + b + c\n\ndef make() -> int:\n  add_with_one = partial add3(a=1)\n  return add_with_one(9, 2, 3)\n";
    let (too_many_module, diagnostics) = build_after_expected_typecheck_errors(too_many, &["m", "partial_too_many"])?;
    let too_many_snapshot = too_many_module.render_snapshot();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("expects 2 argument(s), got 3")),
        "the source checker must use the residual arity: {diagnostics:?}"
    );
    assert!(
        too_many_snapshot
            .contains("unsupported(local callable `add_with_one` expects at most 2 positional arguments, got 3)"),
        "a partial invocation may not provide more residual positional arguments than its target accepts: {too_many_snapshot}"
    );
    assert!(
        !too_many_snapshot.contains("call fn:add_with_one("),
        "invalid residual arity must not be approximated as a named-function call: {too_many_snapshot}"
    );
    Ok(())
}

#[test]
fn stored_partial_passes_positional_residual_arguments_in_target_declaration_order()
-> Result<(), Box<dyn std::error::Error>> {
    // Positional calls skip the defaulted preset `a`, while Body IR records their target slots explicitly.
    let source = "def add3(a: int, b: int, c: int) -> int:\n  return a + b + c\n\ndef make() -> int:\n  add_with_one = partial add3(a=1)\n  return add_with_one(9, 2)\n";
    let module = build(source, &["m", "partial_order"])?;
    let snapshot = module.render_snapshot();
    let local_call = snapshot
        .lines()
        .find(|line| line.contains("call local:"))
        .ok_or("stored partial call missing from Body IR snapshot")?;
    assert!(
        local_call.contains("const(9), const(2)"),
        "residual positional arguments must remain b/c ordered while the preset default stays captured: {local_call}"
    );
    assert!(
        local_call.contains("slots=[1, 2]"),
        "positional residual arguments must map to their target declaration slots: {local_call}"
    );
    assert!(
        local_call.contains("defaults=[0]"),
        "the skipped preset slot must be recorded as a defaulted slot rather than left implicit: {local_call}"
    );
    assert!(
        !snapshot.contains("unsupported("),
        "the residual Body IR call itself should be executable once admitted by the typechecker: {snapshot}"
    );
    Ok(())
}

#[test]
fn stored_partial_allows_a_named_preset_override() -> Result<(), Box<dyn std::error::Error>> {
    // The construction-time capture remains the default, but a named argument replaces it for this invocation.
    let source = "def add3(a: int, b: int, c: int) -> int:\n  return a + b + c\n\ndef make() -> int:\n  add_with_one = partial add3(a=1)\n  return add_with_one(a=7, b=9, c=2)\n";
    let module = build(source, &["m", "partial_named_override"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a named preset override must lower as an ordinary local callable invocation: {snapshot}"
    );
    assert!(
        snapshot.contains("const(7), const(9), const(2)"),
        "the local invocation must retain the explicit target slots for the override and residual values: {snapshot}"
    );
    // An absent slot/defaults suffix is the identity binding: every declared slot filled, in declaration order.
    // That is precisely what distinguishes a named override from the positional call above, which skips the
    // preset and therefore renders `slots=[1, 2] defaults=[0]`.
    let local_call = snapshot
        .lines()
        .find(|line| line.contains("call local:"))
        .ok_or("stored partial call missing from Body IR snapshot")?;
    assert!(
        !local_call.contains("slots=") && !local_call.contains("defaults="),
        "a named override must occupy the captured preset's declaration slot rather than skipping it: {local_call}"
    );
    Ok(())
}

#[test]
fn partial_callable_restores_enclosing_bindings_after_lowering() -> Result<(), Box<dyn std::error::Error>> {
    // `partial join(prefix="hi ")` synthesizes a residual closure parameter called `suffix`, but that internal
    // binding must not replace the enclosing function parameter of the same name. The trailing return must read
    // the original function parameter (`_0`), not the closure-only parameter allocated while lowering the
    // partial expression.
    let source = "def join(prefix: str, suffix: str) -> str:\n  return prefix + suffix\n\ndef keep_outer(suffix: str) -> str:\n  formatter = partial join(prefix=\"hi \")\n  return suffix\n";
    let module = build(source, &["m", "partial_binding_restore"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("return move(_0, last_use)"),
        "the trailing return must resolve the enclosing `suffix` parameter, not a synthesized partial local: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_a_single_yield_and_marks_the_body_a_generator() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def numbers() -> Generator[int]:\n  yield 1\n";
    let module = build(source, &["m", "single_yield"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("yield const(1)"),
        "yield should lower to an explicit Yield statement: {snapshot}"
    );
    assert!(
        !snapshot.contains("unsupported("),
        "statement-position yield with a value must not fall back to Unsupported: {snapshot}"
    );
    let body = module
        .bodies
        .iter()
        .find(|b| b.name == "numbers")
        .ok_or("numbers body missing from module")?;
    assert!(
        body.is_generator(),
        "a body containing a yield must report is_generator()"
    );
    Ok(())
}

#[test]
fn lowers_multiple_yields_across_control_flow_inside_a_loop() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "def counter(n: int) -> Generator[int]:\n  mut i = 0\n  while i < n:\n    yield i\n    i = i + 1\n  yield -1\n";
    let module = build(source, &["m", "loop_yield"])?;
    let snapshot = module.render_snapshot();

    // Two yields: one nested inside the normalized `loop:` the `while` desugars into, one at the top level
    // after the loop.
    assert_eq!(
        snapshot.matches("yield ").count(),
        2,
        "expected exactly two yield statements: {snapshot}"
    );
    assert!(
        snapshot.contains("loop:"),
        "while should still desugar to a normalized loop: {snapshot}"
    );
    let body = module
        .bodies
        .iter()
        .find(|b| b.name == "counter")
        .ok_or("counter body missing from module")?;
    assert!(
        body.is_generator(),
        "a yield nested inside a loop must still be found by is_generator()"
    );
    Ok(())
}

#[test]
fn a_non_generator_function_is_not_reported_as_a_generator() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def add(x: int, y: int) -> int:\n  return x + y\n";
    let module = build(source, &["m", "not_a_generator"])?;
    let body = module
        .bodies
        .iter()
        .find(|b| b.name == "add")
        .ok_or("add body missing from module")?;
    assert!(
        !body.is_generator(),
        "an ordinary function body must not be reported as a generator"
    );
    Ok(())
}

#[test]
fn yield_records_the_generator_runtime_requirements() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def numbers() -> Generator[int]:\n  yield 1\n";
    let module = build(source, &["m", "yield_requirements"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("runtime_requirements:"));
    assert!(snapshot.contains("runtime_helper(generator)"));
    assert!(snapshot.contains("hosted_std"));
    assert!(snapshot.contains("allocator"));
    Ok(())
}

#[test]
fn yielded_expression_participates_in_last_use_tracking() -> Result<(), Box<dyn std::error::Error>> {
    // `s` is read once, inside the yielded value, and never again afterward -- it should read as a last-use
    // `move`, not fall back to an undercounted `clone`/`borrow` the way #1101's f-string bucket found and fixed
    // for embedded f-string reads (`count_reads_in_expr`'s `FString` arm); `Yield` needed the same fix.
    let source = "def one(s: str) -> Generator[str]:\n  yield s\n";
    let module = build(source, &["m", "yield_last_use"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("yield move(_0, last_use)"),
        "the yielded value should be a last-use move: {snapshot}"
    );
    Ok(())
}

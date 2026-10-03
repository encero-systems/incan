//! `await` as an explicit suspension point (#1164): async marking of bodies, ordering across suspensions, awaits inside
//! branches and loops, `race for` arms with per-arm bindings and block bodies, and the refusal of prefix surface
//! keywords that are not `await`.

use super::*;

const ASYNC_PRELUDE: &str =
    "import std.async\n\nasync def fast() -> int:\n  return 1\n\nasync def slow() -> int:\n  return 2\n\n";

#[test]
fn lowers_await_as_an_explicit_suspension_point_with_a_destination() -> Result<(), Box<dyn std::error::Error>> {
    let source = format!("{ASYNC_PRELUDE}async def f() -> int:\n  v = await fast()\n  return v\n");
    let module = build(&source, &["m", "await_one"])?;
    let snapshot = module.render_snapshot();
    assert_eq!(
        snapshot,
        build(&source, &["m", "await_one"])?.render_snapshot(),
        "lowering must be deterministic"
    );

    assert!(!snapshot.contains("unsupported("), "await must lower: {snapshot}");
    // The suspension carries a destination and the awaited operand's own ownership fact -- the two facts that
    // distinguish it from a generator `yield`, which produces outward and has no destination.
    assert!(
        snapshot.contains("_1 = await copy(_0, last_use)"),
        "await must record its destination and the awaited read's ownership fact: {snapshot}"
    );
    assert!(
        !snapshot.contains("yield"),
        "await must not be represented as a generator yield: {snapshot}"
    );
    Ok(())
}

#[test]
fn records_the_async_runtime_requirement_on_the_awaiting_body() -> Result<(), Box<dyn std::error::Error>> {
    let source = format!("{ASYNC_PRELUDE}async def f() -> int:\n  return await fast()\n");
    let module = build(&source, &["m", "await_req"])?;
    let rendered = body_named(&module, "f")?.render_snapshot();

    assert!(
        rendered.contains("async_runtime"),
        "the requirement must be recorded on the awaiting body itself, not merely somewhere in the module: {rendered}"
    );
    Ok(())
}

#[test]
fn an_async_body_without_any_await_is_still_marked_async() -> Result<(), Box<dyn std::error::Error>> {
    // The reason `is_async` is a stored declaration fact rather than derived the way `is_generator` is: this
    // body contains no `await` at all, yet its caller still gets an awaitable. Deriving async-ness by scanning
    // for a suspension point would report this body as synchronous.
    let source = "import std.async\n\nasync def f() -> int:\n  return 1\n";
    let module = build(source, &["m", "async_plain"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("body async f"),
        "an `async def` with no await must still be marked async: {snapshot}"
    );
    assert!(
        !snapshot.contains("await "),
        "this body genuinely contains no suspension point, so the async fact cannot have been derived from one: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_synchronous_body_is_not_marked_async() -> Result<(), Box<dyn std::error::Error>> {
    let module = build("def f() -> int:\n  return 1\n", &["m", "sync"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("body f "),
        "a plain function must render unmarked: {snapshot}"
    );
    assert!(
        !snapshot.contains("body async"),
        "a synchronous body must not be marked async: {snapshot}"
    );
    Ok(())
}

#[test]
fn sequential_awaits_keep_their_effect_ordering_across_suspension() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        format!("{ASYNC_PRELUDE}async def f() -> int:\n  x = await fast()\n  y = await slow()\n  return x + y\n");
    let module = build(&source, &["m", "await_seq"])?;
    let rendered = body_named(&module, "f")?.render_snapshot();

    assert!(!rendered.contains("unsupported("), "both awaits must lower: {rendered}");
    let first = rendered.find("call fn:fast(").ok_or("missing first awaitable")?;
    let first_await = rendered.find("await ").ok_or("missing first suspension")?;
    let second = rendered.find("call fn:slow(").ok_or("missing second awaitable")?;
    assert!(
        first < first_await && first_await < second,
        "statements before a suspension must precede it and statements after must follow it: {rendered}"
    );
    assert_eq!(
        rendered.matches("await ").count(),
        2,
        "each source `await` must produce its own suspension point: {rendered}"
    );
    Ok(())
}

#[test]
fn await_inside_a_branch_stays_inside_that_branch() -> Result<(), Box<dyn std::error::Error>> {
    let source = format!(
        "{ASYNC_PRELUDE}async def f(flag: bool) -> int:\n  mut total = 0\n  if flag:\n    total = await fast()\n  else:\n    total = 7\n  return total\n"
    );
    let module = build(&source, &["m", "await_branch"])?;
    let rendered = body_named(&module, "f")?.render_snapshot();

    assert!(
        !rendered.contains("unsupported("),
        "await in a branch must lower: {rendered}"
    );
    let branch_line = rendered
        .lines()
        .find(|line| line.contains("await "))
        .ok_or("missing suspension")?;
    assert!(
        branch_line.starts_with("    "),
        "the suspension must stay nested inside the branch block: {rendered}"
    );
    Ok(())
}

#[test]
fn await_inside_a_loop_stays_inside_the_loop_body() -> Result<(), Box<dyn std::error::Error>> {
    let source = format!(
        "{ASYNC_PRELUDE}async def f() -> int:\n  mut total = 0\n  mut i = 0\n  while i < 3:\n    total = total + await fast()\n    i = i + 1\n  return total\n"
    );
    let module = build(&source, &["m", "await_loop"])?;
    let rendered = body_named(&module, "f")?.render_snapshot();

    assert!(
        !rendered.contains("unsupported("),
        "await in a loop must lower: {rendered}"
    );
    let await_line = rendered
        .lines()
        .find(|line| line.contains("await "))
        .ok_or("missing suspension")?;
    assert!(
        await_line.starts_with("    "),
        "the suspension must stay inside the loop body: {rendered}"
    );
    Ok(())
}

#[test]
fn lowers_a_two_arm_race_with_per_arm_bindings_and_pre_selection_awaitables() -> Result<(), Box<dyn std::error::Error>>
{
    let source = format!(
        "{ASYNC_PRELUDE}async def f() -> int:\n  race for value:\n    await fast() => value\n    await slow() => value\n"
    );
    let module = build(&source, &["m", "race_two"])?;
    let body = body_named(&module, "f")?;
    let rendered = body.render_snapshot();

    assert!(
        !rendered.contains("unsupported("),
        "a two-arm race must lower: {rendered}"
    );
    // Every awaitable is evaluated before selection, in source order -- observable here as both calls being
    // emitted ahead of the race statement rather than inside an arm.
    let fast_at = rendered.find("call fn:fast(").ok_or("missing first awaitable")?;
    let slow_at = rendered.find("call fn:slow(").ok_or("missing second awaitable")?;
    let race_at = rendered.find("race:").ok_or("missing race statement")?;
    assert!(
        fast_at < slow_at && slow_at < race_at,
        "all arm awaitables must be evaluated, in source order, before selection: {rendered}"
    );
    // Each arm owns a type-refined local, but both locals are projections of the one authored race-header binding.
    assert_eq!(
        rendered.matches("value : int [binding]").count(),
        2,
        "each arm must bind its own local rather than sharing one: {rendered}"
    );
    let identities: Vec<_> = body
        .locals
        .iter()
        .filter(|local| local.name.as_deref() == Some("value"))
        .filter_map(|local| local.identity.as_ref())
        .collect();
    let [first_arm, second_arm] = identities.as_slice() else {
        return Err(format!("both arm bindings need canonical identities: {body:?}").into());
    };
    assert_eq!(first_arm, second_arm, "both arm locals must retain the header identity");
    let header = source.find("value:").ok_or("missing race header binding")?;
    for local in body
        .locals
        .iter()
        .filter(|local| local.name.as_deref() == Some("value"))
    {
        assert_eq!(
            local.span,
            incan_semantics_core::HirSourceSpan::new(header, header + "value".len()),
            "each arm local must stay anchored to the exact race header token"
        );
    }
    Ok(())
}

#[test]
fn a_race_arm_binding_does_not_escape_its_arm() -> Result<(), Box<dyn std::error::Error>> {
    // The arm binding shadows an enclosing name only for the duration of its own arm. Restoring it is the same
    // discipline `lower_closure` follows, and getting it wrong is silent: reads after the race would resolve to
    // the last arm's local, so the body would compute the wrong value with no unsupported node to show for it.
    let source = format!(
        "{ASYNC_PRELUDE}async def f() -> int:\n  value = 100\n  winner = race for value:\n    await fast() => value\n    await slow() => value\n  return value + winner\n"
    );
    let module = build(&source, &["m", "race_shadow"])?;
    let body = body_named(&module, "f")?;
    let rendered = body.render_snapshot();

    // `value` is declared first, so it is local 0; the trailing `value + winner` must read exactly that local.
    let outer = local_for_binding(&rendered, "value").ok_or("missing outer binding")?;
    assert_eq!(
        outer, "_0",
        "the outer binding should be the first declared local: {rendered}"
    );
    let sum_line = rendered
        .lines()
        .find(|line| line.contains(" + "))
        .ok_or("missing the trailing sum")?;
    assert!(
        sum_line.contains("copy(_0)"),
        "a read after the race must resolve to the enclosing binding, not an arm local: {rendered}"
    );
    Ok(())
}

#[test]
fn a_block_arm_local_does_not_leak_past_its_arm() -> Result<(), Box<dyn std::error::Error>> {
    // A block arm lowers ordinary statements, so `let total = ...` inside it declares a lexical local through
    // the same path any assignment uses. Restoring only the shared race binding would leave that arm-local
    // installed, and the trailing read of `total` would silently resolve to it instead of the outer binding --
    // a wrong value with no unsupported node to show for it.
    let source = format!(
        "{ASYNC_PRELUDE}async def f() -> int:\n  let total = 100\n  winner = race for value:\n    await fast() => value\n    await slow() =>\n      let total = value * 2\n      total\n  return total + winner\n"
    );
    let module = build(&source, &["m", "race_arm_local"])?;
    let body = body_named(&module, "f")?;
    let rendered = body.render_snapshot();

    // The outer binding is distinct from the arm's `total`, and the post-race expression must use that outer
    // local regardless of earlier parameters or temporaries that might affect local numbering.
    let outer = local_for_binding(&rendered, "total").ok_or("missing outer binding")?;
    assert!(
        rendered.matches("total : int [binding]").count() >= 2,
        "the arm must declare its own `total` rather than reusing the outer one: {rendered}"
    );
    let sum_line = rendered
        .lines()
        .find(|line| line.contains(" + ") && !line.starts_with("      "))
        .ok_or("missing the trailing sum")?;
    assert!(
        sum_line.contains(&format!("copy({outer})")),
        "a read after the race must resolve to the enclosing local, not one an arm body declared: {rendered}"
    );
    Ok(())
}

#[test]
fn a_race_arm_block_body_lowers_its_statements_and_trailing_value() -> Result<(), Box<dyn std::error::Error>> {
    let source = format!(
        "{ASYNC_PRELUDE}async def f() -> int:\n  race for value:\n    await fast() => value\n    await slow() =>\n      doubled = value * 2\n      doubled\n"
    );
    let module = build(&source, &["m", "race_block"])?;
    let rendered = body_named(&module, "f")?.render_snapshot();

    assert!(
        !rendered.contains("unsupported("),
        "a block arm body must lower: {rendered}"
    );
    // The arm body's statements live inside the arm, indented under it -- only the winning arm runs, so they
    // must not be hoisted into the enclosing block alongside the awaitables.
    let arm_stmt = rendered
        .lines()
        .find(|line| line.contains("* const(2)"))
        .ok_or("missing the arm body computation")?;
    assert!(
        arm_stmt.starts_with("      "),
        "an arm body statement must stay nested inside its arm: {rendered}"
    );
    // The block's trailing expression becomes the arm's result, not merely a statement inside it.
    assert!(
        rendered.contains("-> copy(_5)"),
        "the block's trailing expression must become the arm's result operand: {rendered}"
    );
    Ok(())
}

#[test]
fn an_unsupported_construct_in_a_race_arm_does_not_collapse_the_whole_race() -> Result<(), Box<dyn std::error::Error>> {
    // The issue's explicit requirement: a construct Body IR cannot represent keeps its own node *inside* a
    // represented race, so a consumer loses only that construct rather than the entire expression.
    //
    // The arm takes its block form so it can hold the shared by-design stand-in refusal. Its previous stand-in was
    // collection membership, which #1246 represented -- see STAND_IN_REFUSAL_LABEL for why a gap is never a safe
    // choice here.
    let source = format!(
        "{ASYNC_PRELUDE}async def f() -> bool:\n  race for value:\n    await fast() => value == 1\n    await slow() =>\n{}      value == 2\n",
        stand_in_refusal_stmt("      ")
    );
    let module = build(&source, &["m", "race_partial"])?;
    let rendered = body_named(&module, "f")?.render_snapshot();

    assert!(
        rendered.contains("race:"),
        "the race itself must still be represented: {rendered}"
    );
    // Asserting the node exists somewhere in the body would also pass if it had been hoisted into the enclosing
    // block -- the exact regression this test exists to catch -- so require it to be indented inside an arm.
    let refusal = rendered
        .lines()
        .find(|line| line.contains("unsupported("))
        .ok_or("missing the refusal for the unrepresentable arm construct")?;
    assert!(
        refusal.starts_with("      "),
        "the refusal must stay inside its arm rather than collapsing or escaping the race: {rendered}"
    );
    Ok(())
}

#[test]
fn an_async_method_body_is_marked_async() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import std.async\n\nclass C:\n  async def m(self) -> int:\n    return 1\n";
    let module = build(source, &["m", "async_method"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("body async m"),
        "an async method body must carry the same async fact as an async function: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_prefix_surface_keyword_that_is_not_await_is_refused_rather_than_treated_as_a_suspension() {
    // `SurfaceExprPayload::PrefixUnary` is generic over any prefix soft keyword; `await` is merely the only one
    // registered today. Dispatching on the payload alone would silently lower a future prefix keyword as a
    // suspension point, so lowering matches the surface *key*. This pins that.
    let type_info = TypeCheckInfo::default();
    let function_default_sources = FunctionDefaultSources::new();
    let local_function_declarations = LocalFunctionDeclarations::new();
    let local_nominal_declarations = LocalNominalDeclarations::new();
    let local_fieldless_enum_declarations = LocalFieldlessEnumDeclarations::new();
    let local_value_enum_declarations = LocalValueEnumDeclarations::new();
    let provider_operations = ProviderOperationCatalog::new();
    let lowering_facts = BodyIrLoweringFacts {
        type_info: &type_info,
        function_default_sources: &function_default_sources,
        local_function_declarations: &local_function_declarations,
        local_nominal_declarations: &local_nominal_declarations,
        local_fieldless_enum_declarations: &local_fieldless_enum_declarations,
        local_value_enum_declarations: &local_value_enum_declarations,
        module_identity: "m",
        provider_operations: &provider_operations,
    };
    let mut builder = BodyBuilder::new(&lowering_facts, IncanType::Unknown);
    let scope = builder.new_scope(None, HirSourceSpan::new(0, 1));
    let mut out = Vec::new();
    let surface = ast::SurfaceExpr {
        key: SurfaceFeatureKey::SoftKeyword(KeywordId::Async),
        payload: ast::SurfaceExprPayload::PrefixUnary(Box::new(ast::Spanned::new(
            ast::Expr::Ident("placeholder".to_string()),
            ast::Span::new(0, 1),
        ))),
    };

    let _ = builder.lower_surface_expr(&surface, ast::Span::new(0, 1), scope, &mut out);

    assert!(
        out.iter().any(|stmt| matches!(
            &stmt.kind,
            bir::StatementKind::Unsupported { description } if description.contains("prefix-keyword")
        )),
        "a non-`await` prefix keyword must keep the generic surface refusal, not become an await: {out:?}"
    );
    assert!(
        !out.iter()
            .any(|stmt| matches!(&stmt.kind, bir::StatementKind::Await { .. })),
        "no suspension point may be emitted for a keyword that is not `await`: {out:?}"
    );
}

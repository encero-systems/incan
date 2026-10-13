//! `await` as an explicit suspension point (#1164): async marking of bodies, ordering across suspensions, awaits inside
//! branches and loops, `race for` arms with per-arm bindings and block bodies, and the refusal of prefix surface
//! keywords that are not `await`.

use super::*;

const ASYNC_PRELUDE: &str =
    "import std.async\n\nasync def fast() -> int:\n  return 1\n\nasync def slow() -> int:\n  return 2\n\n";

/// Public SDK arms retain their checked callback outputs through a nested variadic race call.
#[test]
fn public_sdk_nested_race_retains_callback_output() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"import std.async
from std.async.race import arm, race
from std.async.time import sleep_ms
async def slow() -> int:
    while true:
        await sleep_ms(1)
    return 1
async def fast() -> int:
    await sleep_ms(5)
    return 2
def describe(value: int) -> str:
    return f"winner {value}"
async def main() -> None:
    println(await race(arm(slow(), describe), arm(fast(), describe)))
"#;
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    assert!(
        checker
            .type_info()
            .calls
            .inferred_call_result_types
            .values()
            .any(|ty| matches!(ty,
                crate::symbols::ResolvedType::Generic(name, outputs)
                if name == "RaceArm" && outputs == &[crate::symbols::ResolvedType::Str]
            )),
        "retained calls: {:#?}",
        checker.type_info().calls.inferred_call_result_types
    );
    let module = build(source, &["main"])?;
    let body = body_named(&module, "main")?;
    assert!(
        body.locals.iter().all(|local| local.ty != IncanType::Unknown),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}

/// A source-owned equal spelling cannot acquire the SDK's opaque rest-pack storage contract.
#[test]
fn source_owned_race_arm_rest_pack_stays_refused() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model RaceArm:\n    value: int\n\ndef consume(*arms: RaceArm) -> None:\n    pass\n\ndef main() -> None:\n    consume(RaceArm(value=1))\n";
    let module = build(source, &["main"])?;
    let body = body_named(&module, "main")?;
    assert!(
        body.render_snapshot()
            .contains("rest call with an unretained concrete pack element"),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}

/// The stdlib rusttype alias retains its native enum owner, ordered payloads and checked pattern arguments.
#[test]
fn sdk_timeout_outcome_retains_payloads() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"import std.async
from std.async.time import TimeoutJoinOutcome
def inspect(outcome: TimeoutJoinOutcome[int]) -> None:
    match outcome:
        TimeoutJoinOutcome.Completed(value) => println(value)
        TimeoutJoinOutcome.JoinFailed(error) => println(error.message())
        TimeoutJoinOutcome.TimedOut(handle) => pass
"#;
    let module = build(source, &["m", "deadline_outcome"])?;
    let declaration = module
        .enum_declarations
        .iter()
        .find(|item| item.name == "TimeoutJoinOutcome")
        .ok_or("native outcome layout")?;
    assert_eq!(declaration.type_parameters, ["T"]);
    assert_eq!(
        declaration
            .variants
            .iter()
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>(),
        ["Completed", "JoinFailed", "TimedOut"]
    );
    assert_eq!(declaration.variants[0].fields, [IncanType::TypeVar("T".into())]);
    assert_eq!(
        declaration.variants[1].fields,
        [IncanType::Named("TaskJoinError".into())]
    );
    assert_eq!(
        declaration.variants[2].fields,
        [IncanType::Generic {
            base: "JoinHandle".into(),
            args: vec![IncanType::TypeVar("T".into())]
        }]
    );
    let body = body_named(&module, "inspect")?;
    for (name, expected) in [
        ("value", IncanType::Primitive(IncanPrimitiveType::Int)),
        ("error", IncanType::Named("TaskJoinError".into())),
        (
            "handle",
            IncanType::Generic {
                base: "JoinHandle".into(),
                args: vec![IncanType::Primitive(IncanPrimitiveType::Int)],
            },
        ),
    ] {
        let local = body
            .locals
            .iter()
            .find(|local| local.name.as_deref() == Some(name))
            .ok_or("payload local")?;
        assert_eq!(local.ty, expected, "{name}");
    }
    Ok(())
}

/// An equal-spelled source enum never acquires the SDK's native payload contract.
#[test]
fn source_timeout_outcome_keeps_its_owner() -> Result<(), Box<dyn std::error::Error>> {
    let source = "enum TimeoutJoinOutcome:\n  Own(int)\ndef inspect(value: TimeoutJoinOutcome) -> None:\n  match value:\n    TimeoutJoinOutcome.Own(number) => println(number)\n";
    let module = build(source, &["m", "source_outcome"])?;
    let [declaration] = module.enum_declarations.as_slice() else {
        return Err("source layout".into());
    };
    assert!(matches!(declaration.canonical.origin, SymbolOrigin::Module(_)));
    assert_eq!(declaration.variants[0].name, "Own");
    Ok(())
}

/// The exact SDK FnOnce bridge retains a checked zero-argument callable's output in native task storage.
#[test]
fn sdk_blocking_callback_retains_output() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"import std.async
from std.rust import Send, Static
from rust::incan_std_async::task import JoinHandle, RuntimeFnOnce, spawn_blocking as native_spawn
def launch[T with (Send, Static), F with RuntimeFnOnce[T]](task: F) -> JoinHandle[T]:
    return native_spawn(task)
def work() -> int:
    return 2
def main() -> None:
    task = launch(work)
"#;
    let module = build(source, &["m", "blocking_callback"])?;
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    assert!(
        library_nominal_declarations(checker.type_info())
            .iter()
            .any(|declaration| declaration.name == "TaskJoinError")
    );
    let body = body_named(&module, "main")?;
    let task = body
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("task"))
        .ok_or("task")?;
    assert_eq!(
        task.ty,
        IncanType::Generic {
            base: "JoinHandle".into(),
            args: vec![IncanType::Primitive(IncanPrimitiveType::Int)]
        }
    );
    Ok(())
}

/// A checked native race callback closes both its input and output against the future's retained output.
#[test]
fn sdk_race_callback_retains_output() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"import std.async
from std.rust import Send, Static
from rust::incan_std_async::task import RuntimeFuture
from rust::incan_std_async::race import RaceArm, RuntimeRaceCallback, arm as native_arm
def package[T with Send, R with (Send, Static), F with RuntimeFuture[T], C with RuntimeRaceCallback[T, R]](task: F, callback: C) -> RaceArm[R]:
    return native_arm(task, callback)
async def work() -> int:
    return 1
def describe(value: int) -> str:
    return str(value)
def main() -> None:
    packaged = package(work(), describe)
"#;
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    assert!(checker.type_info().calls.inferred_call_result_types.values().any(|ty| matches!(ty, crate::symbols::ResolvedType::Generic(name, outputs) if name == "RaceArm" && outputs == &[crate::symbols::ResolvedType::Str])));
    let module = build(source, &["m", "race_callback"])?;
    let body = body_named(&module, "main")?;
    let arm = body
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("packaged"))
        .ok_or("arm")?;
    assert_eq!(
        arm.ty,
        IncanType::Generic {
            base: "RaceArm".into(),
            args: vec![IncanType::Primitive(IncanPrimitiveType::Str)]
        }
    );
    Ok(())
}

/// A parent call reads an already checked nested task result without reusing legacy's open SDK placeholder.
#[test]
fn nested_sdk_task_call_retains_output() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"import std.async
from std.rust import Send, Static
from rust::incan_std_async::task import JoinHandle, RuntimeFuture, spawn as native_spawn
def launch[T with (Send, Static), F with RuntimeFuture[T]](task: F) -> JoinHandle[T]:
    return native_spawn(task)
def forward[T](task: JoinHandle[T]) -> JoinHandle[T]:
    return task
async def work() -> int:
    return 3
def main() -> None:
    task = forward(launch(work()))
"#;
    let module = build(source, &["m", "nested_task_call"])?;
    let body = body_named(&module, "main")?;
    let task = body
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("task"))
        .ok_or("task")?;
    assert_eq!(
        task.ty,
        IncanType::Generic {
            base: "JoinHandle".into(),
            args: vec![IncanType::Primitive(IncanPrimitiveType::Int)]
        }
    );
    Ok(())
}

/// The SDK's positional rest slot is one owned list in its published native ABI.
#[test]
fn sdk_variadic_race_retains_list_signature() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import std.async\nfrom std.async.race import race, RaceArm\nasync def collect(arm: RaceArm[int]) -> int:\n    return await race(arm)\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let calls = library_call_delegations(checker.type_info());
    let call = calls
        .iter()
        .find(|call| call.canonical.declaration_name == "race")
        .ok_or("SDK race signature")?;
    assert_eq!(
        call.parameters,
        vec![IncanType::Generic {
            base: "List".into(),
            args: vec![IncanType::Generic {
                base: "RaceArm".into(),
                args: vec![IncanType::TypeVar("R".into())]
            }]
        }]
    );
    let module = build(source, &["m", "race_rest_storage"])?;
    let body = body_named(&module, "collect")?;
    assert!(!body.locals.iter().any(|local| matches!(&local.ty, IncanType::Generic { base, args } if base == "List" && args.iter().any(|ty| matches!(ty, IncanType::Generic { base, args } if base == "RaceArm" && args.iter().any(|ty| matches!(ty, IncanType::TypeVar(_) | IncanType::RustInteropPath(_))))))));
    Ok(())
}

/// Equal-spelled foreign callback traits cannot authorize native SDK output retention.
#[test]
fn foreign_callback_outputs_stay_open() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "from rust::foreign_runtime::task import RuntimeFnOnce\ndef launch[T, F with RuntimeFnOnce[T]](task: F) -> None:\n    pass\ndef work() -> int:\n    return 1\ndef main() -> None:\n    launch(work)\n",
        "from rust::foreign_runtime::race import RuntimeRaceCallback\ndef launch[T, R, F with RuntimeRaceCallback[T, R]](task: F) -> None:\n    pass\ndef work(value: int) -> str:\n    return str(value)\ndef main() -> None:\n    launch(work)\n",
    ] {
        let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
        let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
        let mut checker = TypeChecker::new();
        checker
            .check_program(&program)
            .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
        assert!(checker.type_info().calls.body_ir_call_type_arguments.is_empty());
    }
    Ok(())
}

/// A join result retains the SDK-owned error even when no error import appears in the source.
#[test]
fn retains_sdk_join_error_from_await() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import std.async\nfrom std.async.task import JoinHandle\nasync def join(handle: JoinHandle[int]) -> None:\n  match await handle:\n    Ok(value) => println(value)\n    Err(error) => println(error.message())\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let declarations = library_nominal_declarations(checker.type_info());
    assert!(
        declarations
            .iter()
            .any(|declaration| declaration.name == "TaskJoinError"),
        "join error must have its SDK nominal record: {declarations:?}"
    );
    Ok(())
}

/// Await consumes the owned future and records a separately typed resumed destination.
/// A source declaration sharing the task error's spelling keeps its own layout and methods.
#[test]
fn source_join_error_name_keeps_its_own_layout() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "model TaskJoinError:\n  detail: int\ndef describe(error: TaskJoinError) -> int:\n  return error.detail\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    assert!(
        !library_nominal_declarations(checker.type_info())
            .iter()
            .any(|declaration| declaration.name == "TaskJoinError")
    );
    let module = build(source, &["m", "source_error"])?;
    assert!(
        module
            .nominal_declarations
            .iter()
            .any(|declaration| declaration.name == "TaskJoinError" && declaration.fields == ["detail"])
    );
    Ok(())
}

/// Await consumes the owned future and records a separately typed resumed destination.
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
        snapshot.contains("_1 = await move(_0, last_use)"),
        "await must record its destination and the awaited read's ownership fact: {snapshot}"
    );
    assert!(
        !snapshot.contains("yield"),
        "await must not be represented as a generator yield: {snapshot}"
    );
    Ok(())
}

/// Future construction retains an owned carrier, while the await destination and source binding keep its output.
#[test]
fn retains_async_call_carriers_before_await() -> Result<(), Box<dyn std::error::Error>> {
    let source = format!("{ASYNC_PRELUDE}async def f() -> int:\n  value = await fast()\n  return value\n");
    let module = build(&source, &["m", "async_carrier"])?;
    let body = body_named(&module, "f")?;
    let output = IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Int);
    let future = IncanType::Generic {
        base: "Awaitable".to_string(),
        args: vec![output.clone()],
    };
    assert_eq!(body.locals[0].ty, future);
    assert_eq!(body.locals[1].ty, output);
    assert_eq!(body.locals[2].ty, output);
    assert!(body.render_snapshot().contains("await move(_0, last_use)"));
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
    // An injected undesugared vocab item exercises this boundary without freezing a valid language construct as a gap.
    let source = format!(
        "{ASYNC_PRELUDE}async def f() -> bool:\n  race for value:\n    await fast() => value == 1\n    await slow() =>\n{}      value == 2\n",
        stand_in_refusal_stmt("      ")
    );
    let module = build_with_stand_in_refusal(&source, &["m", "race_partial"])?;
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
    let published_constants = HashMap::new();
    let published_callable_returns = HashMap::new();
    let local_enum_declarations = HashMap::new();
    let runtime_type_conflicts = std::collections::BTreeSet::new();
    let lowering_facts = BodyIrLoweringFacts {
        local_enum_declarations: &local_enum_declarations,
        published_methods: &[],
        published_callable_returns: &published_callable_returns,
        published_constants: &published_constants,
        type_info: &type_info,
        runtime_type_conflicts: &runtime_type_conflicts,
        function_default_sources: &function_default_sources,
        local_function_declarations: &local_function_declarations,
        local_receiver_methods: &[],
        local_nominal_declarations: &local_nominal_declarations,
        local_fieldless_enum_declarations: &local_fieldless_enum_declarations,
        local_value_enum_declarations: &local_value_enum_declarations,
        module_identity: "m",
        provider_operations: &provider_operations,
        rust_module: None,
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
/// SDK provider native spellings retain the same task carrier and argument widths in expressions and parameter frames.
#[test]
fn native_task_paths_retain_sdk_carriers() -> Result<(), Box<dyn std::error::Error>> {
    let facts = TypeCheckInfo::default();
    let actual = super::super::parameter_types::checked_semantic_type(
        &crate::symbols::ResolvedType::RustPath("incan_std_async::task::JoinHandle<i64>".to_owned()),
        &facts,
    );
    assert_eq!(
        actual,
        IncanType::Generic {
            base: "JoinHandle".to_owned(),
            args: vec![IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Int)]
        }
    );
    let sized = super::super::parameter_types::checked_semantic_type(
        &crate::symbols::ResolvedType::RustPath("incan_std_async::task::JoinHandle<i32>".to_owned()),
        &facts,
    );
    assert_eq!(
        sized,
        IncanType::Generic {
            base: "JoinHandle".to_owned(),
            args: vec![IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Numeric(
                incan_lang::lang::types::numerics::NumericTypeId::I32
            ))]
        }
    );
    assert_eq!(
        super::super::parameter_types::checked_parameter_type(
            &crate::symbols::ResolvedType::RustPath("incan_std_async::task::JoinHandle<i64>".to_owned()),
            &facts,
            &[],
        ),
        actual
    );
    assert_eq!(
        super::super::parameter_types::checked_context_type(
            &crate::symbols::ResolvedType::RustPath("incan_std_async::task::JoinHandle<i32>".to_owned()),
            &facts,
            &[],
            &std::collections::BTreeSet::new(),
        ),
        sized
    );
    assert_eq!(
        super::super::parameter_types::checked_parameter_type(
            &crate::symbols::ResolvedType::Named("JoinHandle".to_owned()),
            &facts,
            &["JoinHandle".to_owned()],
        ),
        IncanType::TypeVar("JoinHandle".to_owned())
    );
    Ok(())
}

/// An arbitrary same-tail Rust type never receives SDK ownership or a known runtime layout.
#[test]
fn foreign_task_paths_remain_opaque() {
    let facts = TypeCheckInfo::default();
    let ty = crate::symbols::ResolvedType::RustPath("foreign::task::JoinHandle<i64>".to_owned());
    assert_eq!(
        super::super::parameter_types::checked_semantic_type(&ty, &facts),
        semantic_type_from_resolved(&ty)
    );
    assert_eq!(
        super::super::parameter_types::checked_parameter_type(&ty, &facts, &[]),
        semantic_type_from_resolved(&ty)
    );
}
/// An SDK async call retains its executable crate target separately from the future's output carrier.
#[test]
fn retains_sdk_async_future_delegation() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from std.async.time import sleep\nasync def wait() -> None:\n  await sleep(0.01)\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let delegations = library_call_delegations(checker.type_info());
    let call = delegations
        .iter()
        .find(|call| call.canonical.declaration_name == "sleep")
        .ok_or("SDK sleep must retain its executable delegation")?;
    assert_eq!(
        call.return_type,
        IncanType::Generic {
            base: "Awaitable".into(),
            args: vec![semantic_type_from_resolved(&crate::symbols::ResolvedType::Unit)],
        }
    );
    assert!(
        call.rust_path().is_some(),
        "SDK future construction needs its canonical crate item"
    );
    Ok(())
}

/// A native SDK generic argument uses its declaration's retained binder rather than an opaque Rust path.
#[test]
fn retains_native_task_binder() -> Result<(), Box<dyn std::error::Error>> {
    let ty = crate::symbols::ResolvedType::RustPath("incan_std_async::task::JoinHandle<T>".into());
    let ty = super::super::parameter_types::checked_semantic_type(&ty, &Default::default());
    let ty = super::super::parameter_types::retain_parameter_type(ty, &["T".into()]);
    assert_eq!(
        ty,
        IncanType::Generic {
            base: "JoinHandle".into(),
            args: vec![IncanType::TypeVar("T".into())]
        }
    );
    Ok(())
}

/// A spawning SDK wrapper retains its runtime-owned task return and its own future parameter binder.
#[test]
fn retains_native_task_sdk_signature() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = crate::typechecker::stdlib_loader::StdlibAstCache::new();
    let path = vec!["std".to_owned(), "async".to_owned(), "task".to_owned()];
    let identity = cache
        .lookup_identity(&path, "spawn")
        .ok_or("SDK spawn source identity")?;
    let mut info = TypeCheckInfo::default();
    info.references.resolved_identities.insert((0, 1), identity);
    let calls = library_call_delegations(&info);
    let call = calls
        .iter()
        .find(|call| call.canonical.declaration_name == "spawn")
        .ok_or_else(|| format!("SDK spawn declaration: {calls:?}"))?;
    assert_eq!(
        call.return_type,
        IncanType::Generic {
            base: "JoinHandle".into(),
            args: vec![IncanType::TypeVar("T".into())]
        },
        "{call:?}"
    );
    assert_eq!(
        call.parameters,
        vec![IncanType::TypeVar("TaskFuture".into())],
        "{call:?}"
    );
    Ok(())
}

/// A checked source async argument closes the native RuntimeFuture output only in Body IR's additive tables.
#[test]
fn retains_runtime_future_argument_and_output() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import std.async\nfrom rust::incan_std_async::task import RuntimeFuture\ndef accept[T, F with RuntimeFuture[T]](task: F) -> None:\n  pass\nasync def work() -> int:\n  return 1\ndef launch() -> None:\n  accept(work())\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let retained = &checker.type_info().calls.body_ir_call_type_arguments;
    assert!(
        retained.values().any(|arguments| arguments
            == &[
                crate::symbols::ResolvedType::Int,
                crate::symbols::ResolvedType::Generic("Awaitable".into(), vec![crate::symbols::ResolvedType::Int]),
            ]),
        "native future arguments: {retained:?}"
    );
    assert!(
        checker
            .type_info()
            .calls
            .call_site_monomorph_type_args
            .values()
            .all(|arguments| arguments.len() != 2),
        "legacy inference stays open"
    );
    Ok(())
}

/// Conflicting actual future outputs do not publish a complete private instantiation for one shared binder.
#[test]
fn conflicting_runtime_future_arguments_stay_open() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import std.async\nfrom rust::incan_std_async::task import RuntimeFuture\ndef accept[T, F with RuntimeFuture[T]](first: F, second: F) -> None:\n  pass\nasync def integer() -> int:\n  return 1\nasync def text() -> str:\n  return \"text\"\ndef launch() -> None:\n  accept(integer(), text())\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    let errors = checker
        .check_program(&program)
        .err()
        .ok_or("conflicting future outputs must fail checking")?;
    assert!(
        errors.iter().any(|error| error.message.contains("Type mismatch")),
        "{errors:?}"
    );
    assert!(
        checker.type_info().calls.body_ir_call_type_arguments.is_empty(),
        "conflicting future arguments: {:?}",
        checker.type_info().calls.body_ir_call_type_arguments
    );
    Ok(())
}

/// An equal-spelled foreign RuntimeFuture bound cannot authorize the SDK's output inference bridge.
#[test]
fn foreign_runtime_future_output_stays_open() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import std.async\nfrom rust::foreign_runtime::task import RuntimeFuture\ndef accept[T, F with RuntimeFuture[T]](task: F) -> None:\n  pass\nasync def work() -> int:\n  return 1\ndef launch() -> None:\n  accept(work())\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    assert!(
        checker.type_info().calls.body_ir_call_type_arguments.is_empty(),
        "foreign future arguments: {:?}",
        checker.type_info().calls.body_ir_call_type_arguments
    );
    Ok(())
}

/// A native task handle followed by a source future cannot erase the earlier argument's binding evidence.
#[test]
fn mixed_runtime_future_arguments_stay_open() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import std.async\nfrom std.async.task import JoinHandle\nfrom rust::incan_std_async::task import RuntimeFuture\ndef accept[T, F with RuntimeFuture[T]](first: F, second: F) -> None:\n  pass\nasync def work() -> int:\n  return 1\ndef launch(handle: JoinHandle[int]) -> None:\n  accept(handle, work())\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    let errors = checker
        .check_program(&program)
        .err()
        .ok_or("mixed native future carriers must fail checking")?;
    assert!(
        errors.iter().any(|error| error.message.contains("Type mismatch")),
        "{errors:?}"
    );
    assert!(
        checker.type_info().calls.body_ir_call_type_arguments.is_empty(),
        "mixed native arguments: {:?}",
        checker.type_info().calls.body_ir_call_type_arguments
    );
    Ok(())
}

/// A runtime-owned join error retains its shared receiver and owned message result without inventing fields.
#[test]
fn retains_sdk_join_error_method_contract() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from std.async.task import TaskJoinError\ndef describe(error: TaskJoinError) -> str:\n  return error.message()\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let calls = library_call_delegations(checker.type_info());
    let call = calls
        .iter()
        .find(|call| call.rust_path().as_deref() == Some("incan_std_async::task::TaskJoinError::message"))
        .ok_or_else(|| format!("SDK message contract: {calls:?}"))?;
    assert_eq!(
        call.parameters,
        vec![IncanType::Ref(Box::new(IncanType::Named("TaskJoinError".into())))]
    );
    assert_eq!(call.return_type, IncanType::Primitive(IncanPrimitiveType::Str));
    let module = build(source, &["m", "sdk_error_method"])?;
    let body = body_named(&module, "describe")?;
    let (target, destination) = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                destination: Some(destination),
                callee: bir::Callee::Method(target),
                ..
            } if target.name == "message" => Some((target, destination)),
            _ => None,
        })
        .ok_or("message call")?;
    assert_eq!(target.canonical.as_ref(), Some(&call.canonical));
    assert_eq!(target.binding, bir::ArgumentBinding::resolved_positional(0));
    let local = destination.local_id().ok_or("message destination")?;
    assert_eq!(
        body.locals[local.index()].ty,
        IncanType::Primitive(IncanPrimitiveType::Str)
    );
    Ok(())
}

/// The SDK error source getter retains its distinct owned Option[str] carrier.
#[test]
fn retains_sdk_join_error_source_contract() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from std.async.task import TaskJoinError\ndef source_of(error: TaskJoinError) -> Option[str]:\n  return error.source()\n";
    let module = build(source, &["m", "sdk_error_source"])?;
    let body = body_named(&module, "source_of")?;
    let destination = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                destination: Some(place),
                callee: bir::Callee::Method(target),
                ..
            } if target.name == "source" && target.binding == bir::ArgumentBinding::resolved_positional(0) => {
                place.local_id()
            }
            _ => None,
        })
        .ok_or("source getter destination")?;
    assert_eq!(
        body.locals[destination.index()].ty,
        IncanType::Generic {
            base: "Option".into(),
            args: vec![IncanType::Primitive(IncanPrimitiveType::Str)],
        }
    );
    Ok(())
}

/// An inferred task binding keeps the checked native future output that closed its SDK spawn call.
#[test]
fn inferred_task_binding_retains_closed_output() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import std.async\nfrom std.rust import Send, Static\nfrom rust::incan_std_async::task import JoinHandle, RuntimeFuture, spawn as native_spawn\ndef launch[T with (Send, Static), F with RuntimeFuture[T]](future: F) -> JoinHandle[T]:\n  return native_spawn(future)\nasync def work() -> int:\n  return 1\nasync def main() -> None:\n  task = launch(work())\n  task.abort()\n";
    let module = build(source, &["m", "inferred_task"])?;
    let body = body_named(&module, "main")?;
    let task = body
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("task"))
        .ok_or("task binding")?;
    assert_eq!(
        task.ty,
        IncanType::Generic {
            base: "JoinHandle".into(),
            args: vec![IncanType::Primitive(IncanPrimitiveType::Int)],
        }
    );
    Ok(())
}

/// Source and foreign equal-spelled errors do not acquire the SDK's native method records.
#[test]
fn non_sdk_join_error_methods_keep_their_owner() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "model TaskJoinError:\n  def message(self) -> int:\n    return 7\ndef describe(error: TaskJoinError) -> int:\n  return error.message()\n",
        "from rust::foreign_tasks::task import TaskJoinError\ndef describe(error: TaskJoinError) -> str:\n  return error.message()\n",
    ] {
        let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
        let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
        let mut checker = TypeChecker::new();
        checker
            .check_program(&program)
            .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
        assert!(
            !library_call_delegations(checker.type_info())
                .iter()
                .any(|call| call.rust_path().as_deref() == Some("incan_std_async::task::TaskJoinError::message"))
        );
    }
    Ok(())
}
/// The checker-selected consuming loop keeps closed handle storage and moves the list; cloneable loops keep borrowing.
#[test]
fn checked_task_list_iteration_keeps_closed_items() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import std.async\nfrom std.rust import Send, Static\nfrom rust::incan_std_async::task import JoinHandle, RuntimeFuture, spawn as native_spawn\ndef launch[T with (Send, Static), F with RuntimeFuture[T]](future: F) -> JoinHandle[T]:\n  return native_spawn(future)\nasync def work(value: int) -> int:\n  return value\ndef take(handle: JoinHandle[int]) -> None:\n  pass\ndef shout(text: str) -> None:\n  println(text)\nasync def main() -> None:\n  handles = [launch(work(1)), launch(work(2))]\n  for handle in handles:\n    take(handle)\n  names = [\"a\"]\n  for name in names:\n    shout(name)\n  println(len(names))\n";
    let module = build(source, &["m", "taking_task_list"])?;
    let body = body_named(&module, "main")?;
    let handle = IncanType::Generic {
        base: "JoinHandle".into(),
        args: vec![IncanType::Primitive(IncanPrimitiveType::Int)],
    };
    let handles = body
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("handles"))
        .ok_or("handles")?;
    assert_eq!(
        handles.ty,
        IncanType::Generic {
            base: "List".into(),
            args: vec![handle.clone()]
        }
    );
    let item = body
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("handle"))
        .ok_or("handle")?;
    assert_eq!(item.ty, handle);
    let mut loop_sources = Vec::new();
    for statement in &body.block.stmts {
        let bir::StatementKind::Loop { body: loop_body, .. } = &statement.kind else {
            continue;
        };
        let iterator = loop_body
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                bir::StatementKind::IterNext {
                    iterator: bir::Operand::Place(read),
                    ..
                } => read.place.local_id(),
                _ => None,
            })
            .ok_or("loop iterator")?;
        let fact = body
            .block
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                bir::StatementKind::Assign {
                    place,
                    rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
                } if place.local_id() == Some(iterator) => Some(read.fact),
                _ => None,
            })
            .ok_or("iterator materialization")?;
        loop_sources.push(fact);
    }
    assert_eq!(loop_sources, vec![bir::OwnershipFact::Move, bir::OwnershipFact::Borrow]);
    Ok(())
}

/// Owned list expressions move into iteration even when the checker has no named-list consumption record.
#[test]
fn temporary_lists_keep_consuming_iteration() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def made() -> list[int]:\n  return [1]\ndef main() -> None:\n  for value in made():\n    println(value)\n  for value in [2]:\n    println(value)\n  for value in [n for n in [3]]:\n    println(value)\n";
    let module = build(source, &["m", "temporary_lists"])?;
    let body = body_named(&module, "main")?;
    for statement in &body.block.stmts {
        let bir::StatementKind::Loop { body: loop_body, .. } = &statement.kind else {
            continue;
        };
        let iterator = loop_body
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                bir::StatementKind::IterNext {
                    iterator: bir::Operand::Place(read),
                    ..
                } => read.place.local_id(),
                _ => None,
            })
            .ok_or("loop iterator")?;
        let fact = body
            .block
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                bir::StatementKind::Assign {
                    place,
                    rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
                } if place.local_id() == Some(iterator) => Some(read.fact),
                _ => None,
            })
            .ok_or("iterator materialization")?;
        assert_eq!(fact, bir::OwnershipFact::Move);
    }
    Ok(())
}

/// A comprehension's checked binder never borrows its output type from an outer same-spelled task.
#[test]
fn task_comprehension_storage_respects_its_binder() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import std.async\nfrom std.rust import Send, Static\nfrom rust::incan_std_async::task import JoinHandle, RuntimeFuture, spawn as native_spawn\ndef launch[T with (Send, Static), F with RuntimeFuture[T]](future: F) -> JoinHandle[T]:\n  return native_spawn(future)\nasync def integer() -> int:\n  return 1\nasync def text() -> str:\n  return \"s\"\ndef collect() -> None:\n  handle = launch(integer())\n  items = [launch(text())]\n  result = [handle for handle in items]\n";
    let module = build(source, &["m", "task_comprehension_scope"])?;
    let body = body_named(&module, "collect")?;
    let result = body
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("result"))
        .ok_or("result")?;
    assert_eq!(
        result.ty,
        IncanType::Generic {
            base: "List".into(),
            args: vec![IncanType::Generic {
                base: "JoinHandle".into(),
                args: vec![IncanType::Primitive(IncanPrimitiveType::Str)],
            }]
        }
    );
    let handles: Vec<_> = body
        .locals
        .iter()
        .filter(|local| local.name.as_deref() == Some("handle"))
        .map(|local| &local.ty)
        .collect();
    assert_eq!(
        handles,
        vec![
            &IncanType::Generic {
                base: "JoinHandle".into(),
                args: vec![IncanType::Primitive(IncanPrimitiveType::Int)]
            },
            &IncanType::Generic {
                base: "JoinHandle".into(),
                args: vec![IncanType::Primitive(IncanPrimitiveType::Str)]
            },
        ]
    );
    Ok(())
}

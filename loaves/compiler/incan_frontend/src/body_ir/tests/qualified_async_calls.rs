//! Module-qualified async arguments retain checked future contracts without replacing eventual-output snapshots.

use super::{TypeChecker, body_named, build, lexer, local_types_at, parser};
use crate::symbols::ResolvedType;
use crate::typechecker::TypeCheckInfo;

/// The complete original's module-qualified sleep call closes the spawned Unit-output task carrier.
#[test]
fn complete_module_await_retains_closed_task_carrier() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/emit_calls_and_builtins/await_through_a_module_binding.incn"
    );
    let module = build(source, &["m", "qualified_async_task"])?;
    let body = body_named(&module, "main")?;
    let locals = serde_json::to_string(&body.locals)?;
    assert!(
        !locals.contains("\"Unknown\"") && !locals.contains("\"TypeVar\""),
        "{}",
        body.render_snapshot()
    );
    for call in ["time.sleep(0.01)", "time.sleep_ms(1)"] {
        let types = local_types_at(body, source, call);
        assert!(!types.is_empty(), "missing async call {call}");
        assert!(types.iter().all(|ty| ty == "Awaitable[Unit]"), "{call}: {types:?}");
    }
    Ok(())
}

/// Future arguments are private native instantiations; legacy keeps the async call's eventual Unit expression type.
#[test]
fn qualified_async_argument_preserves_legacy_output_snapshot() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import std.async.time\nimport std.async.prelude\nasync def main() -> None:\n    handle = prelude.spawn(time.sleep_ms(1))\n";
    let facts = checked_facts(source, false)?;
    let inner = call_span(source, "time.sleep_ms(1)")?;
    assert_eq!(facts.expr_type(inner), Some(&ResolvedType::Unit));
    let outer = call_span(source, "prelude.spawn(time.sleep_ms(1))")?;
    let arguments = facts
        .calls
        .body_ir_call_type_arguments
        .get(&(outer.start, outer.end))
        .ok_or("missing closed spawn arguments")?;
    assert!(arguments.iter().any(|argument| matches!(argument, ResolvedType::Generic(name, outputs) if name == "Awaitable" && outputs == &[ResolvedType::Unit])), "{arguments:?}");
    Ok(())
}

/// A synchronous source function named sleep_ms cannot authorize a future instantiation.
#[test]
fn synchronous_sleep_spelling_does_not_grant_future_argument() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import std.async\nfrom std.async.task import spawn\ndef sleep_ms(value: int) -> None:\n    pass\nasync def main() -> None:\n    handle = spawn(sleep_ms(1))\n";
    let facts = checked_facts(source, true)?;
    let span = call_span(source, "spawn(sleep_ms(1))")?;
    assert!(
        facts
            .calls
            .body_ir_call_type_arguments
            .get(&(span.start, span.end))
            .is_none_or(|arguments| !arguments
                .iter()
                .any(|argument| matches!(argument, ResolvedType::Generic(name, _) if name == "Awaitable")))
    );
    Ok(())
}

/// A module wrapper's failed arity gate cannot authorize its nested argument as a future.
#[test]
fn invalid_module_async_call_does_not_close_spawn() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import std.async.time\nimport std.async.prelude\nasync def main() -> None:\n    handle = prelude.spawn(time.sleep_ms(1, 2))\n";
    let facts = checked_facts(source, true)?;
    let span = call_span(source, "prelude.spawn(time.sleep_ms(1, 2))")?;
    assert!(
        !facts
            .calls
            .body_ir_call_type_arguments
            .contains_key(&(span.start, span.end))
    );
    Ok(())
}

/// Synthetic protocol resolution takes, clears and restores the selected async target independently of legacy types.
#[test]
fn checked_async_call_fact_survives_call_site_transactions() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import std.async.time\nasync def main() -> None:\n    await time.sleep_ms(1)\n";
    let mut facts = checked_facts(source, false)?;
    let span = call_span(source, "time.sleep_ms(1)")?;
    let key = (span.start, span.end);
    let target = facts
        .calls
        .body_ir_async_call_targets
        .get(&key)
        .ok_or("missing checked async target")?
        .clone();
    assert_eq!(facts.resolved_identity(span), Some(&target));
    let legacy = facts.expressions.expr_types.clone();
    let saved = facts.take_call_site_facts(span);
    assert!(!facts.calls.body_ir_async_call_targets.contains_key(&key));
    facts.calls.body_ir_async_call_targets.insert(key, target.clone());
    facts.clear_body_ir_call_site_facts(span);
    assert!(!facts.calls.body_ir_async_call_targets.contains_key(&key));
    facts.restore_call_site_facts(span, saved);
    assert_eq!(facts.calls.body_ir_async_call_targets.get(&key), Some(&target));
    assert_eq!(facts.expressions.expr_types, legacy);
    Ok(())
}

/// Failed module arity and argument checks leave no async target, including the wrapper's earlier diagnostics.
#[test]
fn failed_module_call_validation_does_not_publish_async_target() -> Result<(), Box<dyn std::error::Error>> {
    for arguments in ["1, 2", "\"bad\""] {
        let source =
            format!("import std.async.time\nasync def main() -> None:\n    await time.sleep_ms({arguments})\n");
        let facts = checked_facts(&source, true)?;
        assert!(facts.calls.body_ir_async_call_targets.is_empty(), "{arguments}");
    }
    Ok(())
}

/// Check source normally, or require diagnostics for a deliberately invalid guard case.
fn checked_facts(source: &str, invalid: bool) -> Result<TypeCheckInfo, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["m".into(), "qualified_async_facts".into()]));
    let checked = checker.check_program(&program);
    if invalid {
        assert!(checked.is_err(), "invalid call should be diagnosed");
    } else {
        checked.map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    }
    Ok(checker.type_info().clone())
}

/// Select the exact source call span, including its qualified callee and complete argument list.
fn call_span(source: &str, text: &str) -> Result<crate::ast::Span, Box<dyn std::error::Error>> {
    let start = source.find(text).ok_or("missing written call")?;
    Ok(crate::ast::Span::new(start, start + text.len()))
}

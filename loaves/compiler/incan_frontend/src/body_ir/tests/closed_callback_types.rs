//! A callback's checked return contract closes its intrinsic constructor storage without changing legacy facts.

use super::{body_named, build, local_types_at};

/// The receiver fixes the error channel of an and_then callback's Ok constructor.
#[test]
fn and_then_constructor_retains_checked_error_channel() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def passed() -> Result[int, str]:\n    return Ok(2)\n\ndef main() -> None:\n    result = passed().and_then((x) => Ok(x * 2))\n    println(result.unwrap_or(0))\n";
    let module = build(source, &["m", "closed_and_then"])?;
    let body = body_named(&module, "main")?;
    assert_eq!(local_types_at(body, source, "Ok(x * 2)"), ["Result[int, str]"]);
    Ok(())
}

/// A closure's settled Unit channel is retained on an or_else callback's Ok constructor.
#[test]
fn or_else_constructor_retains_settled_error_channel() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def failed() -> Result[int, str]:\n    return Err(\"x\")\n\ndef main() -> None:\n    result = failed().or_else((message) => Ok(len(message)))\n    println(result.unwrap_or(0))\n";
    let module = build(source, &["m", "closed_or_else"])?;
    let body = body_named(&module, "main")?;
    assert_eq!(local_types_at(body, source, "Ok(len(message))"), ["Result[int, Unit]"]);
    Ok(())
}

/// A generic callback retains the enclosing declaration's own error parameter, rather than an unknown channel.
#[test]
fn generic_callback_retains_own_error_parameter() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "def forward[T, E](value: Result[T, E]) -> Result[int, E]:\n    return value.and_then((item) => Ok(1))\n";
    let module = build(source, &["m", "closed_generic_callback"])?;
    let body = body_named(&module, "forward")?;
    assert_eq!(local_types_at(body, source, "Ok(1)"), ["Result[int, E]"]);
    Ok(())
}

/// Constructor return types stay closed through chained combinators and inferred locals in the original program.
#[test]
fn complete_result_constructor_callbacks_retain_closed_locals() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/emit_calls_and_builtins/result_combinator_closures_return_constructors.incn"
    );
    let module = build(source, &["m", "complete_callback_constructors"])?;
    let main = body_named(&module, "main")?;
    assert!(
        !serde_json::to_string(&main.locals)?.contains("\"Unknown\""),
        "{}",
        main.render_snapshot()
    );
    Ok(())
}

/// Captured callbacks preserve their checked Result channels through unwrap_or and inspection calls.
#[test]
fn complete_captured_callbacks_retain_closed_locals() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/cli_values_and_calls/capturing_closures_as_result_combinator_callbacks.incn"
    );
    let module = build(source, &["m", "complete_captured_callbacks"])?;
    let main = body_named(&module, "main")?;
    assert!(
        !serde_json::to_string(&main.locals)?.contains("\"Unknown\""),
        "{}",
        main.render_snapshot()
    );
    Ok(())
}

/// Reusing a Result preserves both the receiver and the callback's own result storage in the complete fixture.
#[test]
fn complete_reused_result_callbacks_retain_closed_locals() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/ownership_parameters_and_receivers/result_methods_leave_their_receiver_usable.incn"
    );
    let module = build(source, &["m", "complete_reused_result_callbacks"])?;
    let body = body_named(&module, "twice")?;
    assert!(
        !serde_json::to_string(&body.locals)?.contains("\"Unknown\""),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}

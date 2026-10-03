//! Async tasks and `race`: lifecycle evidence, source-order ties with loser cancellation, failure propagation
//! at the child statement span, and the CLI-driven `std.async` activation with its task receipts and refusals.

use super::*;

/// An async declaration constructs a direct task even when its child body has no suspension of its own.
#[test]
fn replacement_executes_a_source_local_async_task_and_binds_its_lifecycle_evidence()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
import std.async

async def child() -> int:
  return 42

async def main() -> int:
  return await child()
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(42));
    let lifecycle = execution.task_lifecycle_evidence();
    assert!(
        lifecycle.iter().filter(|event| event.event == "constructed").count() == 2,
        "the root and child must each construct a direct task frame: {lifecycle:?}"
    );
    assert!(
        lifecycle.iter().any(|event| event.event == "await_suspended")
            && lifecycle.iter().any(|event| event.event == "await_resumed"),
        "the parent task must retain an explicit await suspension and resume: {lifecycle:?}"
    );
    assert!(
        lifecycle.iter().filter(|event| event.event == "completed").count() == 2,
        "both task frames must complete through direct Body IR: {lifecycle:?}"
    );
    assert!(
        execution
            .runtime_requirement_evidence()
            .iter()
            .any(|requirement| requirement.requirement == "async_runtime"),
        "an async body without an explicit child suspension still needs direct async runtime evidence"
    );
    Ok(())
}

/// Constructing and dropping a same-module task records construction without polling the child's body.
#[test]
fn replacement_records_construct_only_task_lifecycle_evidence() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
import std.async

async def child() -> int:
  assert false
  return 7

def main() -> int:
  child()
  return 1
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(1));
    let lifecycle = execution.task_lifecycle_evidence();
    assert_eq!(
        lifecycle.len(),
        1,
        "only the unpolled child task should be observed: {lifecycle:?}"
    );
    let event = lifecycle
        .first()
        .ok_or("construction-only task must record its construction")?;
    assert_eq!(event.event, "constructed");
    Ok(())
}

/// A ready race constructs every arm, then polls and selects the first source-order arm while cancelling later arms
/// without running their bodies.
#[test]
fn replacement_executes_source_order_async_race_ties_with_loser_cancellation() -> Result<(), Box<dyn std::error::Error>>
{
    let source = r#"
import std.async

async def first() -> int:
  return 1

async def second() -> int:
  return 2

async def main() -> int:
  winner = race for value:
    await first() => value
    await second() =>
      assert value == 999
      value
  return winner
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(1));
    let lifecycle = execution.task_lifecycle_evidence();
    let winner = lifecycle
        .iter()
        .position(|event| event.event == "race_winner")
        .ok_or("race must retain its winning task transition")?;
    let last_constructed = lifecycle
        .iter()
        .rposition(|event| event.event == "constructed")
        .ok_or("race must construct its arm tasks")?;
    assert!(
        last_constructed < winner,
        "every race arm must be constructed before ready-tie selection: {lifecycle:?}"
    );
    assert!(
        lifecycle.iter().any(|event| event.event == "cancelled"),
        "the non-winning constructed task must be cancelled at the race boundary: {lifecycle:?}"
    );
    Ok(())
}

/// Reversing simultaneously ready source arms reverses the selected result without polling the cancelled loser.
#[test]
fn replacement_race_ready_ties_follow_source_order_and_cancel_unpolled_losers() -> Result<(), Box<dyn std::error::Error>>
{
    let source = r#"
import std.async

async def first() -> int:
  return 1

async def second() -> int:
  return 2

async def main() -> int:
  winner = race for value:
    await second() => value
    await first() => value
  return winner
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(2));
    let lifecycle = execution.task_lifecycle_evidence();
    let first_winner = lifecycle
        .iter()
        .position(|event| event.event == "race_winner")
        .ok_or("race must retain its winning task transition")?;
    let polls_before_winner = lifecycle[..first_winner]
        .iter()
        .filter(|event| event.event == "polled")
        .count();
    assert_eq!(
        polls_before_winner, 2,
        "the root and source-order winner must poll before selection, while the loser stays unpolled: {lifecycle:?}"
    );
    assert!(
        lifecycle.iter().any(|event| event.event == "cancelled"),
        "the unpolled later arm must be cancelled at the winner boundary: {lifecycle:?}"
    );
    Ok(())
}

/// A failing first race arm retains its source failure while later constructed arms stay unpolled and are cancelled.
#[test]
fn replacement_race_failure_preserves_the_first_arm_span_and_cancels_constructed_losers()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
import std.async

async def first() -> int:
  assert false
  return 1

async def loser() -> int:
  assert false
  return 2

async def main() -> int:
  winner = race for value:
    await first() => value
    await loser() => value
  return winner
"#;
    let module = lower_typed_body_ir(source)?;
    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!("failing first race arm executed as {:?}", execution.value).into());
        }
        Err(error) => error,
    };
    let expected_start = source
        .find("assert false")
        .ok_or("fixture must contain the first arm assertion")?;
    let span = error
        .primary_span()
        .ok_or("race winner failure must retain its original source span")?;
    assert_eq!(span.start, expected_start);
    assert!(
        error.to_string().contains("assertion failed"),
        "the first task failure must remain the direct runtime failure: {error}"
    );
    Ok(())
}

/// A failure in an admitted child task stays a direct runtime failure at its original source statement.
#[test]
fn replacement_propagates_async_task_failure_at_the_child_statement_span() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
import std.async

async def child() -> int:
  assert false
  return 7

async def main() -> int:
  return await child()
"#;
    let module = lower_typed_body_ir(source)?;
    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!("failing async child executed as {:?}", execution.value).into());
        }
        Err(error) => error,
    };
    let expected_start = source
        .find("assert false")
        .ok_or("fixture must contain the child assertion")?;
    let span = error
        .primary_span()
        .ok_or("async child failure must retain its original source span")?;
    assert_eq!(span.start, expected_start);
    assert!(
        error.to_string().contains("assertion failed"),
        "child task failure must stay a direct runtime failure: {error}"
    );
    Ok(())
}

/// The only admitted import is `std.async` activation; its direct execution projects receipt-bound task evidence
/// without creating generated Rust or claiming a source-observable comparison.
#[test]
fn replacement_cli_executes_exact_std_async_activation_with_task_receipts() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(
        &entrypoint,
        r#"import std.async

async def child() -> int:
  return 7

async def main() -> int:
  return await child()
"#,
    )?;

    let output = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
            "--report",
            "json",
            "--report-output",
            temporary
                .path()
                .join("replacement-report.json")
                .to_string_lossy()
                .as_ref(),
        ])
        .output()?;
    assert!(
        output.status.success(),
        "direct async replacement build must succeed. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(temporary.path().join("replacement-report.json"))?)?;
    assert_eq!(report["backend"]["executed_backend"], "replacement");
    assert_eq!(report["backend"]["fallback_outcome"], "not_needed");
    assert_eq!(report["replacement_execution"]["result"], "7");
    assert!(
        report["replacement_execution"]["task_lifecycle"].is_array()
            && report["replacement_execution"]["task_lifecycle"]
                .as_array()
                .is_some_and(|events| events.iter().any(|event| event["event"] == "await_resumed")),
        "the direct execution report must project receipt-bound task lifecycle evidence: {report}"
    );
    assert!(
        !temporary.path().join("target/incan").exists(),
        "the direct async profile must not create a generated legacy project"
    );
    let receipt: serde_json::Value = serde_json::from_str(&fs::read_to_string(
        temporary.path().join(".incan/backend/receipt.json"),
    )?)?;
    assert_eq!(receipt["executed_backend"], "replacement");
    assert_eq!(receipt["fallback_outcome"], "not_needed");
    assert_eq!(receipt["shadow_comparison"], "not_requested");
    Ok(())
}

/// A direct child-task failure neither falls back nor writes a success receipt.
#[test]
fn replacement_cli_refuses_async_task_failure_without_a_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    let source = r#"import std.async

async def child() -> int:
  assert false
  return 7

async def main() -> int:
  return await child()
"#;
    fs::write(&entrypoint, source)?;
    let output = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
        ])
        .output()?;
    assert!(
        !output.status.success(),
        "a failing direct child task must not fall back to legacy execution"
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("INCAN-R988-RUNTIME") && combined.contains("assertion failed"),
        "the original direct task failure must remain visible: {combined}"
    );
    let assertion_start = source
        .find("assert false")
        .ok_or("fixture must contain the child assertion")?;
    let assertion_end = assertion_start + "assert false".len();
    assert!(
        combined.contains(&format!(
            "primary Incan source location: {}:{assertion_start}..{assertion_end}",
            entrypoint.display()
        )),
        "the CLI error must retain the child assertion's original source span: {combined}"
    );
    assert!(
        !temporary.path().join("target/incan").exists()
            && !temporary.path().join(".incan/backend/receipt.json").exists(),
        "a failing direct task must not create generated output or a misleading receipt"
    );
    Ok(())
}

/// A failing source-order race winner cannot leave an unpolled loser on a legacy or receipt-producing path.
#[test]
fn replacement_cli_refuses_failing_race_winner_without_a_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    let source = r#"import std.async

async def first() -> int:
  assert false
  return 1

async def loser() -> int:
  assert false
  return 2

async def main() -> int:
  winner = race for value:
    await first() => value
    await loser() => value
  return winner
"#;
    fs::write(&entrypoint, source)?;
    let output = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
        ])
        .output()?;
    assert!(
        !output.status.success(),
        "a failing source-order race winner must not fall back to legacy execution"
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("INCAN-R988-RUNTIME") && combined.contains("assertion failed"),
        "the original direct race winner failure must remain visible: {combined}"
    );
    let assertion_start = source
        .find("assert false")
        .ok_or("fixture must contain the first race-arm assertion")?;
    let assertion_end = assertion_start + "assert false".len();
    assert!(
        combined.contains(&format!(
            "primary Incan source location: {}:{assertion_start}..{assertion_end}",
            entrypoint.display()
        )),
        "the CLI error must retain the first race-arm assertion span: {combined}"
    );
    assert!(
        !temporary.path().join("target/incan").exists()
            && !temporary.path().join(".incan/backend/receipt.json").exists(),
        "a failing race winner must not create generated output or a misleading receipt"
    );
    Ok(())
}

/// Duplicate standard-library activation is a canonical binding error before backend selection begins.
#[test]
fn duplicate_async_activation_fails_during_typechecking() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import std.async\nimport std.async\n\ndef main() -> int:\n  return 42\n";
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("duplicate-async-activation.incn");
    fs::write(&entrypoint, source)?;
    let output = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
        ])
        .output()?;
    assert!(!output.status.success(), "duplicate activation must not compile");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("Duplicate definition of 'async'")
            && combined.contains("First canonical identity:")
            && combined.contains("Second canonical identity:"),
        "duplicate activation must report its conflicting canonical bindings: {combined}"
    );
    assert!(
        !combined.contains("INCAN-R988-UNSUPPORTED"),
        "backend refusal must not replace the earlier type error: {combined}"
    );
    assert!(
        !temporary.path().join("target/incan").exists()
            && !temporary.path().join(".incan/backend/receipt.json").exists(),
        "a compile-time binding error must not create backend output or a receipt"
    );
    Ok(())
}

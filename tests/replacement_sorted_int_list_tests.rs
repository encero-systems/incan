//! Direct execution coverage for canonical `sorted` over one nonempty represented integer list.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use incan::backend::replacement::{ReplacementExecutionError, ReplacementValue, execute_free_function};
use incan::frontend::body_ir::build_body_ir_module_v0;
use incan::frontend::{lexer, parser, typechecker::TypeChecker};
use incan_core::lang::builtins::BuiltinFnId;
use incan_semantics_core::body_ir::{BodyIrModule, CallableTarget, Callee, StatementKind};

const SORTED_INT_SOURCE: &str = include_str!("fixtures/replacement/sorted_int_list.incn");

/// Lower checked source through the retained semantic facts normal replacement execution consumes.
fn lower(source: &str) -> Result<BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    let path = vec!["sorted_int_list".to_string()];
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    Ok(build_body_ir_module_v0(&program, &path, checker.type_info()))
}

/// Count direct calls that retain the compiler-owned canonical `Sorted` identity.
fn canonical_sorted_call_count(module: &BodyIrModule, body_name: &str) -> Result<usize, Box<dyn std::error::Error>> {
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == body_name)
        .ok_or_else(|| format!("missing `{body_name}` body"))?;
    Ok(body
        .block
        .stmts
        .iter()
        .filter(|statement| {
            matches!(
                &statement.kind,
                StatementKind::Call {
                    callee: Callee::Function(CallableTarget::Named(target)),
                    ..
                } if target.builtin == Some(BuiltinFnId::Sorted)
            )
        })
        .count())
}

/// Ascending order preserves duplicates and leaves the source list in its original order.
#[test]
fn canonical_sorted_returns_a_fresh_ordered_integer_list() -> Result<(), Box<dyn std::error::Error>> {
    let module = lower(SORTED_INT_SOURCE)?;
    assert_eq!(canonical_sorted_call_count(&module, "sorted_int_list")?, 1);

    let execution = execute_free_function(&module, "sorted_int_list", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(29_320_233));
    assert_eq!(execution.output.stdout(), b"sorted int list\n");
    assert!(execution.output.stderr().is_empty());
    Ok(())
}

/// The source operand runs once before canonical sorting consumes its returned list.
#[test]
fn canonical_sorted_evaluates_its_operand_once() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"def marked() -> list[int]:
  println("sorted operand")
  return [2, 1]

def observe() -> int:
  ordered = sorted(marked())
  mut score = 0
  for value in ordered:
    score = score * 10 + value
  return score
"#;
    let module = lower(source)?;
    assert_eq!(canonical_sorted_call_count(&module, "observe")?, 1);

    let execution = execute_free_function(&module, "observe", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(12));
    assert_eq!(execution.output.stdout(), b"sorted operand\n");
    assert!(execution.output.stderr().is_empty());
    Ok(())
}

/// A source declaration named `sorted` remains an ordinary direct call rather than borrowing builtin ordering.
#[test]
fn source_local_sorted_keeps_its_declaration_identity() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"def sorted(value: int) -> int:
  return value + 1

def observe() -> int:
  return sorted(41)
"#;
    let module = lower(source)?;
    assert_eq!(canonical_sorted_call_count(&module, "observe")?, 0);
    assert_eq!(
        execute_free_function(&module, "observe", &[])?.value,
        ReplacementValue::Int(42)
    );
    Ok(())
}

/// Removing the retained builtin identity must not be repaired from the source spelling `sorted`.
#[test]
fn missing_sorted_identity_refuses_instead_of_guessing_from_its_name() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def observe() -> int:\n  return len(sorted([2, 1]))\n";
    let mut module = lower(source)?;
    let body = module
        .bodies
        .iter_mut()
        .find(|body| body.name == "observe")
        .ok_or("missing observe body")?;
    let target = body
        .block
        .stmts
        .iter_mut()
        .find_map(|statement| match &mut statement.kind {
            StatementKind::Call {
                callee: Callee::Function(CallableTarget::Named(target)),
                ..
            } if target.builtin == Some(BuiltinFnId::Sorted) => Some(target),
            _ => None,
        })
        .ok_or("missing canonical sorted target")?;
    target.builtin = None;

    let error = execute_free_function(&module, "observe", &[])
        .err()
        .ok_or("a missing builtin identity must refuse")?;
    assert!(
        matches!(error, ReplacementExecutionError::Unsupported { .. }),
        "{error}"
    );
    let span = error
        .primary_span()
        .ok_or("identity refusal needs its original source span")?;
    assert_eq!(source.get(span.start..span.end), Some("sorted([2, 1])"));
    Ok(())
}

/// Empty lists lack a runtime element-type witness in this packet and refuse at the canonical call.
#[test]
fn empty_sorted_list_remains_an_original_call_span_refusal() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"def observe() -> int:
  values: list[int] = []
  return len(sorted(values))
"#;
    let module = lower(source)?;
    let error = execute_free_function(&module, "observe", &[])
        .err()
        .ok_or("empty sorted list must remain outside this bounded profile")?;
    assert!(
        matches!(error, ReplacementExecutionError::Unsupported { .. }),
        "{error}"
    );
    let span = error
        .primary_span()
        .ok_or("empty-list refusal needs its original source span")?;
    assert_eq!(source.get(span.start..span.end), Some("sorted(values)"));
    Ok(())
}

/// String ordering remains outside this integer-only packet and refuses at the canonical call.
#[test]
fn string_sorted_list_remains_an_original_call_span_refusal() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def observe() -> int:\n  return len(sorted([\"b\", \"a\"]))\n";
    let module = lower(source)?;
    let error = execute_free_function(&module, "observe", &[])
        .err()
        .ok_or("string sorted list must remain outside this bounded profile")?;
    assert!(
        matches!(error, ReplacementExecutionError::Unsupported { .. }),
        "{error}"
    );
    let span = error
        .primary_span()
        .ok_or("string-list refusal needs its original source span")?;
    assert_eq!(source.get(span.start..span.end), Some("sorted([\"b\", \"a\"])"));
    Ok(())
}

/// Ordinary CLI execution publishes direct evidence without materializing a legacy generated project.
#[test]
fn sorted_int_list_cli_publishes_its_result_and_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    fs::write(
        temporary.path().join("main.incn"),
        format!("{SORTED_INT_SOURCE}\n\ndef main() -> int:\n    return sorted_int_list()\n"),
    )?;
    let binary = std::env::var_os("CARGO_BIN_EXE_incan")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_incan")));
    let output = Command::new(binary)
        .current_dir(temporary.path())
        .env("INCAN_HOME", temporary.path().join("incan-home"))
        .args([
            "build",
            "main.incn",
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
            "--report",
            "json",
            "--report-output",
            "report.json",
        ])
        .output()?;

    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(output.stdout, b"sorted int list\n");
    assert!(output.stderr.is_empty());
    let report: serde_json::Value = serde_json::from_slice(&fs::read(temporary.path().join("report.json"))?)?;
    assert_eq!(report["status"], "success");
    assert_eq!(report["backend"]["executed_backend"], "replacement");
    assert_eq!(report["replacement_execution"]["result"], "29320233");
    assert!(temporary.path().join(".incan/backend/receipt.json").is_file());
    assert!(
        !temporary.path().join("target/incan").exists(),
        "direct sorted integer lists must not create a legacy generated project"
    );
    Ok(())
}

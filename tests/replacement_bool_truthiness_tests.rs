//! Direct execution coverage for canonical `bool` over the replacement profile's represented scalar and structural
//! values.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use incan::backend::replacement::{ReplacementExecutionError, ReplacementValue, execute_free_function};
use incan::frontend::body_ir::build_body_ir_module_v0;
use incan::frontend::{lexer, parser, typechecker::TypeChecker};
use incan_core::lang::builtins::BuiltinFnId;
use incan_semantics_core::body_ir::{BodyIrModule, CallableTarget, Callee, StatementKind};

const TRUTHINESS_SOURCE: &str = include_str!("fixtures/replacement/bool_truthiness.incn");

/// Lower checked source through the retained semantic facts normal replacement execution consumes.
fn lower(source: &str) -> Result<BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    let path = vec!["bool_truthiness".to_string()];
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    Ok(build_body_ir_module_v0(&program, &path, checker.type_info()))
}

/// Count direct calls that retain the compiler-owned canonical `Bool` identity.
fn canonical_bool_call_count(module: &BodyIrModule, body_name: &str) -> Result<usize, Box<dyn std::error::Error>> {
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
                } if target.builtin == Some(BuiltinFnId::Bool)
            )
        })
        .count())
}

/// Existing represented scalar and container values use the same empty/nonempty truthiness as native execution.
#[test]
fn canonical_bool_executes_bounded_scalar_and_structural_truthiness() -> Result<(), Box<dyn std::error::Error>> {
    let module = lower(TRUTHINESS_SOURCE)?;
    assert_eq!(canonical_bool_call_count(&module, "bool_truthiness")?, 12);

    let execution = execute_free_function(&module, "bool_truthiness", &[])?;
    assert_eq!(execution.value, ReplacementValue::Bool(true));
    assert_eq!(execution.output.stdout(), b"bool truthiness\n");
    assert!(execution.output.stderr().is_empty());
    Ok(())
}

/// The source operand runs once before canonical truthiness observes its returned value.
#[test]
fn canonical_bool_evaluates_its_operand_once() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"def marked() -> str:
  println("bool operand")
  return ""

def observe() -> bool:
  return bool(marked())
"#;
    let module = lower(source)?;
    assert_eq!(canonical_bool_call_count(&module, "observe")?, 1);

    let execution = execute_free_function(&module, "observe", &[])?;
    assert_eq!(execution.value, ReplacementValue::Bool(false));
    assert_eq!(execution.output.stdout(), b"bool operand\n");
    assert!(execution.output.stderr().is_empty());
    Ok(())
}

/// A source declaration named `bool` remains an ordinary direct call rather than borrowing builtin truthiness.
#[test]
fn source_local_bool_keeps_its_declaration_identity() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"def bool(value: int) -> int:
  return value + 1

def observe() -> int:
  return bool(41)
"#;
    let module = lower(source)?;
    assert_eq!(canonical_bool_call_count(&module, "observe")?, 0);
    assert_eq!(
        execute_free_function(&module, "observe", &[])?.value,
        ReplacementValue::Int(42)
    );
    Ok(())
}

/// Removing the retained builtin identity must not be repaired from the source spelling `bool`.
#[test]
fn missing_bool_identity_refuses_instead_of_guessing_from_its_name() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def observe() -> bool:\n  return bool(1)\n";
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
            } if target.builtin == Some(BuiltinFnId::Bool) => Some(target),
            _ => None,
        })
        .ok_or("missing canonical bool target")?;
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
    assert_eq!(source.get(span.start..span.end), Some("bool(1)"));
    Ok(())
}

/// Float truthiness remains outside this dev.2-based packet and refuses at the original call.
#[test]
fn float_bool_remains_an_original_call_span_refusal() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def observe() -> bool:\n  return bool(0.5)\n";
    let module = lower(source)?;
    let error = execute_free_function(&module, "observe", &[])
        .err()
        .ok_or("float bool must remain outside this bounded profile")?;
    assert!(
        matches!(error, ReplacementExecutionError::Unsupported { .. }),
        "{error}"
    );
    let span = error
        .primary_span()
        .ok_or("bool refusal needs its original source span")?;
    assert_eq!(source.get(span.start..span.end), Some("bool(0.5)"));
    Ok(())
}

/// Ordinary CLI execution publishes direct evidence without materializing a legacy generated project.
#[test]
fn bool_truthiness_cli_publishes_its_result_and_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    fs::write(
        temporary.path().join("main.incn"),
        format!("{TRUTHINESS_SOURCE}\n\ndef main() -> bool:\n    return bool_truthiness()\n"),
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
    assert_eq!(output.stdout, b"bool truthiness\n");
    assert!(output.stderr.is_empty());
    let report: serde_json::Value = serde_json::from_slice(&fs::read(temporary.path().join("report.json"))?)?;
    assert_eq!(report["status"], "success");
    assert_eq!(report["backend"]["executed_backend"], "replacement");
    assert_eq!(report["replacement_execution"]["result"], "true");
    assert!(temporary.path().join(".incan/backend/receipt.json").is_file());
    assert!(
        !temporary.path().join("target/incan").exists(),
        "direct bool truthiness must not create a legacy generated project"
    );
    Ok(())
}

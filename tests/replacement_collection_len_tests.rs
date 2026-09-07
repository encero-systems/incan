//! Direct execution coverage for bounded `len` over the replacement profile's immutable hashed carriers.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use incan::backend::replacement::{ReplacementExecutionError, ReplacementValue, execute_free_function};
use incan::frontend::body_ir::build_body_ir_module_v0;
use incan::frontend::{lexer, parser, typechecker::TypeChecker};
use incan_core::lang::builtins::BuiltinFnId;
use incan_semantics_core::body_ir::{BodyIrModule, CallableTarget, Callee, StatementKind};

/// Lower checked source through the same retained facts consumed by normal replacement execution.
fn lower(source: &str) -> Result<BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    let path = vec!["collection_len".to_string()];
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    Ok(build_body_ir_module_v0(&program, &path, checker.type_info()))
}

/// Count the named function calls carrying the compiler-owned `Len` identity.
fn canonical_len_call_count(module: &BodyIrModule, body_name: &str) -> Result<usize, Box<dyn std::error::Error>> {
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
                } if target.builtin == Some(BuiltinFnId::Len)
            )
        })
        .count())
}

/// Populated and typed-empty hashed containers return entry counts, after duplicate collapse.
#[test]
fn set_and_dict_len_execute_from_canonical_builtin_identity() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"def observe() -> int:
  values = {1, 1, 2}
  mapping = {"a": 1, "a": 2, "b": 3}
  empty_values: set[int] = Set()
  empty_mapping: dict[str, int] = Dict()
  return len(values) * 1000 + len(mapping) * 100 + len(empty_values) * 10 + len(empty_mapping)
"#;
    let module = lower(source)?;

    assert_eq!(canonical_len_call_count(&module, "observe")?, 4);
    assert_eq!(
        execute_free_function(&module, "observe", &[])?.value,
        ReplacementValue::Int(2200)
    );
    Ok(())
}

/// Constructing the operand happens once and preserves written program output before the count is returned.
#[test]
fn collection_len_evaluates_its_operand_once() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"def mark(value: int) -> int:
  println("len element")
  return value

def observe() -> int:
  return len({mark(1), mark(2)})
"#;
    let module = lower(source)?;
    assert_eq!(canonical_len_call_count(&module, "observe")?, 1);

    let execution = execute_free_function(&module, "observe", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(2));
    assert_eq!(execution.output.stdout(), b"len element\nlen element\n");
    assert!(execution.output.stderr().is_empty());
    Ok(())
}

/// This packet must not turn arbitrary values into length-bearing containers.
#[test]
fn non_collection_len_remains_an_original_call_span_refusal() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def observe() -> int:\n  return len(1)\n";
    let module = lower(source)?;
    let error = execute_free_function(&module, "observe", &[])
        .err()
        .ok_or("integer len must remain outside this profile")?;
    assert!(
        matches!(error, ReplacementExecutionError::Unsupported { .. }),
        "{error}"
    );
    let span = error
        .primary_span()
        .ok_or("len refusal needs its original source span")?;
    assert_eq!(source.get(span.start..span.end), Some("len(1)"));
    Ok(())
}

/// Ordinary CLI execution keeps program streams separate and publishes direct replacement evidence.
#[test]
fn collection_len_cli_publishes_its_result_and_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    fs::write(
        temporary.path().join("main.incn"),
        r#"def main() -> int:
  values = {1, 1, 2}
  mapping = {"a": 1, "a": 2, "b": 3}
  empty_values: set[int] = Set()
  empty_mapping: dict[str, int] = Dict()
  println("collection len")
  return len(values) * 1000 + len(mapping) * 100 + len(empty_values) * 10 + len(empty_mapping)
"#,
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
    assert_eq!(output.stdout, b"collection len\n");
    assert!(output.stderr.is_empty());
    let report: serde_json::Value = serde_json::from_slice(&fs::read(temporary.path().join("report.json"))?)?;
    assert_eq!(report["status"], "success");
    assert_eq!(report["backend"]["executed_backend"], "replacement");
    assert_eq!(report["replacement_execution"]["result"], "2200");
    assert!(temporary.path().join(".incan/backend/receipt.json").is_file());
    assert!(
        !temporary.path().join("target/incan").exists(),
        "direct collection length must not create a legacy generated project"
    );
    Ok(())
}

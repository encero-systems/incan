//! RED-first coverage for compiler-selected scalar conversion builtins (#1249).

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use incan::backend::replacement::{
    ProgramIo, ReplacementExecution, ReplacementExecutionError, ReplacementValue, execute_free_function,
    execute_free_function_with_io,
};
use incan::frontend::body_ir::build_body_ir_module_v0;
use incan::frontend::typechecker::TypeChecker;
use incan::frontend::{lexer, parser};
use incan_core::lang::builtins::BuiltinFnId;
use incan_semantics_core::body_ir::{
    ArgumentElement, Body, BodyIrModule, CallableTarget, Callee, Constant, NamedCallableTarget, Operand, StatementKind,
};

/// Lower one source module through the same checked Body-IR path the replacement executor consumes.
fn lower_typed_body_ir(source: &str) -> Result<BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let module_path = vec!["replacement_scalar_conversions".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// Locate the compiler binary Cargo built for this integration-test invocation.
fn incan_binary() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_incan")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_incan")))
}

/// Execute a direct replacement CLI build in an isolated project and runtime home.
fn replacement_command(directory: &std::path::Path) -> Command {
    let mut command = Command::new(incan_binary());
    command
        .current_dir(directory)
        .env("INCAN_HOME", directory.join("incan-home"))
        .args([
            "build",
            "main.incn",
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
        ]);
    command
}

/// Require one direct execution to refuse without using panic-oriented test helpers.
fn require_execution_error(
    result: Result<ReplacementExecution, ReplacementExecutionError>,
    context: &str,
) -> Result<ReplacementExecutionError, Box<dyn std::error::Error>> {
    match result {
        Err(error) => Ok(error),
        Ok(execution) => Err(format!("{context}; unexpectedly completed with {:?}", execution.value).into()),
    }
}

/// Find one named Body-IR body by its source declaration name.
fn body_named<'module>(module: &'module BodyIrModule, name: &str) -> Result<&'module Body, Box<dyn std::error::Error>> {
    module
        .bodies
        .iter()
        .find(|body| body.name == name)
        .ok_or_else(|| std::io::Error::other(format!("expected Body IR for `{name}`")))
        .map_err(Into::into)
}

/// Return the operand carried by one compiler-selected scalar builtin call in a named body.
fn selected_builtin_argument<'module>(
    module: &'module BodyIrModule,
    body_name: &str,
    builtin: BuiltinFnId,
) -> Result<(&'module NamedCallableTarget, &'module Operand), Box<dyn std::error::Error>> {
    let body = body_named(module, body_name)?;
    let (target, args) = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            StatementKind::Call {
                callee: Callee::Function(CallableTarget::Named(target)),
                args,
                ..
            } if target.builtin == Some(builtin) => Some((target, args.as_slice())),
            _ => None,
        })
        .ok_or_else(|| std::io::Error::other(format!("expected selected `{builtin:?}` call in `{body_name}`")))?;
    let [ArgumentElement::One(operand)] = args else {
        return Err(std::io::Error::other(format!(
            "selected `{builtin:?}` call in `{body_name}` must retain exactly one ordinary operand"
        ))
        .into());
    };
    Ok((target, operand))
}

/// Resolve a selected builtin's bare-local operand back to the compiler-owned Body-IR local type.
fn selected_builtin_argument_type(
    module: &BodyIrModule,
    body_name: &str,
    builtin: BuiltinFnId,
) -> Result<String, Box<dyn std::error::Error>> {
    let (target, operand) = selected_builtin_argument(module, body_name, builtin)?;
    assert!(
        target.direct_call_id.is_none(),
        "a selected builtin cannot carry a source direct-call identity"
    );
    let Operand::Place(read) = operand else {
        return Err(std::io::Error::other(format!(
            "selected `{builtin:?}` call in `{body_name}` must retain a local operand"
        ))
        .into());
    };
    assert!(
        read.place.projection.is_empty(),
        "the source probe must stay a bare local read"
    );
    let local_id = read
        .place
        .local_id()
        .ok_or_else(|| std::io::Error::other("selected builtin argument must use local storage"))?;
    let body = body_named(module, body_name)?;
    body.locals
        .iter()
        .find(|local| local.id == local_id)
        .map(|local| local.ty.to_string())
        .ok_or_else(|| std::io::Error::other("selected builtin argument local is missing its declared type"))
        .map_err(Into::into)
}

/// Execute only compiler-selected unary `Str`, `Int`, and `Float` calls through Body IR.
#[test]
fn replacement_executes_checked_unary_scalar_conversions() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> str:
  parsed_int = int("1_000")
  bool_int = int(true)
  parsed_float = float("1_000.50")
  widened_float = float(10)
  truncated_float = int(3.9)
  return f"{str(parsed_int)} {bool_int} {parsed_float} {widened_float} {truncated_float}"
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;

    assert_eq!(execution.value, ReplacementValue::Str("1000 1 1000.5 10 3".to_string()));
    assert!(execution.emitted_output().is_empty());
    Ok(())
}

/// Parse failures remain canonical runtime failures at the selected conversion call, not generic refusals.
#[test]
fn replacement_reports_canonical_scalar_parse_failures_at_original_call_spans() -> Result<(), Box<dyn std::error::Error>>
{
    let source = r#"
def parse_int() -> int:
  return int("AssertionError overflow division by zero")

def parse_float() -> float:
  return float("AssertionError overflow division by zero")

def parse_int_with_invalid_separators() -> int:
  return int("1__000")

def parse_float_with_invalid_separators() -> float:
  return float("1_000._50")
"#;
    let module = lower_typed_body_ir(source)?;
    for (body_name, call, expected_detail) in [
        (
            "parse_int",
            "int(\"AssertionError overflow division by zero\")",
            "ValueError: cannot convert 'AssertionError overflow division by zero' to int",
        ),
        (
            "parse_float",
            "float(\"AssertionError overflow division by zero\")",
            "ValueError: cannot convert 'AssertionError overflow division by zero' to float",
        ),
        (
            "parse_int_with_invalid_separators",
            "int(\"1__000\")",
            "ValueError: cannot convert '1__000' to int",
        ),
        (
            "parse_float_with_invalid_separators",
            "float(\"1_000._50\")",
            "ValueError: cannot convert '1_000._50' to float",
        ),
    ] {
        let error = match execute_free_function(&module, body_name, &[]) {
            Ok(execution) => {
                return Err(format!("{body_name} unexpectedly completed with {:?}", execution.value).into());
            }
            Err(error) => error,
        };
        let expected_start = source
            .find(call)
            .ok_or("conversion fixture must retain its call spelling")?;
        match error {
            ReplacementExecutionError::RuntimeFailure { detail, span, .. } => {
                assert_eq!(detail, expected_detail);
                assert_eq!(span.start, expected_start);
                assert_eq!(span.end, expected_start + call.len());
            }
            other => {
                return Err(
                    format!("{body_name} must report its canonical conversion failure instead of {other}").into(),
                );
            }
        }
    }
    Ok(())
}

/// Source float literals and runtime strings share the language's valid underscore-separator behavior.
#[test]
fn replacement_normalizes_ordinary_float_literal_and_runtime_string_spelling() -> Result<(), Box<dyn std::error::Error>>
{
    let source = r#"
def main() -> str:
  return f"{str(1_000.50)} {str(1.25e2)} {float('1_000.50')} {float('1.25e1_0')}"
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;
    assert_eq!(
        execution.value,
        ReplacementValue::Str("1000.5 125 1000.5 12500000000".to_string())
    );
    Ok(())
}

/// Keep compiler-distinguished numeric locals outside every selected scalar-conversion builtin.
#[test]
fn replacement_executes_checked_typed_numeric_locals_through_selected_scalar_conversions()
-> Result<(), Box<dyn std::error::Error>> {
    for (case_name, source, expected) in [
        (
            "str f32 local",
            r#"
def main() -> str:
  value: f32 = 1.23456789
  return str(value)
"#,
            ReplacementValue::Str(1.234_567_9_f32.to_string()),
        ),
        (
            "int f64 local",
            r#"
def main() -> int:
  value: f64 = 1.23456789
  return int(value)
"#,
            ReplacementValue::Int(1),
        ),
        (
            "float sized integer local",
            r#"
def main() -> float:
  value: u128 = 10
  return float(value)
"#,
            ReplacementValue::Float(10.0),
        ),
        (
            "str f32 closure",
            r#"
def main() -> str:
  value: f32 = 1.23456789
  render: () -> str = () => str(value)
  return render()
"#,
            ReplacementValue::Str(1.234_567_9_f32.to_string()),
        ),
    ] {
        let module = lower_typed_body_ir(source)?;
        let execution = execute_free_function(&module, "main", &[])?;
        assert_eq!(execution.value, expected, "{case_name}");
    }
    Ok(())
}

/// Decimal values retain their fixed decimal carrier and native Display spelling.
#[test]
fn replacement_preserves_decimal_observation_without_entering_binary_float() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> None:
  value: decimal[5, 2] = 19.99d
  println(value)
"#;
    let module = lower_typed_body_ir(source)?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut io = ProgramIo::new(&mut stdout, &mut stderr);
    let execution = execute_free_function_with_io(&module, "main", &[], &mut io)?;
    drop(io);
    assert_eq!(execution.value, ReplacementValue::Unit);
    assert_eq!(stdout, b"19.99\n");
    assert!(stderr.is_empty());
    Ok(())
}

/// Typed numeric Display paths retain their checked carrier's native spelling.
#[test]
fn replacement_preserves_typed_numeric_values_in_print_and_fstring_paths() -> Result<(), Box<dyn std::error::Error>> {
    let rounded = 1.234_567_9_f32.to_string();
    let cases = vec![
        (
            "f32 print",
            r#"
def main() -> None:
  value: f32 = 1.23456789
  println(value)
"#,
            ReplacementValue::Unit,
            format!("{rounded}\n").into_bytes(),
        ),
        (
            "f32 f-string",
            r#"
def main() -> str:
  value: f32 = 1.23456789
  return f"{value}"
"#,
            ReplacementValue::Str(rounded.clone()),
            Vec::new(),
        ),
        (
            "f32 sibling return",
            r#"
def identity(value: f32) -> f32:
  return value

def main() -> str:
  return f"{identity(1.23456789)}"
"#,
            ReplacementValue::Str(rounded.clone()),
            Vec::new(),
        ),
        (
            "f32 closure print",
            r#"
def main() -> None:
  value: f32 = 1.23456789
  render: () -> None = () => println(value)
  render()
"#,
            ReplacementValue::Unit,
            format!("{rounded}\n").into_bytes(),
        ),
        (
            "sized integer string conversion",
            r#"
def main() -> str:
  value: u128 = 10
  return str(value)
"#,
            ReplacementValue::Str("10".to_string()),
            Vec::new(),
        ),
    ];
    for (case_name, source, expected, expected_stdout) in cases {
        let module = lower_typed_body_ir(source)?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut io = ProgramIo::new(&mut stdout, &mut stderr);
        let execution = execute_free_function_with_io(&module, "main", &[], &mut io)?;
        drop(io);
        assert_eq!(execution.value, expected, "{case_name}");
        assert_eq!(stdout, expected_stdout, "{case_name}");
        assert!(stderr.is_empty(), "{case_name}");
    }
    Ok(())
}

/// Ordinary Float display is admitted, but Debug formatting remains outside the parity-backed profile.
#[test]
fn replacement_refuses_float_debug_interpolation() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> str:
  value: float = 1.5
  return f"{value:?}"
"#;
    let module = lower_typed_body_ir(source)?;
    let error = require_execution_error(
        execute_free_function(&module, "main", &[]),
        "ordinary Float Debug is not in the replacement display profile",
    )?;
    assert!(error.to_string().contains("f-string interpolation of float"), "{error}");
    Ok(())
}

/// Negative parsed floats may be converted, but the separate float-negation operation remains unsupported.
#[test]
fn scalar_conversions_do_not_admit_float_negation() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def main() -> int:\n    return int(-3.9)\n";
    let module = lower_typed_body_ir(source)?;
    let error = require_execution_error(
        execute_free_function(&module, "main", &[]),
        "the conversion packet must not silently admit a float arithmetic operation",
    )?;
    let expected_start = source.find("-3.9").ok_or("fixture must retain the negation")?;
    match error {
        ReplacementExecutionError::Unsupported { description, span, .. } => {
            assert_eq!(description, "negation applied to float");
            assert_eq!(span.start, expected_start);
            assert_eq!(span.end, expected_start + "-3.9".len());
        }
        other => return Err(format!("expected the original-span negation refusal, got {other}").into()),
    }
    Ok(())
}

/// Float conversion values do not widen the separate binary-arithmetic dispatcher.
#[test]
fn scalar_conversions_do_not_admit_float_binary_arithmetic() -> Result<(), Box<dyn std::error::Error>> {
    let expression = "float(\"1\") + float(\"2\")";
    let source = format!("def main() -> float:\n    return {expression}\n");
    let module = lower_typed_body_ir(&source)?;
    let error = require_execution_error(
        execute_free_function(&module, "main", &[]),
        "ordinary Float carriers must not silently admit binary arithmetic",
    )?;
    let expected_start = source
        .find(expression)
        .ok_or("fixture must retain the binary expression")?;
    match error {
        ReplacementExecutionError::Unsupported { description, span, .. } => {
            assert_eq!(description, "addition between float and float");
            assert_eq!(span.start, expected_start);
            assert_eq!(span.end, expected_start + expression.len());
        }
        other => return Err(format!("expected the original-span binary arithmetic refusal, got {other}").into()),
    }
    Ok(())
}

/// A source declaration with its own direct-call identity keeps that meaning for every selected spelling.
#[test]
fn replacement_keeps_same_spelled_source_scalar_callables() -> Result<(), Box<dyn std::error::Error>> {
    for (name, source, expected) in [
        (
            "str",
            r#"
def str(value: int) -> str:
  return "source str"

def main() -> str:
  return str(42)
"#,
            ReplacementValue::Str("source str".to_string()),
        ),
        (
            "int",
            r#"
def int(value: str) -> int:
  return 99

def main() -> int:
  return int("not a builtin conversion")
"#,
            ReplacementValue::Int(99),
        ),
        (
            "float",
            r#"
def float(value: int) -> float:
  return 99.0

def main() -> str:
  return str(float(42))
"#,
            ReplacementValue::Str("99".to_string()),
        ),
    ] {
        let module = lower_typed_body_ir(source)?;
        let execution = execute_free_function(&module, "main", &[])?;
        assert_eq!(
            execution.value, expected,
            "source-owned `{name}` must not dispatch as a builtin"
        );
    }
    Ok(())
}

/// A missing checked builtin fact must refuse at the call rather than dispatching from the source spelling.
#[test]
fn replacement_refuses_scalar_conversion_calls_missing_checked_builtin_identity()
-> Result<(), Box<dyn std::error::Error>> {
    for (builtin, source, call) in [
        (
            BuiltinFnId::Str,
            r#"
def main() -> str:
  return str(42)
"#,
            "str(42)",
        ),
        (
            BuiltinFnId::Int,
            r#"
def main() -> int:
  return int("42")
"#,
            "int(\"42\")",
        ),
        (
            BuiltinFnId::Float,
            r#"
def main() -> float:
  return float("42")
"#,
            "float(\"42\")",
        ),
    ] {
        let mut module = lower_typed_body_ir(source)?;
        let expected_start = source.find(call).ok_or("fixture must retain the conversion spelling")?;
        let body = module
            .bodies
            .iter_mut()
            .find(|body| body.name == "main")
            .ok_or("fixture must lower main")?;
        let statement = body
            .block
            .stmts
            .iter_mut()
            .find(|statement| matches!(&statement.kind, StatementKind::Call { .. }))
            .ok_or("fixture must lower the conversion call")?;
        let StatementKind::Call {
            callee: Callee::Function(CallableTarget::Named(target)),
            ..
        } = &mut statement.kind
        else {
            return Err("fixture must lower a named conversion call".into());
        };
        assert_eq!(target.builtin, Some(builtin));
        assert!(target.direct_call_id.is_none());
        target.builtin = None;

        let error = require_execution_error(
            execute_free_function(&module, "main", &[]),
            "a cleared builtin identity must not fall back to source spelling",
        )?;
        match error {
            ReplacementExecutionError::Unsupported { description, span, .. } => {
                assert_eq!(span.start, expected_start, "{builtin:?}");
                assert_eq!(span.end, expected_start + call.len(), "{builtin:?}");
                assert_eq!(
                    description,
                    format!("call to function `{}`", incan_core::lang::builtins::as_str(builtin))
                );
            }
            other => {
                return Err(format!(
                    "a missing {builtin:?} fact must refuse instead of dispatching by spelling: {other}"
                )
                .into());
            }
        }
    }
    Ok(())
}

/// Ordinary Float is a first-class direct entrypoint argument and result without becoming an exact `f64` carrier.
#[test]
fn replacement_preserves_float_direct_arguments_and_results() -> Result<(), Box<dyn std::error::Error>> {
    let argument_source = r#"
def show(value: float) -> str:
  println("argument ran")
  return str(value)
"#;
    let module = lower_typed_body_ir(argument_source)?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut io = ProgramIo::new(&mut stdout, &mut stderr);
    let argument_execution = execute_free_function_with_io(&module, "show", &[ReplacementValue::Float(1.5)], &mut io)?;
    drop(io);
    assert_eq!(argument_execution.value, ReplacementValue::Str("1.5".to_string()));
    assert_eq!(stdout, b"argument ran\n");
    assert!(stderr.is_empty());

    let result_source = r#"
def main() -> float:
  println("before float result")
  return float(10)
"#;
    let module = lower_typed_body_ir(result_source)?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut io = ProgramIo::new(&mut stdout, &mut stderr);
    let result_execution = execute_free_function_with_io(&module, "main", &[], &mut io)?;
    drop(io);
    assert_eq!(result_execution.value, ReplacementValue::Float(10.0));
    assert_eq!(stdout, b"before float result\n");
    assert!(stderr.is_empty());
    Ok(())
}

/// Source-checked Float arguments and unsupplied source defaults are not caller-supplied direct Float transport.
#[test]
fn checked_source_float_parameters_and_defaults_remain_internal_values() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def render(value: float = 1.5) -> str:
    return str(value)

def main() -> str:
    return f"{render(3.14)} {render()}"
"#;
    let module = lower_typed_body_ir(source)?;
    let direct = execute_free_function(&module, "render", &[])?;
    assert_eq!(direct.value, ReplacementValue::Str("1.5".to_string()));
    let source_calls = execute_free_function(&module, "main", &[])?;
    assert_eq!(source_calls.value, ReplacementValue::Str("3.14 1.5".to_string()));
    Ok(())
}

/// A caller cannot bypass a checked Float entrypoint boundary by supplying a different runtime carrier.
#[test]
fn replacement_refuses_nonfloat_carriers_supplied_to_a_float_entrypoint_parameter()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "def show(value: float) -> str:\n    println(\"must not run\")\n    return str(value)\n";
    let module = lower_typed_body_ir(source)?;
    for argument in [ReplacementValue::Int(1), ReplacementValue::Str("1.5".to_string())] {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut io = ProgramIo::new(&mut stdout, &mut stderr);
        let error = require_execution_error(
            execute_free_function_with_io(&module, "show", &[argument], &mut io),
            "a different runtime carrier must not bypass the checked float entrypoint boundary",
        )?;
        drop(io);
        assert!(
            matches!(error, ReplacementExecutionError::Unsupported { .. }),
            "{error}"
        );
        assert!(error.to_string().contains("float"), "{error}");
        assert!(
            error.primary_span().is_some(),
            "the checked parameter supplies its source span"
        );
        assert!(
            stdout.is_empty(),
            "the entrypoint boundary must refuse before source output"
        );
        assert!(stderr.is_empty());
    }
    Ok(())
}

/// Preserve source-checked numeric boundaries before a direct scalar conversion can observe a value.
#[test]
fn checked_body_ir_retains_scalar_conversion_numeric_boundaries() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def ordinary_float(value: float) -> str:
  return str(value)

def f32_local() -> str:
  x: f32 = 1.23456789
  return str(x)

def f64_parameter(value: f64) -> str:
  return str(value)

def real_alias(value: real) -> str:
  return str(value)

def double_alias(value: double) -> str:
  return str(value)

def decimal_value(value: decimal[5, 2]) -> str:
  return str(value)

def f32_identity(value: f32) -> f32:
  return value

def f32_sibling_return() -> str:
  return str(f32_identity(1.23456789))

def ordinary_literal() -> str:
  return str(1.23456789)
"#;
    let module = lower_typed_body_ir(source)?;

    assert_eq!(
        selected_builtin_argument_type(&module, "ordinary_float", BuiltinFnId::Str)?,
        "float"
    );
    assert_eq!(
        selected_builtin_argument_type(&module, "f32_local", BuiltinFnId::Str)?,
        "f32"
    );
    assert_eq!(
        selected_builtin_argument_type(&module, "f64_parameter", BuiltinFnId::Str)?,
        "f64"
    );
    assert_eq!(
        selected_builtin_argument_type(&module, "real_alias", BuiltinFnId::Str)?,
        "f32"
    );
    assert_eq!(
        selected_builtin_argument_type(&module, "double_alias", BuiltinFnId::Str)?,
        "f64"
    );
    assert!(
        selected_builtin_argument_type(&module, "decimal_value", BuiltinFnId::Str)?.starts_with("decimal["),
        "the decimal operand must not be lowered as general float"
    );
    assert_eq!(
        selected_builtin_argument_type(&module, "f32_sibling_return", BuiltinFnId::Str)?,
        "f32"
    );

    let (target, operand) = selected_builtin_argument(&module, "ordinary_literal", BuiltinFnId::Str)?;
    assert!(target.direct_call_id.is_none());
    assert!(
        matches!(operand, Operand::Constant(Constant::Float(value)) if value == "1.23456789"),
        "an uncontextualized source float remains the ordinary binary literal Body IR carries: {operand:?}"
    );
    Ok(())
}

/// Preserve the committed conversion example's normal stdout and successful replacement receipt.
#[test]
fn replacement_cli_executes_unchanged_type_conversions_with_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let example = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/advanced/type_conversions.incn");
    fs::copy(example, temporary.path().join("main.incn"))?;

    let output = replacement_command(temporary.path())
        .args(["--report", "json", "--report-output", "conversion-report.json"])
        .output()?;

    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(
        output.stdout,
        b"String '42' -> int 42\nInt 123 -> string '123'\nString '3.14' -> float 3.14\nInt 10 -> float 10\n10 + 20 = 30\n"
    );
    assert!(
        output.stderr.is_empty(),
        "successful replacement output must not use stderr"
    );
    let receipt_path = temporary.path().join(".incan/backend/receipt.json");
    assert!(
        receipt_path.is_file(),
        "a successful replacement execution must publish its receipt"
    );
    let receipt: serde_json::Value = serde_json::from_slice(&fs::read(receipt_path)?)?;
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(temporary.path().join("conversion-report.json"))?)?;
    assert_eq!(report["status"], "success");
    assert_eq!(report["mode"], "executable");
    assert_eq!(report["backend"]["executed_backend"], "replacement");
    assert_eq!(report["backend"]["selection"]["selected_backend"], "replacement");
    assert_eq!(report["backend"]["fallback_outcome"], "not_needed");
    assert_eq!(report["backend"]["shadow_comparison"], "not_requested");
    assert_eq!(receipt["executed_backend"], "replacement");
    assert_eq!(receipt["selection"]["selected_backend"], "replacement");
    assert_eq!(receipt["fallback_outcome"], "not_needed");
    assert_eq!(receipt["shadow_comparison"], "not_requested");
    assert_eq!(
        report["backend"], receipt,
        "the report must project the persisted receipt verbatim"
    );
    assert_eq!(
        report["replacement_execution"]["output_identity"], receipt["output_identity"],
        "the successful receipt must commit to the direct execution output"
    );
    assert!(
        receipt["output_identity"]
            .as_str()
            .is_some_and(|identity| identity.starts_with("sha256:")),
        "the receipt must carry a content-derived output identity: {receipt}"
    );
    assert_eq!(
        report["replacement_execution"]["emitted_output"],
        serde_json::json!([
            "String '42' -> int 42",
            "Int 123 -> string '123'",
            "String '3.14' -> float 3.14",
            "Int 10 -> float 10",
            "10 + 20 = 30"
        ])
    );
    assert_eq!(
        report["replacement_execution"]["stdout_bytes"],
        serde_json::json!(output.stdout)
    );
    assert_eq!(report["replacement_execution"]["stderr_bytes"], serde_json::json!([]));
    assert!(
        !temporary.path().join("target/incan").exists(),
        "replacement execution must not create a generated legacy artifact"
    );
    Ok(())
}

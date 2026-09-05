//! Replacement-execution evidence for one checked builtin `abs`/`sum` overflow contract.

use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::process::Command;

use incan::backend::replacement::{
    ProgramIo, ReplacementExecutionError, ReplacementValue, execute_free_function_with_io,
};
use incan::frontend::body_ir::build_body_ir_module_v0;
use incan::frontend::typechecker::TypeChecker;
use incan::frontend::{lexer, parser};
use incan_semantics_core::body_ir::BodyIrModule;

const ABS_MIN_SOURCE: &str = "def abs_min(value: int) -> int:\n    println(\"before abs\")\n    return abs(value)\n";
const SUM_OVERFLOW_SOURCE: &str =
    "def overflowing_sum() -> int:\n    println(\"before sum\")\n    return sum([9223372036854775807, 1])\n";
const NESTED_SOURCE: &str = r#"import std.async

def generated(value: int) -> Generator[int]:
    println("before generator abs")
    yield abs(value)

async def child(value: int) -> int:
    println("before task abs")
    return abs(value)

async def through_generator(value: int) -> int:
    values = generated(value).collect()
    return values[0]

async def through_task(value: int) -> int:
    return await child(value)
"#;

/// Lower one self-contained source module without generation or a native process.
fn lower_typed_body_ir(source: &str) -> Result<BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let module_path = vec!["replacement_abs_sum_checked".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// Execute an overflowing builtin without allowing a compiler-host panic to escape the direct runtime boundary.
fn direct_runtime_failure(
    module: &BodyIrModule,
    name: &str,
    args: &[ReplacementValue],
    io: &mut ProgramIo<'_>,
) -> Result<ReplacementExecutionError, Box<dyn std::error::Error>> {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        execute_free_function_with_io(module, name, args, io)
    }));
    let execution = outcome.map_err(|_| {
        std::io::Error::other(format!(
            "builtin overflow in `{name}` panicked the direct executor instead of returning RuntimeFailure"
        ))
    })?;
    match execution {
        Ok(success) => Err(format!(
            "builtin overflow in `{name}` completed successfully with {:?}",
            success.value
        )
        .into()),
        Err(error) => Ok(error),
    }
}

/// Locate the compiler binary Cargo built for this integration-test invocation.
fn incan_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_incan"))
}

/// Both compiler-selected integer builtins fail at their original call span and preserve accepted stdout.
#[test]
fn replacement_abs_and_sum_overflow_are_always_checked() -> Result<(), Box<dyn std::error::Error>> {
    for (source, name, args, call, prefix) in [
        (
            ABS_MIN_SOURCE,
            "abs_min",
            vec![ReplacementValue::Int(i64::MIN)],
            "abs(value)",
            b"before abs\n".as_slice(),
        ),
        (
            SUM_OVERFLOW_SOURCE,
            "overflowing_sum",
            Vec::new(),
            "sum([9223372036854775807, 1])",
            b"before sum\n".as_slice(),
        ),
    ] {
        let module = lower_typed_body_ir(source)?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        {
            let mut io = ProgramIo::new(&mut stdout, &mut stderr);
            let error = direct_runtime_failure(&module, name, &args, &mut io)?;
            let ReplacementExecutionError::RuntimeFailure { detail, span, .. } = error else {
                return Err(format!("expected a typed direct runtime failure, got {error}").into());
            };
            assert!(detail.to_ascii_lowercase().contains("overflow"), "{detail}");
            assert_eq!(source.get(span.start..span.end), Some(call));
            assert_eq!(io.output().stdout(), prefix);
            assert!(io.output().stderr().is_empty());
        }
        assert_eq!(stdout, prefix);
        assert!(stderr.is_empty());
    }
    Ok(())
}

/// Safe values retain their ordinary integer results without any caller-selectable arithmetic mode.
#[test]
fn replacement_abs_and_sum_safe_values_execute_normally() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def observe(value: int) -> int:\n    return abs(value) + sum([1, 2, 3])\n";
    let module = lower_typed_body_ir(source)?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let execution = execute_free_function_with_io(
        &module,
        "observe",
        &[ReplacementValue::Int(-9)],
        &mut ProgramIo::new(&mut stdout, &mut stderr),
    )?;
    assert_eq!(execution.value, ReplacementValue::Int(15));
    assert!(stdout.is_empty() && stderr.is_empty());
    Ok(())
}

/// Nested generator and task frames cannot acquire a different overflow policy from their parent execution.
#[test]
fn replacement_nested_frames_keep_the_checked_contract() -> Result<(), Box<dyn std::error::Error>> {
    let module = lower_typed_body_ir(NESTED_SOURCE)?;
    for (name, call, prefix) in [
        ("through_generator", "abs(value)", b"before generator abs\n".as_slice()),
        ("through_task", "abs(value)", b"before task abs\n".as_slice()),
    ] {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        {
            let mut io = ProgramIo::new(&mut stdout, &mut stderr);
            let error = direct_runtime_failure(&module, name, &[ReplacementValue::Int(i64::MIN)], &mut io)?;
            let ReplacementExecutionError::RuntimeFailure { detail, span, .. } = error else {
                return Err(format!("expected nested RuntimeFailure, got {error}").into());
            };
            assert!(detail.to_ascii_lowercase().contains("overflow"), "{detail}");
            assert_eq!(NESTED_SOURCE.get(span.start..span.end), Some(call));
            assert_eq!(io.output().stdout(), prefix);
            assert!(io.output().stderr().is_empty());
        }
        assert_eq!(stdout, prefix);
        assert!(stderr.is_empty());
    }
    Ok(())
}

/// The default release-oriented build command must not turn replacement overflow into a wrapped success.
#[test]
fn replacement_cli_release_contract_still_reports_checked_sum_overflow() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    fs::write(
        temporary.path().join("main.incn"),
        "def main() -> int:\n    println(\"before cli sum\")\n    return sum([9223372036854775807, 1])\n",
    )?;
    let output = Command::new(incan_binary())
        .current_dir(temporary.path())
        .env("INCAN_HOME", temporary.path().join("incan-home"))
        .args([
            "build",
            "main.incn",
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
        ])
        .output()?;
    assert!(
        !output.status.success(),
        "overflow must not wrap in the default release contract"
    );
    assert_eq!(output.stdout, b"before cli sum\n");
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        stderr.contains("INCAN-R988-RUNTIME") && stderr.contains("integer overflow in builtin `sum`"),
        "{stderr}"
    );
    assert!(
        !temporary.path().join(".incan/backend/receipt.json").exists(),
        "a failed execution must not publish a successful backend receipt"
    );
    assert!(
        !temporary.path().join("target/incan").exists(),
        "direct replacement execution must not create generated legacy output"
    );
    Ok(())
}

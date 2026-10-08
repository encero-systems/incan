//! RED characterizations for checked containment of builtin `abs` and `sum` overflow.
//!
//! The first two cases retain the original public-API RED: before the repair, direct execution inherited debug-host
//! arithmetic. The panic catches remain as a regression guard that any future change returns a typed error instead.

use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::process::Command;

use incan::backend::replacement::{
    BuiltinAbsSumOverflowBehavior, ProgramIo, ReplacementExecutionError, ReplacementExecutionOptions, ReplacementValue,
    execute_free_function_with_io, execute_prevalidated_free_function_with_io,
    prepare_free_function_execution_with_options,
};
use incan::backend::selection::{
    BackendExecutionReceipt, BackendKind, FallbackOutcome, FallbackPolicy, ShadowComparisonState, digest_output,
    finalize_receipt, select_backend,
};
use incan::frontend::body_ir::build_body_ir_module_v0;
use incan::frontend::typechecker::TypeChecker;
use incan::frontend::{lexer, parser};
use incan_semantics_core::body_ir::BodyIrModule;

const ABS_MIN_SOURCE: &str = "def abs_min(value: int) -> int:\n    println(\"before abs\")\n    return abs(value)\n";
const SUM_OVERFLOW_SOURCE: &str =
    "def overflowing_sum() -> int:\n    println(\"before sum\")\n    return sum([9223372036854775807, 1])\n";
const NESTED_GENERATOR_TASK_SOURCE: &str = r#"import std.async

def generated(value: int) -> Generator[int]:
    println("before generator abs")
    yield abs(value)

async def child(value: int) -> int:
    println("before task abs")
    return abs(value)

async def observe(value: int) -> int:
    generated_values = generated(value).collect()
    child_value = await child(value)
    return generated_values[0]
"#;

/// Lower one self-contained source module without generation or a native process.
fn lower_typed_body_ir(source: &str) -> Result<BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let module_path = vec!["replacement_abs_sum_profile".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// Require the compatibility direct wrapper to retain checked overflow as an ordinary typed execution error.
///
/// Catching is intentional: the initial debug-host panic was the RED behavior. Returning an error keeps any regression
/// from aborting unrelated native observations in the same test run.
fn direct_runtime_failure(
    module: &BodyIrModule,
    name: &str,
    args: &[ReplacementValue],
    io: &mut ProgramIo<'_>,
) -> Result<ReplacementExecutionError, Box<dyn std::error::Error>> {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        execute_free_function_with_io(module, name, args, io)
    }));
    let execution = match outcome {
        Ok(execution) => execution,
        Err(_) => {
            return Err(format!(
                "builtin overflow in `{name}` panicked the direct executor instead of returning RuntimeFailure"
            )
            .into());
        }
    };
    match execution {
        Ok(success) => Err(format!(
            "builtin overflow in `{name}` completed successfully with {:?}; expected RuntimeFailure",
            success.value
        )
        .into()),
        Err(error) => Ok(error),
    }
}

/// Execute a checked Body-IR plan under an explicit Abs/Sum behavior without changing the convenience API.
fn execute_with_options(
    module: &BodyIrModule,
    name: &str,
    args: &[ReplacementValue],
    options: ReplacementExecutionOptions,
    io: &mut ProgramIo<'_>,
) -> Result<
    Result<incan::backend::replacement::ReplacementExecution, ReplacementExecutionError>,
    Box<dyn std::error::Error>,
> {
    let plan = prepare_free_function_execution_with_options(module, name, args, options)?;
    match catch_unwind(AssertUnwindSafe(|| {
        execute_prevalidated_free_function_with_io(plan, io)
    })) {
        Ok(execution) => Ok(execution),
        Err(_) => Err(format!("explicit Abs/Sum profile execution for `{name}` panicked").into()),
    }
}

/// Locate the compiler binary Cargo built for this integration-test invocation.
fn incan_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_incan"))
}

/// A parameter carries the true `i64::MIN` input without exercising source unary-negation first.
#[test]
fn abs_min_retains_prior_stdout_and_returns_a_typed_overflow_at_the_builtin_span()
-> Result<(), Box<dyn std::error::Error>> {
    let module = lower_typed_body_ir(ABS_MIN_SOURCE)?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    {
        let mut io = ProgramIo::new(&mut stdout, &mut stderr);
        let error = direct_runtime_failure(&module, "abs_min", &[ReplacementValue::Int(i64::MIN)], &mut io)?;
        let ReplacementExecutionError::RuntimeFailure { detail, span, .. } = error else {
            return Err(format!("expected a typed direct runtime failure, got {error}").into());
        };
        assert!(detail.to_ascii_lowercase().contains("overflow"), "{detail}");
        assert_eq!(ABS_MIN_SOURCE.get(span.start..span.end), Some("abs(value)"));
        assert_eq!(io.output().stdout(), b"before abs\n");
        assert!(io.output().stderr().is_empty());
    }
    assert_eq!(stdout, b"before abs\n");
    assert!(stderr.is_empty());
    Ok(())
}

/// A zero-argument source function must contain its builtin `sum` overflow without hiding prior output.
#[test]
fn zero_argument_sum_overflow_retains_prior_stdout_and_returns_a_typed_overflow_at_the_builtin_span()
-> Result<(), Box<dyn std::error::Error>> {
    let module = lower_typed_body_ir(SUM_OVERFLOW_SOURCE)?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    {
        let mut io = ProgramIo::new(&mut stdout, &mut stderr);
        let error = direct_runtime_failure(&module, "overflowing_sum", &[], &mut io)?;
        let ReplacementExecutionError::RuntimeFailure { detail, span, .. } = error else {
            return Err(format!("expected a typed direct runtime failure, got {error}").into());
        };
        assert!(detail.to_ascii_lowercase().contains("overflow"), "{detail}");
        assert_eq!(
            SUM_OVERFLOW_SOURCE.get(span.start..span.end),
            Some("sum([9223372036854775807, 1])")
        );
        assert_eq!(io.output().stdout(), b"before sum\n");
        assert!(io.output().stderr().is_empty());
    }
    assert_eq!(stdout, b"before sum\n");
    assert!(stderr.is_empty());
    Ok(())
}

/// Explicit checked behavior is a typed failure, while explicit release behavior mirrors the observed wrapping.
#[test]
fn explicit_abs_sum_modes_contain_debug_overflow_and_mirror_release_wrapping() -> Result<(), Box<dyn std::error::Error>>
{
    for (source, name, args, prefix, call) in [
        (
            ABS_MIN_SOURCE,
            "abs_min",
            vec![ReplacementValue::Int(i64::MIN)],
            b"before abs\n".as_slice(),
            "abs(value)",
        ),
        (
            SUM_OVERFLOW_SOURCE,
            "overflowing_sum",
            Vec::new(),
            b"before sum\n".as_slice(),
            "sum([9223372036854775807, 1])",
        ),
    ] {
        let module = lower_typed_body_ir(source)?;
        let checked = ReplacementExecutionOptions {
            builtin_abs_sum_overflow: BuiltinAbsSumOverflowBehavior::Checked,
        };
        let mut checked_stdout = Vec::new();
        let mut checked_stderr = Vec::new();
        {
            let mut io = ProgramIo::new(&mut checked_stdout, &mut checked_stderr);
            let error = match execute_with_options(&module, name, &args, checked, &mut io)? {
                Ok(execution) => {
                    return Err(format!(
                        "checked Abs/Sum behavior for `{name}` completed as {:?}",
                        execution.value
                    )
                    .into());
                }
                Err(error) => error,
            };
            let ReplacementExecutionError::RuntimeFailure { detail, span, .. } = error else {
                return Err(format!("checked `{name}` must return RuntimeFailure, got {error}").into());
            };
            assert!(detail.to_ascii_lowercase().contains("overflow"), "{detail}");
            assert_eq!(source.get(span.start..span.end), Some(call));
            assert_eq!(io.output().stdout(), prefix);
            assert!(io.output().stderr().is_empty());
        }
        assert_eq!(checked_stdout, prefix);
        assert!(checked_stderr.is_empty());

        let release = ReplacementExecutionOptions {
            builtin_abs_sum_overflow: BuiltinAbsSumOverflowBehavior::ReleaseWrapping,
        };
        let mut release_stdout = Vec::new();
        let mut release_stderr = Vec::new();
        let release_execution = {
            let mut io = ProgramIo::new(&mut release_stdout, &mut release_stderr);
            let execution = execute_with_options(&module, name, &args, release, &mut io)?
                .map_err(|error| format!("release Abs/Sum behavior for `{name}` failed: {error}"))?;
            assert_eq!(io.output().stdout(), prefix);
            assert!(io.output().stderr().is_empty());
            execution
        };
        assert_eq!(release_execution.value, ReplacementValue::Int(i64::MIN));
        assert_eq!(
            release_execution.builtin_abs_sum_overflow,
            BuiltinAbsSumOverflowBehavior::ReleaseWrapping
        );
        assert_eq!(release_stdout, prefix);
        assert!(release_stderr.is_empty());
    }
    Ok(())
}

/// Identical safe program outputs remain non-substitutable when their selected Abs/Sum behavior differs.
#[test]
fn explicit_abs_sum_behavior_is_bound_into_execution_and_receipt_identity() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def absolute(value: int) -> int:\n    return abs(value)\n";
    let module = lower_typed_body_ir(source)?;
    let arguments = [ReplacementValue::Int(-9)];
    let checked = ReplacementExecutionOptions {
        builtin_abs_sum_overflow: BuiltinAbsSumOverflowBehavior::Checked,
    };
    let release = ReplacementExecutionOptions {
        builtin_abs_sum_overflow: BuiltinAbsSumOverflowBehavior::ReleaseWrapping,
    };

    let mut checked_stdout = Vec::new();
    let mut checked_stderr = Vec::new();
    let checked_execution = {
        let mut io = ProgramIo::new(&mut checked_stdout, &mut checked_stderr);
        execute_with_options(&module, "absolute", &arguments, checked, &mut io)?
            .map_err(|error| format!("checked safe abs must execute: {error}"))?
    };
    let mut release_stdout = Vec::new();
    let mut release_stderr = Vec::new();
    let release_execution = {
        let mut io = ProgramIo::new(&mut release_stdout, &mut release_stderr);
        execute_with_options(&module, "absolute", &arguments, release, &mut io)?
            .map_err(|error| format!("release safe abs must execute: {error}"))?
    };

    assert_eq!(checked_execution.value, ReplacementValue::Int(9));
    assert_eq!(release_execution.value, ReplacementValue::Int(9));
    assert_eq!(checked_execution.output, release_execution.output);
    assert_ne!(checked_execution.output_identity, release_execution.output_identity);
    assert_eq!(
        checked_execution.builtin_abs_sum_overflow,
        BuiltinAbsSumOverflowBehavior::Checked
    );
    assert_eq!(
        release_execution.builtin_abs_sum_overflow,
        BuiltinAbsSumOverflowBehavior::ReleaseWrapping
    );

    let selection = select_backend(
        BackendKind::Replacement,
        true,
        false,
        digest_output(&[source]),
        FallbackPolicy::Refuse,
    );
    let checked_receipt = finalize_receipt(
        &selection,
        BackendKind::Replacement,
        checked_execution.output_identity.clone(),
        ShadowComparisonState::NotRequested,
        incan::frontend::diagnostics::DIAGNOSTIC_SCHEMA_VERSION,
    )?;
    let release_receipt = finalize_receipt(
        &selection,
        BackendKind::Replacement,
        release_execution.output_identity.clone(),
        ShadowComparisonState::NotRequested,
        incan::frontend::diagnostics::DIAGNOSTIC_SCHEMA_VERSION,
    )?;
    checked_receipt.verify_identity()?;
    release_receipt.verify_identity()?;
    assert_ne!(checked_receipt.output_identity, release_receipt.output_identity);
    assert_ne!(checked_receipt.identity, release_receipt.identity);
    let mut substituted_receipt = checked_receipt.clone();
    substituted_receipt.output_identity = release_execution.output_identity.clone();
    assert!(
        substituted_receipt.verify_identity().is_err(),
        "a checked receipt must reject a substituted release-wrapping output identity"
    );
    assert!(checked_stdout.is_empty() && checked_stderr.is_empty());
    assert!(release_stdout.is_empty() && release_stderr.is_empty());
    Ok(())
}

/// The build command's release contract executes builtin `sum` directly and records the selected behavior in reports.
#[test]
fn replacement_cli_uses_release_wrapping_for_sum_and_binds_it_to_the_receipt() -> Result<(), Box<dyn std::error::Error>>
{
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    let report_path = temporary.path().join("replacement-report.json");
    fs::write(
        &entrypoint,
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
            "--report",
            "json",
            "--report-output",
            report_path.to_string_lossy().as_ref(),
        ])
        .output()?;
    assert!(
        output.status.success(),
        "release replacement build must execute sum directly. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"before cli sum\n");
    assert!(output.stderr.is_empty());
    let report: serde_json::Value = serde_json::from_slice(&fs::read(&report_path)?)?;
    assert_eq!(report["status"], "success");
    assert_eq!(report["backend"]["executed_backend"], "replacement");
    assert_eq!(report["replacement_execution"]["result"], i64::MIN.to_string());
    assert_eq!(
        report["replacement_execution"]["builtin_abs_sum_overflow"],
        "release_wrapping"
    );
    assert_eq!(
        report["replacement_execution"]["stdout_bytes"],
        serde_json::json!(output.stdout)
    );
    assert_eq!(report["replacement_execution"]["stderr_bytes"], serde_json::json!([]));
    let receipt: BackendExecutionReceipt =
        serde_json::from_slice(&fs::read(temporary.path().join(".incan/backend/receipt.json"))?)?;
    receipt.verify_identity()?;
    assert_eq!(receipt.executed_backend, BackendKind::Replacement);
    assert_eq!(receipt.fallback_outcome, FallbackOutcome::NotNeeded);
    assert_eq!(receipt.shadow_comparison, ShadowComparisonState::NotRequested);
    assert_eq!(
        receipt.output_identity,
        report["replacement_execution"]["output_identity"]
            .as_str()
            .ok_or("replacement CLI report must retain a string output identity")?
    );
    assert!(
        !temporary.path().join("target/incan").exists(),
        "direct replacement build must not create generated legacy output"
    );
    Ok(())
}

/// Explicit release behavior propagates through both a polled generator frame and an awaited task frame.
#[test]
fn release_abs_sum_behavior_reaches_nested_generator_and_task_frames() -> Result<(), Box<dyn std::error::Error>> {
    let module = lower_typed_body_ir(NESTED_GENERATOR_TASK_SOURCE)?;
    let options = ReplacementExecutionOptions {
        builtin_abs_sum_overflow: BuiltinAbsSumOverflowBehavior::ReleaseWrapping,
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let execution = {
        let mut io = ProgramIo::new(&mut stdout, &mut stderr);
        execute_with_options(&module, "observe", &[ReplacementValue::Int(i64::MIN)], options, &mut io)?
            .map_err(|error| format!("nested release overflow behavior must execute: {error}"))?
    };
    assert_eq!(execution.value, ReplacementValue::Int(i64::MIN));
    assert_eq!(
        execution.builtin_abs_sum_overflow,
        BuiltinAbsSumOverflowBehavior::ReleaseWrapping
    );
    assert_eq!(stdout, b"before generator abs\nbefore task abs\n");
    assert!(stderr.is_empty());
    assert!(execution.body_snapshot.contains("executed generator-function frame"));
    assert!(
        execution
            .task_lifecycle_evidence()
            .iter()
            .any(|event| event.event == "await_resumed"),
        "the awaited task frame must inherit the selected behavior: {:?}",
        execution.task_lifecycle_evidence()
    );
    Ok(())
}

/// Explicit checked behavior reaches a polled generator frame as a source-span-preserving runtime failure.
#[test]
fn checked_abs_sum_behavior_contains_overflow_inside_a_nested_generator_frame() -> Result<(), Box<dyn std::error::Error>>
{
    let module = lower_typed_body_ir(NESTED_GENERATOR_TASK_SOURCE)?;
    let options = ReplacementExecutionOptions {
        builtin_abs_sum_overflow: BuiltinAbsSumOverflowBehavior::Checked,
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    {
        let mut io = ProgramIo::new(&mut stdout, &mut stderr);
        let error =
            match execute_with_options(&module, "observe", &[ReplacementValue::Int(i64::MIN)], options, &mut io)? {
                Ok(execution) => {
                    return Err(format!("checked nested generator behavior completed as {:?}", execution.value).into());
                }
                Err(error) => error,
            };
        let ReplacementExecutionError::RuntimeFailure { detail, span, .. } = error else {
            return Err(format!("checked nested generator must return RuntimeFailure, got {error}").into());
        };
        assert!(detail.to_ascii_lowercase().contains("overflow"), "{detail}");
        assert_eq!(
            NESTED_GENERATOR_TASK_SOURCE.get(span.start..span.end),
            Some("abs(value)")
        );
        assert_eq!(io.output().stdout(), b"before generator abs\n");
        assert!(io.output().stderr().is_empty());
    }
    assert_eq!(stdout, b"before generator abs\n");
    assert!(stderr.is_empty());
    Ok(())
}

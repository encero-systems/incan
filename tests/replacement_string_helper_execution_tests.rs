//! Direct execution and source-span boundaries for compiler-selected runtime string helpers.

use incan::backend::replacement::{ProgramIo, ReplacementValue, execute_free_function_with_io};
use incan::frontend::body_ir::{apply_body_ir_input_contract, build_body_ir_module_v0};
use incan::frontend::{lexer, parser, typechecker::TypeChecker};
use incan_semantics_core::body_ir::BodyIrModule;

const STRING_HELPERS_SOURCE: &str = include_str!("fixtures/replacement/string_helpers.incn");

/// Retain one checked source module without invoking generated Rust or repeating source resolution in execution.
fn lower(source: &str) -> Result<BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    let path = vec!["selected_string_execution".to_string()];
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    Ok(build_body_ir_module_v0(&program, &path, checker.type_info()))
}

/// A shared authored source fixture exercises every selected helper, including the optional split separator.
#[test]
fn selected_string_helpers_execute_from_checked_operations() -> Result<(), Box<dyn std::error::Error>> {
    let module = lower(STRING_HELPERS_SOURCE)?;
    let snapshot = module.render_snapshot();
    for helper in [
        "str_upper",
        "str_lower",
        "str_strip",
        "str_replace",
        "str_join",
        "str_split",
        "str_contains",
    ] {
        assert!(snapshot.contains(&format!("call helper:{helper}")), "{snapshot}");
    }
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut io = ProgramIo::new(&mut stdout, &mut stderr);
    let execution = execute_free_function_with_io(&module, "string_helpers", &[], &mut io)?;
    assert_eq!(execution.value, ReplacementValue::Bool(true));
    assert_eq!(execution.output.stdout(), b"string helper checks\n");
    assert!(execution.output.stderr().is_empty());
    assert_eq!(stdout, b"string helper checks\n");
    assert!(stderr.is_empty());
    Ok(())
}

/// Individual results keep Unicode, empty patterns, and separator defaults visible when a fixture regresses.
#[test]
fn selected_string_helpers_preserve_shared_edge_semantics() -> Result<(), Box<dyn std::error::Error>> {
    for (expression, expected) in [
        ("\"Straße\".upper()", "STRASSE"),
        ("\"HÉLLO\".lower()", "héllo"),
        ("\" hi \".strip()", "hi"),
        ("\"ababa\".replace(\"aba\", \"x\")", "xba"),
        ("\"é\".replace(\"\", \"-\")", "-é-"),
        ("\", \".join([\"α\", \"β\"])", "α, β"),
        ("\"|\".join(\" a b \".split())", " a b "),
        ("\"|\".join(\"é\".split(\"\"))", "|é|"),
        ("\"|\".join(\"a,,b,\".split(\",\"))", "a||b|"),
    ] {
        let module = lower(&format!("def result() -> str:\n    return {expression}\n"))?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut io = ProgramIo::new(&mut stdout, &mut stderr);
        let execution = execute_free_function_with_io(&module, "result", &[], &mut io)?;
        assert_eq!(
            execution.value,
            ReplacementValue::Str(expected.to_string()),
            "{expression}"
        );
        assert!(execution.output.stdout().is_empty());
        assert!(execution.output.stderr().is_empty());
    }
    Ok(())
}

/// String membership and the selected contains method share one helper with normalized haystack-first operands.
#[test]
fn canonical_string_membership_uses_the_selected_contains_helper() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def membership(needle: str, haystack: str) -> bool:\n    return needle in haystack\n";
    let module = lower(source)?;
    assert!(module.render_snapshot().contains("call helper:str_contains"));
    for (needle, haystack, expected) in [("éll", "héllo", true), ("héllo", "éll", false), ("", "", true)] {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut io = ProgramIo::new(&mut stdout, &mut stderr);
        let execution = execute_free_function_with_io(
            &module,
            "membership",
            &[
                ReplacementValue::Str(needle.to_string()),
                ReplacementValue::Str(haystack.to_string()),
            ],
            &mut io,
        )?;
        assert_eq!(execution.value, ReplacementValue::Bool(expected));
        assert!(execution.output.stdout().is_empty());
        assert!(execution.output.stderr().is_empty());
    }
    Ok(())
}

/// The normal CLI admits string membership with a separate true result and an actual replacement receipt.
#[test]
fn string_membership_cli_records_the_shared_helper_result() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    std::fs::write(
        temporary.path().join("main.incn"),
        "def main() -> bool:\n    return \"a\" in \"abc\"\n",
    )?;
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_incan"))
        .current_dir(temporary.path())
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
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    let report: serde_json::Value = serde_json::from_slice(&std::fs::read(temporary.path().join("report.json"))?)?;
    assert_eq!(report["replacement_execution"]["result"], "true");
    let receipt: incan::backend::selection::BackendExecutionReceipt =
        serde_json::from_slice(&std::fs::read(temporary.path().join(".incan/backend/receipt.json"))?)?;
    receipt.verify_identity()?;
    assert_eq!(
        receipt.executed_backend,
        incan::backend::selection::BackendKind::Replacement
    );
    assert_eq!(
        receipt.fallback_outcome,
        incan::backend::selection::FallbackOutcome::NotNeeded
    );
    assert!(!temporary.path().join("target/incan").exists());
    Ok(())
}

/// All six normalized string comparisons retain Unicode scalar ordering through their canonical helper operations.
#[test]
fn canonical_string_comparisons_use_shared_unicode_order() -> Result<(), Box<dyn std::error::Error>> {
    for (operator, expected) in [
        ("==", false),
        ("!=", true),
        ("<", true),
        ("<=", true),
        (">", false),
        (">=", false),
    ] {
        let source = format!("def compare(left: str, right: str) -> bool:\n    return left {operator} right\n");
        let module = lower(&source)?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut io = ProgramIo::new(&mut stdout, &mut stderr);
        let execution = execute_free_function_with_io(
            &module,
            "compare",
            &[
                ReplacementValue::Str("é".to_string()),
                ReplacementValue::Str("猫".to_string()),
            ],
            &mut io,
        )?;
        assert_eq!(execution.value, ReplacementValue::Bool(expected), "{operator}");
    }
    Ok(())
}

/// The unchanged committed example must run to completion, not merely lower the first selected helper.
#[test]
fn committed_strings_example_executes_with_ordinary_output() -> Result<(), Box<dyn std::error::Error>> {
    let source_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/simple/strings.incn");
    let source = std::fs::read_to_string(&source_path)?;
    let tokens = lexer::lex(&source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let program = apply_body_ir_input_contract(program, &source_path).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    let path = vec!["committed_strings_example".to_string()];
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let module = build_body_ir_module_v0(&program, &path, checker.type_info());
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut io = ProgramIo::new(&mut stdout, &mut stderr);
    let execution = execute_free_function_with_io(&module, "main", &[], &mut io)?;
    assert_eq!(execution.value, ReplacementValue::Unit);
    assert_eq!(
        execution.output.stdout(),
        concat!(
            "Original: hello, world\n",
            "Upper: HELLO, WORLD\n",
            "Lower: hello, world\n",
            "Split CSV: first name is alice\n",
            "Joined: alice, bob, carol\n",
            "Padded: '  hello  '\n",
            "Stripped: 'hello'\n",
            "Sentence contains 'quick'\n",
            "Replaced: the quick brown dog\n",
        )
        .as_bytes()
    );
    assert!(execution.output.stderr().is_empty());
    Ok(())
}

/// Non-admitted methods cannot acquire helper behavior from a familiar source spelling.
#[test]
fn unselected_string_methods_refuse_before_program_output() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def result(text: str) -> str:\n    println(\"must not run\")\n    return text.to_string()\n";
    let module = lower(source)?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut io = ProgramIo::new(&mut stdout, &mut stderr);
    let error = execute_free_function_with_io(&module, "result", &[ReplacementValue::Str("x".to_string())], &mut io)
        .err()
        .ok_or("unselected method must refuse")?;
    let span = error.primary_span().ok_or("refusal must retain the call span")?;
    assert_eq!(source.get(span.start..span.end), Some("text.to_string()"));
    assert!(io.output().stdout().is_empty());
    assert!(io.output().stderr().is_empty());
    Ok(())
}

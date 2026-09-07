//! Direct execution coverage for Unicode-scalar string length.

use incan::backend::replacement::{ProgramIo, ReplacementValue, execute_free_function_with_io};
use incan::frontend::body_ir::build_body_ir_module_v0;
use incan::frontend::typechecker::TypeChecker;
use incan::frontend::{lexer, parser};
use incan_semantics_core::body_ir::BodyIrModule;

fn lower(source: &str) -> Result<BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["replacement_string_len".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    Ok(build_body_ir_module_v0(&program, &path, checker.type_info()))
}

fn execute_length(source: &str, value: &str) -> Result<ReplacementValue, Box<dyn std::error::Error>> {
    let module = lower(source)?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut io = ProgramIo::new(&mut stdout, &mut stderr);
    let execution =
        execute_free_function_with_io(&module, "length", &[ReplacementValue::Str(value.to_string())], &mut io)?;
    assert!(execution.output.stdout().is_empty());
    assert!(execution.output.stderr().is_empty());
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
    Ok(execution.value)
}

/// Global `len(str)` counts Unicode scalar values rather than UTF-8 bytes.
#[test]
fn global_len_str_executes_unicode_scalar_rows() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def length(value: str) -> int:\n    return len(value)\n";
    for (value, expected) in [("", 0), ("abc", 3), ("é", 1), ("😀", 1), ("e\u{301}", 2)] {
        assert_eq!(
            execute_length(source, value)?,
            ReplacementValue::Int(expected),
            "{value:?}"
        );
    }
    Ok(())
}

/// The canonical zero-argument method has the same Unicode-scalar behavior.
#[test]
fn method_len_str_executes_unicode_scalar_rows() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def length(value: str) -> int:\n    return value.len()\n";
    for (value, expected) in [("", 0), ("abc", 3), ("é", 1), ("😀", 1), ("e\u{301}", 2)] {
        assert_eq!(
            execute_length(source, value)?,
            ReplacementValue::Int(expected),
            "{value:?}"
        );
    }
    Ok(())
}

/// Program output is delivered once and stays separate from the returned length.
#[test]
fn string_len_output_is_delivered_exactly_once() -> Result<(), Box<dyn std::error::Error>> {
    for expression in ["len(value)", "value.len()"] {
        let source = format!("def length(value: str) -> int:\n    println(value)\n    return {expression}\n");
        let module = lower(&source)?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut io = ProgramIo::new(&mut stdout, &mut stderr);
        let execution =
            execute_free_function_with_io(&module, "length", &[ReplacementValue::Str("é".to_string())], &mut io)?;
        assert_eq!(execution.value, ReplacementValue::Int(1));
        assert_eq!(execution.output.stdout(), "é\n".as_bytes());
        assert!(execution.output.stderr().is_empty());
        assert_eq!(stdout, "é\n".as_bytes());
        assert!(stderr.is_empty());
    }
    Ok(())
}

/// A same-module declaration named `len` keeps its own behavior.
#[test]
fn lexical_len_shadow_executes_the_user_declaration() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def len(value: str) -> int:
    return 99

def length(value: str) -> int:
    return len(value)
"#;
    assert_eq!(execute_length(source, "é")?, ReplacementValue::Int(99));
    Ok(())
}

//! A fresh value appended to a list moves into it (#1821).
//!
//! The checker accepts `handles.append(spawn(work()))` although a task handle has no `Clone`, on the ground that the
//! generated push takes a call result over instead of copying it. This pins that ground: the push of the call result
//! carries no copy.

use crate::IrCodegen;
use incan_frontend::{lexer, parser};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The #1821 program: a spawned task's handle appended straight into a list.
const TASK_HANDLES: &str = r#"
from std.async import spawn


async def work() -> int:
    return 1


async def main() -> None:
    mut handles = []
    handles.append(spawn(work()))
    println(len(handles))
"#;

/// Parse, check and generate one program with the builtin stdlib inventory, returning the emitted Rust.
fn generated(source: &'static str) -> Result<String, String> {
    incan_frontend::compiler_stack::run_on_compiler_stack(move || {
        let tokens = lexer::lex(source).map_err(|errors| format!("lex: {errors:?}"))?;
        let program = parser::parse(&tokens).map_err(|errors| format!("parse: {errors:?}"))?;
        let mut codegen = IrCodegen::new();
        codegen.set_sdk_provider_module_paths(incan_test_support::builtin_stdlib::artifact_module_paths());
        codegen
            .try_generate(&program)
            .map_err(|error| format!("generate: {error:?}"))
    })
}

/// Return every `push(...)` argument in `rust`, with whitespace removed.
fn push_arguments(rust: &str) -> Vec<String> {
    let compact = rust.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    compact
        .split(".push(")
        .skip(1)
        .filter_map(|rest| rest.split_once(");").map(|(argument, _)| argument.to_string()))
        .collect()
}

/// #1821: the task handle a call returns is pushed as it is, with no copy the handle could not supply.
#[test]
fn a_fresh_value_moves_into_the_list_issue1821() -> TestResult {
    let rust = generated(TASK_HANDLES)?;
    let pushes = push_arguments(&rust);
    let [pushed] = pushes.as_slice() else {
        return Err(format!("expected one push, got {pushes:?}:\n{rust}").into());
    };
    assert!(
        pushed.contains("spawn"),
        "the push must take the spawn result: {pushed}\n{rust}"
    );
    assert!(
        !pushed.contains(".clone()"),
        "a fresh value must not be copied: {pushed}\n{rust}"
    );
    Ok(())
}

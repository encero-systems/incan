//! Diagnostics-focused tests for construction semantics.
//!
//! These tests are intentionally “frontend only”: they run lexer+parser+typechecker
//! and assert that errors are produced at the Incan level (not leaked as Rust errors).

use incan::frontend::{lexer, parser, typechecker};

fn typecheck_err_messages(src: &str) -> Result<Vec<String>, Vec<String>> {
    let tokens = lexer::lex(src).map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>())?;
    let ast = parser::parse(&tokens).map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>())?;
    let mut tc = typechecker::TypeChecker::new();
    match tc.check_program(&ast) {
        Ok(()) => Ok(vec![]),
        Err(errs) => Ok(errs.into_iter().map(|e| e.message).collect()),
    }
}

#[test]
fn model_constructor_missing_required_field_is_reported_by_typechecker() -> Result<(), Vec<String>> {
    let src = r#"
model User:
    name: str
    age: int = 1

def main() -> None:
    u = User(age=3)
"#;
    let errs = typecheck_err_messages(src)?;
    assert!(
        !errs.is_empty(),
        "expected typechecker error for missing required field; got none"
    );
    Ok(())
}

#[test]
fn model_constructor_unknown_field_is_reported_by_typechecker() -> Result<(), Vec<String>> {
    let src = r#"
model User:
    name: str

def main() -> None:
    u = User(name="Alice", bogus=123)
"#;
    let errs = typecheck_err_messages(src)?;
    assert!(
        !errs.is_empty(),
        "expected typechecker error for unknown field; got none"
    );
    Ok(())
}

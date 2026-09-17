//! Helpers the emission tests share with the crates above them: parse, generate, and the assertions on generated
//! Rust. On under `cfg(test)` and the `test_support` feature, which the dependants' dev-dependencies turn on.

use incan_frontend::ast::Program;

use crate::codegen::IrCodegen;
use incan_frontend::{lexer, parser};

/// Unwrap a result a test has already established cannot fail, naming the failure when it does.
pub fn must_ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => panic!("unexpected error: {err:?}"),
    }
}

/// Unwrap an option a test has already established is present, naming the missing value when it is not.
pub fn must_some<T>(value: Option<T>, context: &str) -> T {
    match value {
        Some(v) => v,
        None => panic!("{context}"),
    }
}

/// Lex, parse and generate Rust for one source with a default `IrCodegen`.
pub fn generate(source: &str) -> String {
    let tokens = must_ok(lexer::lex(source));
    let ast = must_ok(parser::parse(&tokens));
    must_ok(IrCodegen::new().try_generate(&ast))
}

/// Fail when generated Rust carries an `#[allow(unused…)]` the emitter should never need.
pub fn assert_no_generated_unused_lint_allows(code: &str) {
    assert!(!code.contains("#[allow(dead_code)]"), "{code}");
    assert!(!code.contains("#[allow(unused_imports)]"), "{code}");
    assert!(!code.contains("#[allow(dead_code, unused_variables)]"), "{code}");
}

/// Lex and parse one source into a program, panicking on a syntax error a test did not expect.
pub fn parse_program(source: &str) -> Program {
    let tokens = must_ok(lexer::lex(source));
    must_ok(parser::parse(&tokens))
}

/// Lex and parse one source, returning the syntax errors for the test to inspect.
pub fn parse_program_result(source: &str) -> Result<Program, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    Ok(ast)
}

#[cfg(feature = "rust_inspect")]
/// Extract rust-inspect metadata for `paths` in `manifest_dir` so a test can typecheck against real Rust items.
pub fn prewarm_metadata(manifest_dir: &std::path::Path, paths: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    let inspector = rust_inspect::Inspector::new(rust_inspect::InspectorConfig::new(manifest_dir.to_path_buf()));
    inspector.prewarm(paths.iter().map(|p| (*p).to_string()).collect::<Vec<_>>(), &|_| ())?;
    Ok(())
}

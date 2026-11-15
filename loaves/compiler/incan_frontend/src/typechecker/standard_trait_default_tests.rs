use super::TypeChecker;
use crate::ast::Program;

/// Parse a source module without changing its declarations or depending on a published standard library.
fn program(source: &str) -> Result<Program, Box<dyn std::error::Error>> {
    let tokens = crate::lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    Ok(crate::parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?)
}

/// A standard-library trait default unwraps a sibling default's closed result with `?`, as a program's does (#3153).
#[test]
fn standard_trait_default_unwraps_a_sibling_default_result() -> Result<(), Box<dyn std::error::Error>> {
    let source = "pub trait ValueReading:\n    def aaa(mut self) -> Result[int, str]:\n        return Ok(1)\n\n    def bbb(mut self) -> Result[int, str]:\n        return Ok(self.aaa()? + 1)\n";
    let parsed = program(source)?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["std".to_string(), "probe".to_string()]));
    let checked = checker.check_program(&parsed);
    assert!(checked.is_ok(), "standard trait default refused: {checked:?}");
    Ok(())
}

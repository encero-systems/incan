#![allow(dead_code)] // Split test binaries intentionally use different parts of shared support fragments.

//! Integration tests for the Incan compiler frontend

include!("support/integration_tests_root.rs");

mod numeric_semantics_tests {
    include!("support/integration_tests_numeric_semantics_tests.rs");

    #[test]
    fn test_python_like_numeric_ops_compile() {
        let source = r#"
def main() -> None:
  a: int = 7
  b: int = -3
  x = a / b       # float
  y = a // b      # floor div
  z = a % b       # python remainder
  f: float = 7.0
  g = f % 2.0
  h = f // 2.0
"#;
        let Ok(tokens) = lexer::lex(source) else {
            panic!("lexing failed");
        };
        let Ok(ast) = parser::parse(&tokens) else {
            panic!("parse failed");
        };
        let Ok(()) = typechecker::check(&ast) else {
            panic!("typecheck failed");
        };
    }
}

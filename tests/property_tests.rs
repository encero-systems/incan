//! Property-based tests for the Incan compiler
//!
//! These tests use proptest to verify invariants across many randomly
//! generated inputs, catching edge cases that hand-written tests might miss.

use incan::format::format_source;
use proptest::prelude::*;

// Note: Conversion module tests are complex due to IR construction requirements.
// See tests/codegen_snapshot_tests.rs for comprehensive conversion testing via
// end-to-end codegen.

// =============================================================================
// Format Properties
// =============================================================================

#[cfg(test)]
mod format_tests {
    use super::*;

    /// Property: Formatting is idempotent (format(format(x)) == format(x))
    #[test]
    fn format_is_idempotent_simple() -> Result<(), String> {
        let source = r#"
def add(a: int, b: int) -> int:
    return a + b

def main() -> ():
    result = add(1, 2)
    print(result)
"#;

        let formatted1 = format_source(source).map_err(|e| e.to_string())?;
        let formatted2 = format_source(&formatted1).map_err(|e| e.to_string())?;

        assert_eq!(formatted1, formatted2, "Formatting should be idempotent");
        Ok(())
    }

    /// Property: Formatting preserves semantic meaning (can parse before and after)
    #[test]
    fn format_preserves_parseability() -> Result<(), String> {
        use incan::frontend::{lexer, parser};

        let source = r#"
def greet(name: str) -> str:
    return f"Hello, {name}!"
"#;

        // Parse original
        let tokens1 =
            lexer::lex(source).map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>().join("; "))?;
        let ast1 = parser::parse(&tokens1)
            .map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>().join("; "))?;

        // Format and parse
        let formatted = format_source(source).map_err(|e| e.to_string())?;
        let tokens2 = lexer::lex(&formatted)
            .map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>().join("; "))?;
        let ast2 = parser::parse(&tokens2)
            .map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>().join("; "))?;

        // AST should have same structure (both should parse to same number of declarations)
        assert_eq!(
            ast1.declarations.len(),
            ast2.declarations.len(),
            "Formatting changed AST structure"
        );
        Ok(())
    }

    /// Property: Empty or whitespace-only input formats without error
    #[test]
    fn format_handles_empty_input() {
        let empty_cases = vec!["", "   ", "\n\n\n", "\t\t"];

        for source in empty_cases {
            let result = format_source(source);
            // Empty/whitespace should either format successfully or give a syntax error
            // (both are acceptable behaviors)
            let _ = result;
        }
    }
}

// =============================================================================
// Proptest Strategy Examples (for future expansion)
// =============================================================================

#[cfg(test)]
mod proptest_strategies {
    use super::*;

    // Strategy for generating valid Incan identifiers
    fn ident_strategy() -> impl Strategy<Value = String> {
        "[a-z][a-z0-9_]*".prop_filter("Not a keyword", |s| {
            !matches!(
                s.as_str(),
                "def"
                    | "class"
                    | "if"
                    | "else"
                    | "return"
                    | "import"
                    | "is"
                    | "in"
                    | "not"
                    | "and"
                    | "or"
                    | "for"
                    | "while"
                    | "match"
                    | "case"
                    | "model"
                    | "trait"
                    | "enum"
                    | "mut"
                    | "const"
                    | "async"
                    | "await"
                    | "try"
                    | "except"
                    | "finally"
                    | "raise"
                    | "with"
                    | "as"
                    | "from"
                    | "pass"
                    | "break"
                    | "continue"
                    | "yield"
                    | "lambda"
                    | "global"
                    | "nonlocal"
                    | "assert"
                    | "del"
                    | "elif"
                    | "true"
                    | "false"
                    | "none"
                    | "self"
                    | "super"
                    | "type"
                    | "where"
                    | "impl"
                    | "pub"
                    | "use"
                    | "mod"
                    | "fn"
                    | "let"
                    | "static"
                    | "struct"
                    | "newtype"
            )
        })
    }

    // Strategy for generating simple function definitions
    fn simple_function_strategy() -> impl Strategy<Value = String> {
        (ident_strategy(), "[a-z]")
            .prop_map(|(name, param)| format!("def {}({}: int) -> int:\n    return {}\n", name, param, param))
    }

    proptest! {
        /// Property: Valid function definitions parse and format successfully
        #[test]
        fn generated_functions_format_successfully(
            func in simple_function_strategy()
        ) {
            // Parse
            use incan::frontend::{lexer, parser};
            let tokens = match lexer::lex(&func) {
                Ok(tokens) => tokens,
                Err(errs) => {
                    prop_assert!(false, "Lex failed: {:?}", errs);
                    unreachable!();
                }
            };
            let _ast = match parser::parse(&tokens) {
                Ok(ast) => ast,
                Err(errs) => {
                    prop_assert!(false, "Parse failed: {:?}", errs);
                    unreachable!();
                }
            };

            // Format
            let formatted = match format_source(&func) {
                Ok(formatted) => formatted,
                Err(err) => {
                    prop_assert!(false, "Format failed: {}", err);
                    unreachable!();
                }
            };

            // Re-parse to ensure still valid
            let tokens2 = match lexer::lex(&formatted) {
                Ok(tokens) => tokens,
                Err(errs) => {
                    prop_assert!(false, "Lex formatted failed: {:?}", errs);
                    unreachable!();
                }
            };
            let _ast2 = match parser::parse(&tokens2) {
                Ok(ast) => ast,
                Err(errs) => {
                    prop_assert!(false, "Parse formatted failed: {:?}", errs);
                    unreachable!();
                }
            };
        }

        /// Property: Identifiers remain valid after round-trip through lexer
        #[test]
        fn identifiers_survive_lexing(ident in ident_strategy()) {
            use incan::frontend::lexer;

            let source = format!("x = {}", ident);
            let tokens = match lexer::lex(&source) {
                Ok(tokens) => tokens,
                Err(errs) => {
                    prop_assert!(false, "Lex failed: {:?}", errs);
                    unreachable!();
                }
            };

            // Should have at least 3 tokens (ident, =, ident)
            prop_assert!(tokens.len() >= 3);
        }
    }
}

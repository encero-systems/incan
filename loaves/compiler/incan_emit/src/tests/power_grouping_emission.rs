//! A prefix operator over a power emits over the whole power (#1786): `-x ** 2` is `-(x ** 2)`, while a parenthesized
//! negated base stays the base.

use crate::codegen::IrCodegen;
use incan_frontend::{lexer, parser};

/// Generate Rust for one checked source with all whitespace removed, so a match does not depend on line breaks.
fn generate_compact(source: &str) -> Result<String, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?;
    let code = IrCodegen::new()
        .try_generate(&program)
        .map_err(|error| format!("generation failed: {error:?}"))?;
    Ok(code.split_whitespace().collect())
}

#[test]
fn prefix_operator_over_a_power_emits_over_the_whole_power_issue1786() -> Result<(), String> {
    let code = generate_compact(
        r#"
def negated_square(x: int) -> int:
    return -x ** 2


def negated_base_square(x: int) -> int:
    return (-x) ** 2


def main() -> None:
    println(negated_square(3))
    println(negated_base_square(3))
"#,
    )?;
    assert!(
        code.contains("-{(x).pow(2asu32)}"),
        "`-x ** 2` must negate the grouped power:\n{code}"
    );
    assert!(
        code.contains("(-x).pow(2asu32)"),
        "`(-x) ** 2` must raise the negated base:\n{code}"
    );
    Ok(())
}

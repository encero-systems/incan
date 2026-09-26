//! The generated text of numeric operators whose lowering or helper choice fixes a check-then-rustc failure: a `**`
//! base in its result type (#1811), and the checked helpers every zero divisor reaches (#1813).

use crate::IrCodegen;
use incan_frontend::{lexer, parser};

/// Generate Rust for `source` and return it with all whitespace removed, so assertions do not depend on layout.
fn compact_generated(source: &str) -> Result<String, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?;
    let generated = IrCodegen::new()
        .try_generate(&program)
        .map_err(|error| format!("generation failed: {error:?}"))?;
    Ok(generated.chars().filter(|ch| !ch.is_whitespace()).collect())
}

/// #1811: the base of `**` reaches Rust in the operation's result type, so no receiver is an ambiguous literal.
#[test]
fn power_base_is_emitted_in_the_result_type_issue1811() -> Result<(), String> {
    let code = compact_generated(
        r#"
def main() -> None:
    println(2 ** 3)
    println(2.0 ** 0.5)
    println((-2) ** 3)
    println((1 + 2) ** 2)
    base = 2
    println(base ** 3)
"#,
    )?;
    for expected in [
        "(2asi64).pow(3asu32)",
        "(2.0asf64).powf(0.5)",
        "(-2asi64).pow(3asu32)",
        "({1+2}asi64).pow(2asu32)",
        "(baseasi64).pow(3asu32)",
    ] {
        assert!(code.contains(expected), "missing `{expected}` in:\n{code}");
    }
    assert!(!code.contains("(2).pow("), "a bare literal receiver survived:\n{code}");
    Ok(())
}

/// #1813: unsigned `//` and `%` (and their compound forms) call the checked unsigned helpers instead of Rust's
/// native operators, and integer true division calls the integer helper.
#[test]
fn zero_divisors_reach_checked_helpers_issue1813() -> Result<(), String> {
    let code = compact_generated(
        r#"
def unsigned_ops(b: u8, c: u8) -> u8:
    mut total: u8 = b // 2
    total %= c
    return total % c

def integer_division(a: int, b: int) -> float:
    return a / b

def float_division(a: float, b: int) -> float:
    return a / b

def main() -> None:
    println(unsigned_ops(7, 3))
    println(integer_division(7, 2))
    println(float_division(7.0, 2))
"#,
    )?;
    for expected in [
        "incan_std_core::num::py_floor_div_unsigned(b,2)",
        "incan_std_core::num::py_mod_unsigned(total,c)",
        "incan_std_core::num::py_div_int((a)asf64,(b)asf64)",
        "incan_std_core::num::py_div(a,(b)asf64)",
    ] {
        assert!(code.contains(expected), "missing `{expected}` in:\n{code}");
    }
    Ok(())
}

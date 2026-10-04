//! The generated text of numeric operators whose lowering or helper choice fixes a check-then-rustc failure: a `**`
//! base in its result type (#1811), the checked helpers every zero divisor reaches (#1813), exact-width integer
//! arithmetic that keeps its type and `f64` values that are plain `float` values (RFC 009).

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

/// RFC 009: same-type integer arithmetic keeps its exact width in the generated Rust. An integer literal beside an
/// exact-width operand is left for Rust to infer in that type, signed `//` and `%` call the width-keeping signed
/// helpers, and `**` raises the base in its own type.
#[test]
fn exact_width_integer_arithmetic_keeps_its_type() -> Result<(), String> {
    let code = compact_generated(
        r#"
def signed_ops(a: i8, b: i8) -> i8:
    mut total: i8 = a + 1
    total -= b
    total //= 2
    return total % b * a ** 2

def wide_ops(a: i128, b: i128) -> i128:
    return a // b

def main() -> None:
    println(signed_ops(7, 3))
    println(wide_ops(7, 3))
"#,
    )?;
    for expected in [
        "letmuttotal:i8=a+1;",
        "total=total-b;",
        "total=incan_std_core::num::py_floor_div_signed(total,2);",
        "incan_std_core::num::py_mod_signed(total,b)",
        "(a).pow(2asu32)",
        "incan_std_core::num::py_floor_div_signed(a,b)",
    ] {
        assert!(code.contains(expected), "missing `{expected}` in:\n{code}");
    }
    assert!(
        !code.contains("asi64"),
        "an exact-width operand was widened to int:\n{code}"
    );
    Ok(())
}

/// RFC 009: `f64` is `float`, so an `f64` value is not guarded as a finite-only exact carrier.
#[test]
fn f64_values_are_ordinary_floats() -> Result<(), String> {
    let code = compact_generated(
        r#"
def scale(value: f64, factor: float) -> f64:
    product: f64 = value * factor
    return product

def main() -> None:
    println(scale(1.5, 2.0))
"#,
    )?;
    assert!(
        code.contains("letproduct:f64=value*factor;"),
        "missing the plain float product in:\n{code}"
    );
    assert!(
        !code.contains("require_finite"),
        "an f64 value was guarded as finite-only:\n{code}"
    );
    Ok(())
}

/// RFC 009: every lossless numeric widening the checker accepts builds. The value reaches a binding, a reassignment,
/// an argument, a return, a `yield`, a model field, a class field, a collection element and slot, an `Option` payload
/// and a union member widened to the destination's type, the arms of a `match` and the `break` values of a `loop:`
/// are widened to their one type, a lookup probe is widened to the element or key type, and two integer types compare
/// in the narrowest type holding both. Each program is compiled by rustc.
#[test]
fn accepted_numeric_widenings_build() -> Result<(), Box<dyn std::error::Error>> {
    for (case, source) in [
        (
            "binding",
            "pub def f(small: i8) -> int:\n    wide: int = small\n    return wide\n",
        ),
        (
            "reassignment",
            "pub def f(small: i8) -> int:\n    mut wide: int = 1\n    wide = small\n    return wide\n",
        ),
        (
            "argument",
            "def take(value: int) -> int:\n    return value\n\npub def f(small: u8) -> int:\n    return take(small)\n",
        ),
        ("return", "pub def f(small: i16) -> i64:\n    return small\n"),
        (
            "yield",
            "pub def f(small: i8) -> Generator[int]:\n    yield small\n    yield 2\n",
        ),
        (
            "match arms",
            "pub def f(n: int, small: i8, wide: int) -> int:\n    chosen = match n:\n        0 => small\n        _ => wide\n    maybe: Option[int] = match n:\n        0 => 5\n        _ => None\n    return chosen + maybe.unwrap_or(0)\n",
        ),
        (
            "loop break values",
            "pub def f(n: int, small: i8, wide: int) -> int:\n    x = loop:\n        if n > 0:\n            break small\n        break wide\n    return x\n",
        ),
        (
            "unsigned into signed",
            "pub def f(value: u64) -> i128:\n    return value\n",
        ),
        (
            "float",
            "pub def f(narrow: f32) -> float:\n    wide: f64 = narrow\n    return wide\n",
        ),
        (
            "model field",
            "pub model Box:\n    pub value: int\n\npub def f(small: u32) -> Box:\n    return Box(value=small)\n",
        ),
        (
            "class field",
            "pub class Box:\n    pub value: int\n\npub def f(mut b: Box, small: i32) -> None:\n    b.value = small\n",
        ),
        (
            "collection element and slot",
            "pub def f(small: i8) -> list[int]:\n    mut values: list[int] = [small, 2]\n    values[0] = small\n    values.append(small)\n    return values\n",
        ),
        (
            "option payload",
            "def take(value: Option[int]) -> int:\n    return value.unwrap_or(0)\n\npub def f(small: u16) -> Option[int]:\n    maybe: Option[int] = small\n    println(take(small) + maybe.unwrap_or(small))\n    return small\n",
        ),
        (
            "union member",
            "def take(value: int | str) -> bool:\n    return isinstance(value, int)\n\npub def f(small: u16) -> int | str:\n    held: int | str = small\n    println(take(small) and take(held))\n    return small\n",
        ),
        (
            "lookup probes",
            "pub def f(small: i8, values: list[int], table: dict[int, str]) -> bool:\n    return small in values and values.count(small) > 0 and table[small] == \"a\" and small in table\n",
        ),
        (
            "comparisons",
            "pub def f(small: i8, byte: u8, count: int, big: u128) -> bool:\n    return small < byte and small == count and byte != -1 and big == 0\n",
        ),
    ] {
        let code = crate::IrCodegen::new().try_generate(
            &parser::parse(&lexer::lex(source).map_err(|errors| format!("{case}: lex failed: {errors:?}"))?)
                .map_err(|errors| format!("{case}: parse failed: {errors:?}"))?,
        )?;
        super::mut_ownership_regressions::compile_generated_rust(&code)
            .map_err(|error| format!("{case} did not build: {error}"))?;
    }
    Ok(())
}

/// RFC 009: bit arithmetic keeps its operands' one integer type (`u8 & u8` is a `u8`, also in `|=`), a shift keeps its
/// left operand's type whatever integer type it counts with, and `~` keeps its operand's type, so each result reaches
/// a destination of that type or widens to a wider one. Each program is compiled by rustc.
#[test]
fn bitwise_operators_keep_the_integer_type_build() -> Result<(), Box<dyn std::error::Error>> {
    for (case, source) in [
        (
            "same-type operands",
            "pub def f(a: u8, b: u8) -> u8:\n    both: u8 = a & b\n    either: u8 = a | 1\n    return ~(both ^ either)\n",
        ),
        (
            "shifts",
            "pub def f(a: i16, count: int, byte: u8) -> int:\n    wide: int = a << 2\n    narrow: i16 = a >> count\n    widened: int = narrow\n    return wide + (1 << byte) + widened\n",
        ),
        (
            "compound assignment",
            "pub def f(mask: u8, count: int) -> u8:\n    mut flags: u8 = 0\n    flags |= 1\n    flags &= mask\n    flags ^= 3\n    flags <<= 1\n    flags >>= count\n    return flags\n",
        ),
    ] {
        let code = crate::IrCodegen::new().try_generate(
            &parser::parse(&lexer::lex(source).map_err(|errors| format!("{case}: lex failed: {errors:?}"))?)
                .map_err(|errors| format!("{case}: parse failed: {errors:?}"))?,
        )?;
        super::mut_ownership_regressions::compile_generated_rust(&code)
            .map_err(|error| format!("{case} did not build: {error}"))?;
    }
    Ok(())
}

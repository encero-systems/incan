//! Numeric assignability and compound assignment: a decimal is assignable only where no digit is lost (#1809), two
//! decimals compare whatever their shapes (#1810), and `x op= y` takes the operator's result type from the operator
//! result table before the assignment rule applies (#1812).

use super::*;

/// Check `source`, returning every diagnostic message when the checker refuses it.
fn refusal_messages(source: &str) -> Result<Vec<String>, String> {
    match check_str(source) {
        Ok(()) => Err(format!("expected a refusal for:\n{source}")),
        Err(errors) => Ok(errors
            .into_iter()
            .map(|error| {
                let mut text = error.message;
                for note in error.notes {
                    text.push('\n');
                    text.push_str(&note);
                }
                for hint in error.hints {
                    text.push('\n');
                    text.push_str(&hint);
                }
                text
            })
            .collect()),
    }
}

/// RFC 009: `decimal128[p, s]` is an alias of `decimal[p, s]`, including in nested positions.
#[test]
fn decimal128_is_the_decimal_type_numeric_contract() -> Result<(), String> {
    let source = r#"
def preserve(value: decimal128[5, 2]) -> decimal[5, 2]:
    return value

def main() -> None:
    canonical: decimal[5, 2] = 19.99d
    aliased: decimal128[5, 2] = canonical
    values: list[decimal128[5, 2]] = [aliased]
    restored: decimal[5, 2] = values[0]
    println(preserve(restored))
"#;
    check_str(source).map_err(|errors| format!("{errors:?}"))
}

/// RFC 009: a suffixed literal has the suffix's type in inferred, destination-typed and nested positions.
#[test]
fn suffixed_numeric_literals_keep_their_named_type_numeric_contract() -> Result<(), String> {
    let source = r#"
def preserve(value: u16) -> u16:
    return value

def main() -> None:
    inferred = 42u16
    narrow: i8 = 7i8
    single = 3.14f32
    nested: tuple[u16, list[i8], f32] = (inferred, [narrow], single)
    wider: u32 = 42u16
    widened: u32 = preserve(nested[0])
    println(widened + wider)
"#;
    check_str(source).map_err(|errors| format!("{errors:?}"))
}

/// RFC 009: suffixed literals outside the suffix type's range are refused with the ordinary type diagnostic.
#[test]
fn suffixed_numeric_literals_refuse_out_of_range_values_numeric_contract() -> Result<(), String> {
    for source in [
        "def main() -> None:\n    too_large = 256u8\n",
        "def main() -> None:\n    too_small = -129i8\n",
        "def main() -> None:\n    too_large = 1e100f32\n",
    ] {
        let errors = check_str_err(source, "out-of-range suffixed literal must be refused");
        assert!(
            errors.iter().any(|error| error.message.contains("does not fit")),
            "expected a range diagnostic for:\n{source}\ngot {errors:?}"
        );
    }
    Ok(())
}

/// RFC 009: a suffixed pattern literal is held to its suffix's range and must name the matched position's exact type,
/// integer and float suffixes alike; one that names the position's type is accepted.
#[test]
fn suffixed_pattern_literals_keep_their_range_and_type_numeric_contract() -> Result<(), String> {
    for (source, expected) in [
        (
            "def main() -> None:\n    value: u8 = 1u8\n    match value:\n        256u8 => println(\"big\")\n        _ => println(\"small\")\n",
            "does not fit",
        ),
        (
            "def main() -> None:\n    value: int = 1\n    match value:\n        255u8 => println(\"u8\")\n        _ => println(\"int\")\n",
            "u8",
        ),
        (
            "def main() -> None:\n    value: float = 1.5\n    match value:\n        1.5f32 => println(\"f32\")\n        _ => println(\"float\")\n",
            "f32",
        ),
    ] {
        let errors = check_str_err(source, "a suffixed pattern literal must be refused");
        assert!(
            errors.iter().any(|error| error.message.contains(expected)),
            "expected a diagnostic naming `{expected}` for:\n{source}\ngot {errors:?}"
        );
    }
    check_str("def main() -> None:\n    value: u8 = 1u8\n    match value:\n        255u8 => println(\"max\")\n        _ => println(\"other\")\n")
        .map_err(|errors| format!("{errors:?}"))
}

/// RFC 009: the negative minimum of a signed suffix remains in range.
#[test]
fn suffixed_numeric_literal_accepts_signed_minimum_numeric_contract() -> Result<(), String> {
    check_str("def main() -> None:\n    minimum = -128i8\n").map_err(|errors| format!("{errors:?}"))
}

/// RFC 009: a decimal literal has no standalone default type and requires a decimal destination.
#[test]
fn decimal_literal_requires_a_decimal_destination_numeric_contract() -> Result<(), String> {
    for source in [
        "def main() -> None:\n    value = 19.99d\n",
        "def main() -> None:\n    value: int = 19.99d\n",
    ] {
        let errors = check_str_err(source, "context-free decimal literal must be refused");
        let Some(error) = errors
            .iter()
            .find(|error| error.message.contains("requires a decimal destination"))
        else {
            return Err(format!(
                "missing decimal-destination diagnostic for:\n{source}\ngot {errors:?}"
            ));
        };
        assert_eq!(
            crate::diagnostics::code_for_error(error, crate::diagnostics::DiagnosticPhase::Typecheck,),
            "INCAN-T0001"
        );
    }
    Ok(())
}

/// RFC 009: a zero integer part consumes no precision, including when it contains leading zeroes.
#[test]
fn decimal_literal_zero_integer_part_uses_no_digits_numeric_contract() -> Result<(), String> {
    check_str(
        r#"
def main() -> None:
    exact: decimal[5, 5] = 0.12345d
    padded: decimal[5, 5] = 00.12345d
    zero: decimal[1, 1] = 0.0d
    println(exact)
    println(padded)
    println(zero)
"#,
    )
    .map_err(|errors| format!("{errors:?}"))?;

    let errors = check_str_err(
        "def main() -> None:\n    too_large: decimal[5, 5] = 1.12345d\n",
        "a nonzero integer part must still consume precision",
    );
    assert!(
        errors.iter().any(|error| error.message.contains("allows at most 0")),
        "expected the nonzero integer digit to be refused: {errors:?}"
    );
    Ok(())
}

/// #1809: a decimal value widens to a decimal type that keeps at least its digits before the point and its scale.
#[test]
fn decimal_assignment_accepts_a_lossless_shape_issue1809() -> Result<(), String> {
    let source = r#"
def widen(value: decimal[5, 2]) -> decimal[12, 4]:
    return value

def main() -> None:
    short: decimal[3, 1] = 1.5d
    wider: decimal[4, 2] = short
    same: decimal[3, 1] = short
    alias: numeric[6, 3] = wider
    println(widen(19.99d))
"#;
    check_str(source).map_err(|errors| format!("{errors:?}"))
}

/// #1809: every decimal assignment that could lose a digit is refused, naming both shapes and the target to declare.
#[test]
fn decimal_assignment_refuses_a_lossy_shape_issue1809() -> Result<(), String> {
    for (source, expected, found) in [
        (
            "def main() -> None:\n    price: decimal[10, 2] = 12345678.90d\n    tiny: decimal[1, 1] = price\n",
            "decimal[1, 1]",
            "decimal[10, 2]",
        ),
        (
            "def narrow(value: decimal[10, 2]) -> decimal[1, 1]:\n    return value\n",
            "decimal[1, 1]",
            "decimal[10, 2]",
        ),
        (
            "def take(value: decimal[4, 0]) -> None:\n    pass\n\ndef main() -> None:\n    fine: decimal[4, 3] = 1.234d\n    take(fine)\n",
            "decimal[4, 0]",
            "decimal[4, 3]",
        ),
        (
            "def main() -> None:\n    whole: decimal[4, 0] = 1234d\n    mut fine: decimal[4, 3] = 1.234d\n    fine = whole\n",
            "decimal[4, 3]",
            "decimal[4, 0]",
        ),
    ] {
        let messages = refusal_messages(source)?;
        let names_both = |message: &&String| message.contains(expected) && message.contains(found);
        let Some(message) = messages.iter().find(names_both) else {
            return Err(format!("no diagnostic names `{expected}` and `{found}`: {messages:?}"));
        };
        assert!(
            message.contains("no decimal rounding or resize")
                && message.contains(&format!("declare the target as '{found}'")),
            "the refusal must name the shape to declare: {message}"
        );
    }
    Ok(())
}

/// #1810: two decimals compare by value whatever their precision and scale.
#[test]
fn decimal_comparison_accepts_any_shape_issue1810() -> Result<(), String> {
    let source = r#"
def main() -> None:
    a: decimal[4, 2] = 1.50d
    b: decimal[3, 1] = 1.5d
    whole: decimal[5, 0] = 12345d
    fine: decimal[5, 4] = 1.2345d
    println(a == b)
    println(b < a)
    println(whole > fine)
    println(fine != whole)
"#;
    check_str(source).map_err(|errors| format!("{errors:?}"))
}

/// #1812: compound assignment takes the operator's result type from the result table, so exact floats keep their
/// width and unsigned `//` and `%` keep their type.
#[test]
fn compound_assignment_uses_the_operator_result_table_issue1812() -> Result<(), String> {
    let source = r#"
static SCALE: f32 = 1.5

def main() -> None:
    mut s: f32 = 1.5
    s *= s
    s += s
    s -= s
    s /= s
    s //= s
    s %= s
    mut d: f64 = 2.5
    d *= d
    d *= 2.0
    mut b: u8 = 7
    b //= 2
    b %= 3
    step: u8 = 2
    b //= step
    mut wide: i128 = 1
    wide += 1
    mut n: i32 = 1
    n += 1
    n *= n
    n //= 2
    n %= 3
    SCALE *= SCALE
"#;
    check_str(source).map_err(|errors| format!("{errors:?}"))
}

/// RFC 009: exact-width modulo assignment has the same typing as assigning the corresponding binary expression.
#[test]
fn exact_width_modulo_compound_assignment_numeric_contract() -> Result<(), String> {
    check_str(
        r#"
def reduce(value: u16) -> u16:
    mut n: u16 = value
    n %= 3
    return n

def reduce_expanded(value: u16) -> u16:
    mut n: u16 = value
    n = n % 3
    return n
"#,
    )
    .map_err(|errors| format!("{errors:?}"))
}

/// #1812: the assignment rule still refuses a result the binding cannot hold: `float` into `f32`, and `float` from
/// `/=` into `int`. RFC 009 refuses an operand of another integer type, and a literal outside the binding's type.
#[test]
fn compound_assignment_refuses_a_result_the_binding_cannot_hold_issue1812() -> Result<(), String> {
    for (body, needle) in [
        (
            "    mut n: i32 = 1\n    step: int = 1\n    n += step\n",
            "Mixed-width integer arithmetic: 'i32 + int'",
        ),
        (
            "    mut b: u8 = 1\n    b += 256\n",
            "Integer literal 256 does not fit in u8",
        ),
        ("    mut s: f32 = 1.5\n    s *= 2.0\n", "expected 'f32', found 'float'"),
        ("    mut x: int = 10\n    x /= 2\n", "expected 'int', found 'float'"),
        (
            "    mut b: u8 = 7\n    divisor: int = 2\n    b //= divisor\n",
            "Mixed-width integer arithmetic: 'u8 // int'",
        ),
        (
            "    mut x: int = 7\n    divisor: u8 = 2\n    x //= divisor\n",
            "Mixed-width integer arithmetic: 'int // u8'",
        ),
    ] {
        let source = format!("def main() -> None:\n{body}");
        let messages = refusal_messages(&source)?;
        assert!(
            messages.iter().any(|message| message.contains(needle)),
            "expected `{needle}` for:\n{source}\ngot {messages:?}"
        );
    }
    Ok(())
}

/// RFC 009: same-type integer arithmetic yields that type, an unsuffixed integer literal operand takes the other
/// operand's type, `**` with a non-negative literal exponent keeps the base's type, and `/` still divides into `float`.
#[test]
fn same_type_integer_arithmetic_keeps_its_type_rfc009() -> Result<(), String> {
    let source = r#"
def main() -> None:
    x: i8 = 7
    y: i8 = 3
    p: u16 = 300
    q: u16 = 2
    big: i128 = 1
    wide: i64 = 5
    count: int = 4
    sum: i8 = x + y
    product: u16 = p * q
    literal_right: i8 = x + 1
    literal_left: i8 = 1 - x
    negative_literal: i8 = x * -2
    floor: i8 = x // 2
    rest: i8 = x % (3)
    square: i8 = x ** 2
    negated: i8 = -x
    huge: i128 = big + big
    same_identity: int = count + wide
    ordinary: int = 1 + 2
    quotient: float = x / y
    power: float = x ** y
"#;
    check_str(source).map_err(|errors| format!("{errors:?}"))
}

/// RFC 009: arithmetic over two different integer types is refused, with a hint naming the resize methods.
#[test]
fn mixed_width_integer_arithmetic_is_refused_rfc009() -> Result<(), String> {
    for (declarations, expression, needle) in [
        ("    a: i8 = 1\n    b: i16 = 2\n", "a + b", "'i8 + i16'"),
        ("    a: i8 = 1\n    b: int = 2\n", "a - b", "'i8 - int'"),
        ("    a: u8 = 1\n    b: u16 = 2\n", "a * b", "'u8 * u16'"),
        ("    a: int = 1\n    b: i32 = 2\n", "a // b", "'int // i32'"),
        ("    a: u16 = 1\n    b: u8 = 2\n", "a % b", "'u16 % u8'"),
    ] {
        let source = format!("def main() -> None:\n{declarations}    println({expression})\n");
        let messages = refusal_messages(&source)?;
        let message = messages
            .iter()
            .find(|message| message.contains("Mixed-width integer arithmetic") && message.contains(needle))
            .ok_or_else(|| {
                format!("expected a mixed-width refusal naming {needle} for:\n{source}\ngot {messages:?}")
            })?;
        assert!(
            message.contains("'resize()'") && message.contains("'try_resize()'"),
            "the refusal must name the resize methods: {message}"
        );
    }
    Ok(())
}

/// RFC 009: an integer literal operand takes its partner's type, so a value outside that type is refused, and a
/// negated unsigned value has no unsigned result.
#[test]
fn integer_literal_operand_is_checked_against_its_partner_type_rfc009() -> Result<(), String> {
    for (body, needle) in [
        (
            "    x: i8 = 1\n    println(x + 300)\n",
            "Integer literal 300 does not fit in i8",
        ),
        (
            "    b: u8 = 1\n    println(b - -1)\n",
            "Integer literal -1 does not fit in u8",
        ),
        (
            "    b: u8 = 1\n    println(-b)\n",
            "expected 'signed numeric', found 'u8'",
        ),
    ] {
        let source = format!("def main() -> None:\n{body}");
        let messages = refusal_messages(&source)?;
        assert!(
            messages.iter().any(|message| message.contains(needle)),
            "expected `{needle}` for:\n{source}\ngot {messages:?}"
        );
    }
    Ok(())
}

/// RFC 009: `float` is an alias of `f64`, so the two are one type: each is assignable to the other in both
/// directions, an `f64` holds IEEE infinity and NaN as `float` does, and `f64` arithmetic is `float` arithmetic.
#[test]
fn float_and_f64_are_one_type_rfc009() -> Result<(), String> {
    let source = r#"
const LIMIT: f64 = 1e309

def exact(value: str) -> f64:
    return float(value)

def ordinary(value: f64) -> float:
    return value

def main() -> None:
    infinite: f64 = 1e309
    suffixed: f64 = 1e309f64
    double_value: double = infinite
    ordinary_value: float = double_value
    back: fp64 = ordinary_value * 2.0
    parsed: f64 = exact("nan")
    total: float = ordinary(parsed) + back + LIMIT
    println(total)
"#;
    check_str(source).map_err(|errors| format!("{errors:?}"))
}

/// RFC 009: lossless widening converts one value, so a numeric type inside a collection, tuple, `Option` value or
/// callable matches only its own type, while a value still widens into an `Option` payload or a union member.
#[test]
fn numeric_widening_applies_to_values_not_to_types_inside_types_rfc009() -> Result<(), String> {
    for (source, needle) in [
        (
            "def f(small: list[i8]) -> list[int]:\n    return small\n",
            "expected 'List[int]', found 'List[i8]'",
        ),
        (
            "def f(pair: tuple[i8, str]) -> tuple[int, str]:\n    return pair\n",
            "expected 'Tuple[int, str]', found 'Tuple[i8, str]'",
        ),
        (
            "def f(maybe: Option[i8]) -> Option[int]:\n    return maybe\n",
            "expected 'Option[int]', found 'Option[i8]'",
        ),
        (
            "def g(value: int) -> i8:\n    return 1\n\ndef f() -> None:\n    h: (int) -> int = g\n",
            "expected '(int) -> int', found '(int) -> i8'",
        ),
    ] {
        let messages = refusal_messages(source)?;
        assert!(
            messages.iter().any(|message| message.contains(needle)),
            "expected `{needle}` for:\n{source}\ngot {messages:?}"
        );
    }
    check_str(
        r#"
def f(small: i8, values: list[int]) -> Option[int]:
    wide: list[i64] = values
    held: int | str = small
    maybe: Option[int] = small
    println(small in values)
    return small
"#,
    )
    .map_err(|errors| format!("{errors:?}"))
}

/// RFC 009: two integer types compare in the narrowest type holding both, an integer literal that fits the other
/// operand's type takes it, and a pair no integer type holds is refused.
#[test]
fn integer_comparisons_need_a_type_holding_both_rfc009() -> Result<(), String> {
    check_str(
        r#"
def f(small: i8, byte: u8, count: int, wide: u64, big: u128) -> bool:
    return small < byte and small == count and count < wide and byte != -1 and big == 0 and big > 340282366920938463463374607431768211454
"#,
    )
    .map_err(|errors| format!("{errors:?}"))?;
    let messages = refusal_messages("def f(big: u128, count: int) -> bool:\n    return big == count\n")?;
    assert!(
        messages
            .iter()
            .any(|message| message.contains("Cannot compare 'u128 == int'")),
        "got {messages:?}"
    );
    Ok(())
}

/// RFC 009: bit arithmetic over two values of one integer type keeps that type, a shift keeps its left operand's type,
/// `~` keeps its operand's type, and an unsuffixed literal operand takes the exact-width type of the other operand.
#[test]
fn bitwise_operators_keep_the_integer_type_numeric_contract() -> Result<(), String> {
    check_str(
        r#"
def masks(a: u8, b: u8, count: int, wide: u64) -> u8:
    both: u8 = a & b
    either: u8 = a | 1
    flipped: u8 = ~(a ^ b)
    shifted: u8 = a << count
    back: u64 = wide >> 2
    mut flags: u8 = 0
    flags |= 1
    flags &= b
    flags ^= 3
    flags <<= 1
    flags >>= count
    return both | either | flipped | shifted | flags
"#,
    )
    .map_err(|errors| format!("{errors:?}"))?;
    let messages = refusal_messages(
        "def f(a: u64, b: u64, small: u8, count: int) -> int:\n    c: int = a | b\n    d = small & count\n    return c\n",
    )?;
    assert!(
        messages
            .iter()
            .any(|message| message.contains("expected 'int', found 'u64'")),
        "a u64 result is not assignable to int, got {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("Mixed-width integer arithmetic: 'u8 & int'")),
        "two different integer types are refused, got {messages:?}"
    );
    Ok(())
}

/// RFC 009: a `yield` writes its value to the generator's element type as a `return` writes to the return type, so a
/// narrower numeric value is recorded for widening.
#[test]
fn yield_of_a_narrower_numeric_is_recorded_for_widening_numeric_contract() -> Result<(), String> {
    let source = "def numbers(small: i8) -> Generator[int]:\n    yield small\n    yield 2\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let start = source.find("yield small").ok_or("yield missing")? + "yield ".len();
    assert_eq!(
        checker
            .type_info()
            .value_destination_type(Span::new(start, start + "small".len())),
        Some(&ResolvedType::Int)
    );
    Ok(())
}

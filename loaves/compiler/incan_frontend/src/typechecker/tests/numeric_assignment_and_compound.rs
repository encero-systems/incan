//! Numeric assignability and compound assignment: a decimal is assignable only where no digit is lost (#1809), two
//! decimals of one constructor compare whatever their shapes (#1810), and `x op= y` takes the operator's result type
//! from the operator result table before the assignment rule applies (#1812).

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

/// #1809: `decimal128[p, s]` and `decimal[p, s]` stay separate types even when the shapes match.
#[test]
fn decimal128_is_not_assignable_to_decimal_issue1809() -> Result<(), String> {
    let messages = refusal_messages(
        "def main() -> None:\n    wide: decimal128[5, 2] = 1.25d\n    narrow: decimal[5, 2] = wide\n",
    )?;
    assert!(
        messages
            .iter()
            .any(|message| message.contains("decimal[5, 2]") && message.contains("decimal128[5, 2]")),
        "{messages:?}"
    );
    Ok(())
}

/// #1810: two decimals of one constructor compare by value whatever their shapes; a `decimal128` does not compare
/// with a `decimal`.
#[test]
fn decimal_comparison_accepts_any_shape_of_one_constructor_issue1810() -> Result<(), String> {
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
    check_str(source).map_err(|errors| format!("{errors:?}"))?;
    let messages = refusal_messages(
        "def main() -> None:\n    a: decimal[4, 2] = 1.50d\n    b: decimal128[4, 2] = 1.50d\n    println(a == b)\n",
    )?;
    assert!(
        messages.iter().any(|message| message.contains("decimal128[4, 2]")),
        "{messages:?}"
    );
    Ok(())
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
    SCALE *= SCALE
"#;
    check_str(source).map_err(|errors| format!("{errors:?}"))
}

/// #1812: the assignment rule still refuses a result the binding cannot hold: `int` into a narrower integer, `float`
/// into `f32`, `float` from `/=` into `int`, and an unsigned operand beside an `int` that is not a literal.
#[test]
fn compound_assignment_refuses_a_result_the_binding_cannot_hold_issue1812() -> Result<(), String> {
    for (body, needle) in [
        ("    mut n: i32 = 1\n    n += 1\n", "expected 'i32', found 'int'"),
        ("    mut b: u8 = 1\n    b += 1\n", "expected 'u8', found 'int'"),
        ("    mut s: f32 = 1.5\n    s *= 2.0\n", "expected 'f32', found 'float'"),
        ("    mut x: int = 10\n    x /= 2\n", "expected 'int', found 'float'"),
        (
            "    mut b: u8 = 7\n    divisor: int = 2\n    b //= divisor\n",
            "u8 or a non-negative integer literal",
        ),
        (
            "    mut x: int = 7\n    divisor: u8 = 2\n    x //= divisor\n",
            "u8 or a non-negative integer literal",
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

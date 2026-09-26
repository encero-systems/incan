//! A literal written to a typed destination is emitted as a value of the destination's Rust type: a tuple literal's
//! `None` and `Ok(...)` elements are typed from the annotation (#1847), an empty or `None`-only list literal assigned
//! to an `Option` or union binding is wrapped (#1832), and an integer literal assigned to a `float` binding is a float
//! literal (#1831).

use crate::codegen::IrCodegen;
use incan_frontend::{lexer, parser};

/// Generate Rust for one checked source, with every run of whitespace collapsed to one space.
fn generate_collapsed(source: &str) -> Result<String, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?;
    let code = IrCodegen::new()
        .try_generate(&program)
        .map_err(|error| format!("generation failed: {error:?}"))?;
    Ok(code.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Assert that `code` contains every snippet in `expected`.
fn assert_contains_all(code: &str, expected: &[&str]) {
    for snippet in expected {
        assert!(code.contains(snippet), "missing `{snippet}` in:\n{code}");
    }
}

/// #1847: the elements of an annotated tuple literal are typed from the annotation, so `None` is an `Option<String>`
/// and `Ok(1)` a `Result<i64, i64>`, and an integer element in a `float` slot is a float literal.
#[test]
fn annotated_tuple_literal_types_its_elements_issue1847() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
def main() -> None:
    pair: tuple[Option[str], int] = (None, 1)
    ok_pair: tuple[Result[int, int], int] = (Ok(1), 0)
    floats: tuple[float, int] = (1, 2)
    println(pair[1] + ok_pair[1] + floats[1])
"#,
    )?;
    assert_contains_all(
        &code,
        &[
            "let pair: (Option<String>, i64) = (None::<String>, 1);",
            "let ok_pair: (Result<i64, i64>, i64) = (Ok::<i64, i64>(1), 0);",
            "let floats: (f64, i64) = (1.0, 2);",
        ],
    );
    Ok(())
}

/// #1847: inside a generic body, a `None` element of an annotated tuple or list literal is typed with the body's own
/// type parameter, not `()`.
#[test]
fn annotated_literal_in_a_generic_body_types_its_none_issue1847() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
def mk[T](x: T) -> int:
    pair: tuple[T, Option[T]] = (x, None)
    items: list[Option[T]] = [None]
    return len(items)


def main() -> None:
    println(mk("a"))
"#,
    )?;
    assert_contains_all(
        &code,
        &[
            "let _pair: (T, Option<T>) = (x, None::<T>);",
            "let items: Vec<Option<T>> = vec![None:: < T >];",
        ],
    );
    assert!(!code.contains("None::<()>"), "no `None` may be typed `()`:\n{code}");
    Ok(())
}

/// #1832: an empty or `None`-only list literal assigned to an existing `Option[list[...]]` binding is wrapped in
/// `Some` with its element type, and an empty list assigned to a `list[int] | str` binding is wrapped in the union's
/// list variant.
#[test]
fn list_literal_assigned_to_option_or_union_binding_is_wrapped_issue1832() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
def main() -> None:
    mut a: Option[list[int]] = None
    a = []
    mut b: Option[list[Option[str]]] = None
    b = [None]
    mut u: list[int] | str = "x"
    u = []
    println(len(a.unwrap_or([])) + len(b.unwrap_or([])))
"#,
    )?;
    assert_contains_all(
        &code,
        &[
            "a = Some(Vec::<i64>::new());",
            "b = Some(vec![None:: < String >]);",
            "::V1(Vec::<i64>::new());",
        ],
    );
    Ok(())
}

/// #1831: an integer literal assigned to an existing `float` binding, or to an `f32` binding through a negation, is
/// written as a float literal, as it is in a `float` declaration.
#[test]
fn integer_literal_assigned_to_float_binding_is_a_float_literal_issue1831() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
def main() -> None:
    declared: float = 3
    mut f: float = 0.5
    f = 1
    mut g: f32 = 0.5
    g = -2
    println(declared + f + g)
"#,
    )?;
    assert_contains_all(
        &code,
        &[
            "let declared: f64 = 3.0;",
            "f = 1.0;",
            "g = incan_std_core::num::require_finite_f32(-2.0);",
        ],
    );
    Ok(())
}

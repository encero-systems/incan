//! A literal written to a typed destination is emitted as a value of the destination's Rust type: a tuple literal's
//! `None` and `Ok(...)` elements are typed from the annotation (#1847), an empty or `None`-only list literal assigned
//! to an `Option` or union binding is wrapped (#1832), and an integer literal assigned to a `float` binding, alone or
//! in a chained assignment (#1831), written to a float element (#1854) or passed to a float slot elsewhere (#1859), is
//! a float literal. A value written to an `Option` field, element or return type (#1858), or to a nested `Option`
//! binding (#1860), is wrapped in each `Some` layer; an empty list passed to a generic parameter is written with the
//! instantiated element type (#1862); and `Option[str].unwrap_or` takes an owned fallback (#1875).

use super::generated_programs::run_with_stdlib;
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

/// #1831: a chained assignment of an integer literal over an `int` binding and a `float` binding writes the integer
/// literal to the `int` binding and the float literal to the `float` binding, whichever of them comes last.
#[test]
fn chained_integer_literal_is_written_in_each_targets_type_issue1831() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
def main() -> None:
    mut n: int = 0
    mut f: float = 0.5
    n = f = 1
    f = n = 2
    println(n)
    println(f)
"#,
    )?;
    assert_contains_all(&code, &["n = 1;", "f = 1.0;", "f = 2.0;", "n = 2;"]);
    assert!(
        !code.contains("n = 1.0;") && !code.contains("n = 2.0;"),
        "the `int` binding takes an integer literal:\n{code}"
    );
    Ok(())
}

/// Return the statement of `code` that starts with `prefix`, through its closing `;`.
fn statement<'code>(code: &'code str, prefix: &str) -> Result<&'code str, String> {
    let start = code
        .find(prefix)
        .ok_or_else(|| format!("missing `{prefix}` in:\n{code}"))?;
    let end = code[start..]
        .find(';')
        .map(|offset| start + offset + 1)
        .ok_or_else(|| format!("unterminated `{prefix}` in:\n{code}"))?;
    Ok(&code[start..end])
}

/// #1854: an integer literal written to an element of a `list[float]`, directly or through a negation, or to a value
/// of a `dict[str, float]`, is a float literal.
#[test]
fn integer_literal_written_to_a_float_element_is_a_float_literal_issue1854() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
def main() -> None:
    mut xs: list[float] = [1, 2]
    xs[0] = 3
    xs[1] = -4
    mut scores: dict[str, float] = {}
    scores["a"] = 5
    println(xs[0] + xs[1] + scores["a"])
"#,
    )?;
    assert_contains_all(
        &code,
        &[
            "list_get_mut(&mut xs, (0) as i64) = 3.0;",
            "list_get_mut(&mut xs, (1) as i64) = -4.0;",
            "scores.insert(\"a\".to_string(), 5.0);",
        ],
    );
    Ok(())
}

/// #1858: a value of an `Option`'s payload type written to an `Option` return type, field, constructor field or list
/// element is wrapped in `Some`; a value already of the `Option` type is written as it is.
#[test]
fn value_written_to_an_option_place_is_wrapped_in_some_issue1858() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
model Box:
    items: Option[list[int]] = None
    count: Option[int] = None
    name: Option[str] = None


def make() -> Option[int]:
    return 5


def label(text: str) -> Option[str]:
    return text


def main() -> None:
    mut box = Box(count=1)
    box.items = [1]
    box.name = "x"
    box.count = Some(2)
    mut slots: list[Option[int]] = [None]
    slots[0] = 3
    println(make().unwrap_or(0) + slots[0].unwrap_or(0))
    println(label("a").unwrap_or(""))
"#,
    )?;
    assert_contains_all(
        &code,
        &[
            "return Some(5);",
            "return Some(text);",
            "count: Some(1)",
            "r#box.items = Some(vec![1]);",
            "r#box.name = Some(\"x\".to_string());",
            "r#box.count = Some(2);",
            "list_get_mut(&mut slots, (0) as i64) = Some(3);",
        ],
    );
    assert!(!code.contains("Some(Some("), "no value may be wrapped twice:\n{code}");
    Ok(())
}

/// #1859: an integer literal is a float literal at an `Option[float]` or `float | str` destination, an `Option[float]`
/// parameter, the payload of `Ok` at a `Result[float, str]`, the argument of `append`, `insert` and `unwrap_or` on a
/// float collection or `Option`, the exponent of `powf`, and an argument of a generic call whose other argument binds
/// its type parameter to `float`.
#[test]
fn integer_literal_in_more_float_slots_is_a_float_literal_issue1859() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
def pick[T](a: T, b: T) -> T:
    return a


def takes(value: Option[float]) -> float:
    return value.unwrap_or(0.0)


def main() -> None:
    x = 1.5
    squared = x.powf(2)
    maybe: Option[float] = 3
    either: float | str = 4
    result: Result[float, str] = Ok(5)
    mut xs: list[float] = []
    xs.append(6)
    mut d: dict[str, float] = {}
    d.insert("k", 7)
    fallback = maybe.unwrap_or(8)
    first = pick(2.5, 9)
    second = pick(10, 2.5)
    println(squared + takes(11) + fallback + first + second + xs[0] + d["k"] + result.unwrap_or(0.0))
"#,
    )?;
    assert_contains_all(
        &code,
        &[
            "x.powf(2.0)",
            "let maybe: Option<f64> = Some(3.0);",
            "let result: Result<f64, String> = Ok::<f64, String>(5.0);",
            ".push(6.0);",
            ".insert(\"k\".to_string(), 7.0);",
            "maybe.unwrap_or(8.0);",
            "2.5, 9.0,",
            "10.0, 2.5,",
            "(Some(11.0))",
        ],
    );
    let either = statement(&code, "let _either:")?;
    assert!(
        either.contains("(4.0)"),
        "the union binding holds the float 4.0: {either}"
    );
    Ok(())
}

/// #1860: a value assigned to a binding of a nested `Option` type is wrapped in one `Some` per layer, in a declaration
/// and a reassignment, for a scalar, an empty list and a list; a value one layer short and a `None` keep one wrap and
/// none.
#[test]
fn value_assigned_to_a_nested_option_binding_is_wrapped_in_each_layer_issue1860() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
def main() -> None:
    a: Option[Option[int]] = 5
    mut b: Option[Option[list[int]]] = None
    b = []
    c: Option[Option[list[int]]] = [1]
    one: Option[int] = 6
    println(a.unwrap().unwrap() + one.unwrap_or(0))
    println(len(b.unwrap().unwrap_or([])) + len(c.unwrap().unwrap_or([])))
"#,
    )?;
    assert_contains_all(
        &code,
        &[
            "let a: Option<Option<i64>> = Some(Some(5));",
            "let mut b: Option<Option<Vec<i64>>> = None::<Option<Vec<i64>>>;",
            "b = Some(Some(Vec::<i64>::new()));",
            "let c: Option<Option<Vec<i64>>> = Some(Some(vec![1]));",
            "let one: Option<i64> = Some(6);",
        ],
    );
    Ok(())
}

/// #1862: an empty list literal passed to a generic `list[T]` parameter whose `T` another argument or an explicit type
/// argument binds is written with that element type, never with the callee's `T`.
#[test]
fn empty_list_argument_is_written_with_the_instantiated_element_type_issue1862() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
def first_or[T](items: list[T], default: T) -> T:
    return default


def count[T](items: list[T]) -> int:
    return len(items)


def main() -> None:
    value = first_or([], 5)
    size = count[str]([])
    println(value + size)
"#,
    )?;
    assert_contains_all(&code, &["Vec::<i64>::new(), 5,", "(Vec::<String>::new());"]);
    assert!(
        !code.contains("Vec::<T>") && !code.contains("Vec:: < T >"),
        "the caller cannot name the callee's `T`:\n{code}"
    );
    Ok(())
}

/// #1875: `unwrap_or` on an `Option[str]` passes a `str` binding as the owned fallback, not a reference to it, in a
/// call argument, an assignment and a `return`.
#[test]
fn option_str_unwrap_or_passes_an_owned_fallback_issue1875() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
def pick(value: Option[str]) -> str:
    missing = "none"
    return value.unwrap_or(missing)


def main() -> None:
    d: Option[str] = None
    missing = "none"
    text = d.unwrap_or(missing)
    println(text)
    println(pick(None))
"#,
    )?;
    assert_contains_all(&code, &["return value.unwrap_or(missing", "d.unwrap_or(missing"]);
    assert!(
        !code.contains("unwrap_or(&missing)") && !code.contains("unwrap_or(& missing)"),
        "the fallback must not be a reference:\n{code}"
    );
    Ok(())
}

/// A value passed to a nested `Option` parameter is wrapped once for every missing layer.
#[test]
fn nested_option_call_argument_is_wrapped_in_each_layer_followups_a() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
def nested(value: Option[Option[int]]) -> int:
    return value.unwrap().unwrap_or(0)


def main() -> None:
    println(nested(5))
"#,
    )?;
    assert!(
        code.contains("Some(Some(5))"),
        "the argument needs two Some layers:\n{code}"
    );
    Ok(())
}

/// `None` inside an explicit `Some` takes the nested destination's payload type.
#[test]
fn some_none_at_nested_option_destination_has_the_inner_type_followups_a() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
def main() -> None:
    value: Option[Option[int]] = Some(None)
    println(value.unwrap_or(None).unwrap_or(0))
"#,
    )?;
    assert!(
        code.contains("Some(None::<i64>)"),
        "the inner None needs the nested payload type:\n{code}"
    );
    assert!(
        !code.contains("None::<()"),
        "no nested None may be typed as unit:\n{code}"
    );
    Ok(())
}

/// A string literal assigned to an owned `str` field is materialized as a `String`.
#[test]
fn string_literal_assigned_to_str_field_is_owned_followups_a() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
model Box:
    label: str


def main() -> None:
    mut box = Box(label="x")
    box.label = "y"
    println(box.label)
"#,
    )?;
    assert!(
        code.contains("r#box.label = \"y\".to_string();"),
        "the field write needs an owned string:\n{code}"
    );
    Ok(())
}

/// #1986: a direct assignment of `None` to a generic model's `Option[T]` field keeps `T` as the stored payload type.
#[test]
fn generic_option_field_none_assignment_statement_issue1986() -> Result<(), Box<dyn std::error::Error>> {
    let stdout = run_with_stdlib(
        r#"
model Slot[T]:
    pub current: Option[T] = None

    def clear(mut self) -> None:
        self.current = None


def main() -> None:
    mut slot = Slot[int]()
    slot.clear()
    match slot.current:
        Some(value) => println(value)
        None => println(0)
"#,
    )?;
    assert_eq!(stdout, "0\n");
    Ok(())
}

/// #1986: an assignment arm keeps the declared `Option[T]` field type instead of the arm's unit result type.
#[test]
fn generic_option_field_none_assignment_match_arm_issue1986() -> Result<(), Box<dyn std::error::Error>> {
    let stdout = run_with_stdlib(
        r#"
model Slot[T]:
    pub current: Option[T] = None

    def clear(mut self, requested: bool) -> None:
        match requested:
            true => self.current = None
            false => return


def main() -> None:
    mut slot = Slot[int]()
    slot.clear(true)
    match slot.current:
        Some(value) => println(value)
        None => println(0)
"#,
    )?;
    assert_eq!(stdout, "0\n");
    Ok(())
}

/// #1986: a nested assignment arm keeps the declared `Option[T]` field type through both enclosing matches.
#[test]
fn generic_option_field_none_assignment_nested_match_issue1986() -> Result<(), Box<dyn std::error::Error>> {
    let stdout = run_with_stdlib(
        r#"
model Slot[T]:
    pub current: Option[T] = None

    def clear(mut self, outer: bool, inner: bool) -> None:
        match outer:
            true =>
                match inner:
                    true => self.current = None
                    false => return
            false => return


def main() -> None:
    mut slot = Slot[int]()
    slot.clear(true, true)
    match slot.current:
        Some(value) => println(value)
        None => println(0)
"#,
    )?;
    assert_eq!(stdout, "0\n");
    Ok(())
}

/// A method call on a copied list element applies the prefix dereference to the lookup before the method call.
#[test]
fn indexed_option_method_call_groups_the_lookup_followups_a() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
def main() -> None:
    slots: list[Option[int]] = [Some(1)]
    println(slots[0].unwrap_or(0))
"#,
    )?;
    assert!(
        code.contains("{ * incan_std_core::collections::list_get") && code.contains("} .unwrap_or(0)"),
        "the lookup must be grouped before unwrap_or:\n{code}"
    );
    Ok(())
}

/// A type that adopts `Error` and has no `__str__` gets a Rust `Display` that writes its `message()`, so a generic
/// display through a `Display` bound compiles and shows the message.
#[test]
fn error_message_display_implements_rust_display_followups_a() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
from std.traits.error import Error


model Failure with Error:
    detail: str

    def message(self) -> str:
        return self.detail


def show[T with Display](value: T) -> str:
    return f"{value}"


def written(failure: Failure) -> str:
    return failure.message()


def main() -> None:
    println(show(Failure(detail="bad")))
    println(written(Failure(detail="bad")))
"#,
    )?;
    // A long method name is wrapped onto its own line before the dot, which collapses to ` .`.
    let code = code.replace(" .", ".");
    let message_method = code
        .split("return failure.")
        .nth(1)
        .and_then(|rest| rest.split("()").next())
        .ok_or_else(|| format!("the written `failure.message()` must be a method call:\n{code}"))?;
    assert_contains_all(
        &code,
        &[
            "T: std::fmt::Display",
            "impl std::fmt::Display for Failure { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { \
             write!(f, \"{}\", self.__str__()) } }",
            &format!("fn __str__(&self) -> String {{ return self.{message_method}(); }}"),
        ],
    );
    Ok(())
}

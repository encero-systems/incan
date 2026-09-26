//! Literal sub-patterns checked against the type of their position, and variant coverage that counts a constructor
//! arm only as far as its payload patterns cover (#1741).

use super::*;

/// Return the messages of every diagnostic a refused source produces, or say that it was accepted.
fn refusal_messages(source: &str) -> Result<Vec<String>, String> {
    match check_str(source) {
        Ok(()) => Err("the checker accepted a program it must refuse".to_string()),
        Err(errors) => Ok(errors.into_iter().map(|error| error.message).collect()),
    }
}

/// Accept a source or return its diagnostics as the failure.
fn accepted(source: &str) -> Result<(), String> {
    check_str(source).map_err(|errors| {
        format!(
            "expected the program to check, got: {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        )
    })
}

/// #1741: a `str` literal in an `int` tuple position can never match, so the arm is refused with both types named.
#[test]
fn literal_sub_pattern_of_another_type_is_refused_issue1741() -> Result<(), String> {
    let messages = refusal_messages(
        r#"
def classify(pair: tuple[int, int]) -> str:
    match pair:
        (0, "a") => return "x"
        _ => return "y"
"#,
    )?;
    if messages.iter().any(|message| {
        message.contains("Pattern type mismatch") && message.contains("'str'") && message.contains("'int'")
    }) {
        Ok(())
    } else {
        Err(format!(
            "expected a pattern literal mismatch naming str and int, got: {messages:?}"
        ))
    }
}

/// #1741: literals whose type fits their position keep checking, at the top level and nested in tuples and payloads.
#[test]
fn literal_sub_patterns_of_the_position_type_are_accepted_issue1741() -> Result<(), String> {
    accepted(
        r#"
def classify(pair: tuple[int, str]) -> str:
    match pair:
        (0, "a") => return "x"
        _ => return "y"

def sign(n: int) -> str:
    match n:
        0 => return "zero"
        _ => return "other"

def flag(value: Option[bool]) -> str:
    match value:
        Some(true) => return "yes"
        Some(false) => return "no"
        None => return "unknown"

def ratio(value: float) -> str:
    match value:
        1.5 => return "one and a half"
        _ => return "other"

def byte(value: u8) -> str:
    match value:
        255 => return "max"
        _ => return "other"
"#,
    )
}

/// #1741: a numeric literal is held to the position's width and family: `300` does not fit `u8`, `1.5` is not an
/// integer, `1` is an `int` rather than a float, and decimal and bytes literals have no pattern form at all.
#[test]
fn numeric_and_unmatchable_literal_patterns_are_refused_issue1741() -> Result<(), String> {
    for (source, needle) in [
        (
            "def f(value: u8) -> str:\n    match value:\n        300 => return \"big\"\n        _ => return \"other\"\n",
            "does not fit in u8",
        ),
        (
            "def f(value: u8) -> str:\n    match value:\n        1.5 => return \"odd\"\n        _ => return \"other\"\n",
            "Pattern type mismatch",
        ),
        (
            "def f(value: float) -> str:\n    match value:\n        1 => return \"one\"\n        _ => return \"other\"\n",
            "Pattern type mismatch",
        ),
        (
            "def f(value: f32) -> str:\n    match value:\n        1 => return \"one\"\n        _ => return \"other\"\n",
            "Pattern type mismatch",
        ),
        (
            "def f(value: decimal[10, 2]) -> str:\n    match value:\n        1.50d => return \"price\"\n        _ => return \"other\"\n",
            "decimal literal cannot be used as a match pattern",
        ),
        (
            "def f(value: bytes) -> str:\n    match value:\n        b\"ab\" => return \"ab\"\n        _ => return \"other\"\n",
            "bytes literal cannot be used as a match pattern",
        ),
    ] {
        let messages = refusal_messages(source)?;
        if !messages.iter().any(|message| message.contains(needle)) {
            return Err(format!("expected `{needle}` for {source:?}, got: {messages:?}"));
        }
    }
    Ok(())
}

/// #1741: a match over a scalar or a tuple whose arms are all literals leaves values unmatched and is refused; a
/// wildcard or a name arm, or literals that cover every value, make it exhaustive.
#[test]
fn literal_only_matches_over_open_values_need_a_catch_all_issue1741() -> Result<(), String> {
    for source in [
        "def f(n: int) -> str:\n    match n:\n        0 => return \"zero\"\n        1 => return \"one\"\n",
        "def f(s: str) -> str:\n    match s:\n        \"a\" => return \"a\"\n",
        "def f(pair: tuple[int, bool]) -> str:\n    match pair:\n        (0, true) => return \"a\"\n        (_, false) => return \"b\"\n",
    ] {
        let messages = refusal_messages(source)?;
        if !messages.iter().any(|message| message.contains("Non-exhaustive match")) {
            return Err(format!(
                "expected a non-exhaustive match for {source:?}, got: {messages:?}"
            ));
        }
    }
    accepted(
        r#"
def f(n: int) -> str:
    match n:
        0 => return "zero"
        _ => return "other"

def g(flag: bool) -> str:
    match flag:
        true => return "yes"
        false => return "no"

def h(pair: tuple[int, bool]) -> str:
    match pair:
        (0, true) => return "a"
        (_, true) => return "b"
        (n, false) => return f"{n}"
"#,
    )
}

/// #1741: a `None` literal where the position is not an `Option`, and an `int` literal where it is one, are refused.
#[test]
fn none_literal_and_option_position_mismatches_are_refused_issue1741() -> Result<(), String> {
    let messages = refusal_messages(
        r#"
def label(n: int) -> str:
    match n:
        None => return "none"
        _ => return "some"
"#,
    )?;
    if !messages.iter().any(|message| message.contains("Pattern type mismatch")) {
        return Err(format!("expected a mismatch for None over int, got: {messages:?}"));
    }
    let messages = refusal_messages(
        r#"
def label(value: Option[int]) -> str:
    match value:
        0 => return "zero"
        _ => return "other"
"#,
    )?;
    if messages.iter().any(|message| message.contains("Pattern type mismatch")) {
        Ok(())
    } else {
        Err(format!(
            "expected a mismatch for an int literal over Option[int], got: {messages:?}"
        ))
    }
}

/// #1741: `Some(0)` covers one payload of `Some`, so without another `Some` arm the match misses `Some(_)`.
#[test]
fn constructor_with_a_refutable_payload_is_partial_coverage_issue1741() -> Result<(), String> {
    let messages = refusal_messages(
        r#"
def describe(value: Option[int]) -> str:
    match value:
        Some(0) => return "zero"
        None => return "none"
"#,
    )?;
    if messages
        .iter()
        .any(|message| message.contains("Non-exhaustive match") && message.contains("Some(_)"))
    {
        Ok(())
    } else {
        Err(format!("expected the match to miss Some(_), got: {messages:?}"))
    }
}

/// #1741: an enum variant whose only arm names one payload value is reported, spelled with its payload wildcard.
#[test]
fn enum_variant_with_a_literal_payload_is_partial_coverage_issue1741() -> Result<(), String> {
    let messages = refusal_messages(
        r#"
enum Shape:
    Circle(int)
    Square(int)

def area(shape: Shape) -> int:
    match shape:
        Shape.Circle(0) => return 0
        Shape.Square(side) => return side * side
"#,
    )?;
    if messages
        .iter()
        .any(|message| message.contains("Non-exhaustive match") && message.contains("Circle(_)"))
    {
        Ok(())
    } else {
        Err(format!("expected the match to miss Circle(_), got: {messages:?}"))
    }
}

/// #1741: payload patterns that are refutable one by one but cover the payload together still cover the variant.
#[test]
fn payload_patterns_that_cover_together_are_exhaustive_issue1741() -> Result<(), String> {
    accepted(
        r#"
enum Light:
    Red
    Green

model Point:
    x: int
    y: int

def nested(value: Result[Option[int], str]) -> int:
    match value:
        Ok(Some(n)) => return n
        Ok(None) => return 0
        Err(_) => return -1

def flags(value: Option[bool]) -> int:
    match value:
        Some(true) => return 1
        Some(false) => return 0
        None => return -1

def lights(value: Option[Light]) -> int:
    match value:
        Some(Light.Red) => return 1
        Some(Light.Green) => return 2
        None => return 0

def literal_then_binding(value: Option[int]) -> int:
    match value:
        Some(0) => return 0
        Some(n) => return n
        None => return -1

def pairs(value: Option[tuple[bool, int]]) -> int:
    match value:
        Some((true, n)) => return n
        Some((false, _)) => return 0
        None => return -1

def points(value: Option[Point]) -> int:
    match value:
        Some(Point(x=x, y=y)) => return x + y
        None => return 0

def alternatives(value: Option[int]) -> int:
    match value:
        Some(0) | Some(1) => return 0
        Some(_) => return 1
        None => return -1
"#,
    )
}

/// #1741: a guarded arm never counts toward coverage, even when its payload pattern would cover the variant.
#[test]
fn guarded_payload_arm_does_not_cover_its_variant_issue1741() -> Result<(), String> {
    let messages = refusal_messages(
        r#"
def describe(value: Option[int]) -> str:
    match value:
        Some(n) if n > 0 => return "positive"
        None => return "none"
"#,
    )?;
    if messages.iter().any(|message| message.contains("Non-exhaustive match")) {
        Ok(())
    } else {
        Err(format!("expected a non-exhaustive match, got: {messages:?}"))
    }
}

/// #1741: a match over a generic enum names the variant it misses, as a match over a non-generic enum does, and arms
/// that cover every variant of the instantiated enum are exhaustive.
#[test]
fn generic_enum_subject_names_its_missing_variant_issue1741() -> Result<(), String> {
    let messages = refusal_messages(
        r#"
enum Shape[T]:
    Filled(T)
    Empty

def area(shape: Shape[int]) -> int:
    match shape:
        Filled(n) => return n
"#,
    )?;
    if !messages
        .iter()
        .any(|message| message.contains("Non-exhaustive match: missing patterns for Empty"))
    {
        return Err(format!("expected the refusal to name 'Empty', got: {messages:?}"));
    }
    accepted(
        r#"
enum Shape[T]:
    Filled(T)
    Empty

def area(shape: Shape[int]) -> int:
    match shape:
        Filled(0) => return 0
        Filled(n) => return n
        Empty => return -1
"#,
    )
}

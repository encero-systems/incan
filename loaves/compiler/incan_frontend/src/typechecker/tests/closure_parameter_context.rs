//! A closure's parameter types come from the function type its context gives it (#1561): the element type of an
//! RFC 088 iterator adapter or terminal, a fold's accumulator, and a generic parameter's function type once the other
//! arguments fix its type parameters. A capturing closure is refused where a lazy adapter stores it and as the
//! callback of other standard-library functions and methods, and accepted by a `Result` combinator, which only calls
//! it.

use super::*;

/// Check a program and return the messages of its errors.
fn error_messages(source: &str) -> Vec<String> {
    check_str(source)
        .err()
        .unwrap_or_default()
        .into_iter()
        .map(|error| error.message)
        .collect()
}

/// Fail unless checking `source` reports an error containing each of `needles`.
fn assert_refused(source: &str, needles: &[&str]) {
    let messages = error_messages(source);
    for needle in needles {
        assert!(
            messages.iter().any(|message| message.contains(needle)),
            "expected an error containing `{needle}`, got {messages:?}\n{source}"
        );
    }
}

/// A closure without parameter annotations passed to an adapter, a terminal, a fold or a generic function takes the
/// parameter types its context gives: arithmetic, string methods and comparisons on its parameters are checked
/// against them.
#[test]
fn closure_parameters_take_the_types_their_context_gives_issue1561() {
    assert_check_ok(
        r#"
def apply_twice[T](f: (T) -> T, value: T) -> T:
    return f(f(value))

def numbers() -> Generator[int]:
    yield 1
    yield 2

def main() -> None:
    items = [1, 2, 3]
    names = ["ada", "lin"]
    doubled = items.iter().map((x) => x * 2).collect()
    shouted: list[str] = names.iter().map((name) => name.upper()).collect()
    letters: list[str] = names.iter().flat_map((name) => [name, name.upper()]).collect()
    big = items.iter().filter((x) => x % 2 == 1).collect()
    small = items.iter().take_while((x) => x * 2 < 5).collect()
    has_long = names.iter().any((name) => len(name.strip()) > 2)
    first = names.iter().find((name) => name.startswith("l"))
    total = items.iter().fold(0, (acc, x) => acc + x * 10)
    items.iter().for_each((x) => println(x + 1))
    println(apply_twice((x) => x + 1, 3))
    println(apply_twice((text) => text.upper(), "a"))
    evens = numbers().filter((x) => x % 2 == 0).collect()
    println(doubled)
    println(shouted)
    println(letters)
    println(big)
    println(small)
    println(has_long)
    println(first is not None)
    println(total)
    println(evens)
"#,
    );
}

/// The parameter types a context gives are checked like declared ones: an `int` element has no `upper`, and a
/// generic parameter fixed to `int` by the other argument does not take a `str` method either.
#[test]
fn closure_parameters_typed_by_their_context_refuse_what_the_type_lacks_issue1561() {
    assert_refused(
        r#"
def main() -> None:
    items = [1, 2, 3]
    shouted = items.iter().map((x) => x.upper()).collect()
"#,
        &["Type 'int' has no method 'upper"],
    );
    assert_refused(
        r#"
def apply_twice[T](f: (T) -> T, value: T) -> T:
    return f(f(value))

def main() -> None:
    println(apply_twice((x) => x.upper(), 3))
"#,
        &["Type 'int' has no method 'upper"],
    );
    let refused = check_str(
        r#"
def main() -> None:
    names = ["ada", "lin"]
    bad = names.iter().map((name) => name + 1).collect()
"#,
    )
    .err()
    .unwrap_or_default();
    assert!(
        refused.iter().any(|error| error.message.contains("str")
            && crate::diagnostics::code_for_error(error, crate::diagnostics::DiagnosticPhase::Typecheck)
                == "INCAN-T0001"),
        "a str parameter plus an int is refused with INCAN-T0001: {refused:?}"
    );
}

/// `map`, `filter`, `flat_map`, `take_while` and `skip_while` keep their callback in the lazy iterator they return, as
/// a function pointer, so a closure that captures local values, or a local holding one, is refused there. A closure
/// that captures nothing and a named function are taken.
#[test]
fn capturing_closures_stored_by_iterator_adapters_are_refused_issue1561() {
    for adapter in [
        "items.iter().map((x) => x * n).collect()",
        "items.iter().filter((x) => x > n).collect()",
        "items.iter().flat_map((x) => [x, n]).collect()",
        "items.iter().take_while((x) => x < n).collect()",
        "items.iter().skip_while((x) => x < n).collect()",
    ] {
        let source = format!("def main() -> None:\n    n = 2\n    items = [1, 2, 3]\n    kept = {adapter}\n");
        assert_refused(
            &source,
            &["A closure that captures local values cannot be stored by the iterator adapter"],
        );
    }
    assert_refused(
        r#"
def main() -> None:
    n = 2
    items = [1, 2, 3]
    scale = (x) => x * n
    scaled = items.iter().map(scale).collect()
"#,
        &["'scale', which holds a closure that captures local values, cannot be stored by the iterator adapter 'map'"],
    );
    assert_check_ok(
        r#"
def small(x: int) -> bool:
    return x < 2

def main() -> None:
    items = [1, 2, 3]
    doubled = items.iter().map((x) => x * 2).collect()
    kept = items.iter().filter(small).collect()
    println(doubled)
    println(kept)
"#,
    );
}

/// A callback of an iterator terminal or of a generator's own `map` and `filter`, and a function-typed parameter of a
/// function the standard-library source declares, are parameters of a library function, which holds a named function
/// or a closure that captures nothing, so a closure that captures local values, or a local holding one, is refused
/// there; a function of this module that only calls its parameter takes one (#1561).
#[test]
fn capturing_closures_passed_to_standard_library_callbacks_are_refused_issue1561() {
    for (call, method) in [
        ("items.iter().any((x) => x == n)", "any"),
        ("items.iter().all((x) => x > n)", "all"),
        ("items.iter().find((x) => x > n)", "find"),
        ("items.iter().fold(0, (acc, x) => acc + x * n)", "fold"),
        ("items.iter().reduce(0, (acc, x) => acc + x * n)", "reduce"),
        ("items.iter().for_each((x) => println(x + n))", "for_each"),
    ] {
        let source = format!("def main() -> None:\n    n = 2\n    items = [1, 2, 3]\n    kept = {call}\n");
        assert_refused(
            &source,
            &[&format!(
                "A closure that captures local values cannot be passed to the standard-library method '{method}'"
            )],
        );
    }
    for (call, method) in [
        ("numbers().map((x) => x * n).collect()", "map"),
        ("numbers().filter((x) => x < n).collect()", "filter"),
    ] {
        let source = format!(
            "def numbers() -> Generator[int]:\n    yield 1\n    yield 2\n\n\
             def main() -> None:\n    n = 2\n    kept = {call}\n"
        );
        assert_refused(
            &source,
            &[&format!(
                "A closure that captures local values cannot be stored by the iterator adapter '{method}'"
            )],
        );
    }
    assert_refused(
        r#"
def main() -> None:
    n = 2
    items = [1, 2, 3]
    at_least = (x) => x >= n
    println(items.iter().any(at_least))
"#,
        &[
            "'at_least', which holds a closure that captures local values, cannot be passed to the standard-library method 'any'",
        ],
    );
    assert_refused(
        r#"
from std.testing import assert_raises

def check(text: str) -> None:
    int(text)

def main() -> None:
    text = "x"
    assert_raises[ValueError](() => check(text))
"#,
        &["A closure that captures local values cannot be passed to the parameter 'block' of 'assert_raises'"],
    );
    assert_check_ok(
        r#"
def apply(f: (int) -> int, x: int) -> int:
    return f(x)

def numbers() -> Generator[int]:
    yield 1
    yield 2

def is_odd(x: int) -> bool:
    return x % 2 == 1

def main() -> None:
    n = 2
    items = [1, 2, 3]
    println(apply((x) => x + n, 1))
    println(items.iter().any(is_odd))
    println(items.iter().fold(0, (acc, x) => acc + x))
    println(numbers().map((x) => x * 2).collect())
    println(numbers().filter(is_odd).collect())
"#,
    );
}

/// A closure that captures local values is accepted as the callback of each `Result` combinator (`map`, `map_err`,
/// `and_then`, `or_else`, `inspect`, `inspect_err`), which calls it before it returns and keeps no function pointer
/// (#1561).
#[test]
fn capturing_closures_passed_to_result_combinators_are_accepted_issue1561() {
    for statement in [
        "println(passed.map((x) => x + n).unwrap_or(0))",
        "println(failed.map_err((m) => m + suffix).unwrap_or(0))",
        "println(passed.and_then((x) => Ok(x * n)).unwrap_or(0))",
        "println(failed.or_else((m) => Ok(len(m) + n)).unwrap_or(0))",
        "println(passed.inspect((x) => println(x + n)).unwrap_or(0))",
        "println(failed.inspect_err((m) => println(len(m) + n)).unwrap_or(0))",
    ] {
        let source = format!(
            "def main() -> None:\n    n = 2\n    suffix = \"!\"\n    \
             passed: Result[int, str] = Ok(1)\n    failed: Result[int, str] = Err(\"x\")\n    {statement}\n"
        );
        assert_check_ok(&source);
    }
}

/// The side of an `or_else` or `and_then` result that the closure's `Ok(...)` or `Err(...)` leaves open, and the side a
/// bound `Ok(...)` or `Err(...)` leaves open, is `None` where the enclosing function returns no `Result` and nothing
/// else types it: a later use that needs another type is refused. An expected type at the call, an annotation and the
/// enclosing function's `Result` return type each type that side instead (#1561).
#[test]
fn open_result_sides_are_none_where_nothing_else_types_them_issue1561() {
    assert_refused(
        r#"
def show(r: Result[int, str]) -> None:
    println(r.unwrap_or(0))

def main() -> None:
    e: Result[int, str] = Err("abc")
    r = e.or_else((m) => Ok(1))
    show(r)
    x = Ok(2)
    y: Result[int, str] = x
"#,
        &[
            "Argument 'r' of 'show' has type mismatch: expected 'Result[int, str]', found 'Result[int, Unit]'",
            "Assignment to 'y' has type mismatch: expected 'Result[int, str]', found 'Result[int, Unit]'",
        ],
    );
    assert_check_ok(
        r#"
def show(r: Result[int, str]) -> None:
    println(r.unwrap_or(0))

def keep(e: Result[int, str]) -> Result[int, str]:
    r = e.or_else((m) => Ok(1))
    x = Ok(2)
    show(x)
    return r

def main() -> None:
    e: Result[int, str] = Err("abc")
    show(e.or_else((m) => Ok(1)))
    annotated: Result[int, str] = e.or_else((m) => Ok(1))
    show(annotated)
    show(keep(e))
"#,
    );
}

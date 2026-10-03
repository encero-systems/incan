//! A closure's parameter types come from the function type its context gives it (#1561): the element type of an
//! RFC 088 iterator adapter or terminal, a fold's accumulator, and a generic parameter's function type once the other
//! arguments fix its type parameters. A capturing closure is refused where a lazy adapter stores it and as the
//! callback of other standard-library functions and methods, and accepted by a `Result` combinator, which only calls
//! it. The side of a `Result` that an `Ok(...)` or `Err(...)` leaves open, in a closure's result or elsewhere, is
//! settled only where nothing else fixes it: not by its destination, another argument, or another `match` arm or
//! `break` value.

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

/// A closure literal whose body returns `Ok(...)` or `Err(...)`, and a list comprehension of them, build the side the
/// constructor leaves open with `None` where the enclosing function returns no `Result`, and with that function's side
/// where it does; the closure's expected type fixes that side instead. The members of a list or dict literal share
/// one `Result` type, each side taken from the member that fixes it, and a side no member fixes is `None`. A later
/// use that needs another type for a `None` side is refused (#1561).
#[test]
fn closure_results_and_collection_literals_settle_their_open_result_sides_issue1561() {
    assert_refused(
        r#"
def use(g: () -> Result[int, str]) -> int:
    return g().unwrap_or(0)

def main() -> None:
    f = () => Ok(1)
    println(use(f))
    mut xs = [Ok(1)]
    xs.append(Err("x"))
    for x in xs:
        match x:
            Ok(v) => println(v)
            Err(m) => println(m)
    both = [Ok(1), Err("x")]
    wrong: list[Result[int, int]] = both
"#,
        &[
            "Argument 'g' of 'use' has type mismatch: expected '() -> Result[int, str]', found '() -> Result[int, Unit]'",
            "expected 'Result[int, Unit]', found 'Result[int, str]'",
            "cannot print the None value 'm'",
            "Assignment to 'wrong' has type mismatch: expected 'List[Result[int, int]]', found 'List[Result[int, str]]'",
        ],
    );
    assert_check_ok(
        r#"
def use(g: () -> Result[int, str]) -> int:
    return g().unwrap_or(0)

def keep() -> Result[int, str]:
    f = () => Ok(1)
    println(use(f))
    ys = [Ok(y) for y in [1, 2]]
    for y in ys:
        match y:
            Ok(v) => println(v)
            Err(m) => println(m.upper())
    return f()

def main() -> None:
    annotated: () -> Result[int, str] = () => Ok(1)
    println(use(annotated))
    println(use(() => Ok(2)))
    xs = [Ok(1), Err("x")]
    for x in xs:
        match x:
            Ok(v) => println(v + 1)
            Err(m) => println(m.upper())
    nested = [[Err("y")], [Ok(2)]]
    d = {"a": Ok(1), "b": Err("z")}
    typed: dict[str, Result[int, str]] = d
    println(keep().unwrap_or(0))
"#,
    );
}

/// The side an `Ok(...)` or `Err(...)` leaves open where the value is used in place, as the subject of a `match` or in
/// the iterable of a `for` statement, is settled as for a bound constructor: `None` where the enclosing function
/// returns no `Result`, so printing that side is refused and an arm that ignores it is accepted, and that function's
/// side where it does (#1561).
#[test]
fn open_result_sides_of_match_subjects_and_loop_iterables_are_settled_issue1561() {
    assert_refused(
        r#"
def main() -> None:
    match Ok(1):
        Ok(v) => println(v)
        Err(m) => println(m)
    for x in [Ok(1)]:
        match x:
            Ok(v) => println(v)
            Err(e) => println(e)
    match (Err("x"), 2):
        (Ok(v), n) => println(v)
        (Err(m), n) => println(m)
"#,
        &[
            "cannot print the None value 'm'",
            "cannot print the None value 'e'",
            "cannot print the None value 'v'",
        ],
    );
    assert_check_ok(
        r#"
def run() -> Result[str, int]:
    match Ok("a"):
        Ok(v) => println(v.upper())
        Err(e) => println(e + 1)
    for x in [Err(2), Ok("b")]:
        match x:
            Ok(v) => println(v)
            Err(e) => println(e)
    return Ok("done")

def main() -> None:
    match Ok(1):
        Ok(v) => println(v + 1)
        Err(_) => println("e")
    for x in [Ok(1), Err("bad")]:
        match x:
            Ok(v) => println(v)
            Err(m) => println(m.upper())
    for row in [[Err("x")]]:
        for r in row:
            match r:
                Ok(_) => println("ok")
                Err(m) => println(m)
    println(run().unwrap_or("none"))
"#,
    );
}

/// An `Ok(...)` or `Err(...)` argument leaves the side it does not build to its destination: a concrete parameter type
/// gives that side, and a side the callee's own type parameter spells is inferred from all of the call's arguments
/// first, so `pick(Err(2), 5)` instantiates `T` with `int` whatever the enclosing function returns, and the call's
/// parameter types say so. A side no argument fixes is the enclosing function's side, or `None` (#1561).
#[test]
fn constructor_arguments_leave_their_open_side_to_the_call_issue1561() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def pick[T](r: Result[T, int], d: T) -> T:
    return r.unwrap_or(d)

def show(r: Result[int, str]) -> None:
    println(r.unwrap_or(0))

def run() -> Result[str, int]:
    println(pick(Err(2), 5))
    show(Err("x"))
    show(Ok(3))
    return Ok("done")

def main() -> None:
    println(pick(Err(2), 5))
    d = "w"
    println(pick(Ok("x"), d))
"#;
    let info = typecheck_info_for_module(source, vec!["main".to_string()], "constructor arguments")?;
    for (text, occurrence, expected) in [
        ("pick(Err(2), 5)", 0, ["Result[int, int]", "int"]),
        ("pick(Err(2), 5)", 1, ["Result[int, int]", "int"]),
        ("pick(Ok(\"x\"), d)", 0, ["Result[str, int]", "str"]),
    ] {
        let start = source
            .match_indices(text)
            .nth(occurrence)
            .map(|(start, _)| start)
            .ok_or_else(|| format!("missing occurrence {occurrence} of `{text}`"))?;
        let params = info
            .call_site_callable_params(Span::new(start, start + text.len()))
            .ok_or_else(|| format!("`{text}` (occurrence {occurrence}) records no parameter types"))?;
        let params: Vec<String> = params.iter().map(|param| param.ty.to_string()).collect();
        assert_eq!(params, expected, "`{text}` (occurrence {occurrence})");
    }
    assert_refused(
        r#"
def first[T](r: Result[T, int]) -> Option[T]:
    match r:
        Ok(v) => return Some(v)
        Err(_) => return None

def main() -> None:
    match first(Err(3)):
        Some(v) => println(v)
        None => println("none")
"#,
        &["cannot print the None value 'v'"],
    );
    Ok(())
}

/// Return the span of the value of the `match` arm spelled `arm` (`None => None`) in `source`.
fn value_span(source: &str, arm: &str) -> Result<Span, String> {
    let start = source
        .find(arm)
        .ok_or_else(|| format!("`{arm}` is not in the program"))?;
    let value = arm.find("=> ").ok_or_else(|| format!("`{arm}` is not an arm"))? + "=> ".len();
    Ok(Span::new(start + value, start + arm.len()))
}

/// The arms of a `match` and the `break` values of a `loop:` fill the `Option` payload or `Result` side one of them
/// leaves open from another, whichever comes first, and each value records the filled type: `None` then `Some("a")` is
/// an `Option[str]`, `Err("x")` then `Ok(1)` a `Result[int, str]`, also inside a list. A side none of them fixes stays
/// open for the binding to settle to `None`, so printing it is refused, and an arm that ignores it is accepted (#1561).
#[test]
fn match_arms_and_loop_values_fill_open_parts_before_a_side_settles_issue1561() -> Result<(), String> {
    let source = r#"
def main() -> None:
    content: Option[str] = Some("a")
    upper = match content:
        None => None
        Some(value) => Some(value.upper())
    println(upper.unwrap_or("-"))
    parsed = match content:
        None => Err("missing")
        Some(value) => Ok(len(value))
    println(parsed.unwrap_or(0))
    listed = match content:
        None => [None]
        Some(value) => [Some(len(value))]
    println(len(listed))
    flag = true
    looped = loop:
        if flag:
            break Err("stop")
        break Ok(3)
    println(looped.unwrap_or(0))
    settled = match content:
        Some(value) => Ok(len(value))
        None => Ok(0)
    match settled:
        Ok(v) => println(v)
        Err(_) => println("never")
"#;
    let program = parse_program(source, "match arm open parts");
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("the arms must unify, got {errors:?}"))?;
    let info = checker.type_info();
    let option = |payload: ResolvedType| ResolvedType::Generic("Option".to_string(), vec![payload]);
    let result = |ok: ResolvedType, err: ResolvedType| ResolvedType::Generic("Result".to_string(), vec![ok, err]);
    assert_eq!(
        info.expr_type(value_span(source, "None => None")?),
        Some(&option(ResolvedType::Str)),
        "the `None` arm takes the payload the `Some` arm after it gives"
    );
    assert_eq!(
        info.expr_type(value_span(source, "None => Err(\"missing\")")?),
        Some(&result(ResolvedType::Int, ResolvedType::Str)),
        "the `Err` arm takes the success side the `Ok` arm after it gives"
    );
    assert_eq!(
        info.expr_type(value_span(source, "None => [None]")?),
        Some(&ResolvedType::Generic(
            "List".to_string(),
            vec![option(ResolvedType::Int)]
        )),
        "a list arm's open member payload is filled from the other arm"
    );
    let break_value = source.find("Err(\"stop\")").ok_or("missing `break Err(\"stop\")`")?;
    assert_eq!(
        info.expr_type(Span::new(break_value, break_value + "Err(\"stop\")".len())),
        Some(&result(ResolvedType::Int, ResolvedType::Str)),
        "a `break` value takes the side another `break` value gives"
    );
    assert_eq!(
        info.expr_type(value_span(source, "None => Ok(0)")?),
        Some(&result(ResolvedType::Int, ResolvedType::Unit)),
        "a side no arm fixes settles to `None` for the binding, and every arm records it"
    );

    let errors = check_str(
        r#"
def main() -> None:
    content: Option[int] = Some(1)
    settled = match content:
        Some(value) => Ok(value)
        None => Ok(0)
    match settled:
        Ok(v) => println(v)
        Err(m) => println(m)
    flag = true
    looped = loop:
        if flag:
            break Ok(1)
        break Ok(2)
    match looped:
        Ok(v) => println(v)
        Err(e) => println(e)
"#,
    )
    .err()
    .ok_or("printing a side no arm or `break` value fixes must be refused")?;
    for needle in ["cannot print the None value 'm'", "cannot print the None value 'e'"] {
        if !errors.iter().any(|error| error.message.contains(needle)) {
            return Err(format!("expected an error containing `{needle}`, got {errors:?}"));
        }
    }
    Ok(())
}

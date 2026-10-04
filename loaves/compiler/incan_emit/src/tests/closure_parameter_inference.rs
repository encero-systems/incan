//! Closures whose parameter types come from their context build and run (#1561): the element type of an iterator
//! adapter or terminal, a fold's accumulator, a generic parameter's function type once the other arguments fix it,
//! and a generator's own `map` and `filter`, whose predicate closure is called in place. A `Result` combinator's
//! closure returns an `Ok(...)` or `Err(...)` typed by the call.

use super::generated_programs::run_with_stdlib;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Each closure below is written without parameter annotations; its context gives the parameter types, so string
/// methods lower to their Rust spellings and arithmetic to the element's integer type.
#[test]
fn closures_typed_by_their_context_build_and_run_issue1561() -> TestResult {
    let stdout = run_with_stdlib(
        r#"
def apply_twice[T](f: (T) -> T, value: T) -> T:
    return f(f(value))


def numbers() -> Generator[int]:
    yield 1
    yield 2
    yield 3


def main() -> None:
    items = [1, 2, 3]
    names = ["ada", "lin"]
    println(items.iter().map((x) => x * 2).collect())
    shouted: list[str] = names.iter().map((name) => name.upper()).collect()
    println(shouted)
    println(names.iter().flat_map((name) => [name, name.upper()]).collect())
    println(items.iter().filter((x) => x % 2 == 1).collect())
    println(items.iter().take_while((x) => x * 2 < 5).collect())
    println(names.iter().any((name) => len(name.strip()) > 2))
    println(items.iter().fold(0, (acc, x) => acc + x * 10))
    println(apply_twice((x) => x + 1, 3))
    println(apply_twice((text) => text.upper(), "a"))
    println(numbers().map((x) => x * 10).collect())
    println(numbers().filter((x) => x % 2 == 1).collect())
"#,
    )?;
    assert_eq!(
        stdout,
        "[2, 4, 6]\n[\"ADA\", \"LIN\"]\n[\"ada\", \"ADA\", \"lin\", \"LIN\"]\n[1, 3]\n[1, 2]\ntrue\n60\n5\nA\n[10, 20, 30]\n[1, 3]\n"
    );
    Ok(())
}

/// The `Ok(...)` or `Err(...)` a `Result` combinator's closure returns takes the side it does not spell from the call:
/// `or_else`'s success type and `and_then`'s error type from the receiver, the other side from the call's expected
/// type, or `None` when nothing gives it (#1561).
#[test]
fn result_combinator_closures_return_constructors_typed_by_the_call_issue1561() -> TestResult {
    let stdout = run_with_stdlib(
        r#"
def failed() -> Result[int, str]:
    return Err("abc")


def passed() -> Result[int, str]:
    return Ok(2)


def recover(e: Result[int, str]) -> Result[int, str]:
    return e.or_else((m) => Ok(len(m)))


def keep(e: Result[int, str]) -> Result[int, int]:
    return e.or_else((m) => Ok(len(m) * 10))


def main() -> None:
    r = failed().or_else((m) => Ok(1))
    println(r.unwrap_or(0))
    println(failed().or_else((m) => Ok(len(m))).unwrap_or(0))
    annotated: Result[int, str] = failed().or_else((m) => Ok(7))
    println(annotated.unwrap_or(0))
    println(recover(Err("xy")).unwrap_or(0))
    println(keep(Err("xy")).unwrap_or(0))
    doubled = passed().and_then((v) => Ok(v * 2))
    println(doubled.unwrap_or(0))
    refused = passed().and_then((v) => Err("no"))
    match refused:
        Ok(_) => println("ok")
        Err(message) => println(message)
    shouted = failed().or_else((m) => Err(m.upper()))
    match shouted:
        Ok(_) => println("ok")
        Err(message) => println(message)
"#,
    )?;
    assert_eq!(stdout, "1\n3\n7\n2\n20\n4\nno\nABC\n");
    Ok(())
}

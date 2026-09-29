//! A `Result` stays a value across its methods (#1561): a receiver the program reads again after a method call stays
//! usable, an observer closure passed to `inspect` or `inspect_err` takes the payload it observes, and a side of a
//! `Result` that an `Ok(...)` or `Err(...)` leaves open is built with the type the checker gives it.

use super::generated_programs::run_with_stdlib;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Every Rust `Result` method takes its receiver by value; a local, a parameter, a field and a generic `Result` read
/// again after `map`, `and_then`, `inspect` or `unwrap_or` stay usable, in a loop and in both branches of an `if`.
#[test]
fn result_receivers_stay_usable_after_a_method_call_issue1561() -> TestResult {
    let stdout = run_with_stdlib(
        r#"
model Holder:
    r: Result[int, str]

    def total(self) -> int:
        return self.r.unwrap_or(0) + self.r.map((x) => x * 2).unwrap_or(0)


def twice(r: Result[int, str]) -> int:
    a = r.map((x) => x + 1)
    b = r.and_then((x) => Ok(x * 2))
    return a.unwrap_or(0) + b.unwrap_or(0)


def both[T, E](r: Result[T, E]) -> bool:
    a = r.map((x) => 1)
    b = r.map_err((e) => 2).map((x) => 3)
    return a.unwrap_or(0) == 1 and b.unwrap_or(0) == 3


def main() -> None:
    r: Result[int, str] = Ok(2)
    a = r.map((x) => x + 1)
    b = r.and_then((x) => Ok(x * 10))
    r.inspect((x) => println(x))
    println(a.unwrap_or(0))
    println(b.unwrap_or(0))
    println(r.unwrap_or(0))
    println(twice(Ok(3)))
    println(both(Ok("x")))
    h = Holder(r=Ok(5))
    println(h.r.unwrap_or(0))
    println(h.total())
    words: Result[str, str] = Ok("a")
    for i in range(2):
        println(words.map((w) => len(w)).unwrap_or(0))
    if len("q") > 0:
        println(words.map((w) => len(w) + 1).unwrap_or(0))
    else:
        println(words.map((w) => len(w) + 2).unwrap_or(0))
    match r:
        Ok(v) => println(v)
        Err(m) => println(m)
"#,
    )?;
    assert_eq!(stdout, "2\n3\n20\n2\n10\ntrue\n5\n15\n1\n1\n2\n2\n");
    Ok(())
}

/// The closure passed to `inspect` or `inspect_err` takes the payload it observes: a non-`Copy` payload by borrow and
/// a `Copy` one by value, so string methods, operators, calls and f-strings on it build.
#[test]
fn result_observer_closures_take_the_payload_they_observe_issue1561() -> TestResult {
    let stdout = run_with_stdlib(
        r#"
model Box:
    v: int
    tag: str


def shout(s: str) -> str:
    return s.upper()


def main() -> None:
    suffix = "?"
    e: Result[int, str] = Err("abc")
    e.inspect_err((m) => println(m + "!"))
    e.inspect_err((m) => println(m.upper()))
    e.inspect_err((m) => println(len(m)))
    e.inspect_err((m) => println(shout(m)))
    e.inspect_err((m) => println(f"{m}-{len(m)}"))
    e.inspect_err((m) => println(m + suffix))
    g: Result[str, int] = Ok("v")
    g.inspect((v) => println(v + "?"))
    n: Result[float, str] = Ok(2.5)
    n.inspect((v) => println(v * 2.0))
    b: Result[Box, str] = Ok(Box(v=1, tag="t"))
    b.inspect((x) => println(x.tag + "!"))
    b.inspect((x) => println(x.v + 1))
"#,
    )?;
    assert_eq!(stdout, "abc!\nABC\n3\nABC\nabc-3\nabc?\nv?\n5.0\nt!\n2\n");
    Ok(())
}

/// An open side of a `Result` is built with the enclosing function's `Result` side, so reading it there builds, and
/// with `None` elsewhere, where a program that never reads it builds.
#[test]
fn open_result_sides_build_with_the_type_the_checker_gives_them_issue1561() -> TestResult {
    let stdout = run_with_stdlib(
        r#"
def recover(e: Result[int, str]) -> Result[int, str]:
    r = e.or_else((m) => Ok(1))
    match r:
        Ok(v) => println(v)
        Err(m) => println(m)
    x = Ok(2)
    match x:
        Ok(v) => println(v)
        Err(m) => println(m)
    return Ok(0)


def main() -> None:
    println(recover(Err("x")).unwrap_or(9))
    e: Result[int, str] = Err("abc")
    r = e.or_else((m) => Ok(len(m)))
    match r:
        Ok(v) => println(v)
        Err(_) => println("never")
    x = Ok(4)
    println(x.unwrap_or(0))
    println(x)
"#,
    )?;
    assert_eq!(stdout, "1\n2\n0\n3\n4\nOk(4)\n");
    Ok(())
}

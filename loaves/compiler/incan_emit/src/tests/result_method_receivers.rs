//! A `Result` or `Option` stays a value across its methods (#1561): a receiver the program reads again after a method
//! call stays usable, an `unwrap_or` default is a value of the payload type, an observer closure passed to `inspect` or
//! `inspect_err` takes the payload it observes, and a side of a `Result` that an `Ok(...)` or `Err(...)` leaves open is
//! built with the type the checker gives it, in a binding, a closure's result, a comprehension, a collection literal
//! whose other members fix it, a `match` subject, a loop's iterable and a call argument whose other arguments fix it.

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

/// Rust's `Option` `unwrap` and `unwrap_or` take their receiver by value; a local, a field, a generic `Option`, a `mut`
/// parameter and a static read again after them stay usable, in a loop too. An `unwrap_or` default is a value of the
/// payload type: a `str` literal default of a `Result[str, E]` builds, and a default local the program reads again,
/// of an `Option` or a `Result`, stays usable.
#[test]
fn option_and_result_unwrap_or_keep_their_values_issue1561() -> TestResult {
    let stdout = run_with_stdlib(
        r#"
static NAME: Option[str] = Some("n")


model Holder:
    o: Option[str]
    r: Result[str, str]

    def first(self) -> str:
        head = self.o.unwrap_or("none")
        tail = self.r.unwrap_or("z")
        return f"{head}{tail}"


def both[T](o: Option[T], r: Result[T, int], d: T) -> list[T]:
    return [o.unwrap_or(d), o.unwrap(), r.unwrap_or(d), d]


def twice(mut o: Option[str]) -> str:
    first = o.unwrap_or("b")
    return f"{first}{o.unwrap()}"


def main() -> None:
    o: Option[str] = Some("a")
    println(o.unwrap_or("b"))
    println(o.unwrap_or("b"))
    println(o.unwrap())
    for i in range(2):
        println(o.unwrap_or("c"))
    r: Result[str, str] = Err("e")
    println(r.unwrap_or("z"))
    d = "dflt"
    println(r.unwrap_or(d))
    println(d)
    items: Option[list[int]] = None
    fallback = [1, 2]
    println(len(items.unwrap_or(fallback)) + len(fallback))
    h = Holder(o=Some("h"), r=Ok("r"))
    println(h.first())
    held = h.o.unwrap_or("x")
    println(f"{held}{h.o.unwrap()}")
    failed: Result[str, int] = Err(1)
    println(len(both(Some("s"), failed, "t")))
    named = NAME.unwrap_or("x")
    again = NAME.unwrap_or("y")
    println(f"{named}{again}")
    mut m: Option[str] = Some("m")
    println(twice(m))
"#,
    )?;
    assert_eq!(stdout, "a\na\na\na\na\nz\ndflt\ndflt\n4\nhr\nhh\n4\nnn\nmm\n");
    Ok(())
}

/// A closure literal returning `Ok(...)` or `Err(...)` has a complete `Result` result: the side it leaves open is
/// `None` where the enclosing function returns no `Result` and that function's side where it does, and the side it
/// builds is its own, not the enclosing function's. A call of the closure, a generic callee inferring from it, and a
/// list comprehension of constructors build and run.
#[test]
fn closures_returning_constructors_build_with_their_own_result_issue1561() -> TestResult {
    let stdout = run_with_stdlib(
        r#"
def call[T, E](f: () -> Result[T, E]) -> Result[T, E]:
    return f()


def run() -> Result[str, int]:
    f = () => Ok(1)
    println(f().unwrap_or(0))
    g = () => Err(5)
    match g():
        Ok(_) => println("ok")
        Err(e) => println(e)
    return Ok("done")


def main() -> None:
    f = () => Ok(1)
    r = f()
    println(r.unwrap_or(0))
    e = () => Err("no")
    match e():
        Ok(_) => println("ok")
        Err(m) => println(m)
    println(call(() => Ok(2)).unwrap_or(0))
    ys = [Ok(x * 2) for x in [1, 2]]
    for y in ys:
        println(y.unwrap_or(0))
    match run():
        Ok(v) => println(v)
        Err(code) => println(code)
"#,
    )?;
    assert_eq!(stdout, "1\nno\n2\n2\n4\n1\n5\ndone\n");
    Ok(())
}

/// The members of a list or dict literal share one type: a `Result` side one `Ok(...)` or `Err(...)` leaves open takes
/// the type another member gives it, at any depth, as does an `Option` payload a `None` leaves open, and a side no
/// member fixes is `None`.
#[test]
fn collection_literals_of_constructors_share_one_result_type_issue1561() -> TestResult {
    let stdout = run_with_stdlib(
        r#"
def main() -> None:
    xs = [Ok(1), Err("x")]
    for x in xs:
        match x:
            Ok(v) => println(v + 1)
            Err(m) => println(m.upper())
    firsts = [Err("a"), Ok(2), Ok(3)]
    mut total = 0
    for x in firsts:
        total += x.unwrap_or(10)
    println(total)
    nested = [[Ok(1)], [Err("y")]]
    println(len(nested))
    d = {"a": Ok(1), "b": Err("z")}
    println(len(d))
    maybes = [None, Some(4)]
    for maybe in maybes:
        println(maybe.unwrap_or(0))
    single = [Ok(5)]
    for s in single:
        println(s.unwrap_or(0))
"#,
    )?;
    assert_eq!(stdout, "2\nX\n15\n2\n2\n0\n4\n5\n");
    Ok(())
}

/// An `Ok(...)` or `Err(...)` used in place, as a `match` subject or in a loop's iterable, is built with the side the
/// checker settles for what it leaves open: `None` in a function that returns no `Result`, where an arm that ignores
/// that side runs, and the enclosing function's side where it returns one. Parentheses, tuples, sets and nested lists
/// hold such constructors too.
#[test]
fn open_result_sides_of_match_subjects_and_loop_iterables_build_issue1561() -> TestResult {
    let stdout = run_with_stdlib(
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
        Ok(v) => println(v)
        Err(_) => println("e")
    match (Ok(2)):
        Ok(v) => println(v)
        Err(_) => println("e")
    match Err("only"):
        Ok(_) => println("ok")
        Err(m) => println(m)
    for x in [Ok(3), Err("bad")]:
        match x:
            Ok(v) => println(v)
            Err(m) => println(m)
    for r, n in [(Ok(4), 1)]:
        match r:
            Ok(v) => println(v + n)
            Err(_) => println("e")
    for row in [[Err("x")]]:
        for r in row:
            match r:
                Ok(_) => println("ok")
                Err(m) => println(m)
    for s in {Ok(6)}:
        match s:
            Ok(v) => println(v)
            Err(_) => println("e")
    println(run().unwrap_or("none"))
"#,
    )?;
    assert_eq!(stdout, "1\n2\nonly\n3\nbad\n5\nx\n6\nA\n2\nb\ndone\n");
    Ok(())
}

/// An `Ok(...)` or `Err(...)` argument of a generic call is built with the type arguments the call infers from all of
/// its arguments: `pick(Err(2), 5)` builds `Err` with the `int` that `5` fixes `T` to, in a function returning no
/// `Result` and in one returning another `Result` type alike, for a later, named or non-literal argument, a list of
/// constructors and a generic method. A constructor passed to a concrete `Result` parameter takes that parameter's
/// side, and a side no argument fixes is `None`.
#[test]
fn constructor_arguments_build_with_the_type_arguments_the_call_infers_issue1561() -> TestResult {
    let stdout = run_with_stdlib(
        r#"
model Picker:
    n: int

    def pick[T](self, r: Result[T, int], d: T) -> T:
        return r.unwrap_or(d)


def pick[T](r: Result[T, int], d: T) -> T:
    return r.unwrap_or(d)


def pick_first[T](d: T, r: Result[T, int]) -> T:
    return r.unwrap_or(d)


def pick_list[T](rs: list[Result[T, int]], d: T) -> T:
    return rs[0].unwrap_or(d)


def first[T](r: Result[T, int]) -> Option[T]:
    match r:
        Ok(v) => return Some(v)
        Err(_) => return None


def show(r: Result[int, str]) -> None:
    match r:
        Ok(v) => println(v)
        Err(m) => println(m)


def run() -> Result[str, int]:
    println(pick(Err(2), 5))
    show(Err("x"))
    show(Ok(3))
    return Ok("done")


def main() -> None:
    println(pick(Err(2), 5))
    println(pick(Ok(7), 5))
    println(pick(d=8, r=Err(2)))
    println(pick_first("a", Err(2)))
    d = "w"
    println(pick(Err(2), d))
    println(pick_list([Err(2)], 9))
    println(Picker(n=1).pick(Err(2), 10))
    match first(Err(3)):
        Some(_) => println("some")
        None => println("none")
    println(run().unwrap_or("none"))
"#,
    )?;
    assert_eq!(stdout, "5\n7\n8\na\nw\n9\n10\nnone\n5\nx\n3\ndone\n");
    Ok(())
}

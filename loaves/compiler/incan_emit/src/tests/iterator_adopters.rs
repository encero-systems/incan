//! A type that adopts the standard library's `Iterator[T]` reaches the RFC 088 adapters and terminals as an
//! `Iterator[T]` value does, under the trait's own name, an import alias or its module (#1561): the generated
//! `Iterator` trait declares only `__next__` and `sum`, so each adapter and terminal called on an adopter is the
//! iterator protocol's own, and the adopter's `__next__` is reached through the trait's path. A `for` loop over a call
//! that returns an adopter builds that call as the call it is.

use super::generated_programs::{run_modules_with_stdlib, run_with_stdlib};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A model, a generic model and a class adopting `Iterator[T]`, and a model adopting it under an import alias, build
/// and run their adapters and terminals, with named functions and closures as callbacks.
#[test]
fn iterator_adopters_reach_the_protocol_adapters_and_terminals_issue1561() -> TestResult {
    let stdout = run_with_stdlib(
        r#"
from std.derives.collection import Iterator
from std.derives.collection import Iterator as Stream


model Count with Iterator[int]:
    n: int

    def __next__(mut self) -> Option[int]:
        if self.n >= 5:
            return None
        self.n += 1
        return Some(self.n)


model Repeat[T with Clone] with Iterator[T]:
    value: T
    left: int

    def __next__(mut self) -> Option[T]:
        if self.left <= 0:
            return None
        self.left -= 1
        return Some(self.value.clone())


class Countdown with Iterator[int]:
    n: int

    def __next__(mut self) -> Option[int]:
        if self.n <= 0:
            return None
        self.n -= 1
        return Some(self.n + 1)


model Evens with Stream[int]:
    n: int

    def __next__(mut self) -> Option[int]:
        if self.n >= 6:
            return None
        self.n += 2
        return Some(self.n)


def is_big(x: int) -> bool:
    return x > 3


def double(x: int) -> int:
    return x * 2


def main() -> None:
    println(Count(n=0).any(is_big))
    println(Count(n=0).all(is_big))
    println(Count(n=0).find(is_big))
    println(Count(n=0).fold(0, (acc, x) => acc + x))
    println(Count(n=0).reduce(0, (acc, x) => acc + x * 3))
    println(Count(n=0).count())
    println(Count(n=0).map(double).collect())
    println(Count(n=0).filter(is_big).collect())
    println(Count(n=0).take(2).collect())
    println(Count(n=0).skip(3).collect())
    println(Count(n=0).enumerate().collect())
    println(Count(n=0).any((x) => x > 3))
    Count(n=3).for_each((x) => println(x * 3))
    println(Repeat(value="a", left=2).collect())
    println(Repeat(value=2, left=3).fold(0, (acc, x) => acc + x))
    println(Countdown(n=3).collect())
    println(Countdown(n=3).find((x) => x < 3))
    println(Evens(n=0).map(double).collect())
"#,
    )?;
    assert_eq!(
        stdout,
        "true\nfalse\nSome(4)\n15\n45\n5\n[2, 4, 6, 8, 10]\n[4, 5]\n[1, 2]\n[4, 5]\n[(0, 1), (1, 2), (2, 3), (3, 4), (4, 5)]\ntrue\n12\n15\n[\"a\", \"a\"]\n6\n[3, 2, 1]\nSome(2)\n[4, 8, 12]\n"
    );
    Ok(())
}

/// An adopter that names the trait through its module (`collection.Iterator[int]`, `coll.Iterator[str]` for a module
/// alias, a generic model) builds: its `__next__` is reached through the trait's path, which the module import does
/// not bring into scope, from the adapters, the terminals and a direct `__next__()` call.
#[test]
fn iterator_adopters_spelled_through_their_module_build_issue1561() -> TestResult {
    let stdout = run_with_stdlib(
        r#"
from std.derives import collection
import std.derives.collection as coll


model Count with collection.Iterator[int]:
    n: int

    def __next__(mut self) -> Option[int]:
        if self.n >= 3:
            return None
        self.n += 1
        return Some(self.n)


class Words with coll.Iterator[str]:
    left: int

    def __next__(mut self) -> Option[str]:
        if self.left <= 0:
            return None
        self.left -= 1
        return Some("w")


model Repeat[T with Clone] with collection.Iterator[T]:
    value: T
    left: int

    def __next__(mut self) -> Option[T]:
        if self.left <= 0:
            return None
        self.left -= 1
        return Some(self.value.clone())


def main() -> None:
    println(Count(n=0).collect())
    println(Count(n=0).any((x) => x > 2))
    println(Count(n=0).map((x) => x * 10).collect())
    mut c = Count(n=0)
    println(c.__next__())
    println(Words(left=2).collect())
    println(Repeat(value=7, left=2).fold(0, (acc, x) => acc + x))
"#,
    )?;
    assert_eq!(stdout, "[1, 2, 3]\ntrue\n[10, 20, 30]\nSome(1)\n[\"w\", \"w\"]\n14\n");
    Ok(())
}

/// The adopter of [`loops_over_calls_returning_an_iterator_adopter_build_issue1561`] and a class whose methods return
/// it.
const NUMBERS_ADOPTER: &str = r#"
pub model Numbers with Iterator[int]:
    pub items: list[int]
    pub index: int

    def __iter__(self) -> Numbers:
        return self

    def __next__(mut self) -> Option[int]:
        if self.index >= len(self.items):
            return None
        value = self.items[self.index]
        self.index += 1
        return Some(value)

    @staticmethod
    def of(first: int, second: int) -> Numbers:
        return Numbers(items=[first, second], index=0)


pub class Source:
    pub base: int
    pub numbers: Numbers

    def counted(self, count: int = 2) -> Numbers:
        mut items: list[int] = []
        for i in range(count):
            items.append(self.base + i)
        return Numbers(items=items, index=0)

    def spread(self, *offsets: int) -> Numbers:
        mut items: list[int] = []
        for offset in offsets:
            items.append(self.base + offset)
        return Numbers(items=items, index=0)

    def tagged[T](self, tag: T) -> Numbers:
        return Numbers(items=[self.base], index=0)


pub def source_at(base: int) -> Source:
    return Source(base=base, numbers=Numbers.of(base, base))
"#;

/// A `for` loop over a call that returns an `Iterator` adopter whose `__next__` implements the trait builds and runs,
/// whatever the call: a method with an argument, a default or a variadic parameter, a generic method, a static method,
/// a function and an adapter, and with the adopter declared in another module that the loop's module imports nothing
/// of but the class.
///
/// The loop's `__iter__` and `__next__` hooks were resolved at the iterable's span and replaced the facts the call
/// recorded there, so `source.counted(3)` was built as `__next__`'s call through `Iterator[int]`:
/// `Iterator::<i64>::counted(&mut source, 3)`, and rustc refused the trait in a type's place (E0782). The hook's
/// `__iter__` call then took no arguments, where the call's own signature there would have given it the variadic
/// parameter's empty list (E0061).
#[test]
fn loops_over_calls_returning_an_iterator_adopter_build_issue1561() -> TestResult {
    let stdout = run_with_stdlib(&format!(
        r#"
from std.derives.collection import Iterator

{NUMBERS_ADOPTER}

def make(first: int) -> Numbers:
    return Numbers.of(first, first + 1)


def main() -> None:
    source = Source(base=10, numbers=Numbers.of(1, 2))
    mut seen: list[int] = []
    for value in source.counted(3):
        seen.append(value)
    for value in source.counted():
        seen.append(value)
    for value in source.spread(5, 6):
        seen.append(value)
    for value in source.tagged("x"):
        seen.append(value)
    for value in Numbers.of(7, 8):
        seen.append(value)
    for value in make(20):
        seen.append(value)
    for value in source.numbers:
        seen.append(value)
    for value in make(30).take(1):
        seen.append(value)
    println(seen)
"#
    ))?;
    assert_eq!(stdout, "[10, 11, 12, 10, 11, 15, 16, 10, 7, 8, 20, 21, 1, 2, 30]\n");

    let adopters = format!("from std.derives.collection import Iterator\n\n{NUMBERS_ADOPTER}");
    let stdout = run_modules_with_stdlib(
        &[("iters", adopters.as_str())],
        r#"
from iters import source_at


def main() -> None:
    source = source_at(10)
    mut seen: list[int] = []
    for value in source.counted(3):
        seen.append(value)
    for value in source.tagged(1):
        seen.append(value)
    for value in source.spread(4):
        seen.append(value)
    for value in source.numbers:
        seen.append(value)
    println(seen)
"#,
    )?;
    assert_eq!(stdout, "[10, 11, 12, 10, 14, 10, 10]\n");
    Ok(())
}

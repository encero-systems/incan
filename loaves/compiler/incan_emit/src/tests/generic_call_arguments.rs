//! Generic calls whose type arguments come from their arguments build and run (#1561): a value of a type parameter
//! where its bound makes it the expected type, arguments that bind one type parameter to numeric types that widen to
//! one, and a type argument the call settles or the expected type fixes where the callee bounds it.

use super::generated_programs::run_with_stdlib;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A value of a type parameter is used where its bound makes it the expected type, a trait its bound names, and where
/// the expected type is still to be inferred or wraps the parameter: passed to a trait-typed parameter, through a
/// bound's method, cloned, returned as an `Option` of it and passed to a callee's own type parameter.
#[test]
fn type_parameter_values_build_where_a_bound_makes_them_the_expected_type_issue1561() -> TestResult {
    let stdout = run_with_stdlib(
        r#"
trait Shape:
    def area(self) -> int


model Square with Shape:
    side: int

    def area(self) -> int:
        return self.side * self.side


def measure(shape: Shape) -> int:
    return shape.area()


def doubled[T with Shape](value: T) -> int:
    return measure(value) + value.area()


def copied[T](value: T) -> T:
    return value.clone()


def wrapped[T](value: T) -> Option[T]:
    return value


def ident[U](value: U) -> U:
    return value


def forwarded[T](value: T) -> T:
    return ident(value)


def main() -> None:
    println(doubled(Square(side=3)))
    println(copied("c"))
    match wrapped(4):
        Some(v) => println(v)
        None => println("none")
    println(forwarded(5))
"#,
    )?;
    assert_eq!(stdout, "18\nc\n4\n5\n");
    Ok(())
}

/// Arguments that bind one type parameter to different numeric types bind it to the type the others widen to (RFC
/// 009), in either order, for a function, a generic method and a generic model's constructor, and the narrower value
/// is widened to it: an `i8` beside an `int` binds `T` to `int`, and a float literal beside an `f32` is an `f32`, as an
/// integer literal beside a float is a float.
#[test]
fn arguments_binding_one_type_parameter_build_with_the_type_they_widen_to_issue1561() -> TestResult {
    let stdout = run_with_stdlib(
        r#"
model Pair[T]:
    first: T
    second: T


class Picker:
    count: int

    def pick[T](self, a: T, b: T) -> T:
        return b


def pick[T](a: T, b: T) -> T:
    return b


def last[T](a: T, b: T, c: T) -> T:
    return c


def main() -> None:
    small: i8 = 3
    wide: int = 5
    println(pick(small, wide))
    println(pick(wide, small))
    println(last(small, wide, small))
    single: f32 = 1.5
    println(pick(single, 2.5))
    println(pick(1, 2.5))
    picker = Picker(count=1)
    println(picker.pick(1, 2.5))
    println(picker.pick(small, wide))
    pair = Pair(first=small, second=wide)
    println(pair.first + pair.second)
"#,
    )?;
    assert_eq!(stdout, "5\n3\n3\n2.5\n2.5\n2.5\n5\n8\n");
    Ok(())
}

/// A bounded type parameter the arguments leave open is fixed by what the call settles or expects: the side an `Err`
/// leaves open is the enclosing function's `Result` side, which meets the `Display` bound, an explicit type argument
/// fixes it, and so does the annotated binding a call's result is assigned to.
#[test]
fn bounded_type_arguments_the_call_settles_or_expects_build_issue1561() -> TestResult {
    let stdout = run_with_stdlib(
        r#"
def first[T with Display](r: Result[T, int]) -> str:
    match r:
        Ok(v) => return f"{v}"
        Err(e) => return f"err {e}"


def make[T with Display]() -> list[T]:
    return []


def run() -> Result[str, int]:
    println(first(Err(3)))
    return Ok("done")


def main() -> None:
    println(first(Ok(7)))
    println(first[str](Err(4)))
    items: list[int] = make()
    println(len(items))
    println(run().unwrap_or("none"))
"#,
    )?;
    assert_eq!(stdout, "7\nerr 4\n0\nerr 3\ndone\n");
    Ok(())
}

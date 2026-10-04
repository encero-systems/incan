//! A value of a type parameter inside the declaration that introduces it, and the type arguments a generic call's
//! arguments bind or leave open (#1561): where a value of the parameter is refused and which methods it has, arguments
//! that bind one type parameter to two types, numeric arguments that widen to one, and a bounded type parameter the
//! arguments leave open.

use super::*;

/// A value of a type parameter is not a value of a concrete type inside its declaration: returning, binding, passing
/// or storing one where an `int`, a `str`, a `list[int]`, a union of concrete types, a trait its bounds do not imply or
/// a function type is expected is refused, through `self` of a generic model too. A method call on it is refused
/// unless a bound's trait declares the method or it is `.clone()`, with no bound, a user trait's bound and a builtin
/// trait's bound alike. Where its bound names the expected trait, where the expected type is `T`, `Option[T]` or a
/// callee's own type parameter, and for a bound's own method and `.clone()`, it is accepted (#1561).
#[test]
fn values_of_a_type_parameter_are_not_values_of_a_concrete_type_issue1561() {
    let errors = check_str_err(
        r#"
from std.traits.callable import Callable1

trait Shape:
    def area(self) -> int

model Holder:
    name: str

model Box[T]:
    value: T

    def count(self) -> int:
        return self.value

def takes_int(n: int) -> int:
    return n

def measure(shape: Shape) -> int:
    return shape.area()

def apply(f: (int) -> int) -> int:
    return f(1)

def returned[T](x: T) -> int:
    return x

def bound[T](x: T) -> int:
    y: int = x
    return y

def passed[T](x: T) -> int:
    return takes_int(x)

def listed[T](xs: list[T]) -> list[int]:
    return xs

def stored[T](x: T) -> Holder:
    return Holder(name=x)

def either[T](x: T) -> int | str:
    return x

def unbounded_shape[T](x: T) -> int:
    return measure(x)

def called[F with Callable1[int, int]](f: F) -> int:
    return apply(f)

def unknown_method[T](x: T) -> int:
    return x.size()

def other_method[T with Shape](x: T) -> int:
    return x.perimeter()

def rust_method[T with Display](x: T) -> str:
    return x.to_string()
"#,
        "values of a type parameter where a concrete type is expected",
    );
    let messages: Vec<&str> = errors.iter().map(|error| error.message.as_str()).collect();
    for needle in [
        "Return type mismatch: expected 'int', found 'T'",
        "Assignment to 'y' has type mismatch: expected 'int', found 'T'",
        "Argument 'n' of 'takes_int' has type mismatch: expected 'int', found 'T'",
        "Return type mismatch: expected 'List[int]', found 'List[T]'",
        "Cannot assign 'T' to field 'name' of type 'str'",
        "Return type mismatch: expected 'Union[int, str]', found 'T'",
        "Argument 'shape' of 'measure' has type mismatch: expected 'Shape', found 'T'",
        "Argument 'f' of 'apply' has type mismatch: expected '(int) -> int', found 'F'",
        "Type parameter 'T' has no method 'size(...)': no bound of 'T' declares it",
        "Type parameter 'T' has no method 'perimeter(...)': no bound of 'T' declares it",
        "Type parameter 'T' has no method 'to_string(...)': no bound of 'T' declares it",
    ] {
        assert!(
            messages.iter().any(|message| message.contains(needle)),
            "expected an error containing `{needle}`, got {messages:?}"
        );
    }
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.contains("Return type mismatch: expected 'int', found 'T'"))
            .count(),
        2,
        "`return x` and `return self.value` are each refused: {messages:?}"
    );
    assert_check_ok(
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
    println(forwarded(5))
"#,
    );
}

/// Arguments that bind one type parameter of the callee to types that do not widen to one are refused, for positional,
/// named and three arguments, a collection's element beside a value, a generic body's own type parameter beside a
/// `str` in either order, a generic method and a generic model's constructor. Arguments whose types widen to one bind
/// the parameter to it, in either order, and a literal takes the type of a value beside it (RFC 009); each such
/// narrower value records the wider type as its destination, so it is widened where it is passed (#1561).
#[test]
fn arguments_binding_one_type_parameter_to_two_types_are_refused_issue1561() {
    let errors = check_str_err(
        r#"
model Pair[T]:
    first: T
    second: T

class Picker:
    count: int

    def take[T](self, a: T, b: T) -> int:
        return 1

def take[T](x: T, y: T) -> int:
    return 1

def take3[T](a: T, b: T, c: T) -> int:
    return 1

def push[T](items: list[T], item: T) -> int:
    return len(items)

def str_first[U](x: U) -> int:
    return take("s", x)

def str_last[U](x: U) -> int:
    return take(x, "s")

def main() -> None:
    take(1, "s")
    take(y="t", x=2)
    take3(1, 2, "u")
    items = [1, 2]
    push(items, "v")
    Picker(count=1).take(3, "w")
    Pair(first=4, second="x")
"#,
        "arguments that bind one type parameter to two types",
    );
    let messages: Vec<&str> = errors.iter().map(|error| error.message.as_str()).collect();
    for needle in [
        "Type mismatch: expected 'str', found 'U'",
        "Type mismatch: expected 'U', found 'str'",
        "Type mismatch: expected 'int', found 'str'",
    ] {
        assert!(
            messages.iter().any(|message| message.contains(needle)),
            "expected an error containing `{needle}`, got {messages:?}"
        );
    }
    let conflicts = errors
        .iter()
        .filter(|error| {
            error
                .notes
                .iter()
                .any(|note| note.contains("and 'T' is one type in a call"))
        })
        .count();
    assert_eq!(
        conflicts, 8,
        "each call binding `T` twice is refused once: {messages:?}"
    );

    let source = r#"
model Pair[T]:
    first: T
    second: T

def pick[T](a: T, b: T) -> T:
    return b

def main() -> None:
    small: i8 = 3
    wide: int = 5
    println(pick(small, wide))
    println(pick(wide, small))
    single: f32 = 1.5
    println(pick(single, 2.5))
    println(pick(1, 2.5))
    pair = Pair(first=small, second=wide)
    println(pair.first)
"#;
    let program = parse_program(source, "widening arguments");
    let mut checker = TypeChecker::new();
    if let Err(errs) = checker.check_program(&program) {
        panic!("widening arguments are accepted: {errs:?}");
    }
    let widened = source
        .match_indices("small")
        .filter(|(start, _)| {
            let span = crate::ast::Span {
                start: *start,
                end: start + "small".len(),
            };
            checker.type_info().value_destination_type(span) == Some(&ResolvedType::Int)
        })
        .count();
    assert_eq!(
        widened, 3,
        "the `i8` argument of each `pick` and the `i8` field value are widened to `int`"
    );
}

/// A type parameter with a declared bound that the call's arguments leave open is judged by what the call settles it
/// to: the side an `Err(...)` or `Ok(...)` leaves open is `None` outside a `Result` function, which does not meet
/// `Display` (`INCAN-T0103`), for a function and a generic method. When no argument fixes it at all (`None` for an
/// `Option[T]`, no arguments), the call is refused (`INCAN-T0001`). The enclosing function's `Result` side, an explicit
/// type argument, the annotated binding of the result and an unbounded parameter are accepted (#1561).
#[test]
fn bounded_type_arguments_nothing_fixes_are_refused_issue1561() {
    let errors = check_str_err(
        r#"
class Shower:
    count: int

    def first[T with Display](self, r: Result[T, int]) -> str:
        return "m"

def first[T with Display](r: Result[T, int]) -> str:
    return "f"

def failed[E with Display](r: Result[int, E]) -> str:
    return "e"

def show[T with Display](o: Option[T]) -> str:
    return "o"

def make[T with Display]() -> list[T]:
    return []

def main() -> None:
    println(first(Err(3)))
    println(failed(Ok(3)))
    println(Shower(count=1).first(Err(3)))
    println(show(None))
    items = make()
"#,
        "bounded type arguments nothing fixes",
    );
    let none_display = errors
        .iter()
        .filter(|error| error.stable_code() == Some("INCAN-T0103"))
        .count();
    assert_eq!(none_display, 3, "each settled `None` side fails `Display`: {errors:?}");
    let messages: Vec<&str> = errors.iter().map(|error| error.message.as_str()).collect();
    for needle in [
        "Cannot infer type parameter 'T' of 'show': no argument fixes it, and its bound 'Display' needs a type",
        "Cannot infer type parameter 'T' of 'make': no argument fixes it, and its bound 'Display' needs a type",
    ] {
        assert!(
            messages.iter().any(|message| message.contains(needle)),
            "expected an error containing `{needle}`, got {messages:?}"
        );
    }
    assert_check_ok(
        r#"
def first[T with Display](r: Result[T, int]) -> str:
    return "f"

def loose[T](r: Result[T, int]) -> int:
    return 1

def show[T](o: Option[T]) -> str:
    return "o"

def make[T with Display]() -> list[T]:
    return []

def run() -> Result[str, int]:
    println(first(Err(3)))
    return Ok("done")

def main() -> None:
    println(first(Ok(7)))
    println(first[str](Err(4)))
    println(loose(Err(5)))
    println(show(None))
    items: list[int] = make()
    println(len(items))
"#,
    );
}

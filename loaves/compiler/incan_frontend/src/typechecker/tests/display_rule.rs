//! The display rule shared by `print`/`println` arguments, `str(value)`, f-string `{value}` parts and `Display` bounds
//! (#1725, #1748): structural values display their structure, a type displays through the `Display` it provides, and a
//! value with no printed form is refused with `INCAN-T0103`.

use super::*;

// ---- #1725: a tuple prints its structure, like an f-string renders it ----

#[test]
fn print_of_a_tuple_value_prints_its_structure_issue1725() -> Result<(), Box<dyn std::error::Error>> {
    // The program from #1725, refused under the tuple-only rule, prints `(10, 20)` twice under the shared display rule.
    check_str(
        r#"
def get_coordinates() -> tuple[int, int]:
    return (10, 20)

def main() -> None:
    coords: tuple[int, int] = get_coordinates()
    print(coords)
    println(get_coordinates())
"#,
    )
    .map_err(|errors| format!("a tuple must print, got: {errors:?}"))?;
    Ok(())
}

#[test]
fn print_of_tuple_elements_and_unpacked_names_is_accepted_issue1725() {
    assert_check_ok(
        r#"
def get_coordinates() -> tuple[int, int]:
    return (10, 20)

def main() -> None:
    coords: tuple[int, int] = get_coordinates()
    print(coords[0], coords[1])
    x, y = coords
    println(x, y)
    println(f"{coords[0]},{coords[1]}")
"#,
    );
}

// ---- #1748: one display rule for print, str and f-strings ----

/// One `INCAN-T0103` refusal: its message and its hints.
type PrintedFormRefusal = (String, Vec<String>);

/// Return the `INCAN-T0103` refusals of a program the checker must refuse, as message and hints.
fn printed_form_refusals(source: &str) -> Result<Vec<PrintedFormRefusal>, Box<dyn std::error::Error>> {
    let errors = check_str(source)
        .err()
        .ok_or("a value with no printed form must be refused")?;
    Ok(errors
        .into_iter()
        .filter(|error| error.stable_code() == Some("INCAN-T0103"))
        .map(|error| (error.message, error.hints))
        .collect())
}

#[test]
fn collections_options_and_results_display_in_every_position_issue1748() -> Result<(), Box<dyn std::error::Error>> {
    // The program from #1748, widened to every structural kind and to `str(...)`: each position renders the value's
    // structure, so nothing is refused.
    check_str(
        r#"
def main() -> None:
    items: list[int] = [1, 2, 3]
    print(items)
    counts: dict[str, int] = {"a": 1}
    println(counts)
    seen: set[int] = {1}
    println(seen)
    maybe: Option[int] = Some(1)
    println(maybe)
    outcome: Result[int, str] = Ok(1)
    println(outcome)
    pair: tuple[int, str] = (1, "one")
    println(pair)
    println([1, 2])
    text = str(items) + str(maybe) + str(pair)
    println(f"{items} {counts} {seen} {maybe} {outcome} {pair} {text}")
"#,
    )
    .map_err(|errors| format!("structural values must display, got: {errors:?}"))?;
    Ok(())
}

#[test]
fn values_with_no_printed_form_are_refused_in_every_position_issue1748() -> Result<(), Box<dyn std::error::Error>> {
    let refused = printed_form_refusals(
        r#"
model Point:
    x: int
    y: int

def parse(flag: bool) -> int | str:
    if flag:
        return 1
    return "one"

def numbers() -> Generator[int]:
    yield 1

def double(n: int) -> int:
    return n * 2

def main() -> None:
    value = parse(true)
    println(value)
    text = str(value)
    println(f"{value}")
    point = Point(x=1, y=2)
    print(point)
    println(str(point))
    println(f"{point}")
    gen = numbers()
    println(gen)
    data: bytes = b"abc"
    println(data)
    println(double)
    println(str(Point(x=3, y=4)))
"#,
    )?;
    assert_eq!(
        refused.iter().map(|(message, _)| message.as_str()).collect::<Vec<_>>(),
        vec![
            "'println' cannot print the union value 'value'",
            "'str' cannot convert the union value 'value' to text",
            "f-string cannot interpolate the union value 'value'",
            "'print' cannot print the Point value 'point'",
            "'str' cannot convert the Point value 'point' to text",
            "f-string cannot interpolate the Point value 'point'",
            "'println' cannot print the generator 'gen'",
            "'println' cannot print the bytes value 'data'",
            "'println' cannot print the function 'double'",
            "'str' cannot convert a value of type 'Point' to text",
        ],
        "one refusal per displayed value with no printed form, in every position, in source order"
    );
    assert!(
        refused[3]
            .1
            .iter()
            .any(|hint| hint.contains("__str__") && hint.contains("f\"{point:?}\"")),
        "a model's remedy names __str__ and its structure, got: {:?}",
        refused[3].1
    );
    assert!(
        refused[9]
            .1
            .iter()
            .any(|hint| hint.contains(":? format spec") && !hint.contains("{value")),
        "an unnamed operand's remedy names no binding the program lacks, got: {:?}",
        refused[9].1
    );
    assert!(
        refused[0].1.iter().any(|hint| hint.contains("match or isinstance")),
        "a union value's remedy narrows it first, got: {:?}",
        refused[0].1
    );
    Ok(())
}

#[test]
fn a_model_printing_itself_is_named_self_in_the_refusal_issue1748() -> Result<(), Box<dyn std::error::Error>> {
    let refused = printed_form_refusals(
        r#"
model Point:
    x: int

    def show(self) -> None:
        println(self)
"#,
    )?;
    let [(message, hints)] = refused.as_slice() else {
        return Err(format!("expected one refusal, got {refused:?}").into());
    };
    assert_eq!(message, "'println' cannot print the Point value 'self'");
    assert!(
        hints.iter().any(|hint| hint.contains("f\"{self:?}\"")),
        "the structure form is spelled with `self`, got: {hints:?}"
    );
    Ok(())
}

#[test]
fn values_with_a_printed_form_display_in_every_position_issue1748() -> Result<(), Box<dyn std::error::Error>> {
    // `__str__` on the type, inherited from a base class, or supplied by an adopted trait gives a printed form; a
    // `{value:?}` part asks for the structure; a narrowed union member prints as its own type, a `FrozenStr` member
    // selected by `isinstance(value, str)` included; an `Error` adopter is left to the display rule for errors.
    check_str(
        r#"
model Label:
    text: str

    def __str__(self) -> str:
        return self.text

class Base:
    name: str

    def __str__(self) -> str:
        return self.name

class Child extends Base:
    size: int

trait Named:
    def __str__(self) -> str:
        return "named"

model Tagged with Named:
    tag: int

model Point:
    x: int

model Failure with Error:
    detail: str

    def message(self) -> str:
        return self.detail

def parse(flag: bool) -> int | str:
    if flag:
        return 1
    return "one"

def frozen_text(value: FrozenStr | int) -> str:
    if isinstance(value, str):
        return str(value)
    return "number"

def main() -> None:
    println(Label(text="hi"))
    println(str(Child(name="c", size=1)))
    println(f"{Tagged(tag=1)}")
    point = Point(x=1)
    println(f"{point:?}")
    println(Failure(detail="boom"))
    match parse(true):
        int(n) => println(n)
        str(s) => println(s)
"#,
    )
    .map_err(|errors| format!("values with a printed form must display, got: {errors:?}"))?;
    Ok(())
}

#[test]
fn plain_enums_newtypes_and_derived_display_are_refused_in_every_position_issue1748()
-> Result<(), Box<dyn std::error::Error>> {
    // An enum that declares no values, a newtype, and a model whose only claim is `@derive(Display)` provide no
    // `Display`, so each is refused in all three value positions.
    let refused = printed_form_refusals(
        r#"
enum Color:
    Red
    Green

type UserId = newtype int

@derive(Display)
model Derived:
    x: int

def main() -> None:
    color = Color.Red
    println(color)
    color_text = str(color)
    println(f"{color}")
    user = UserId(1)
    print(user)
    user_text = str(user)
    println(f"{user}")
    derived = Derived(x=1)
    println(derived)
    derived_text = str(derived)
    println(f"{derived}")
"#,
    )?;
    assert_eq!(
        refused.iter().map(|(message, _)| message.as_str()).collect::<Vec<_>>(),
        vec![
            "'println' cannot print the Color value 'color'",
            "'str' cannot convert the Color value 'color' to text",
            "f-string cannot interpolate the Color value 'color'",
            "'print' cannot print the UserId value 'user'",
            "'str' cannot convert the UserId value 'user' to text",
            "f-string cannot interpolate the UserId value 'user'",
            "'println' cannot print the Derived value 'derived'",
            "'str' cannot convert the Derived value 'derived' to text",
            "f-string cannot interpolate the Derived value 'derived'",
        ],
        "one refusal per displayed value whose type provides no Display, in source order"
    );
    Ok(())
}

#[test]
fn every_display_route_displays_and_structure_needs_no_display_issue1748() -> Result<(), Box<dyn std::error::Error>> {
    // `Display` comes from a `__str__` (on an enum or a newtype too, or required by an adopted `Display`), from the
    // values an enum declares, or from the `message()` of an `Error` adopter; `{value:?}` asks for `Debug` instead.
    check_str(
        r#"
enum Level(str):
    WARN = "warn"

enum Code(int):
    OK = 0

enum Mood:
    Happy

    def __str__(self) -> str:
        return "happy"

enum Failure with Error:
    Broken

    def message(self) -> str:
        return "broken"

type Tag = newtype str:
    def __str__(self) -> str:
        return "tag"

model Adopted with Display:
    x: int

    def __str__(self) -> str:
        return "adopted"

enum Color:
    Red

type UserId = newtype int

def main() -> None:
    println(Level.WARN)
    code = str(Code.OK)
    mood = Mood.Happy
    println(f"{mood} {Failure.Broken} {code}")
    tag = Tag("a")
    println(tag)
    println(str(Adopted(x=1)))
    color = Color.Red
    user = UserId(1)
    println(f"{color:?} {user:?}")
"#,
    )
    .map_err(|errors| format!("every Display route must display, got: {errors:?}"))?;
    Ok(())
}

#[test]
fn a_display_bound_takes_the_display_rule_issue1748() -> Result<(), Box<dyn std::error::Error>> {
    // A type argument for `T with Display` provides `Display` by the same routes as a displayed value; one that
    // provides none, and `bytes`, are refused with the display rule's code.
    let errors = check_str(
        r#"
enum Level(str):
    WARN = "warn"

model Shown:
    x: int

    def __str__(self) -> str:
        return "shown"

model Failure with Error:
    detail: str

    def message(self) -> str:
        return self.detail

model Plain:
    x: int

enum Color:
    Red

type UserId = newtype int

@derive(Display)
model Derived:
    x: int

def show[T with Display](value: T) -> str:
    return f"{value}"

def main() -> None:
    accepted = show(1) + show("s") + show(Level.WARN) + show(Shown(x=1)) + show(Failure(detail="x"))
    data: bytes = b"a"
    plain = show(Plain(x=1))
    color = show(Color.Red)
    user = show(UserId(1))
    derived = show(Derived(x=1))
    raw = show(data)
"#,
    )
    .err()
    .ok_or("a type argument that provides no Display must be refused")?;
    let messages = errors
        .iter()
        .map(|error| (error.stable_code(), error.message.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        messages,
        vec![
            (
                Some("INCAN-T0103"),
                "Call to 'show' cannot bind a value of type 'Plain' to 'T', which requires 'Display'"
            ),
            (
                Some("INCAN-T0103"),
                "Call to 'show' cannot bind a value of type 'Color' to 'T', which requires 'Display'"
            ),
            (
                Some("INCAN-T0103"),
                "Call to 'show' cannot bind a value of type 'UserId' to 'T', which requires 'Display'"
            ),
            (
                Some("INCAN-T0103"),
                "Call to 'show' cannot bind a value of type 'Derived' to 'T', which requires 'Display'"
            ),
            (
                Some("INCAN-T0103"),
                "Call to 'show' cannot bind a bytes value to 'T', which requires 'Display'"
            ),
        ],
        "only the type arguments with no Display are refused, each with the display rule's code"
    );
    Ok(())
}

/// A type that adopts `Error` and has no `__str__` satisfies a `Display` bound: a generic display of it checks, as the
/// string-representation reference states.
#[test]
fn error_message_display_satisfies_display_bound_followups_a() -> Result<(), Box<dyn std::error::Error>> {
    check_str(
        r#"
from std.traits.error import Error

model Failure with Error:
    detail: str

    def message(self) -> str:
        return self.detail

def show[T with Display](value: T) -> str:
    return f"{value}"

def main() -> None:
    println(show(Failure(detail="bad")))
"#,
    )
    .map_err(|errors| format!("an Error adopter without __str__ must satisfy a Display bound, got: {errors:?}"))?;
    Ok(())
}

/// Displaying an unbounded type parameter is refused where it is written instead of inventing a Rust bound later.
#[test]
fn unbounded_type_parameter_cannot_be_displayed_followups_a() -> Result<(), Box<dyn std::error::Error>> {
    let errors = check_str(
        r#"
def show[T](value: T) -> str:
    return f"{value}"
"#,
    )
    .err()
    .ok_or("displaying an unbounded type parameter must be refused")?;
    assert!(
        errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0103") && error.message.contains("type parameter 'T'")),
        "expected a type-parameter display refusal, got {errors:?}"
    );
    Ok(())
}

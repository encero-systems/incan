//! A literal takes the type its destination expects: the elements of a tuple literal take the destination's element
//! types (#1847), a list or dict literal inside an `Option` or union destination takes that destination's collection
//! type, or is refused when the destination holds two such types and the literal's elements do not say which (#1832),
//! and an integer literal in a float slot takes the float type (#1831), for a declaration and a reassignment alike, an
//! element or dict value assignment (#1854), an `Option` or union destination, a builtin method argument and a generic
//! call argument (#1859). A value written to an `Option` place records that place's type (#1858), and an empty
//! literal passed to a generic parameter takes the call's instantiation or is refused when nothing binds it (#1862).

use super::*;

/// Parse and check `source`, which the checker must accept, and return the checker.
fn checked(source: &str) -> Result<TypeChecker, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("typecheck failed: {errors:?}"))?;
    Ok(checker)
}

/// Return the type the checker recorded for the first appearance of `text` after `after` in `source`.
fn recorded_type<'checker>(
    checker: &'checker TypeChecker,
    source: &str,
    after: &str,
    text: &str,
) -> Result<&'checker ResolvedType, String> {
    let anchor = source.find(after).ok_or_else(|| format!("missing `{after}`"))?;
    let start = source[anchor..]
        .find(text)
        .map(|offset| anchor + offset)
        .ok_or_else(|| format!("missing `{text}` after `{after}`"))?;
    checker
        .type_info()
        .expr_type(Span::new(start, start + text.len()))
        .ok_or_else(|| format!("no type recorded for `{text}` after `{after}`"))
}

/// Return the `Option` place type the checker recorded for the value written at the first appearance of `text` after
/// `after` in `source`.
fn recorded_option_destination<'checker>(
    checker: &'checker TypeChecker,
    source: &str,
    after: &str,
    text: &str,
) -> Result<Option<&'checker ResolvedType>, String> {
    let anchor = source.find(after).ok_or_else(|| format!("missing `{after}`"))?;
    let start = source[anchor..]
        .find(text)
        .map(|offset| anchor + offset)
        .ok_or_else(|| format!("missing `{text}` after `{after}`"))?;
    Ok(checker
        .type_info()
        .option_destination_type(Span::new(start, start + text.len())))
}

/// Build `name[args...]` for a builtin collection type.
fn collection(id: CollectionTypeId, args: Vec<ResolvedType>) -> ResolvedType {
    ResolvedType::Generic(collection_types::as_str(id).to_string(), args)
}

/// Build `Option[inner]`.
fn option(inner: ResolvedType) -> ResolvedType {
    collection(CollectionTypeId::Option, vec![inner])
}

/// Build `list[element]`.
fn list(element: ResolvedType) -> ResolvedType {
    collection(CollectionTypeId::List, vec![element])
}

/// #1847: the elements of an annotated tuple literal are checked against the annotation's element types, so the
/// literal's type carries `Option[str]` for `None` and `Result[int, int]` for `Ok(1)`, down through a nested tuple.
#[test]
fn tuple_literal_takes_the_annotated_element_types_issue1847() -> Result<(), String> {
    let source = r#"
def main() -> None:
    pair: tuple[Option[str], int] = (None, 1)
    ok_pair: tuple[Result[int, int], int] = (Ok(1), 0)
    nested: tuple[tuple[Option[int], int], int] = ((None, 1), 2)
    println(pair[1] + ok_pair[1] + nested[1])
"#;
    let checker = checked(source)?;
    assert_eq!(
        recorded_type(&checker, source, "pair:", "(None, 1)")?,
        &ResolvedType::Tuple(vec![option(ResolvedType::Str), ResolvedType::Int])
    );
    assert_eq!(
        recorded_type(&checker, source, "ok_pair:", "(Ok(1), 0)")?,
        &ResolvedType::Tuple(vec![
            collection(CollectionTypeId::Result, vec![ResolvedType::Int, ResolvedType::Int]),
            ResolvedType::Int,
        ])
    );
    assert_eq!(
        recorded_type(&checker, source, "nested:", "(None, 1)")?,
        &ResolvedType::Tuple(vec![option(ResolvedType::Int), ResolvedType::Int])
    );
    Ok(())
}

/// #1847: inside a generic body the body's own type parameter is a fixed type, so `(x, None)` in a
/// `tuple[T, Option[T]]` binding records `Option[T]` for its `None`, as it does for a concrete element type.
#[test]
fn tuple_literal_in_a_generic_body_takes_the_type_parameter_issue1847() -> Result<(), String> {
    let source = r#"
def mk[T](x: T) -> int:
    pair: tuple[T, Option[T]] = (x, None)
    return 1


def main() -> None:
    println(mk("a"))
"#;
    let checker = checked(source)?;
    let type_parameter = ResolvedType::Named("T".to_string());
    assert_eq!(
        recorded_type(&checker, source, "pair:", "(x, None)")?,
        &ResolvedType::Tuple(vec![type_parameter.clone(), option(type_parameter)])
    );
    Ok(())
}

/// #1847: an element takes a float, `Option` payload or union slot of the destination, and a tuple inside an `Option`
/// destination takes the `Option`'s tuple type, while a tuple with an element its slot does not accept is still
/// refused by the declaration.
#[test]
fn tuple_literal_elements_take_float_option_and_union_slots_issue1847() -> Result<(), String> {
    let source = r#"
def main() -> None:
    floats: tuple[float, int] = (1, 2)
    payload: tuple[Option[int], int] = (5, 1)
    wrapped: Option[tuple[Option[str], int]] = (None, 2)
    either: tuple[int | str, int] = ("a", 3)
    println(floats[1] + payload[1] + either[1])
"#;
    let checker = checked(source)?;
    assert_eq!(
        recorded_type(&checker, source, "floats:", "(1, 2)")?,
        &ResolvedType::Tuple(vec![ResolvedType::Float, ResolvedType::Int])
    );
    assert_eq!(recorded_type(&checker, source, "floats:", "1")?, &ResolvedType::Float);
    assert_eq!(
        recorded_type(&checker, source, "payload:", "(5, 1)")?,
        &ResolvedType::Tuple(vec![option(ResolvedType::Int), ResolvedType::Int])
    );
    assert_eq!(
        recorded_type(&checker, source, "wrapped:", "(None, 2)")?,
        &ResolvedType::Tuple(vec![option(ResolvedType::Str), ResolvedType::Int])
    );
    let ResolvedType::Tuple(either) = recorded_type(&checker, source, "either:", "(\"a\", 3)")? else {
        return Err("the union tuple literal must record a tuple type".to_string());
    };
    assert!(
        either.first().is_some_and(ResolvedType::is_union),
        "the first element must take the union slot, got {either:?}"
    );

    let errors = check_str_err(
        "def main() -> None:\n    bad: tuple[int, int] = (\"a\", 1)\n",
        "a str element in an int slot must be refused",
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Assignment to 'bad' has type mismatch")),
        "expected the declaration to refuse the tuple, got {errors:?}"
    );
    Ok(())
}

/// #1832: an empty or `None`-only list literal assigned to an existing `Option[list[...]]` binding, or to a union
/// with one list member, takes that list type, as the same literal in a declaration does; so does an empty dict
/// literal for an `Option[dict[...]]` binding.
#[test]
fn list_and_dict_literals_take_the_option_or_union_collection_type_issue1832() -> Result<(), String> {
    let source = r#"
def main() -> None:
    declared: Option[list[int]] = []
    mut a: Option[list[int]] = None
    a = []
    mut b: Option[list[Option[str]]] = None
    b = [None]
    mut u: list[int] | str = "x"
    u = []
    mut d: Option[dict[str, int]] = None
    d = {}
    println("done")
"#;
    let checker = checked(source)?;
    assert_eq!(
        recorded_type(&checker, source, "declared:", "[]")?,
        &list(ResolvedType::Int)
    );
    assert_eq!(recorded_type(&checker, source, "a = ", "[]")?, &list(ResolvedType::Int));
    assert_eq!(
        recorded_type(&checker, source, "b = ", "[None]")?,
        &list(option(ResolvedType::Str))
    );
    assert_eq!(recorded_type(&checker, source, "u = ", "[]")?, &list(ResolvedType::Int));
    assert_eq!(
        recorded_type(&checker, source, "d = ", "{}")?,
        &collection(CollectionTypeId::Dict, vec![ResolvedType::Str, ResolvedType::Int])
    );
    Ok(())
}

/// #1832: a union with two list members gives a list literal no element type, so the literal keeps its own type and
/// selects its member, while a model beside one list member leaves that member the literal's type; an element that
/// the one list type of an `Option` destination does not accept is refused at the element.
#[test]
fn list_literal_destination_is_the_one_list_member_issue1832() -> Result<(), String> {
    let source = r#"
model Point:
    x: int


def main() -> None:
    mut u: list[int] | list[str] = [1]
    u = ["a"]
    mut p: list[int] | Point = Point(x=1)
    p = []
    println("done")
"#;
    let checker = checked(source)?;
    assert_eq!(
        recorded_type(&checker, source, "u = ", "[\"a\"]")?,
        &list(ResolvedType::Str)
    );
    assert_eq!(recorded_type(&checker, source, "p = ", "[]")?, &list(ResolvedType::Int));

    let refused = "def main() -> None:\n    mut a: Option[list[int]] = None\n    a = [\"x\"]\n";
    let errors = check_str_err(
        refused,
        "a str element in an Option[list[int]] destination must be refused",
    );
    let element_start = refused.find("\"x\"").ok_or("missing refused element")?;
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expected 'int', found 'str'")
                && error.span == Span::new(element_start, element_start + "\"x\"".len())),
        "expected the element to be refused at its own span, got {errors:?}"
    );
    Ok(())
}

/// #1832: an empty or `None`-only literal whose destination holds two or more types of its kind is refused at the
/// literal, in a declaration and a reassignment, for a list, a dict and a tuple alike: neither the destination nor the
/// elements say which of those types it is.
#[test]
fn literal_at_two_members_of_its_kind_is_refused_issue1832() -> Result<(), String> {
    for (source, literal) in [
        (
            "def main() -> None:\n    mut u: list[int] | list[str] = [1]\n    u = []\n",
            "[]",
        ),
        ("def main() -> None:\n    u: list[int] | list[str] = []\n", "[]"),
        (
            "def main() -> None:\n    mut u: list[Option[int]] | list[Option[str]] = [Some(1)]\n    u = [None]\n",
            "[None]",
        ),
        (
            "def main() -> None:\n    d: dict[str, int] | dict[str, str] = {}\n",
            "{}",
        ),
        (
            "def main() -> None:\n    p: tuple[Option[str], int] | tuple[Option[int], int] = (None, 1)\n",
            "(None, 1)",
        ),
    ] {
        let errors = check_str_err(source, "a literal at two members of its kind must be refused");
        let start = source.rfind(literal).ok_or("missing refused literal")?;
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("Cannot tell which member of")
                    && error.span == Span::new(start, start + literal.len())),
            "expected `{literal}` to be refused at its own span in:\n{source}\ngot {errors:?}"
        );
    }
    Ok(())
}

/// #1831: an integer literal takes the float type of the binding it is assigned to, as it does for a `float`
/// declaration, and a negated literal takes an `f32` binding's type; an `int` value assigned to a `float` binding is
/// still refused, because integer-to-float assignment is not an implicit conversion.
#[test]
fn integer_literal_takes_the_float_type_of_its_binding_issue1831() -> Result<(), String> {
    let source = r#"
def main() -> None:
    declared: float = 3
    mut f: float = 0.5
    f = 1
    mut g: f32 = 0.5
    g = -2
    println(declared + f + g)
"#;
    let checker = checked(source)?;
    assert_eq!(recorded_type(&checker, source, "declared:", "3")?, &ResolvedType::Float);
    assert_eq!(recorded_type(&checker, source, "f = ", "1")?, &ResolvedType::Float);
    assert_eq!(
        recorded_type(&checker, source, "g = ", "-2")?,
        &ResolvedType::Numeric(NumericTypeId::F32)
    );

    let errors = check_str_err(
        "def main() -> None:\n    n = 1\n    mut f: float = 0.5\n    f = n\n",
        "an int value assigned to a float binding must be refused",
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Assignment to 'f' has type mismatch")),
        "expected the int value to be refused, got {errors:?}"
    );
    Ok(())
}

/// #1854: an integer literal written to an element of a `list[float]` takes the float type, as it does in the list's
/// declaration, through a negation, a nested list and an `f32` element, and so does one written to a value of a
/// `dict[str, float]`; an `int` value written to such an element is still refused.
#[test]
fn integer_literal_written_to_a_float_element_takes_the_float_type_issue1854() -> Result<(), String> {
    let source = r#"
def main() -> None:
    mut xs: list[float] = [1, 2]
    xs[0] = 3
    xs[1] = -4
    mut grid: list[list[float]] = [[1.5]]
    grid[0][0] = 5
    mut narrow: list[f32] = [1.5]
    narrow[0] = 6
    mut scores: dict[str, float] = {}
    scores["a"] = 7
    println(xs[0] + grid[0][0] + scores["a"])
"#;
    let checker = checked(source)?;
    assert_eq!(recorded_type(&checker, source, "xs[0] = ", "3")?, &ResolvedType::Float);
    assert_eq!(recorded_type(&checker, source, "xs[1] = ", "-4")?, &ResolvedType::Float);
    assert_eq!(
        recorded_type(&checker, source, "grid[0][0] = ", "5")?,
        &ResolvedType::Float
    );
    assert_eq!(
        recorded_type(&checker, source, "narrow[0] = ", "6")?,
        &ResolvedType::Numeric(NumericTypeId::F32)
    );
    assert_eq!(
        recorded_type(&checker, source, "scores[\"a\"] = ", "7")?,
        &ResolvedType::Float
    );

    let errors = check_str_err(
        "def main() -> None:\n    n = 1\n    mut xs: list[float] = [0.5]\n    xs[0] = n\n",
        "an int value written to a float element must be refused",
    );
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Cannot assign 'int' to collection element of type 'float'")),
        "expected the int value to be refused, got {errors:?}"
    );
    Ok(())
}

/// #1858: a value of an `Option`'s payload type written to an `Option` place records the place's type, for a
/// `return`, through a type alias, a field assignment, a constructor field and an element assignment; a value already
/// of the place's type records it too, and a value written to a place that is not an `Option` records nothing.
#[test]
fn value_written_to_an_option_place_records_the_place_type_issue1858() -> Result<(), String> {
    let source = r#"
type MaybeInt = Option[int]

model Box:
    items: Option[list[int]] = None
    count: Option[int] = None
    total: int = 0


def make() -> Option[int]:
    return 5


def aliased() -> MaybeInt:
    return 6


def main() -> None:
    mut box = Box(count=7)
    box.items = [1]
    box.total = 8
    mut slots: list[Option[int]] = [None]
    slots[0] = 9
    box.count = Some(10)
    println(make().unwrap_or(0) + aliased().unwrap_or(0) + box.total + slots[0].unwrap_or(0))
"#;
    let checker = checked(source)?;
    let option_int = option(ResolvedType::Int);
    assert_eq!(
        recorded_option_destination(&checker, source, "return ", "5")?,
        Some(&option_int)
    );
    assert_eq!(
        recorded_option_destination(&checker, source, "def aliased", "6")?,
        Some(&option_int)
    );
    assert_eq!(
        recorded_option_destination(&checker, source, "count=", "7")?,
        Some(&option_int)
    );
    assert_eq!(
        recorded_option_destination(&checker, source, "box.items = ", "[1]")?,
        Some(&option(list(ResolvedType::Int)))
    );
    assert_eq!(
        recorded_option_destination(&checker, source, "slots[0] = ", "9")?,
        Some(&option_int)
    );
    assert_eq!(
        recorded_option_destination(&checker, source, "box.count = ", "Some(10)")?,
        Some(&option_int)
    );
    assert_eq!(
        recorded_option_destination(&checker, source, "box.total = ", "8")?,
        None
    );
    Ok(())
}

/// #1859: an integer literal takes the float type of a float slot at an `Option[float]` or `float | str` destination,
/// an `Option[float]` parameter, the payload of `Ok` at a `Result[float, str]`, the argument of `append` on a
/// `list[float]`, `insert` on a `dict[str, float]` and `unwrap_or` on an `Option[float]`, the exponent of `powf`, and
/// an argument of a generic call whose other argument binds its type parameter to `float`, whichever comes first; at a
/// destination holding `int` it stays an `int`.
#[test]
fn integer_literal_takes_the_float_type_in_more_float_slots_issue1859() -> Result<(), String> {
    let source = r#"
def pick[T](a: T, b: T) -> T:
    return a


def takes(value: Option[float]) -> float:
    return value.unwrap_or(0.0)


def main() -> None:
    x = 1.5
    squared = x.powf(2)
    maybe: Option[float] = 3
    either: float | str = 4
    result: Result[float, str] = Ok(5)
    mut xs: list[float] = []
    xs.append(6)
    mut d: dict[str, float] = {}
    d.insert("k", 7)
    fallback = maybe.unwrap_or(8)
    first = pick(2.5, 9)
    second = pick(10, 2.5)
    wide: int | float = 11
    println(squared + takes(12) + fallback + first + second + xs[0] + d["k"] + result.unwrap_or(0.0))
"#;
    let checker = checked(source)?;
    for (after, literal) in [
        ("powf(", "2"),
        ("maybe: Option[float] = ", "3"),
        ("either: float | str = ", "4"),
        ("Ok(", "5"),
        ("append(", "6"),
        ("insert(\"k\", ", "7"),
        ("maybe.unwrap_or(", "8"),
        ("pick(2.5, ", "9"),
        ("second = pick(", "10"),
        ("takes(", "12"),
    ] {
        assert_eq!(
            recorded_type(&checker, source, after, literal)?,
            &ResolvedType::Float,
            "`{literal}` after `{after}` must take the float type"
        );
    }
    assert_eq!(
        recorded_type(&checker, source, "wide: int | float = ", "11")?,
        &ResolvedType::Int
    );
    Ok(())
}

/// #1862: an empty list literal passed to a generic `list[T]` parameter takes the element type another argument binds
/// or the explicit type argument names, and keeps a type parameter of the caller's own body that the call passes on;
/// when nothing binds `T`, the literal is refused at its own span, for an `Option[list[T]]` parameter and an empty
/// dict literal too.
#[test]
fn empty_literal_argument_takes_the_generic_instantiation_issue1862() -> Result<(), String> {
    let source = r#"
def first_or[T](items: list[T], default: T) -> T:
    return default


def count[T](items: list[T]) -> int:
    return len(items)


def outer[T](value: T) -> T:
    return first_or([], value)


def main() -> None:
    println(first_or([], 5))
    println(count[str]([]))
    println(outer(3))
"#;
    let checker = checked(source)?;
    assert_eq!(
        recorded_type(&checker, source, "println(first_or(", "[]")?,
        &list(ResolvedType::Int)
    );
    assert_eq!(
        recorded_type(&checker, source, "count[str](", "[]")?,
        &list(ResolvedType::Str)
    );
    assert_eq!(
        recorded_type(&checker, source, "return first_or(", "[]")?,
        &list(ResolvedType::Named("T".to_string()))
    );

    for (source, literal) in [
        (
            "def count[T](items: list[T]) -> int:\n    return len(items)\n\ndef main() -> None:\n    println(count([]))\n",
            "[]",
        ),
        (
            "def count[T](items: Option[list[T]]) -> int:\n    return 0\n\ndef main() -> None:\n    println(count([]))\n",
            "[]",
        ),
        (
            "def size[K, V](items: dict[K, V]) -> int:\n    return len(items)\n\ndef main() -> None:\n    println(size({}))\n",
            "{}",
        ),
    ] {
        let errors = check_str_err(source, "an empty literal leaving a type parameter open must be refused");
        let start = source.rfind(literal).ok_or("missing refused literal")?;
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("Cannot infer type parameter")
                    && error.span == Span::new(start, start + literal.len())),
            "expected `{literal}` to be refused at its own span in:\n{source}\ngot {errors:?}"
        );
    }
    Ok(())
}

//! A literal takes the type its destination expects: the elements of a tuple literal take the destination's element
//! types (#1847), a list or dict literal inside an `Option` or union destination takes that destination's collection
//! type (#1832), and an integer literal in a float slot takes the float type (#1831), for a declaration and a
//! reassignment alike.

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

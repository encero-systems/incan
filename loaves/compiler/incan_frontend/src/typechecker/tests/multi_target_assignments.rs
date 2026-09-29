//! Tuple assignments whose targets mix fields or elements with names: each name follows the rule `name = value`
//! follows, so a bound `mut` name is reassigned and a name with no binding is declared.

use super::*;

/// A tuple assignment that writes a field or an element beside a name gives the name what `name = value` gives it: a
/// name with no binding is declared with its element's type, and a `mut` binding is reassigned.
#[test]
fn tuple_assignment_beside_a_place_declares_an_unbound_name() -> Result<(), String> {
    check_str(
        r#"
def main() -> None:
    mut items = [1, 2, 3]
    items[0], fresh = (5, 6)
    total: int = fresh + items[0]
    mut count = 0
    items[1], count = (count, 7)
    for n in items:
        items[2], latest = (n, n + 1)
        println(latest)
    println(f"{total} {count}")
"#,
    )
    .map_err(|errors| {
        format!(
            "a name beside a place must be declared or reassigned, got: {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        )
    })
}

/// A name a tuple assignment declares has its element's type, and a bound name that is not `mut` is still refused.
#[test]
fn tuple_assignment_beside_a_place_types_the_declared_name_and_refuses_an_immutable_one() {
    let typed = r#"
def main() -> None:
    mut items = [1, 2, 3]
    items[0], label = (5, "six")
    n: int = label
"#;
    let errors = check_str_err(typed, "the declared name has the element's type, str");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("int") && error.message.contains("str")),
        "expected a mismatch between str and int, got: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    assert!(
        !has_unknown_symbol_error(&errors, "label"),
        "the name must be declared, got: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );

    let immutable = r#"
def main() -> None:
    mut items = [1, 2, 3]
    let fixed = 1
    items[0], fixed = (5, 6)
"#;
    let errors = check_str_err(immutable, "reassigning an immutable binding must be refused");
    assert!(
        errors.iter().any(|error| error.message.contains("fixed")),
        "expected the refusal to name `fixed`, got: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

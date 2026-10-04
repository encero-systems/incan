//! Focused frontend regressions for the dev.6 followups-b lane: ordered generic sorting, stored trait annotations,
//! generic-bound validation, and generic inference through inline list literals.

use super::*;

/// A type parameter explicitly bounded by `Ord` is a valid `sorted()` element type.
#[test]
fn followups_b_sorted_accepts_an_ord_bounded_type_parameter() {
    assert_check_ok(
        r#"
def ordered[T with Ord](items: list[T]) -> list[T]:
    return sorted(items)
"#,
    );
}

/// Trait values have no concrete stored representation, directly or below a list field.
#[test]
fn followups_b_model_fields_refuse_direct_and_nested_trait_types() {
    let errors = check_str_err(
        r#"
from std.serde.json import Serialize

model Envelope:
    item: Serialize
    items: list[Serialize]
"#,
        "trait-typed model fields unexpectedly passed check",
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("Trait-typed field"))
            .count(),
        2,
        "expected one refusal for each stored trait field: {errors:?}"
    );
}

/// Every written type-parameter bound must resolve to a declaration.
#[test]
fn followups_b_unknown_type_parameter_bound_is_refused() {
    let errors = check_str_err(
        "def identity[T with Nonexistent](value: T) -> T:\n    return value\n",
        "an unknown generic bound unexpectedly passed check",
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Unknown symbol 'Nonexistent'")),
        "expected the unknown bound itself to be diagnosed: {errors:?}"
    );
}

/// A concrete constructor inside an inline list closes the callee's list element type parameter.
#[test]
fn followups_b_inline_model_list_argument_infers_the_generic_element() {
    assert_check_ok(
        r#"
model Item:
    value: int

def identity[T](items: list[T]) -> list[T]:
    return items

def main() -> None:
    items = identity([Item(value=1)])
    println(items[0].value)
"#,
    );
}

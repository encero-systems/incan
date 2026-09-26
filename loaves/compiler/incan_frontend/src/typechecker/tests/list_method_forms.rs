//! `count` and `index` on a list, whose form the argument count picks (#1776): `count()` is the iterator terminal,
//! `count(value)` and `index(value)` the list methods.

use super::*;

/// #1776: both `count` forms and `index(value)` check on one list, each typed `int`, and the list stays usable after
/// the no-argument terminal.
#[test]
fn list_count_and_index_forms_follow_the_argument_count_issue1776() -> Result<(), String> {
    check_str(
        r#"
def main() -> None:
    items: list[int] = [4, 9, 2, 9]
    total: int = items.count()
    nines: int = items.count(9)
    position: int = items.index(2)
    println(f"{total} {nines} {position} {len(items)}")

def counted() -> int:
    numbers = [1, 2, 3]
    return numbers.iter().count()
"#,
    )
    .map_err(|errors| {
        format!(
            "expected both count forms and index(value) to check, got: {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        )
    })
}

/// #1776: an argument count that fits neither `count` form is refused with a message naming both forms, and one that
/// does not fit `index(value)` names the one form it has.
#[test]
fn list_count_and_index_with_other_argument_counts_are_refused_issue1776() -> Result<(), String> {
    let errors = match check_str(
        r#"
def main() -> None:
    items: list[int] = [4, 9, 2, 9]
    println(items.count(9, 2))
    println(items.index())
"#,
    ) {
        Ok(()) => return Err("the checker accepted count(9, 2) and index()".to_string()),
        Err(errors) => errors,
    };
    let count_refused = errors.iter().any(|error| {
        error.message.contains("'count'")
            && error.message.contains("no argument or one value")
            && error
                .notes
                .iter()
                .any(|note| note.contains("items.count()") && note.contains("items.count(value)"))
    });
    let index_refused = errors
        .iter()
        .any(|error| error.message.contains("'index'") && error.message.contains("takes one value"));
    if count_refused && index_refused {
        Ok(())
    } else {
        Err(format!(
            "expected refusals naming the count and index forms, got: {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        ))
    }
}

/// #1776: the value `count(value)` counts must fit the list's element type.
#[test]
fn list_count_value_of_another_type_is_refused_issue1776() -> Result<(), String> {
    match check_str(
        r#"
def main() -> None:
    items: list[int] = [4, 9, 2, 9]
    println(items.count("nine"))
"#,
    ) {
        Ok(()) => Err("the checker accepted count(\"nine\") on a list[int]".to_string()),
        Err(errors) if errors.iter().any(|error| error.message.contains("Type mismatch")) => Ok(()),
        Err(errors) => Err(format!(
            "expected a type mismatch, got: {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        )),
    }
}

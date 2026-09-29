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

/// A list, set, frozen collection or `Iterable[T]` value provides `iter()`, and the RFC 088 adapters and terminals are
/// methods of the iterator it returns: calling one on the collection itself is refused with `INCAN-T0001`, while its
/// `.iter()` form and the list's own `count()` and `count(value)` check (#1561).
#[test]
fn iterator_methods_on_a_collection_itself_are_refused_issue1561() -> Result<(), String> {
    for call in [
        "items.map(double)",
        "items.filter(odd)",
        "items.flat_map(twice)",
        "items.take_while(odd)",
        "items.skip_while(odd)",
        "items.take(1)",
        "items.skip(1)",
        "items.enumerate()",
        "items.zip([4, 5])",
        "items.chain([4])",
        "items.batch(2)",
        "items.collect()",
        "items.any(odd)",
        "items.all(odd)",
        "items.find(odd)",
        "items.fold(0, add)",
        "items.reduce(0, add)",
        "items.for_each(show)",
        "items.sum()",
        "{1, 2}.any(odd)",
    ] {
        let source = format!(
            "def double(x: int) -> int:\n    return x * 2\n\ndef odd(x: int) -> bool:\n    return x % 2 == 1\n\n\
             def twice(x: int) -> list[int]:\n    return [x, x]\n\ndef add(a: int, b: int) -> int:\n    return a + b\n\n\
             def show(x: int) -> None:\n    println(x)\n\ndef main() -> None:\n    items = [1, 2, 3]\n    kept = {call}\n"
        );
        let errors = check_str(&source).err().ok_or(format!("`{call}` must be refused"))?;
        let method = call
            .split('.')
            .next_back()
            .and_then(|rest| rest.split('(').next())
            .unwrap_or(call);
        let refusal = errors
            .iter()
            .find(|error| error.message.contains(&format!("has no method '{method}(...)'")))
            .ok_or(format!(
                "`{call}` has no refusal naming '{method}': {:?}",
                errors.iter().map(|error| &error.message).collect::<Vec<_>>()
            ))?;
        let code = crate::diagnostics::code_for_error(refusal, crate::diagnostics::DiagnosticPhase::Typecheck);
        if code != "INCAN-T0001" {
            return Err(format!("`{call}` is refused with {code}, not INCAN-T0001"));
        }
    }
    let errors = check_str(
        r#"
def odd(x: int) -> bool:
    return x % 2 == 1

def any_odd(xs: Iterable[int]) -> bool:
    return xs.any(odd)

def all_odd(xs: FrozenList[int]) -> bool:
    return xs.all(odd)
"#,
    )
    .err()
    .ok_or("`any` on an Iterable[int] and `all` on a FrozenList[int] must be refused")?;
    for needle in [
        "Type 'Iterable[int]' has no method 'any(...)'",
        "has no method 'all(...)'",
    ] {
        if !errors.iter().any(|error| error.message.contains(needle)) {
            return Err(format!(
                "expected a refusal containing `{needle}`, got: {:?}",
                errors.iter().map(|error| &error.message).collect::<Vec<_>>()
            ));
        }
    }
    check_str(
        r#"
def odd(x: int) -> bool:
    return x % 2 == 1

def any_odd(xs: Iterable[int]) -> bool:
    return xs.iter().any(odd)

def main() -> None:
    items = [1, 2, 3]
    kept: list[int] = items.iter().filter(odd).collect()
    total: int = items.count()
    ones: int = items.count(1)
    println(f"{len(kept)} {total} {ones} {len(items)}")
    println({1, 2}.iter().any(odd))
"#,
    )
    .map_err(|errors| {
        format!(
            "expected the iter() forms and the list's count forms to check, got: {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        )
    })
}

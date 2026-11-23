//! Unpacking a fresh tuple into new names in an explicit body: a call result that can hold no borrow, also taken
//! with `?`, is evaluated once and each name owns its element. A stored tuple reached through a view stays refused.

use super::*;

const ROW: &str = "model Row:\n    cells: list[int]\n\nmodel Pair:\n    items: tuple[Row, int]\n\ndef pair(count: int) -> tuple[Row, int]:\n    return (Row(cells=[count]), count)\n\ndef split(count: int) -> Result[tuple[str, int], str]:\n    return Ok((\"a\", count))\n\n";

/// The flow outcome of the named body.
fn outcome(checker: &TypeChecker, name: &str) -> Option<String> {
    flow_summary(checker)
        .into_iter()
        .find(|(owner, _)| owner == name)
        .map(|(_, outcome)| outcome)
}

/// A fresh tuple from a call, and one taken out of a `Result` with `?`, unpack into new owners.
#[test]
fn fresh_tuples_unpack_into_new_names() -> Result<(), Box<dyn std::error::Error>> {
    let source = format!(
        "{ROW}def build(row: Borrowed[Row]) -> Result[int, str]:\n    first, count = pair(len(row.cells))\n    label, size = split(count)?\n    return Ok(len(first.cells) + size + len(label))\n"
    );
    let checker = checked(&source)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    assert_eq!(outcome(&checker, "build").as_deref(), Some("checked []"));
    Ok(())
}

/// A stored tuple reached through a view is not unpacked as fresh: its elements would leave borrowed storage.
#[test]
fn stored_tuples_reached_through_a_view_stay_refused() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        format!("{ROW}def build(holder: Borrowed[Pair]) -> int:\n    first, count = holder.items\n    return count\n");
    let checker = checked(&source)?;
    assert!(
        outcome(&checker, "build")
            .is_some_and(|outcome| outcome.contains("tuple unpacking needs checked aggregate dependency facts")),
        "{:?}",
        flow_summary(&checker)
    );
    Ok(())
}

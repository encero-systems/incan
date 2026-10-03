//! The reads a `const` frozen collection answers as its mutable form does (#1757): membership over `FrozenList`,
//! `FrozenSet` and `FrozenDict`, the probe of `FrozenSet.contains`, and indexing a `FrozenList`.

use super::*;

/// Collect the messages of a refused program, or report that it was accepted.
fn refusal_messages(source: &str, what: &str) -> Result<Vec<String>, String> {
    match check_str(source) {
        Err(errors) => Ok(errors.into_iter().map(|error| error.message).collect()),
        Ok(()) => Err(format!("{what} must be refused")),
    }
}

/// `x in c` over a `const` `FrozenList`, `FrozenSet` or `FrozenDict` is a `bool` membership test of an element or a
/// key, as it is over a `list`, `set` or `dict`. A text element or key takes any text probe, and a probe outside the
/// element or key type is refused (#1757).
#[test]
fn frozen_collection_membership_checks_its_probe_issue1757() -> Result<(), String> {
    check_str(
        r#"
const NAMES: FrozenList[str] = ["alpha", "beta"]
const IDS: FrozenSet[int] = {3}
const CODES: FrozenDict[FrozenStr, int] = {"a": 1}
const LABEL: str = "alpha"


def main() -> None:
    probe = "a"
    found: bool = probe in NAMES
    println(found)
    println(LABEL in NAMES)
    println(3 in IDS)
    println(probe not in CODES)
"#,
    )
    .map_err(|errors| {
        format!(
            "frozen membership must check: {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        )
    })?;

    let messages = refusal_messages(
        r#"
const IDS: FrozenSet[int] = {3}
const SQUARES: FrozenDict[int, int] = {2: 4}


def by_text() -> bool:
    return "three" in IDS


def by_key_text() -> bool:
    return "two" not in SQUARES
"#,
        "a mistyped frozen membership probe",
    )?;
    let refusals = messages
        .iter()
        .filter(|message| message.contains("Type mismatch: expected 'int', found 'str'"))
        .count();
    if refusals != 2 {
        return Err(format!(
            "expected the element and key probe refusals, got: {messages:?}"
        ));
    }
    Ok(())
}

/// `frozen_set.contains(x)` takes one probe under the membership rule of `x in frozen_set`: an element, or any text
/// for a text element; a probe outside the element type and a missing probe are refused (#1757).
#[test]
fn frozen_set_contains_checks_its_probe_issue1757() -> Result<(), String> {
    check_str(
        r#"
const TAGS: FrozenSet[str] = {"a", "b"}
const IDS: FrozenSet[int] = {3}


def main() -> None:
    probe = "a"
    println(TAGS.contains(probe))
    println(IDS.contains(3))
"#,
    )
    .map_err(|errors| {
        format!(
            "a probe of the element type must check: {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        )
    })?;

    let messages = refusal_messages(
        r#"
const IDS: FrozenSet[int] = {3}


def by_text() -> bool:
    return IDS.contains("three")


def without_probe() -> bool:
    return IDS.contains()
"#,
        "a mistyped or missing frozen set probe",
    )?;
    for expected in [
        "Argument to 'FrozenSet.contains' has type mismatch: expected 'int', found 'str'",
        "FrozenSet.contains() expects 1 argument(s), got 0",
    ] {
        if !messages.iter().any(|message| message.contains(expected)) {
            return Err(format!("expected `{expected}`, got: {messages:?}"));
        }
    }
    Ok(())
}

/// `items[i]` on a `const` `FrozenList[T]` is typed `T` at an `int` index, so a `str` element reads as a `str`, and an
/// index that is not an `int` is refused (#1757).
#[test]
fn frozen_list_index_reads_the_element_type_issue1757() -> Result<(), String> {
    check_str(
        r#"
const NAMES: FrozenList[str] = ["alpha", "beta"]
const NUMS: FrozenList[int] = [1, 2]


def main() -> None:
    first: str = NAMES[0]
    shout: str = NAMES[-1].upper()
    total: int = NUMS[0] + NUMS[1]
    println(first + shout)
    println(total)
"#,
    )
    .map_err(|errors| {
        format!(
            "a frozen list read must check as its element type: {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        )
    })?;

    let messages = refusal_messages(
        r#"
const NUMS: FrozenList[int] = [1, 2]


def by_text() -> int:
    return NUMS["first"]


def as_text() -> str:
    return NUMS[0]
"#,
        "a mistyped frozen list read",
    )?;
    if !messages
        .iter()
        .any(|message| message.contains("Index type mismatch: expected 'int', found 'str'"))
    {
        return Err(format!("expected the index type refusal, got: {messages:?}"));
    }
    if messages.len() < 2 {
        return Err(format!(
            "expected the element type refusal for `str` as well, got: {messages:?}"
        ));
    }
    Ok(())
}

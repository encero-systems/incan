//! RFC 129 ordinary parameters inside an explicit body: an ordinary inferred slot receives a value, so a fresh value
//! is evaluated into it and an ordinary place is read as it is passed; a caller-visible `mut` slot writes through, so
//! the call takes its argument's place exclusively.

use super::*;

/// The flow recorded for the named body.
fn flow<'a>(checker: &'a TypeChecker, name: &str) -> Option<&'a InputFlow> {
    checker
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == name)
        .map(|(_, flow)| flow)
}

/// Assert the named body's flow is proved and the check reports nothing.
fn assert_proved(source: &str, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let checker = checked(source)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    assert!(
        matches!(flow(&checker, name), Some(InputFlow::Checked { .. })),
        "{name}: {:?}",
        flow(&checker, name)
    );
    Ok(())
}

/// A field of an owned input, an ordinary input, a literal and a fresh result all pass to ordinary value slots. A
/// place reached through a shared input stays refused: whether the callee shares it or would clone it is not known.
#[test]
fn ordinary_value_slots_take_places_and_fresh_values() -> Result<(), Box<dyn std::error::Error>> {
    let declarations = "model Row:\n    name: str\n    tags: list[str]\n\ndef encode(row: Borrowed[Row]) -> bytes:\n    return b\"row\"\n\ndef size(data: bytes) -> int:\n    return len(data)\n\ndef count(tags: list[str], label: str) -> int:\n    return len(tags) + len(label)\n\n";
    assert_proved(
        &format!(
            "{declarations}def digest(row: Owned[Row], label: str) -> int:\n    return size(encode(row)) + count(row.tags, row.name) + count(row.tags, label) + count([], \"x\")\n"
        ),
        "digest",
    )?;
    let shared = format!("{declarations}def shared(row: Borrowed[Row]) -> int:\n    return count(row.tags, \"x\")\n");
    let checker = checked(&shared)?;
    let Some(InputFlow::Unavailable(unavailable)) = flow(&checker, "shared") else {
        return Err(format!(
            "a shared input's place passed to an ordinary slot was admitted: {:?}",
            flow(&checker, "shared")
        )
        .into());
    };
    assert_eq!(
        unavailable.reason,
        "ordinary argument requires inferred access or mutable binding facts"
    );
    Ok(())
}

/// An owned input's field forwarded through an ordinary helper into an owned result is a value the helper owns: the
/// result retains no dependency on the input (#3022).
#[test]
fn owned_field_forwarded_through_a_helper_retains_nothing() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Inputs:\n    values: list[str]\n\nmodel Outcome:\n    values: list[str]\n\ndef retain(values: list[str]) -> Outcome:\n    return Outcome(values=values)\n\ndef resume(inputs: Owned[Inputs]) -> Outcome:\n    return retain(inputs.values)\n";
    assert_proved(source, "resume")
}

/// A caller-visible `mut` slot writes through: an exclusive input passes to it, and a shared one is refused.
#[test]
fn caller_visible_mut_slots_take_their_argument_exclusively() -> Result<(), Box<dyn std::error::Error>> {
    let declarations = "model Remap:\n    seen: list[int]\n\ndef note(mut remap: Remap, value: int) -> None:\n    remap.seen.append(value)\n\n";
    assert_proved(
        &format!(
            "{declarations}def record(mut remap: BorrowedMut[Remap]) -> int:\n    note(remap, 1)\n    return len(remap.seen)\n"
        ),
        "record",
    )?;
    let refused = format!(
        "{declarations}def record(remap: Borrowed[Remap]) -> int:\n    note(remap, 1)\n    return len(remap.seen)\n"
    );
    let checker = checked(&refused)?;
    assert!(
        checker
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0126")),
        "{:?}",
        checker.errors
    );
    Ok(())
}

/// A builtin method whose result is closed plain data reads its receiver and returns a fresh value: a clone, a
/// transformed copy and a length of a shared input. A fresh result taken with `?` passes into a builtin collection.
#[test]
fn builtin_methods_with_plain_results_read_their_receiver() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def parse(text: str) -> Result[str, str]:\n    return Ok(text)\n\ndef shout(text: Borrowed[str]) -> str:\n    return text.upper() + text.clone()\n\ndef collect(mut items: BorrowedMut[list[str]], text: str) -> Result[int, str]:\n    items.append(parse(text)?)\n    return Ok(len(items))\n";
    assert_proved(source, "shout")?;
    assert_proved(source, "collect")
}

/// A call result that can hold no borrow is a fresh value in every position that takes ownership: a constructor
/// field, an enum payload, a list element and a builtin collection argument, also when taken with `?`.
#[test]
fn fresh_call_results_fill_owned_positions() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Tag:\n    name: str\n\nmodel Entry:\n    tag: Tag\n    tags: list[Tag]\n\nenum Slot:\n    Filled(Tag)\n\ndef tag(name: str) -> Tag:\n    return Tag(name=name)\n\ndef parse(name: str) -> Result[Tag, str]:\n    return Ok(Tag(name=name))\n\ndef build(names: Borrowed[list[str]]) -> Result[Entry, str]:\n    mut tags = [tag(\"a\"), parse(\"b\")?]\n    tags.append(tag(\"c\"))\n    slot = Slot.Filled(tag(\"d\"))\n    match slot:\n        Slot.Filled(value) => println(value.name)\n    println(len(tags))\n    return Ok(Entry(tag=tag(\"e\"), tags=[tag(\"f\")]))\n";
    assert_proved(source, "build")
}

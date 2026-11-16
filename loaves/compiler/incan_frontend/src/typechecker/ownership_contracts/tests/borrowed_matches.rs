//! A match on a place reached through an explicit view borrows that place for its arms: each payload binding is a
//! view of its storage that keeps the borrow while used (RFC 164).

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

const NODE: &str = "enum Node:\n    Scalar(str)\n    Pair(str, list[int])\n    Other\n\n";

/// Matching a shared input binds views of its payloads, which each arm reads.
#[test]
fn match_on_a_shared_input_binds_views() -> Result<(), Box<dyn std::error::Error>> {
    let source = format!(
        "{NODE}def size(node: Borrowed[Node], extra: Option[int]) -> int:\n    match node:\n        Node.Scalar(name) => return len(name)\n        Node.Pair(name, items) => return len(name) + len(items)\n        Node.Other => pass\n    match extra:\n        Some(amount) => return amount\n        None => pass\n    return 0\n"
    );
    let checker = checked(&source)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    assert!(
        matches!(flow(&checker, "size"), Some(InputFlow::Checked { .. })),
        "{:?}",
        flow(&checker, "size")
    );
    Ok(())
}

/// A payload bound from a shared input is a view: returning it as an owned value is refused, and an explicit clone
/// is accepted.
#[test]
fn borrowed_match_payload_moves_only_by_clone() -> Result<(), Box<dyn std::error::Error>> {
    let moved = format!(
        "{NODE}def first(node: Borrowed[Node]) -> str:\n    match node:\n        Node.Scalar(name) => return name\n        _ => pass\n    return \"\"\n"
    );
    let checker = checked(&moved)?;
    assert!(
        checker
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0126")),
        "{:?}",
        checker.errors
    );
    let cloned = format!(
        "{NODE}def first(node: Borrowed[Node]) -> str:\n    match node:\n        Node.Scalar(name) => return name.clone()\n        _ => pass\n    return \"\"\n"
    );
    let checker = checked(&cloned)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    Ok(())
}

/// A projection of a returned view is reached through the view's result loan: it passes to a shared slot as a
/// borrow of the projected storage, and a match on it borrows it too.
#[test]
fn projections_of_a_returned_view_are_borrowed_through_it() -> Result<(), Box<dyn std::error::Error>> {
    let source = format!(
        "{NODE}model Holder:\n    node: Node\n    name: str\n\nmodel Frame:\n    holder: Holder\n\ndef holder_of(frame: Borrowed[Frame]) -> Borrowed[Holder] from frame.holder:\n    return frame.holder\n\ndef size(text: Borrowed[str]) -> int:\n    return len(text)\n\ndef measure(frame: Borrowed[Frame]) -> int:\n    total = size(holder_of(frame).name)\n    match holder_of(frame).node:\n        Node.Scalar(name) => return total + len(name)\n        _ => pass\n    return total\n"
    );
    let checker = checked(&source)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    assert!(
        matches!(flow(&checker, "measure"), Some(InputFlow::Checked { .. })),
        "{:?}",
        flow(&checker, "measure")
    );
    Ok(())
}

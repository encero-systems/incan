//! Places reached through an explicit view, passed to a call next to other arguments: an observer argument such as
//! `len(view.items)` completes before the call's own argument markers, and an indexed projection evaluates its index
//! once, as the projection walker claims it.

use super::*;

const BODY: &str = "model Param:\n    ty: str\n\nmodel Body:\n    params: list[Param]\n    names: list[str]\n    ret: str\n\ndef take(ty: Borrowed[str], names: list[str], slot: int) -> None:\n    pass\n\n";

/// Assert the named body's flow is proved and the check reports nothing.
fn assert_proved(source: &str, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let checker = checked(source)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    assert!(
        flow_summary(&checker)
            .iter()
            .any(|(owner, outcome)| owner == name && outcome == "checked []"),
        "{:?}",
        flow_summary(&checker)
    );
    Ok(())
}

/// An observer argument after view projections builds a valid graph: the call's argument end marker at the
/// observer's span is not an operand evaluated after the observer completes.
#[test]
fn observer_argument_after_view_projections_is_proved() -> Result<(), Box<dyn std::error::Error>> {
    assert_proved(
        &format!("{BODY}def b(body: Borrowed[Body]) -> None:\n    take(body.ret, body.names, len(body.params))\n"),
        "b",
    )
}

/// An indexed projection through a view passed to a borrowed slot evaluates its index once, and the projection
/// walker claims that read.
#[test]
fn indexed_view_projection_argument_is_proved() -> Result<(), Box<dyn std::error::Error>> {
    assert_proved(
        &format!(
            "{BODY}def c(body: Borrowed[Body]) -> None:\n    for slot in range(len(body.params)):\n        take(body.params[slot].ty, body.names, slot)\n"
        ),
        "c",
    )
}

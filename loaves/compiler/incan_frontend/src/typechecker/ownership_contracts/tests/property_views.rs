//! RFC 164 property results: a property is a getter of an immutable receiver, so a `Borrowed[T] from self.<field>`
//! result is a returned view of that field, proved over the getter's body and projected at every read.

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

const FRAME: &str = "model Frame:\n    nominals: list[int]\n    count: int\n\n    property nominals_view -> Borrowed[list[int]] from self.nominals:\n        return self.nominals\n\n";

/// A property's `Borrowed[T] from self.<field>` result is a contract: its getter is proved to return a view of that
/// field, and an explicit body reading the property projects the view, while a disjoint sibling field stays usable.
#[test]
fn property_view_result_is_a_proved_contract() -> Result<(), Box<dyn std::error::Error>> {
    let source = format!(
        "{FRAME}def total(frame: Borrowed[Frame]) -> int:\n    view = frame.nominals_view\n    return len(view) + view[0] + frame.count + len(frame.nominals_view)\n"
    );
    let checker = checked(&source)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    for name in ["nominals_view", "total"] {
        assert!(
            matches!(flow(&checker, name), Some(InputFlow::Checked { .. })),
            "{name}: {:?}",
            flow(&checker, name)
        );
    }
    Ok(())
}

/// The view a property returns keeps its field borrowed: replacing the field while the view is still used is refused,
/// and a borrowed result without a declared source is refused at the property's annotation.
#[test]
fn property_view_result_keeps_its_field_borrowed() -> Result<(), Box<dyn std::error::Error>> {
    let conflict = format!(
        "{FRAME}def grow(mut frame: BorrowedMut[Frame]) -> int:\n    view = frame.nominals_view\n    frame.nominals = []\n    return len(view)\n"
    );
    let checker = checked(&conflict)?;
    assert!(
        checker
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0126")),
        "{:?}",
        checker.errors
    );
    let unsourced = "model Frame:\n    nominals: list[int]\n\n    property nominals_view -> Borrowed[list[int]]:\n        return self.nominals\n";
    let checker = checked(unsourced)?;
    assert!(
        checker
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0123")),
        "{:?}",
        checker.errors
    );
    Ok(())
}

/// A property whose result holds no borrow is a getter call in an explicit body: its result is a fresh value.
#[test]
fn owned_property_result_is_a_fresh_value() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Frame:\n    nominals: list[int]\n\n    property size -> int:\n        return len(self.nominals)\n\n    property first_two -> list[int]:\n        return [self.nominals[0], self.nominals[1]]\n\ndef total(frame: Borrowed[Frame]) -> int:\n    pair = frame.first_two\n    return frame.size + len(pair)\n";
    let checker = checked(source)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    assert!(
        matches!(flow(&checker, "total"), Some(InputFlow::Checked { .. })),
        "{:?}",
        flow(&checker, "total")
    );
    Ok(())
}

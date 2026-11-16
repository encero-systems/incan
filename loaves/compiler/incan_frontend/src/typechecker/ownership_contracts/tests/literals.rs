//! Tuple, set and dict literals in an explicit body construct fresh storage from the values they are given: a
//! Copy value, a literal or a fresh call result fills an element, as it fills a list element.

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

const ROW: &str = "class Row:\n    pub cells: list[int]\n\n";

/// Fresh constructor results, literals and Copy reads fill tuple, set and dict elements.
#[test]
fn literals_take_fresh_values() -> Result<(), Box<dyn std::error::Error>> {
    let source = format!(
        "{ROW}def build(row: Borrowed[Row]) -> int:\n    pair = (Row(cells=[3]), Row(cells=[len(row.cells)]))\n    table: dict[str, Row] = {{\"a\": Row(cells=[5]), \"b\": Row(cells=[])}}\n    marks = {{1, len(row.cells)}}\n    return len(row.cells)\n"
    );
    let checker = checked(&source)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    assert!(
        matches!(flow(&checker, "build"), Some(InputFlow::Checked { .. })),
        "{:?}",
        flow(&checker, "build")
    );
    Ok(())
}

/// A non-Copy place reached through a view cannot fill an element: that would move or silently clone it.
#[test]
fn literal_elements_refuse_borrowed_places() -> Result<(), Box<dyn std::error::Error>> {
    for literal in ["(row.cells, 1)", "{\"a\": row.cells}"] {
        let source = format!("{ROW}def build(row: Borrowed[Row]) -> int:\n    value = {literal}\n    return 0\n");
        let checker = checked(&source)?;
        assert!(
            !matches!(flow(&checker, "build"), Some(InputFlow::Checked { .. })) || !checker.errors.is_empty(),
            "{literal}: {:?}",
            flow(&checker, "build")
        );
    }
    Ok(())
}

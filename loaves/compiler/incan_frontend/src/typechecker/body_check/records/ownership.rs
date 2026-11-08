//! Primitive adapters for the Incan-owned ownership field grammar. No native layout crosses this boundary.

use super::types::{next, read_type, type_fields};
use super::{Fields, NodeBindingError, Positions, Span, TypeChecker};
use crate::ownership_metadata::{
    AccessMode, CheckedOwnershipContract, OwnershipContract, ResultDependency, SourceSlot, TypeComponent,
};
use incan_lang::lang::types::ownership;

/// Persist the declaration's canonical explicit contract beside both declaration and settled-body records.
pub(super) fn project(
    checker: &TypeChecker,
    owner: Span,
    positions: &Positions,
    fields: &mut Fields,
) -> Result<(), NodeBindingError> {
    let Some(contract) = checker
        .declaration_view()
        .function(owner)
        .and_then(|binding| binding.identity.as_ref())
        .and_then(|identity| checker.type_info.declarations.ownership_contracts.get(identity))
    else {
        return Ok(());
    };
    let mut row = vec![
        "ownership".into(),
        positions.nodes.anchor(owner)?.path,
        "contract".into(),
        contract.schema_version.to_string(),
        contract.modes.len().to_string(),
    ];
    for access in &contract.modes {
        row.push(access.mode.as_str().into());
        row.push(
            access
                .component
                .parameter
                .map_or_else(|| "result".into(), |slot| slot.to_string()),
        );
        row.push(access.component.children.len().to_string());
        row.extend(access.component.children.iter().map(usize::to_string));
        type_fields(&access.underlying, &mut row)?;
    }
    if let Some(source) = &contract.result_source {
        match source.slot {
            SourceSlot::Parameter(slot) => row.extend(["parameter".into(), slot.to_string()]),
            SourceSlot::Receiver => row.push("receiver".into()),
        }
        row.push(source.fields.len().to_string());
        for field in &source.fields {
            row.push(positions.identity(field)?);
        }
    } else {
        row.push("none".into());
    }
    fields.push(row);
    Ok(())
}

/// Rebuild from checked field primitives and current identity handles; no current semantic contract is consulted.
pub(super) fn read(positions: &Positions, fields: &[String]) -> Result<CheckedOwnershipContract, NodeBindingError> {
    let mut values = fields.iter();
    if next(&mut values)? != "1" {
        return Err(NodeBindingError::Unsupported("ownership record schema"));
    }
    let mode_count = count(&mut values)?;
    let mut modes = Vec::new();
    for _ in 0..mode_count {
        let name = next(&mut values)?;
        let mode = ownership::from_str(name)
            .filter(|mode| mode.as_str() == name)
            .ok_or(NodeBindingError::Unsupported("ownership record mode"))?;
        let slot = next(&mut values)?;
        let parameter = if slot == "result" { None } else { Some(number(slot)?) };
        let size = count(&mut values)?;
        let children = (0..size)
            .map(|_| number(next(&mut values)?))
            .collect::<Result<Vec<_>, _>>()?;
        let underlying = read_type(&mut values)?;
        modes.push(AccessMode {
            mode,
            component: TypeComponent { parameter, children },
            underlying,
        });
    }
    let result_source = match next(&mut values)? {
        "none" => None,
        tag @ ("parameter" | "receiver") => {
            let slot = if tag == "receiver" {
                SourceSlot::Receiver
            } else {
                SourceSlot::Parameter(number(next(&mut values)?)?)
            };
            let size = count(&mut values)?;
            let fields = (0..size)
                .map(|_| positions.canonical(next(&mut values)?))
                .collect::<Result<Vec<_>, _>>()?;
            Some(ResultDependency {
                slot,
                fields,
                aggregate: false,
            })
        }
        _ => return Err(NodeBindingError::MissingNode),
    };
    if values.next().is_some() {
        return Err(NodeBindingError::MissingNode);
    }
    Ok(OwnershipContract {
        schema_version: 1,
        modes,
        result_source,
    })
}

/// Refuse oversized counts before allocating or looping over untrusted record fields.
fn count(values: &mut std::slice::Iter<'_, String>) -> Result<usize, NodeBindingError> {
    let count = number(next(values)?)?;
    if count > values.len() {
        return Err(NodeBindingError::MissingNode);
    }
    Ok(count)
}

/// Match the canonical decimal grammar, refusing aliases such as leading zeros.
fn number(value: &str) -> Result<usize, NodeBindingError> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|digit| digit.is_ascii_digit())
    {
        return Err(NodeBindingError::MissingNode);
    }
    value.parse().map_err(|_| NodeBindingError::MissingNode)
}

#[cfg(test)]
mod tests {
    use super::super::{NodeBindingError, fixture_projection, verify_fixture};

    /// Body ownership facts survive record storage and source movement instead of disappearing at the protocol
    /// boundary.
    #[test]
    fn explicit_input_body_records_rebind_all_authority() -> Result<(), Box<dyn std::error::Error>> {
        let cases = [
            (
                "def read(value: Borrowed[int]) -> int:\n    return value\ndef use() -> int:\n    value = 3\n    return read(value)\n",
                "use",
            ),
            (
                "def replace(mut value: BorrowedMut[int]) -> int:\n    mut alias: BorrowedMut[int] = value\n    alias = 7\n    return value\n",
                "replace",
            ),
            (
                "def consume(value: Owned[str]) -> bool:\n    return value == \"yes\"\ndef use() -> int:\n    value = \"yes\"\n    consume(value)\n    return len(value)\n",
                "use",
            ),
            (
                "def fresh() -> int:\n    value: Owned[int] = 3\n    return value\n",
                "fresh",
            ),
            (
                "def copy(value: Owned[int]) -> int:\n    first: Owned[int] = value\n    second: Owned[int] = first\n    return second\n",
                "copy",
            ),
            (
                "def transfer(value: Owned[str]) -> int:\n    retained: Owned[str] = value\n    return len(retained)\n",
                "transfer",
            ),
        ];
        for (source, owner) in cases {
            let fields =
                fixture_projection(source, owner, false).map_err(|error| format!("owner {owner}: {error:?}"))?;
            assert!(
                fields
                    .iter()
                    .any(|row| row.first().is_some_and(|family| family == "input-flow"))
            );
            let moved = format!("# source positions moved\n{source}");
            assert_eq!(fields, fixture_projection(&moved, owner, false)?);
            verify_fixture(&moved, owner, fields.clone())?;
            let missing = fields.into_iter().filter(|row| row[0] != "input-flow").collect();
            assert!(verify_fixture(&moved, owner, missing).is_err());
        }
        Ok(())
    }

    /// Reordered permission maps cannot acquire a second canonical byte representation of the same decoded graph.
    #[test]
    fn input_graph_map_order_is_canonical() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def together(left: Borrowed[int], right: Borrowed[int]) -> bool:\n    return left == right\n";
        let mut fields = fixture_projection(source, "together", false)?;
        verify_fixture(source, "together", fields.clone())?;
        let row = fields
            .iter_mut()
            .find(|row| row[0] == "input-flow")
            .ok_or("missing input flow")?;
        assert_eq!(row[7], "2");
        assert_eq!(row[8], "binding");
        assert_eq!(row[13], "binding");
        row[8..18].rotate_left(5);
        assert!(
            matches!(verify_fixture(source, "together", fields), Err(NodeBindingError::InvalidField(message)) if message == "noncanonical ownership map order")
        );
        Ok(())
    }

    /// Declaration contracts and refused body projections retain their distinct complete record payloads.
    #[test]
    fn checked_ownership_records_round_trip() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def view(value: Borrowed[str]) -> Borrowed[str] from value:\n    return value\n";
        let moved = format!("# moved source\n{source}");
        let fields = fixture_projection(source, "view", true)?;
        assert!(
            fields
                .iter()
                .any(|row| row.first().is_some_and(|family| family == "ownership"))
        );
        assert_eq!(fields, fixture_projection(&moved, "view", true)?);
        verify_fixture(&moved, "view", fields.clone())?;
        let missing: Vec<_> = fields.iter().filter(|row| row[0] != "ownership").cloned().collect();
        assert!(verify_fixture(source, "view", missing).is_err());
        let mut obsolete = fields;
        obsolete[0][4] = "incan.checker-free-functions/3".into();
        assert!(verify_fixture(source, "view", obsolete).is_err());
        let body = fixture_projection(source, "view", false)?;
        assert_eq!(body, fixture_projection(&moved, "view", false)?);
        verify_fixture(&moved, "view", body)?;
        Ok(())
    }
}

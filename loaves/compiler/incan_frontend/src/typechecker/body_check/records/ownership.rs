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
            Some(ResultDependency { slot, fields })
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
    use super::super::{fixture_projection, verify_fixture};

    /// A source contract survives canonical primitive round trips, source movement and name-independent slot encoding.
    #[test]
    fn checked_ownership_records_round_trip() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def view(value: Borrowed[str]) -> Borrowed[str] from value:\n    return value\n";
        let moved = format!("# moved source\n{source}");
        for declaration in [true, false] {
            let fields = fixture_projection(source, "view", declaration)?;
            assert!(
                fields
                    .iter()
                    .any(|row| row.first().is_some_and(|family| family == "ownership"))
            );
            assert_eq!(fields, fixture_projection(&moved, "view", declaration)?);
            verify_fixture(&moved, "view", fields.clone())?;
            let missing: Vec<_> = fields.iter().filter(|row| row[0] != "ownership").cloned().collect();
            assert!(verify_fixture(source, "view", missing).is_err());
            let mut obsolete = fields;
            obsolete[0][4] = "incan.checker-free-functions/1".into();
            assert!(verify_fixture(source, "view", obsolete).is_err());
        }
        Ok(())
    }
}

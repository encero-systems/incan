//! Primitive completed mutation facts; pending calls and unrepresented forwarding remain explicit capability refusals.

use super::input_flow::Reader;
use super::{Fields, NodeBindingError, Positions, span_of};
use crate::typechecker::ownership_contracts::mutation::{MutationLocal, RecordedMutation};
use std::collections::BTreeSet;

/// Keep deterministic owner membership, changed-parameter inventories and reassigned local declarations.
pub(super) fn project(
    positions: &Positions,
    local: &MutationLocal,
    fields: &mut Fields,
) -> Result<(), NodeBindingError> {
    let mutation = local
        .record_projection()
        .ok_or(NodeBindingError::Unsupported("mutation facts"))?;
    for owner in mutation.bodies {
        fields.push(vec![
            "effect".into(),
            String::new(),
            positions.identity(&owner)?,
            "checked".into(),
        ]);
    }
    for (owner, parameters) in mutation.changed {
        let mut row = vec![
            "mutation".into(),
            positions.nodes.anchor(span_of(&owner))?.path,
            "changed".into(),
            positions.identity(&owner)?,
            parameters.len().to_string(),
        ];
        row.extend(parameters);
        fields.push(row);
    }
    for span in mutation.reassigned {
        fields.push(vec![
            "mutation".into(),
            positions.nodes.anchor(span)?.path,
            "reassigned".into(),
            "true".into(),
        ]);
    }
    Ok(())
}

/// Rebuild one supported mutation row without asking the current checker for any effect payload.
pub(super) fn restore(
    positions: &Positions,
    value: &mut RecordedMutation,
    row: &[String],
) -> Result<(), NodeBindingError> {
    let span = positions.span(&row[1])?;
    let mut reader = Reader {
        positions,
        values: row[3..].iter(),
    };
    match row[2].as_str() {
        "changed" => {
            let owner = reader.identity()?;
            if span_of(&owner) != span {
                return Err(NodeBindingError::WrongOwner);
            }
            let mut parameters = BTreeSet::new();
            let mut previous = String::new();
            for _ in 0..reader.count()? {
                let name = reader.token()?.to_owned();
                if name <= previous {
                    return Err(NodeBindingError::MissingNode);
                }
                previous = name.clone();
                parameters.insert(name);
            }
            if value.changed.insert(owner, parameters).is_some() {
                return Err(NodeBindingError::MissingNode);
            }
        }
        "reassigned" => {
            if !reader.boolean()? || !value.reassigned.insert(span) {
                return Err(NodeBindingError::MissingNode);
            }
        }
        _ => return Err(NodeBindingError::MissingNode),
    }
    reader.finish()
}

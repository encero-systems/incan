//! Shared compiler-graph logical cells for declaration and executable records.

use super::NamespaceCell as Cell;
use internal_models::{NamespaceAtom as Atom, NamespaceEntry, NamespaceField};

/// Convert the existing identity-addressed fields while moving their logical values.
fn atom_fields(fields: Vec<(u64, Cell)>) -> Vec<NamespaceField> {
    fields
        .into_iter()
        .map(|(id, value)| NamespaceField {
            identity: id,
            value: to_atom(value),
        })
        .collect()
}
/// Restore identity-addressed fields from a checked wire value.
fn cell_fields(fields: Vec<NamespaceField>) -> Result<Vec<(u64, Cell)>, String> {
    fields
        .into_iter()
        .map(|field| Ok((field.identity, to_cell(field.value)?)))
        .collect()
}
/// Move native compiler cells into the compiler record ABI.
pub fn to_atom(value: Cell) -> Atom {
    match value {
        Cell::Text(s) => Atom::Text(s),
        Cell::Binary(v) => Atom::Binary(v),
        Cell::Boolean(v) => Atom::Flag(v),
        Cell::Signed(v) => Atom::Signed(v),
        Cell::Unsigned(v) => Atom::Unsigned(v),
        Cell::WideSigned(v) => Atom::WideSigned(v),
        Cell::WideUnsigned(v) => Atom::WideUnsigned(v),
        Cell::Float(v) => Atom::Real(v),
        Cell::None => Atom::Missing,
        Cell::Some(item) => Atom::Present(vec![to_atom(*item)]),
        Cell::List(items) => Atom::Items(items.into_iter().map(to_atom).collect()),
        Cell::Tuple(items) => Atom::TupleItems(items.into_iter().map(to_atom).collect()),
        Cell::Map(items) => Atom::Mapping(
            items
                .into_iter()
                .map(|(key, value)| NamespaceEntry {
                    key: to_atom(key),
                    value: to_atom(value),
                })
                .collect(),
        ),
        Cell::Record(fields) => Atom::Fields(atom_fields(fields)),
        Cell::Variant(id, fields) => Atom::Alternative(id, atom_fields(fields)),
    }
}
/// Restore native compiler cells from an admitted record ABI value.
pub fn to_cell(value: Atom) -> Result<Cell, String> {
    Ok(match value {
        Atom::Text(s) => Cell::Text(s),
        Atom::Binary(v) => Cell::Binary(v),
        Atom::Flag(v) => Cell::Boolean(v),
        Atom::Signed(v) => Cell::Signed(v),
        Atom::Unsigned(v) => Cell::Unsigned(v),
        Atom::WideSigned(v) => Cell::WideSigned(v),
        Atom::WideUnsigned(v) => Cell::WideUnsigned(v),
        Atom::Real(v) => Cell::Float(v),
        Atom::Missing => Cell::None,
        Atom::Present(items) => {
            let [item] = items
                .try_into()
                .map_err(|_| "checked namespace option requires one value")?;
            Cell::Some(Box::new(to_cell(item)?))
        }
        Atom::Items(items) => Cell::List(items.into_iter().map(to_cell).collect::<Result<_, _>>()?),
        Atom::TupleItems(items) => Cell::Tuple(items.into_iter().map(to_cell).collect::<Result<_, _>>()?),
        Atom::Mapping(items) => Cell::Map(
            items
                .into_iter()
                .map(|entry| Ok((to_cell(entry.key)?, to_cell(entry.value)?)))
                .collect::<Result<_, String>>()?,
        ),
        Atom::Fields(fields) => Cell::Record(cell_fields(fields)?),
        Atom::Alternative(id, fields) => Cell::Variant(id, cell_fields(fields)?),
    })
}

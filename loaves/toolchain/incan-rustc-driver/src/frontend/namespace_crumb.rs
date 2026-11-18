//! Bind frontend declaration values to the Incan Crumb codec at the existing compiler graph boundary.

use incan_frontend::typechecker::{NamespaceCell as Cell, NamespaceCrumb};
use internal_models::{NamespaceAtom as Atom, NamespaceEntry, NamespaceField};

pub(super) struct CheckedNamespaceCrumb;

pub(super) fn publish(
    interface: &incan_frontend::typechecker::CheckedNamespaceInterface,
) -> Result<internal_models::NamespacePublication, String> {
    internal_models::publish_namespace_interface(&to_atom(interface.wire_value()?))
}

impl NamespaceCrumb for CheckedNamespaceCrumb {
    fn write(&self, root: Cell) -> Result<Vec<u8>, String> {
        internal_models::encode_namespace_interface(&to_atom(root))
    }

    fn read(&self, encoded: &[u8]) -> Result<Cell, String> {
        if encoded.len() > 64 * 1024 * 1024 {
            return Err("checked namespace exceeds its byte limit".into());
        }
        to_cell(internal_models::decode_namespace_interface(&encoded.to_vec())?)
    }
}

fn atom_fields(fields: Vec<(u64, Cell)>) -> Vec<NamespaceField> {
    fields
        .into_iter()
        .map(|(id, value)| NamespaceField {
            identity: id,
            value: to_atom(value),
        })
        .collect()
}
fn cell_fields(fields: Vec<NamespaceField>) -> Result<Vec<(u64, Cell)>, String> {
    fields
        .into_iter()
        .map(|field| Ok((field.identity, to_cell(field.value)?)))
        .collect()
}
pub(super) fn to_atom(value: Cell) -> Atom {
    match value {
        Cell::Text(s) => Atom::Text(s),
        Cell::Binary(v) => Atom::Binary(v),
        Cell::Boolean(v) => Atom::Flag(v),
        Cell::Signed(v) => Atom::Signed(v),
        Cell::Unsigned(v) => Atom::Unsigned(v),
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
pub(super) fn to_cell(value: Atom) -> Result<Cell, String> {
    Ok(match value {
        Atom::Text(s) => Cell::Text(s),
        Atom::Binary(v) => Cell::Binary(v),
        Atom::Flag(v) => Cell::Boolean(v),
        Atom::Signed(v) => Cell::Signed(v),
        Atom::Unsigned(v) => Cell::Unsigned(v),
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

#[cfg(test)]
mod tests {
    use super::CheckedNamespaceCrumb;
    use incan_frontend::typechecker::{CheckedNamespaceInterface, TypeChecker};

    fn program(source: &str) -> Result<incan_frontend::ast::Program, String> {
        let tokens = incan_frontend::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        incan_frontend::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))
    }

    #[test]
    fn documentation_changes_the_hover_product_and_preserves_checked_meaning() -> Result<(), String> {
        fn publication(
            doc: &str,
            ty: &str,
            literal: &str,
        ) -> Result<(internal_models::NamespacePublication, Vec<String>), String> {
            let provider = program(&format!(
                "pub model Page:\n    \"\"\"{doc}\"\"\"\n    pub value: {ty}\n\n    pub def read(self) -> {ty}:\n        return self.value\n\npub def sample() -> {ty}:\n    return {literal}\n"
            ))?;
            let path = vec!["domain".into()];
            let mut checker = TypeChecker::new();
            checker.set_current_module_path(Some(path.clone()));
            checker
                .check_program(&provider)
                .map_err(|errors| format!("{errors:?}"))?;
            let value = CheckedNamespaceInterface::from_checked("domain", &path, &provider, &checker)?
                .ok_or("missing checked namespace")?;
            let owners = value.lowering_member_owners().into_values().collect();
            Ok((super::publish(&value)?, owners))
        }
        let (before, before_owners) = publication("Blue doc.", "int", "24")?;
        let (moved, moved_owners) = publication("A longer documentation string for hover.", "int", "24")?;
        let (changed, _) = publication("Blue doc.", "str", "\"24\"")?;
        assert_ne!(before.interface, moved.interface);
        assert_eq!(before.semantic_interface, moved.semantic_interface);
        assert_ne!(before.semantic_interface, changed.semantic_interface);
        assert_eq!(before.semantic_interface.len(), 32);
        assert_eq!(before_owners.len(), 1);
        assert_eq!(before_owners, moved_owners);
        let restored = CheckedNamespaceInterface::from_bytes(&moved.interface, &CheckedNamespaceCrumb)?;
        assert_eq!(restored.to_bytes(&CheckedNamespaceCrumb)?, moved.interface);
        Ok(())
    }

    /// Downstream uses the restored namespace with an empty provider AST. Only the value the real compiler
    /// published can supply the callable, its model, the field type and the initializer's ownership proof.
    #[test]
    fn checked_namespace_crumb_installs_for_an_ordinary_consumer() -> Result<(), String> {
        let provider = program(
            "pub model Order:\n    pub quantity: int\n    pub labels: list[str] = []\n\npub def quantity(order: Order) -> int:\n    return order.quantity\n",
        )?;
        let path = vec!["domain".to_string()];
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(path.clone()));
        checker
            .check_program(&provider)
            .map_err(|errors| format!("{errors:?}"))?;
        let original = CheckedNamespaceInterface::from_checked("domain", &path, &provider, &checker)?
            .ok_or("missing provider interface")?;
        let codec = CheckedNamespaceCrumb;
        let bytes = original.to_bytes(&codec)?;
        let restored = CheckedNamespaceInterface::from_bytes(&bytes, &codec)?;
        assert_eq!(restored.to_bytes(&codec)?, bytes);
        assert!(CheckedNamespaceInterface::from_bytes(&bytes[..bytes.len() - 1], &codec).is_err());
        assert!(CheckedNamespaceInterface::from_bytes(br#"{"schema":6,"module_name":"domain"}"#, &codec).is_err());
        let mut consumer = TypeChecker::new();
        consumer.set_current_module_path(Some(vec!["app".into()]));
        consumer.register_dependency_module_path_segments("domain", path);
        consumer.select_dependency_interfaces(vec![restored.installable()?]);
        let source = program(
            "from domain import Order, quantity\n\npub def total(seed: Borrowed[int]) -> int:\n    order = Order(quantity=seed)\n    return quantity(order)\n",
        )?;
        consumer
            .check_with_imports(&source, &[("domain", &program("")?)])
            .map_err(|errors| format!("{errors:?}"))
    }
}

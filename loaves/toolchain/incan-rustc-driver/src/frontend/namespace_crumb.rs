//! Bind frontend declaration values to the Incan Crumb codec at the existing compiler graph boundary.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use incan_frontend::typechecker::{CheckedNamespaceInterface, NamespaceCell as Cell, NamespaceCrumb};
use internal_models::{NamespaceAtom as Atom, NamespaceEntry, NamespaceField};
use sha2::{Digest, Sha256};

pub(super) struct CheckedNamespaceCrumb;

pub(super) fn publish(
    interface: &incan_frontend::typechecker::CheckedNamespaceInterface,
) -> Result<internal_models::NamespacePublication, String> {
    internal_models::publish_namespace_interface(&to_atom(interface.wire_value()?))
}

/// Resident builds can consume the same physical namespace the compiler just published. Its full Crumb
/// digest includes documentation and positions as well as the semantic projection.
#[derive(Default)]
struct PublishedInterfaces {
    entries: HashMap<[u8; 32], Arc<CheckedNamespaceInterface>>,
    wire_bytes: usize,
}

const MAX_WIRE_BYTES: usize = 96 * 1024 * 1024;
const MAX_INTERFACES: usize = 512;
static PUBLISHED: LazyLock<Mutex<PublishedInterfaces>> = LazyLock::new(Mutex::default);

fn insert(digest: [u8; 32], size: usize, interface: Arc<CheckedNamespaceInterface>) {
    if size > MAX_WIRE_BYTES {
        return;
    }
    let Ok(mut cache) = PUBLISHED.lock() else {
        return;
    };
    if cache.entries.contains_key(&digest) {
        return;
    }
    if cache.entries.len() >= MAX_INTERFACES || cache.wire_bytes + size > MAX_WIRE_BYTES {
        cache.entries.clear();
        cache.wire_bytes = 0;
    }
    cache.entries.insert(digest, interface);
    cache.wire_bytes += size;
}

/// The successful publication consumed this validated interface and produced these exact bytes.
pub(super) fn remember_published(bytes: &[u8], interface: CheckedNamespaceInterface) {
    insert(Sha256::digest(bytes).into(), bytes.len(), Arc::new(interface));
}

fn published(digest: &[u8; 32]) -> Option<Arc<CheckedNamespaceInterface>> {
    PUBLISHED.lock().ok()?.entries.get(digest).cloned()
}

#[derive(Debug)]
struct EncodedInterface {
    name: String,
    path: Vec<String>,
    bytes: Vec<u8>,
    digest: [u8; 32],
}

impl incan_frontend::checker_records::GraphNamespaceInterface for EncodedInterface {
    fn belongs_to(&self, name: &str, path: &[String]) -> bool {
        self.name == name && self.path == path
    }

    fn installable(&self) -> Result<Arc<incan_frontend::typechecker::ModuleDependencyInterface>, String> {
        let checked = restore_digest(&self.bytes, self.digest)?;
        if checked.module_name() != self.name || checked.module_path() != self.path {
            return Err("checked namespace identity disagrees with the admitted root".into());
        }
        checked.installable()
    }
}

/// The full Crumb envelope is admitted even for namespaces that nobody imports. Read its identity fields
/// without building Rust declarations or lookup tables until an editable consumer actually needs them.
pub(super) fn admit(
    bytes: Vec<u8>,
) -> Result<Arc<dyn incan_frontend::checker_records::GraphNamespaceInterface>, String> {
    let digest = Sha256::digest(&bytes).into();
    if let Some(hit) = published(&digest) {
        return Ok(hit);
    }
    let root = internal_models::namespace_identity(&bytes, CheckedNamespaceInterface::SCHEMA)?;
    Ok(Arc::new(EncodedInterface {
        name: root.name,
        path: root.path,
        bytes,
        digest,
    }))
}

fn restore_digest(bytes: &[u8], digest: [u8; 32]) -> Result<Arc<CheckedNamespaceInterface>, String> {
    if let Some(hit) = published(&digest) {
        return Ok(hit);
    }
    let interface = Arc::new(CheckedNamespaceInterface::from_bytes(bytes, &CheckedNamespaceCrumb)?);
    insert(digest, bytes.len(), Arc::clone(&interface));
    Ok(interface)
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
fn restore(bytes: &[u8]) -> Result<Arc<CheckedNamespaceInterface>, String> {
    restore_digest(bytes, Sha256::digest(bytes).into())
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
            let publication = super::publish(&value)?;
            super::remember_published(&publication.interface, value);
            Ok((publication, owners))
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
        let original = super::restore(&before.interface)?;
        let moved_value = super::restore(&moved.interface)?;
        assert!(!std::sync::Arc::ptr_eq(&original, &moved_value));
        assert_eq!(original.to_bytes(&CheckedNamespaceCrumb)?, before.interface);
        assert_eq!(moved_value.to_bytes(&CheckedNamespaceCrumb)?, moved.interface);
        assert!(std::sync::Arc::ptr_eq(&moved_value, &super::restore(&moved.interface)?));
        assert!(super::restore(&moved.interface[..moved.interface.len() - 1]).is_err());
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
        let selected = super::admit(bytes.clone())?;
        assert!(selected.belongs_to("domain", &path));
        assert!(!selected.belongs_to("domain", &["other".into()]));
        assert!(!selected.belongs_to("other", &path));
        assert!(super::admit(bytes[..bytes.len() - 1].to_vec()).is_err());
        let restored = CheckedNamespaceInterface::from_bytes(&bytes, &codec)?;
        assert_eq!(restored.to_bytes(&codec)?, bytes);
        assert!(CheckedNamespaceInterface::from_bytes(&bytes[..bytes.len() - 1], &codec).is_err());
        assert!(CheckedNamespaceInterface::from_bytes(br#"{"schema":6,"module_name":"domain"}"#, &codec).is_err());
        let mut consumer = TypeChecker::new();
        consumer.set_current_module_path(Some(vec!["app".into()]));
        consumer.register_dependency_module_path_segments("domain", path);
        consumer.select_dependency_interfaces(vec![selected.installable()?]);
        let source = program(
            "from domain import Order, quantity\n\npub def total(seed: Borrowed[int]) -> int:\n    order = Order(quantity=seed)\n    return quantity(order)\n",
        )?;
        consumer
            .check_with_imports(&source, &[("domain", &program("")?)])
            .map_err(|errors| format!("{errors:?}"))
    }
}

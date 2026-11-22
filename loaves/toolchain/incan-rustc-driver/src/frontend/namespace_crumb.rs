//! Bind frontend declaration values to the Incan Crumb codec at the existing compiler graph boundary.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

use incan_frontend::checker_records::GraphNamespaceInterface;
use incan_frontend::typechecker::{
    CheckedNamespaceInterface, ModuleDependencyInterface, NamespaceCell as Cell, NamespaceCrumb,
};
use internal_models_native::{MappedView, NamespaceAtom as Atom, NamespaceEntry, NamespaceField};
use sha2::{Digest, Sha256};

pub(super) struct CheckedNamespaceCrumb;

/// The schema is compiler-owned and immutable for this binary. Each record read still has a fresh codec budget.
static NAMESPACE_READER: LazyLock<Result<internal_models::PreparedNamespaceReader, String>> =
    LazyLock::new(internal_models::prepare_namespace_reader);

fn namespace_reader() -> Result<&'static internal_models::PreparedNamespaceReader, String> {
    NAMESPACE_READER.as_ref().map_err(Clone::clone)
}

pub(super) fn publish(
    interface: &incan_frontend::typechecker::CheckedNamespaceInterface,
) -> Result<internal_models::NamespacePublication, String> {
    internal_models::publish_namespace_interface(&to_atom(interface.wire_value()?))
}

/// One resident declaration's immutable checked product and its installed dependency lookup tables.
#[derive(Debug)]
struct RetainedInterface {
    checked: CheckedNamespaceInterface,
    installed: OnceLock<Result<Arc<ModuleDependencyInterface>, String>>,
}

impl RetainedInterface {
    /// Keep the wire-level declaration and prepare the lookup tables only when a consumer needs them.
    fn new(checked: CheckedNamespaceInterface) -> Self {
        Self {
            checked,
            installed: OnceLock::new(),
        }
    }
}

impl GraphNamespaceInterface for RetainedInterface {
    /// Recognize the logical module before letting a checker observe its declarations.
    fn belongs_to(&self, name: &str, path: &[String]) -> bool {
        self.checked.belongs_to(name, path)
    }

    /// Share the same completed immutable tables for each consumer of the exact same checked namespace.
    fn installable(&self) -> Result<Arc<ModuleDependencyInterface>, String> {
        self.installed
            .get_or_init(|| {
                let _trace =
                    tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::prepare_retained_interface",
                module = self.checked.module_name())
                    .entered();
                self.checked.installable()
            })
            .clone()
    }
}

/// Resident builds can consume the same physical namespace the compiler just published. Its full Crumb
/// digest includes documentation and positions as well as the semantic projection.
#[derive(Default)]
struct PublishedInterfaces {
    entries: HashMap<[u8; 32], Arc<RetainedInterface>>,
    wire_bytes: usize,
}

const MAX_WIRE_BYTES: usize = 96 * 1024 * 1024;
const MAX_INTERFACES: usize = 512;
static PUBLISHED: LazyLock<Mutex<PublishedInterfaces>> = LazyLock::new(Mutex::default);

fn insert(digest: [u8; 32], size: usize, interface: Arc<RetainedInterface>) {
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
pub(super) fn remember_published(
    source: &str,
    bytes: &[u8],
    interface: CheckedNamespaceInterface,
    request_proof: bool,
) -> Result<(Vec<u8>, Arc<dyn GraphNamespaceInterface>), String> {
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    let proof = if request_proof {
        internal_models::encode_namespace_proof(
            &source.to_owned(),
            CheckedNamespaceInterface::SCHEMA,
            interface.module_name(),
            interface.module_path(),
            &digest.to_vec(),
        )?
    } else {
        Vec::new()
    };
    let owned = Arc::new(RetainedInterface::new(interface));
    insert(digest, bytes.len(), Arc::clone(&owned));
    Ok((proof, owned))
}

/// The owned decoded interface of a namespace already admitted in this process.
pub(super) fn resident_interface(digest: &[u8; 32]) -> Option<Arc<dyn GraphNamespaceInterface>> {
    published(digest).map(|interface| interface as Arc<dyn GraphNamespaceInterface>)
}

fn published(digest: &[u8; 32]) -> Option<Arc<RetainedInterface>> {
    PUBLISHED.lock().ok()?.entries.get(digest).cloned()
}

#[derive(Debug)]
struct EncodedInterface {
    name: String,
    path: Vec<String>,
    bytes: MappedView,
    digest: [u8; 32],
}

impl GraphNamespaceInterface for EncodedInterface {
    fn belongs_to(&self, name: &str, path: &[String]) -> bool {
        self.name == name && self.path == path
    }

    fn installable(&self) -> Result<Arc<incan_frontend::typechecker::ModuleDependencyInterface>, String> {
        let byte_count = self.bytes.0.as_bytes().len();
        let checked = restore_digest(byte_count, self.digest, || {
            if byte_count > 64 * 1024 * 1024 {
                return Err("checked namespace exceeds its byte limit".into());
            }
            let root = internal_models::decode_prepared_namespace_view(namespace_reader()?, &self.bytes)?;
            CheckedNamespaceInterface::from_wire_value(to_cell(root)?)
        })?;
        if !checked.belongs_to(&self.name, &self.path) {
            return Err("checked namespace identity disagrees with the admitted root".into());
        }
        checked.installable()
    }
}

/// Graph selection supplies the compiler's physical identity proof. Older rows recover the same
/// identity from the full envelope. Installation independently verifies the declaration's own root.
pub(super) fn admit(
    source: &str,
    proof: &[u8],
    bytes: MappedView,
) -> Result<(Arc<dyn GraphNamespaceInterface>, [u8; 32]), String> {
    let digest = Sha256::digest(bytes.0.as_bytes()).into();
    let root = if proof.is_empty() {
        if let Some(hit) = published(&digest) {
            return Ok((hit, digest));
        }
        internal_models::identify_prepared_namespace_view(
            namespace_reader()?,
            &bytes,
            CheckedNamespaceInterface::SCHEMA,
        )?
    } else {
        internal_models::read_namespace_proof(
            &proof.to_vec(),
            &source.to_owned(),
            CheckedNamespaceInterface::SCHEMA,
            &digest.to_vec(),
        )?
    };
    if let Some(hit) = published(&digest) {
        if !hit.belongs_to(&root.name, &root.path) {
            return Err("retained namespace identity disagrees with the admitted root".into());
        }
        return Ok((hit, digest));
    }
    Ok((
        Arc::new(EncodedInterface {
            name: root.name,
            path: root.path,
            bytes,
            digest,
        }),
        digest,
    ))
}

fn restore_digest(
    size: usize,
    digest: [u8; 32],
    decode: impl FnOnce() -> Result<CheckedNamespaceInterface, String>,
) -> Result<Arc<RetainedInterface>, String> {
    if let Some(hit) = published(&digest) {
        return Ok(hit);
    }
    let interface = Arc::new(RetainedInterface::new(decode()?));
    insert(digest, size, Arc::clone(&interface));
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
        to_cell(internal_models::decode_prepared_namespace(
            namespace_reader()?,
            &encoded.to_vec(),
        )?)
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
fn restore(bytes: &[u8]) -> Result<Arc<RetainedInterface>, String> {
    restore_digest(bytes.len(), Sha256::digest(bytes).into(), || {
        CheckedNamespaceInterface::from_bytes(bytes, &CheckedNamespaceCrumb)
    })
}

#[cfg(test)]
mod tests {
    use super::{CheckedNamespaceCrumb, GraphNamespaceInterface};
    use incan_frontend::typechecker::{CheckedNamespaceInterface, TypeChecker};

    fn program(source: &str) -> Result<incan_frontend::ast::Program, String> {
        let tokens = incan_frontend::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        incan_frontend::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))
    }

    /// Carry interface bytes through a graph namespace record, as a selected page does, and return the anchored view
    /// that admission reads in place.
    fn view(bytes: &[u8]) -> Result<super::MappedView, String> {
        let value = internal_models::GraphNamespaceValue {
            module: "domain".into(),
            interface: bytes.to_vec(),
            identity_proof: Vec::new(),
            body_ir: vec![0],
            lowering_inputs: Vec::new(),
            package_roots: Vec::new(),
            call_reads: Vec::new(),
            call_reads_known: true,
        };
        let encoded = internal_models::encode_graph_namespace_value(&value).map_err(|error| format!("{error:?}"))?;
        internal_models::decode_owned_namespace_view(&encoded)
            .map(|record| record.interface)
            .map_err(|error| format!("{error:?}"))
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
            super::remember_published("domain.incn", &publication.interface, value, true)?;
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
        assert_eq!(original.checked.to_bytes(&CheckedNamespaceCrumb)?, before.interface);
        assert_eq!(moved_value.checked.to_bytes(&CheckedNamespaceCrumb)?, moved.interface);
        assert!(std::sync::Arc::ptr_eq(&moved_value, &super::restore(&moved.interface)?));
        let installed = moved_value.installable()?;
        let second = super::restore(&moved.interface)?.installable()?;
        assert!(std::sync::Arc::ptr_eq(&installed, &second));
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
        let (without_proof, _) = super::remember_published("domain.incn", &bytes, original.clone(), false)?;
        assert!(without_proof.is_empty());
        let (proof, _) = super::remember_published("domain.incn", &bytes, original, true)?;
        let truncated = &bytes[..bytes.len() - 1];
        let (selected, _) = super::admit("domain.incn", &proof, view(&bytes)?)?;
        assert!(super::admit("renamed.incn", &proof, view(&bytes)?).is_err());
        assert!(super::admit("domain.incn", &proof, view(truncated)?).is_err());
        assert!(super::admit("domain.incn", &proof[..proof.len() - 1], view(&bytes)?).is_err());
        let (unproved, _) = super::admit("domain.incn", &[], view(&bytes)?)?;
        assert!(unproved.belongs_to("domain", &path));
        assert!(selected.belongs_to("domain", &path));
        assert!(!selected.belongs_to("domain", &["other".into()]));
        assert!(!selected.belongs_to("other", &path));
        assert!(super::admit("domain.incn", &[], view(truncated)?).is_err());
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

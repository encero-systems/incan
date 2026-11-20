//! The native frontend's typed checked namespace projection, framed by the public Crumb row codec.

use crate::plan_wire_rows::PreparedWireReader;

/// One identity-addressed field of a namespace record or alternative.
#[derive(Clone, PartialEq)]
pub struct NamespaceField {
    pub identity: u64,
    pub value: NamespaceAtom,
}
crate::incan_debug::incan_debug!(NamespaceField { identity, value });

/// One key and value of a namespace mapping.
#[derive(Clone, PartialEq)]
pub struct NamespaceEntry {
    pub key: NamespaceAtom,
    pub value: NamespaceAtom,
}
crate::incan_debug::incan_debug!(NamespaceEntry { key, value });

/// Logical ABI between the Rust-owned checker declarations and their storage contract.
#[derive(Clone, Debug, PartialEq)]
pub enum NamespaceAtom {
    Text(String),
    Binary(Vec<u8>),
    Flag(bool),
    Signed(i64),
    Unsigned(u64),
    Real(f64),
    Missing,
    Present(Vec<NamespaceAtom>),
    Items(Vec<NamespaceAtom>),
    TupleItems(Vec<NamespaceAtom>),
    Mapping(Vec<NamespaceEntry>),
    Fields(Vec<NamespaceField>),
    Alternative(u64, Vec<NamespaceField>),
}

/// The immutable encoded compiler descriptions shared by namespace reads in one driver process.
#[derive(Clone)]
pub struct PreparedNamespaceReader {
    pub(crate) reader: PreparedWireReader,
}
crate::incan_debug::incan_debug!(PreparedNamespaceReader { reader });

/// The module name and path a checked namespace advertises.
#[derive(Clone, PartialEq, Eq)]
pub struct NamespaceIdentity {
    pub name: String,
    pub path: Vec<String>,
}
crate::incan_debug::incan_debug!(NamespaceIdentity { name, path });

/// The full installable namespace and its RFC 124 canonical logical-value digest.
#[derive(Clone, PartialEq, Eq)]
pub struct NamespacePublication {
    pub interface: Vec<u8>,
    pub semantic_interface: Vec<u8>,
}
crate::incan_debug::incan_debug!(NamespacePublication {
    interface,
    semantic_interface
});

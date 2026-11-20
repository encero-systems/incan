//! References to evaluated namespaces in immutable bakery segments.

use crate::record_scope::RecordKey;

pub const NAMESPACE_IDENTITY_PROFILE: &str = "incan.editable-namespace-crumb/7";

/// An evaluated namespace in an immutable segment under the request's bakery anchor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamespaceReference {
    pub key: RecordKey,
    pub segment: String,
    pub offset: i64,
    pub length: i64,
    pub frame_digest: Vec<u8>,
    pub value_digest: String,
}

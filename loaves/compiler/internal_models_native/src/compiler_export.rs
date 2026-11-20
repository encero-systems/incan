//! The candidate compiler's private installation index preserves the ordered logical membership of a sealed record
//! set.

/// One key and the lowercase SHA-256 revision naming its exact exported object.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CompilerExportMember {
    pub key: String,
    pub revision: String,
}

//! A Rust type and function that one Incan library re-exports and a second re-exports again.

/// A record key.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordKey {
    /// The key's value.
    pub value: i64,
}

/// Read a key's value.
pub fn read_key(key: &RecordKey) -> i64 {
    key.value
}

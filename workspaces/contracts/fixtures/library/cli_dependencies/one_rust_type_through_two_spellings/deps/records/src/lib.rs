//! A key defined in a public module and re-exported at the crate root, so it has two public spellings.

/// The module that defines the key.
pub mod scope {
    /// A record key.
    #[derive(Clone, Debug, PartialEq)]
    pub struct RecordKey {
        /// The key's value.
        pub value: i64,
    }

    /// Make a key; its signature names the key by its module path.
    pub fn make(value: i64) -> RecordKey {
        RecordKey { value }
    }

    /// Read a key named by its module path.
    pub fn read(key: &RecordKey) -> i64 {
        key.value
    }
}

pub use scope::RecordKey;

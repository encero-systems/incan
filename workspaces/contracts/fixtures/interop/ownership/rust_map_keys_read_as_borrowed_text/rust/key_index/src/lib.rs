//! A map whose keys an Incan caller reads through `HashMap::keys`, which yields `&String`.

use std::collections::HashMap;

/// Counts by name.
pub struct Index {
    /// The counts, keyed by name.
    pub groups: HashMap<String, i64>,
}

impl Index {
    /// Three named counts.
    pub fn sample() -> Index {
        Index {
            groups: [("alpha", 1), ("beta", 2), ("gamma", 3)]
                .into_iter()
                .map(|(name, count)| (name.to_owned(), count))
                .collect(),
        }
    }
}

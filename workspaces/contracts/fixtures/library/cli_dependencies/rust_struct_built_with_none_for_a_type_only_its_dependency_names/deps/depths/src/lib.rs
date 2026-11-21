//! A depth that consumers reach only through another crate's re-export of this crate.

/// A measured depth.
pub struct Depth {
    /// The depth.
    pub value: i64,
}

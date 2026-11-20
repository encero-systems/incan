//! Counts observed by one graph-backed check, returned to Oven separately from public diagnostics.

/// Top-level bodies checked afresh or restored, and the pages physically decoded during this check. Every count
/// defaults to zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CheckedBodyCounts {
    pub checked: i64,
    pub reused: i64,
    pub decoded: i64,
}

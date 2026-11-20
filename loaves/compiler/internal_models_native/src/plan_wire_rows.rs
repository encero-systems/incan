//! The graph's native plan profile uses compiler-owned Crumb descriptions for its stored functions and layouts.

/// Retain an exact reference dictionary for repeated field reads of one compiler-owned root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedWireReader {
    pub(crate) root: Vec<u8>,
    pub(crate) dictionary: Vec<Vec<u8>>,
    pub(crate) name: String,
}

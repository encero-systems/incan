//! Fallible reads of an optional byte buffer, the shape of a resolved snapshot's value read.

/// Why a read failed.
#[derive(Clone, Debug, PartialEq)]
pub struct Fault {
    /// What went wrong.
    pub detail: String,
}

/// The bytes `value` when `found`, none otherwise; a record fault when the read fails.
pub fn read(found: bool) -> Result<Option<Vec<u8>>, Fault> {
    Ok(found.then(|| b"value".to_vec()))
}

/// The bytes `value` when `found`, none otherwise; a text fault when the read fails.
pub fn read_text(found: bool) -> Result<Option<Vec<u8>>, String> {
    Ok(found.then(|| b"value".to_vec()))
}

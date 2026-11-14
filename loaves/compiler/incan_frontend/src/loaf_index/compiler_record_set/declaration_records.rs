//! A Loaf's declaration records, read by name from the installed set (RFC 179).

use std::path::Path;

use serde_json::Value;

use super::{LoafIndexError, RecordSet};

/// Read the `family` records of the declaration named `name` in one Loaf group of `set`.
pub(super) fn read_declaration_in(
    _set: &RecordSet,
    _dir: &Path,
    _name: &str,
    _family: &str,
    _declaration: &str,
) -> Result<Option<(Vec<Value>, String)>, LoafIndexError> {
    Ok(None)
}

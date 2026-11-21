//! Checked JSON hook-result mapping preserving the selected error's Display form.

use std::fmt::Display;

/// Keep the success owner or the displayed error for serde's ordinary custom-error adapter.
pub fn checked_json_result<T, E: Display>(result: Result<T, E>) -> Result<T, String> {
    result.map_err(|error| error.to_string())
}

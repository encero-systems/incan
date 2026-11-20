//! Debug rendering and payload copies for concrete standard carriers.

use std::fmt::Debug;

/// Render the borrowed carrier with its Debug formatter.
pub fn format_carrier<T: Debug>(value: &T) -> String {
    format!("{value:?}")
}

/// Clone a borrowed active payload with its native Clone implementation.
pub fn clone_carrier_payload<T: Clone>(value: &T) -> T {
    value.clone()
}

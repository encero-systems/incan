//! `assert_raises` over a deferred block: match the panic-hosted error category and keep the exact failures.

use incan_std_primitives::strings::str_starts_with;
use incan_std_testing::{fail_t, panic_text};

/// Pass when `block` raises `expected` (the bare name or `expected: ...`); otherwise fail with the custom message,
/// or with the default naming what was raised instead.
pub fn assert_raises_deferred(block: fn(), expected: String, message: String) {
    match panic_text(block) {
        None => {
            if !message.is_empty() {
                return fail_t(format!("AssertionError: {message}"));
            }
            fail_t(format!("AssertionError: expected {expected} to be raised"))
        }
        Some(actual) => {
            let prefix = format!("{expected}:");
            if actual == expected || str_starts_with(&actual, &prefix) {
                return;
            }
            if !message.is_empty() {
                return fail_t(format!("AssertionError: {message}"));
            }
            fail_t(format!(
                "AssertionError: expected {expected} to be raised, got {actual}"
            ))
        }
    }
}

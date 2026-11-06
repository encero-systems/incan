//! Foundational runtime error helpers shared by primitive and higher-level standard packages.

use incan_lang::errors::{ErrorKind, IncanError};

/// Raise a `ValueError` with the canonical `ValueError: ...` prefix.
#[cold]
#[track_caller]
pub fn raise_value_error(message: &str) -> ! {
    panic!("{}", IncanError::with_message(ErrorKind::ValueError, message));
}

//! Canonical language error primitives for native plans with no normal continuation.

/// Raise the core ValueError with the original message; this never returns normally.
pub fn raise_value_error(message: String) {
    incan_std_primitives::errors::raise_value_error(&message)
}

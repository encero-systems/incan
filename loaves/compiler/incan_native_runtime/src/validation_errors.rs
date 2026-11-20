//! Construct and borrow the dependency-owned validation error without replacing its runtime contract.

use incan_std_primitives::validation::ValidationError;

/// The message-only constructor.
pub fn make_validation_error(message: String) -> ValidationError {
    ValidationError::new(message)
}

/// The message and machine-readable code constructor.
pub fn make_coded_validation_error(message: String, code: String) -> ValidationError {
    ValidationError::with_code(message, code)
}

/// Borrow the runtime Display so later reads retain the error's owned payload.
pub fn format_validation_error(value: &ValidationError) -> String {
    value.to_string()
}

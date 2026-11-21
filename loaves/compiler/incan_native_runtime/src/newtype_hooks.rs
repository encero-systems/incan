//! Checked newtype boundaries: constraint failures, hook-result extraction, integer parsing and newtype sums.

use std::fmt::Display;

use incan_std_primitives::errors::raise;
use incan_std_primitives::validation::{ValidationErrorsBuilder, raise_constraint_error, raise_validation_error};

/// Raise the named constraint error after one saved input fails its checked predicates.
pub fn validate_newtype_constraint(valid: bool, target: String, constraint: String) {
    if !valid {
        raise_constraint_error(target, constraint);
    }
}

/// Return the checked wrapper, or raise the governed validation error naming the target and hook.
pub fn checked_newtype_result<T, E: Display>(result: Result<T, E>, target: String, hook: String) -> T {
    match result {
        Ok(value) => value,
        Err(error) => raise_validation_error(target, hook, error),
    }
}

/// Use the dependency-owned error formatter after the sole field hook of a one-field constructor.
pub fn checked_newtype_field_result<T, E: Display>(result: Result<T, E>, target: String, field: String) -> T {
    match result {
        Ok(value) => value,
        Err(error) => {
            let mut errors = ValidationErrorsBuilder::new(target);
            errors.push_field_error(field, error);
            errors.raise_if_any();
            raise("unreachable empty validation collector after a recorded field error")
        }
    }
}

/// Parse with the native i64 parser, preserving its displayed error without trimming.
pub fn parse_newtype_int(value: String) -> Result<i64, String> {
    value.parse::<i64>().map_err(|error| error.to_string())
}

/// The release summation bridge's wrapping primitive addition.
pub fn add_newtype_sum(left: i64, right: i64) -> i64 {
    left.wrapping_add(right)
}

/// Extract the final hook result, or keep both its construction context and displayed cause.
pub fn checked_sum_result<T, E: Display>(result: Result<T, E>, failure: String) -> T {
    match result {
        Ok(value) => value,
        Err(error) => raise(format!("{failure}: {error}")),
    }
}

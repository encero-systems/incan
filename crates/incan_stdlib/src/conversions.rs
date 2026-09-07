//! Conversion helpers for Incan-generated Rust code.
//!
//! These helpers exist to ensure Python-like conversion errors (e.g. `int("x")`) produce canonical,
//! typed exception messages rather than Rust's default `parse()` panic output.

use crate::errors::raise;
use incan_core::{
    errors::IncanError,
    numeric_strings::{parse_float_string, parse_int_string},
};

/// Parse an Incan/Python-style `int` from a string.
///
/// ## Panics
/// - `ValueError: cannot convert '{input}' to int` if parsing fails.
#[inline]
pub fn int_from_str<S: AsRef<str>>(input: S) -> i64 {
    let input = input.as_ref();
    parse_int_string(input).unwrap_or_else(|| raise(IncanError::cannot_convert_to_int(input)))
}

/// Parse an Incan/Python-style `float` from a string.
///
/// ## Panics
/// - `ValueError: cannot convert '{input}' to float` if parsing fails.
#[inline]
pub fn float_from_str<S: AsRef<str>>(input: S) -> f64 {
    let input = input.as_ref();
    parse_float_string(input).unwrap_or_else(|| raise(IncanError::cannot_convert_to_float(input)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_from_str_ok() {
        assert_eq!(int_from_str("42"), 42);
        assert_eq!(int_from_str("1_000"), 1_000);
    }

    #[test]
    #[should_panic(expected = "ValueError: cannot convert 'abc' to int")]
    fn int_from_str_err_panics_with_value_error() {
        let _ = int_from_str("abc");
    }

    #[test]
    fn float_from_str_ok() {
        assert_eq!(float_from_str("3.5"), 3.5);
        assert_eq!(float_from_str("1_000.50"), 1_000.5);
        assert_eq!(float_from_str("1.25e1_0"), 1.25e10);
    }

    #[test]
    #[should_panic(expected = "ValueError: cannot convert 'abc' to float")]
    fn float_from_str_err_panics_with_value_error() {
        let _ = float_from_str("abc");
    }

    #[test]
    #[should_panic(expected = "ValueError: cannot convert '1__000' to int")]
    fn int_from_str_rejects_invalid_separator_placement() {
        let _ = int_from_str("1__000");
    }

    #[test]
    #[should_panic(expected = "ValueError: cannot convert '1_000._50' to float")]
    fn float_from_str_rejects_invalid_separator_placement() {
        let _ = float_from_str("1_000._50");
    }
}

//! Shared, user-facing “Python-like” exceptions used across compiler and runtime.
//!
//! The semantic core (`incan_core`) must stay **pure/deterministic** and must not panic.
//! Instead, it provides a typed exception taxonomy (`ErrorKind`) and canonical formatting
//! (`IncanError` implements `Display`).
//!
//! The runtime/stdlib (`incan_stdlib`) may choose to `panic!` with these formatted errors.
//!
//! ## Goals
//! - Avoid “stringly-typed” exception identity (`"ValueError: ..."` scattered across the repo).
//! - Keep a single source of truth for exception *kind* and canonical formatting.
//! - Allow dynamic details without heap allocations (borrowed `&str` + primitive fields).

use core::fmt;

use crate::strings::StringAccessError;

/// Stable identifier for builtin exception kinds (Python-like).
///
/// ## Notes
/// - User-facing metadata (canonical spelling, description, examples) lives in the language registry:
///   `crate::lang::errors`.
/// - Keep this enum focused on identity; avoid duplicating docs/meaning here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    AssertionError,
    ValueError,
    TypeError,
    ZeroDivisionError,
    IndexError,
    KeyError,
    JsonDecodeError,
}

/// Arguments used to format an [`IncanError`].
///
/// All variants are allocation-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorArgs<'a> {
    /// A fully static message body (without the `Kind: ` prefix).
    Static(&'static str),
    /// A borrowed message body (without the `Kind: ` prefix).
    Message(&'a str),
    /// Dynamic index out of range details.
    IndexOutOfRange {
        index: i64,
        len: usize,
        /// e.g. `"list"`, `"string"`
        container: &'static str,
    },
    /// `ValueError: cannot convert '{input}' to int`
    CannotConvertToInt { input: &'a str },
    /// `ValueError: cannot convert '{input}' to float`
    CannotConvertToFloat { input: &'a str },
    /// `TypeError: Object of type {type_name} is not JSON serializable`
    ///
    /// Mirrors Python's `json.dumps(...)` error.
    JsonNotSerializable { type_name: &'a str },
}

/// A typed, canonical Incan error (Python-like).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IncanError<'a> {
    kind: ErrorKind,
    args: ErrorArgs<'a>,
}

impl<'a> IncanError<'a> {
    #[inline]
    pub const fn new(kind: ErrorKind, args: ErrorArgs<'a>) -> Self {
        Self { kind, args }
    }

    /// Return the exception kind.
    #[inline]
    pub const fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// `IndexError: string index out of range`
    #[inline]
    pub const fn string_index_out_of_range() -> Self {
        Self::new(ErrorKind::IndexError, ErrorArgs::Static("string index out of range"))
    }

    /// `IndexError: pop from empty list`
    ///
    /// Mirrors Python's `list.pop()` on an empty list.
    #[inline]
    pub const fn list_pop_empty() -> Self {
        Self::new(ErrorKind::IndexError, ErrorArgs::Static("pop from empty list"))
    }

    /// `ValueError: slice step cannot be zero`
    #[inline]
    pub const fn slice_step_zero() -> Self {
        Self::new(ErrorKind::ValueError, ErrorArgs::Static("slice step cannot be zero"))
    }

    /// `ZeroDivisionError: float division by zero`
    #[inline]
    pub const fn zero_division() -> Self {
        Self::new(
            ErrorKind::ZeroDivisionError,
            ErrorArgs::Static("float division by zero"),
        )
    }

    /// `ValueError: range() arg 3 must not be zero`
    ///
    /// Mirrors Python's `range(..., step)` error when `step == 0`.
    #[inline]
    pub const fn range_step_zero() -> Self {
        Self::new(
            ErrorKind::ValueError,
            ErrorArgs::Static("range() arg 3 must not be zero"),
        )
    }

    /// `ValueError: value not found in list`
    #[inline]
    pub const fn list_value_not_found() -> Self {
        Self::new(ErrorKind::ValueError, ErrorArgs::Static("value not found in list"))
    }

    /// `IndexError: index {index} out of range for {container} of length {len}`
    #[inline]
    pub const fn index_out_of_range_for(container: &'static str, index: i64, len: usize) -> Self {
        Self::new(
            ErrorKind::IndexError,
            ErrorArgs::IndexOutOfRange { index, len, container },
        )
    }

    /// `ValueError: cannot convert '{input}' to int`
    #[inline]
    pub const fn cannot_convert_to_int(input: &'a str) -> Self {
        Self::new(ErrorKind::ValueError, ErrorArgs::CannotConvertToInt { input })
    }

    /// `ValueError: cannot convert '{input}' to float`
    #[inline]
    pub const fn cannot_convert_to_float(input: &'a str) -> Self {
        Self::new(ErrorKind::ValueError, ErrorArgs::CannotConvertToFloat { input })
    }

    /// `TypeError: Object of type {type_name} is not JSON serializable`
    ///
    /// Mirrors Python's `json.dumps(...)` error.
    #[inline]
    pub const fn json_not_serializable(type_name: &'a str) -> Self {
        Self::new(ErrorKind::TypeError, ErrorArgs::JsonNotSerializable { type_name })
    }

    /// `JSONDecodeError: {message}`
    ///
    /// Mirrors Python's `json.loads(...)` (though we do not carry Python's rich decode error fields).
    #[inline]
    pub const fn json_decode_error(message: &'a str) -> Self {
        Self::new(ErrorKind::JsonDecodeError, ErrorArgs::Message(message))
    }

    /// Generic message helper (keeps kind typed, avoids allocating for the message body).
    #[inline]
    pub const fn with_message(kind: ErrorKind, message: &'a str) -> Self {
        Self::new(kind, ErrorArgs::Message(message))
    }
}

impl fmt::Display for IncanError<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = crate::lang::errors::as_str(self.kind);
        match self.args {
            ErrorArgs::Static(msg) | ErrorArgs::Message(msg) => write!(f, "{kind}: {msg}"),
            ErrorArgs::IndexOutOfRange { index, len, container } => {
                write!(f, "{kind}: index {index} out of range for {container} of length {len}")
            }
            ErrorArgs::CannotConvertToInt { input } => write!(f, "{kind}: cannot convert '{input}' to int"),
            ErrorArgs::CannotConvertToFloat { input } => write!(f, "{kind}: cannot convert '{input}' to float"),
            ErrorArgs::JsonNotSerializable { type_name } => {
                write!(f, "{kind}: Object of type {type_name} is not JSON serializable")
            }
        }
    }
}

// -------------------------------------------------------------------------------------------------
// Additional formatting helpers for dynamic values that are not easily stored in `ErrorArgs`.
// -------------------------------------------------------------------------------------------------

/// A display wrapper for `KeyError: '{key}' not found in dict`.
///
/// This is intentionally generic so callers can format keys without heap allocation.
#[derive(Debug, Clone, Copy)]
pub struct KeyNotFoundInDict<'a, K: fmt::Display + ?Sized> {
    key: &'a K,
}

impl<'a, K: fmt::Display + ?Sized> KeyNotFoundInDict<'a, K> {
    #[inline]
    pub const fn new(key: &'a K) -> Self {
        Self { key }
    }
}

impl<K: fmt::Display + ?Sized> fmt::Display for KeyNotFoundInDict<'_, K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: '{}' not found in dict",
            crate::lang::errors::as_str(ErrorKind::KeyError),
            self.key
        )
    }
}

/// Construct a `KeyError: '{key}' not found in dict` formatter.
#[inline]
pub const fn key_not_found_in_dict<'a, K: fmt::Display + ?Sized>(key: &'a K) -> KeyNotFoundInDict<'a, K> {
    KeyNotFoundInDict::new(key)
}

impl From<StringAccessError> for IncanError<'static> {
    fn from(err: StringAccessError) -> Self {
        match err {
            StringAccessError::IndexOutOfRange => IncanError::string_index_out_of_range(),
            StringAccessError::SliceStepZero => IncanError::slice_step_zero(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incan_error_display_is_canonical() {
        assert_eq!(
            IncanError::string_index_out_of_range().to_string(),
            "IndexError: string index out of range"
        );
        assert_eq!(
            IncanError::list_pop_empty().to_string(),
            "IndexError: pop from empty list"
        );
        assert_eq!(
            IncanError::slice_step_zero().to_string(),
            "ValueError: slice step cannot be zero"
        );
        assert_eq!(
            IncanError::zero_division().to_string(),
            "ZeroDivisionError: float division by zero"
        );
        assert_eq!(
            IncanError::list_value_not_found().to_string(),
            "ValueError: value not found in list"
        );
    }

    #[test]
    fn incan_error_dynamic_details_format() {
        assert_eq!(
            IncanError::index_out_of_range_for("list", 5, 3).to_string(),
            "IndexError: index 5 out of range for list of length 3"
        );
        assert_eq!(
            IncanError::cannot_convert_to_int("123x").to_string(),
            "ValueError: cannot convert '123x' to int"
        );
    }

    #[test]
    fn json_errors_match_python_style() {
        assert_eq!(
            IncanError::json_not_serializable("ApiRequest").to_string(),
            "TypeError: Object of type ApiRequest is not JSON serializable"
        );
        assert_eq!(
            IncanError::json_decode_error("expected value at line 1 column 1").to_string(),
            "JSONDecodeError: expected value at line 1 column 1"
        );
    }

    #[test]
    fn numeric_parse_errors_are_canonical() {
        assert_eq!(
            IncanError::cannot_convert_to_int("123x").to_string(),
            "ValueError: cannot convert '123x' to int"
        );
        assert_eq!(
            IncanError::cannot_convert_to_float("123x").to_string(),
            "ValueError: cannot convert '123x' to float"
        );
    }

    #[test]
    fn range_step_zero_is_canonical() {
        assert_eq!(
            IncanError::range_step_zero().to_string(),
            "ValueError: range() arg 3 must not be zero"
        );
    }

    #[test]
    fn key_error_not_found_formatter_is_canonical() {
        let key = "missing";
        assert_eq!(
            key_not_found_in_dict(&key).to_string(),
            "KeyError: 'missing' not found in dict"
        );
    }
}

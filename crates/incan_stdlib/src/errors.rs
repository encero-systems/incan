//! Runtime error helpers for Incan-generated Rust code.
//!
//! The semantic core (`incan_core`) owns the canonical error taxonomy + formatting (`IncanError`).
//! The runtime (`incan_stdlib`) provides convenience helpers to *raise* those errors as panics,
//! keeping the compiler/runtime user-facing text aligned.

use core::fmt::Write as _;
use core::fmt::{self, Display};

use incan_core::errors::{ErrorKind, IncanError};
use incan_core::lang;

/// Raise a runtime error (implemented as a panic) with canonical formatting.
#[cold]
#[track_caller]
pub fn raise(err: impl Display) -> ! {
    panic!("{err}");
}

/// Compiler-only runtime helpers used by generated Rust.
#[doc(hidden)]
pub mod __private {
    /// Raise an explicit panic-hosted runtime-misuse boundary.
    ///
    /// This is reserved for generated or stdlib-backed surfaces that are intentionally not ordinary Incan
    /// exceptions, such as runner-only markers or proc-macro-backed decorator placeholders that cannot execute as
    /// normal runtime functions. Keep these behind named helpers so generated Rust does not embed ad hoc `panic!`
    /// stubs inline.
    #[cold]
    #[track_caller]
    pub fn raise_runtime_misuse(message: &str) -> ! {
        panic!("{message}");
    }
}

/// Raise a canonical `Kind: ...` error without allocating an intermediate `String`.
#[cold]
#[track_caller]
pub fn raise_kind_fmt(kind: ErrorKind, msg: fmt::Arguments<'_>) -> ! {
    panic!("{}: {}", lang::errors::as_str(kind), msg);
}

/// Format a canonical `Kind: ...` error into a `String` (single-pass; no intermediate `String`s).
pub fn error_string_kind_fmt(kind: ErrorKind, msg: fmt::Arguments<'_>) -> String {
    let mut out = String::with_capacity(lang::errors::as_str(kind).len() + 2 + 64);
    // Writing to String cannot fail.
    let _ = write!(&mut out, "{}: {}", lang::errors::as_str(kind), msg);
    out
}

/// Raise a `ValueError` with a canonical `ValueError: ...` prefix.
#[cold]
#[track_caller]
pub fn raise_value_error(msg: &str) -> ! {
    raise(IncanError::with_message(ErrorKind::ValueError, msg))
}

/// Raise a `TypeError` with a canonical `TypeError: ...` prefix.
#[cold]
#[track_caller]
pub fn raise_type_error(msg: &str) -> ! {
    raise(IncanError::with_message(ErrorKind::TypeError, msg))
}

/// Raise an `IndexError` with a canonical `IndexError: ...` prefix.
#[cold]
#[track_caller]
pub fn raise_index_error(msg: &str) -> ! {
    raise(IncanError::with_message(ErrorKind::IndexError, msg))
}

/// Raise `IndexError: pop from empty list` (Python-compatible empty `list.pop()`).
#[cold]
#[track_caller]
pub fn raise_list_pop_empty() -> ! {
    raise(IncanError::list_pop_empty())
}

/// Raise `ValueError: value not found in list`.
#[cold]
#[track_caller]
pub fn raise_list_value_not_found() -> ! {
    raise(IncanError::list_value_not_found())
}

/// Raise a `KeyError` with a canonical `KeyError: ...` prefix.
#[cold]
#[track_caller]
pub fn raise_key_error(msg: &str) -> ! {
    raise(IncanError::with_message(ErrorKind::KeyError, msg))
}

/// Raise a canonical `ZeroDivisionError: float division by zero`.
#[cold]
#[track_caller]
pub fn raise_zero_division() -> ! {
    raise(IncanError::zero_division())
}

/// Raise a Python-like JSON serialization error.
///
/// Mirrors Python's `json.dumps(...)` behavior (a `TypeError` with a canonical message).
#[cold]
#[track_caller]
pub fn raise_json_serialization_error(type_name: &str) -> ! {
    raise(IncanError::json_not_serializable(type_name))
}

/// Format a Python-like JSON decode error as a `String`.
///
/// This is useful in APIs that return `Result<T, String>` rather than panicking.
pub fn json_decode_error_string(err: impl Display) -> String {
    error_string_kind_fmt(ErrorKind::JsonDecodeError, format_args!("{err}"))
}

/// Raise a Python-like JSON decode error (panic) with a canonical `JSONDecodeError: ...` prefix.
#[cold]
#[track_caller]
pub fn raise_json_decode_error(message: &str) -> ! {
    raise(IncanError::json_decode_error(message))
}

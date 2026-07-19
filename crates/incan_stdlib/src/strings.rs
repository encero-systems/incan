//! Provide runtime string helpers that delegate to the shared semantic core.
//!
//! This module is used by generated Rust code. Its functions intentionally route behavior through `incan_core::strings`
//! so runtime behavior (including panics/messages) stays aligned with compiler expectations and parity tests.

use crate::errors::raise;
use incan_core::errors::IncanError;
use incan_core::strings::{
    StringAccessError, fstring as semantics_fstring, str_char_at as semantics_str_char_at,
    str_cmp as semantics_str_cmp, str_concat as semantics_str_concat, str_contains as semantics_str_contains,
    str_ends_with as semantics_str_ends_with, str_join as semantics_str_join, str_lower as semantics_str_lower,
    str_replace as semantics_str_replace, str_slice as semantics_str_slice,
    str_slice_byte_range as semantics_str_slice_byte_range,
    str_slice_from_byte_offset as semantics_str_slice_from_byte_offset, str_split as semantics_str_split,
    str_starts_with as semantics_str_starts_with, str_strip as semantics_str_strip, str_upper as semantics_str_upper,
};

/// Index a string by Unicode scalar index at runtime.
///
/// ## Parameters
///
/// - `s`: the string to index.
/// - `idx`: the index (supports negative indices; Python-style).
///
/// ## Returns
///
/// - (`String`): a single-character string (one Unicode scalar).
///
/// ## Panics
///
/// - If `idx` is out of range: with `IndexError: string index out of range`.
pub fn str_index<S: AsRef<str>>(s: S, idx: i64) -> String {
    let s = s.as_ref();
    match semantics_str_char_at(s, idx) {
        Ok(ch) => ch,
        Err(StringAccessError::IndexOutOfRange) => raise(IncanError::string_index_out_of_range()),
        Err(StringAccessError::SliceStepZero) => unreachable!("step zero is not used for index"),
    }
}

/// Slice a string over Unicode scalars at runtime (Python-like semantics).
///
/// ## Parameters
///
/// - `s`: the string to slice.
/// - `start`: optional start index (inclusive).
/// - `end`: optional end index (exclusive).
/// - `step`: optional step; defaults to `1`. Negative steps slice backwards.
///
/// ## Returns
///
/// - (`String`): the sliced string.
///
/// ## Panics
///
/// - If `step == 0`: with `ValueError: slice step cannot be zero`.
pub fn str_slice<S: AsRef<str>>(s: S, start: Option<i64>, end: Option<i64>, step: Option<i64>) -> String {
    let s = s.as_ref();
    match semantics_str_slice(s, start, end, step) {
        Ok(out) => out,
        Err(StringAccessError::SliceStepZero) => raise(IncanError::slice_step_zero()),
        Err(StringAccessError::IndexOutOfRange) => {
            // Should not happen because slice clamps; keep aligned if policy changes.
            raise(IncanError::string_index_out_of_range())
        }
    }
}

/// Slice a string between UTF-8 byte offsets supplied by a host API.
///
/// Ordinary Incan slices use Unicode-scalar indices. This helper is for stdlib interop code that reconstructs strings
/// from byte spans, such as `std.regex`.
///
/// ## Panics
///
/// - If either offset is invalid, out of range, reversed, or not a UTF-8 character boundary.
pub fn str_slice_byte_range(s: &str, start: i64, end: i64) -> String {
    match semantics_str_slice_byte_range(s, start, end) {
        Ok(out) => out,
        Err(StringAccessError::IndexOutOfRange) => raise(IncanError::string_index_out_of_range()),
        Err(StringAccessError::SliceStepZero) => unreachable!("byte-range slices do not accept a step"),
    }
}

/// Slice a string from a UTF-8 byte offset supplied by a host API through the end.
///
/// ## Panics
///
/// - If the offset is invalid, out of range, or not a UTF-8 character boundary.
pub fn str_slice_from_byte_offset(s: &str, start: i64) -> String {
    match semantics_str_slice_from_byte_offset(s, start) {
        Ok(out) => out,
        Err(StringAccessError::IndexOutOfRange) => raise(IncanError::string_index_out_of_range()),
        Err(StringAccessError::SliceStepZero) => unreachable!("byte-offset slices do not accept a step"),
    }
}

/// Concatenate two strings at runtime.
///
/// ## Parameters
///
/// - `lhs`: left-hand string.
/// - `rhs`: right-hand string.
///
/// ## Returns
///
/// - (`String`): the concatenated string.
pub fn str_concat(lhs: &str, rhs: &str) -> String {
    semantics_str_concat(lhs, rhs)
}

/// Compare two strings for equality.
///
/// ## Parameters
///
/// - `lhs`: left-hand value.
/// - `rhs`: right-hand value.
///
/// ## Returns
///
/// - (`bool`): whether the values are equal under shared string semantics.
pub fn str_eq<L: AsRef<str>, R: AsRef<str>>(lhs: L, rhs: R) -> bool {
    semantics_str_cmp(lhs.as_ref(), rhs.as_ref()).is_eq()
}

/// Compare two strings for inequality.
///
/// ## Parameters
///
/// - `lhs`: left-hand value.
/// - `rhs`: right-hand value.
///
/// ## Returns
///
/// - (`bool`): whether the values are not equal under shared string semantics.
pub fn str_ne<L: AsRef<str>, R: AsRef<str>>(lhs: L, rhs: R) -> bool {
    !str_eq(lhs, rhs)
}

/// Compare two strings (`lhs < rhs`).
///
/// ## Parameters
///
/// - `lhs`: left-hand value.
/// - `rhs`: right-hand value.
///
/// ## Returns
///
/// - (`bool`): whether `lhs` is lexicographically less than `rhs`.
pub fn str_lt<L: AsRef<str>, R: AsRef<str>>(lhs: L, rhs: R) -> bool {
    semantics_str_cmp(lhs.as_ref(), rhs.as_ref()).is_lt()
}

/// Compare two strings (`lhs <= rhs`).
///
/// ## Parameters
///
/// - `lhs`: left-hand value.
/// - `rhs`: right-hand value.
///
/// ## Returns
///
/// - (`bool`): whether `lhs` is lexicographically less than or equal to `rhs`.
pub fn str_le<L: AsRef<str>, R: AsRef<str>>(lhs: L, rhs: R) -> bool {
    !semantics_str_cmp(lhs.as_ref(), rhs.as_ref()).is_gt()
}

/// Compare two strings (`lhs > rhs`).
///
/// ## Parameters
///
/// - `lhs`: left-hand value.
/// - `rhs`: right-hand value.
///
/// ## Returns
///
/// - (`bool`): whether `lhs` is lexicographically greater than `rhs`.
pub fn str_gt<L: AsRef<str>, R: AsRef<str>>(lhs: L, rhs: R) -> bool {
    semantics_str_cmp(lhs.as_ref(), rhs.as_ref()).is_gt()
}

/// Compare two strings (`lhs >= rhs`).
///
/// ## Parameters
///
/// - `lhs`: left-hand value.
/// - `rhs`: right-hand value.
///
/// ## Returns
///
/// - (`bool`): whether `lhs` is lexicographically greater than or equal to `rhs`.
pub fn str_ge<L: AsRef<str>, R: AsRef<str>>(lhs: L, rhs: R) -> bool {
    !semantics_str_cmp(lhs.as_ref(), rhs.as_ref()).is_lt()
}

// --- Methods (shared policy wrappers) -----------------------------------------------------------

/// Convert a string to uppercase.
///
/// ## Parameters
///
/// - `s`: the input string.
///
/// ## Returns
///
/// - (`String`): the uppercase string.
pub fn str_upper<S: AsRef<str>>(s: S) -> String {
    semantics_str_upper(s.as_ref())
}

/// Convert a string to lowercase.
///
/// ## Parameters
///
/// - `s`: the input string.
///
/// ## Returns
///
/// - (`String`): the lowercase string.
pub fn str_lower<S: AsRef<str>>(s: S) -> String {
    semantics_str_lower(s.as_ref())
}

/// Strip leading and trailing whitespace.
///
/// ## Parameters
///
/// - `s`: the input string.
///
/// ## Returns
///
/// - (`String`): the stripped string.
pub fn str_strip<S: AsRef<str>>(s: S) -> String {
    semantics_str_strip(s.as_ref())
}

/// Check whether a string starts with a prefix.
///
/// ## Parameters
///
/// - `s`: the input string.
/// - `prefix`: the prefix to test.
///
/// ## Returns
///
/// - (`bool`): whether `s` starts with `prefix`.
pub fn str_starts_with<S: AsRef<str>, P: AsRef<str>>(s: S, prefix: P) -> bool {
    semantics_str_starts_with(s.as_ref(), prefix.as_ref())
}

/// Check whether a string ends with a suffix.
///
/// ## Parameters
///
/// - `s`: the input string.
/// - `suffix`: the suffix to test.
///
/// ## Returns
///
/// - (`bool`): whether `s` ends with `suffix`.
pub fn str_ends_with<S: AsRef<str>, P: AsRef<str>>(s: S, suffix: P) -> bool {
    semantics_str_ends_with(s.as_ref(), suffix.as_ref())
}

/// Replace all occurrences of `from` with `to`.
///
/// ## Parameters
///
/// - `s`: the input string.
/// - `from`: substring to replace.
/// - `to`: replacement string.
///
/// ## Returns
///
/// - (`String`): the replaced string.
pub fn str_replace<S: AsRef<str>, F: AsRef<str>, T: AsRef<str>>(s: S, from: F, to: T) -> String {
    semantics_str_replace(s.as_ref(), from.as_ref(), to.as_ref())
}

/// Split a string by an optional separator.
///
/// ## Parameters
///
/// - `s`: the input string.
/// - `sep`: optional separator; if `None`, returns a single-element vector containing `s`.
///
/// ## Returns
///
/// - (`Vec<String>`): split parts as owned strings.
pub fn str_split<S: AsRef<str>, P: AsRef<str>>(s: S, sep: Option<P>) -> Vec<String> {
    let sep_ref = sep.as_ref().map(|p| p.as_ref());
    semantics_str_split(s.as_ref(), sep_ref)
}

/// Join items with a separator.
///
/// ## Parameters
///
/// - `sep`: separator placed between items.
/// - `items`: items to join.
///
/// ## Returns
///
/// - (`String`): the joined string.
pub fn str_join<S: AsRef<str>>(sep: S, items: &[String]) -> String {
    // Accept owned Strings for ergonomics in generated code.
    semantics_str_join(sep.as_ref(), items)
}

/// Check whether `needle` is a substring of `haystack`.
///
/// ## Parameters
///
/// - `haystack`: the string to search in.
/// - `needle`: the string to search for.
///
/// ## Returns
///
/// - (`bool`): whether `needle` is contained in `haystack`.
pub fn str_contains<H: AsRef<str>, N: AsRef<str>>(haystack: H, needle: N) -> bool {
    semantics_str_contains(haystack.as_ref(), needle.as_ref())
}

/// Runtime f-string composition using shared semantics.
///
/// `parts` length must be one greater than `args` length.
pub fn fstring(parts: &[&str], args: &[String]) -> String {
    semantics_fstring(parts, args)
}

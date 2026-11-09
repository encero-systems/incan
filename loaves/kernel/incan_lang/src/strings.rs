//! Define shared string semantics (policy + pure helpers).
//!
//! This module holds **pure/deterministic** helpers used by both the compiler (typechecking, const-eval, lowering
//! decisions) and the runtime/stdlib to avoid semantic drift.
//!
//! ## Notes
//! - **Indexing model**: Unicode scalar indexing (Rust `char`), not bytes or grapheme clusters.
//! - **Negative indices**: supported (Python-style): `s[-1]` is the last scalar.
//! - **Slicing**: Python-like `start`, `end`, `step` (default `step = 1`), with negative indices and bounds clamping.
//! - **Error messages**: user-facing exception formatting lives in [`crate::errors::IncanError`].

use core::fmt;
use std::cmp::Ordering;

use crate::errors::IncanError;
/// Represent string access errors produced by semantic-core helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringAccessError {
    IndexOutOfRange,
    SliceStepZero,
}

impl fmt::Display for StringAccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", IncanError::from(*self))
    }
}

/// Check whether a substring is contained in a string (Python-like `in`).
///
/// ## Parameters
/// - `haystack`: String to search in.
/// - `needle`: String to search for.
///
/// ## Returns
/// - `bool`: `true` if `needle` is contained in `haystack`.
pub fn str_contains(haystack: &str, needle: &str) -> bool {
    haystack.contains(needle)
}

/// Count the Unicode scalar values in a string.
///
/// This is intentionally a scalar count rather than a UTF-8 byte count or a user-perceived grapheme count. It is the
/// canonical policy named by the Incan string-method registry and shared by direct execution and generated Rust.
///
/// ## Parameters
/// - `s`: String to measure.
///
/// ## Returns
/// - `i64`: Number of Unicode scalar values.
pub fn str_len(s: &str) -> i64 {
    s.chars().count() as i64
}

/// Concatenate two strings.
///
/// ## Parameters
/// - `lhs`: Left-hand string.
/// - `rhs`: Right-hand string.
///
/// ## Returns
/// - `String`: Concatenated string.
pub fn str_concat(lhs: &str, rhs: &str) -> String {
    let mut out = String::with_capacity(lhs.len() + rhs.len());
    out.push_str(lhs);
    out.push_str(rhs);
    out
}

/// Compare two strings lexicographically (Unicode scalar order).
///
/// ## Parameters
/// - `lhs`: left-hand string.
/// - `rhs`: right-hand string.
///
/// ## Returns
/// - (`Ordering`): the lexicographic ordering.
pub fn str_cmp(lhs: &str, rhs: &str) -> Ordering {
    lhs.cmp(rhs)
}

/// Index a string by Unicode scalar index.
///
/// ## Parameters
/// - `s`: String to index.
/// - `idx`: Index (supports negative indices; Python-style).
///
/// ## Returns
/// - `Ok(String)`: Single-character string (one Unicode scalar).
/// - `Err(StringAccessError)`: If the index is out of range.
pub fn str_char_at(s: &str, idx: i64) -> Result<String, StringAccessError> {
    let character = if idx >= 0 {
        usize::try_from(idx).ok().and_then(|position| s.chars().nth(position))
    } else {
        usize::try_from(idx.unsigned_abs() - 1)
            .ok()
            .and_then(|position| s.chars().rev().nth(position))
    };
    let Some(ch) = character else {
        return Err(StringAccessError::IndexOutOfRange);
    };
    Ok(ch.to_string())
}

/// Find the byte boundary before a clamped Unicode scalar index.
///
/// Positive indices scan from the front, negative indices scan from the back, and an index outside the string clamps
/// to the corresponding end. The returned offset is always a valid UTF-8 boundary.
fn byte_before_scalar(s: &str, index: i64) -> usize {
    if index >= 0 {
        let Ok(position) = usize::try_from(index) else {
            return s.len();
        };
        return s.char_indices().nth(position).map_or(s.len(), |(offset, _)| offset);
    }

    let Ok(position) = usize::try_from(index.unsigned_abs() - 1) else {
        return 0;
    };
    s.char_indices().rev().nth(position).map_or(0, |(offset, _)| offset)
}

/// Find the byte boundary after a clamped Unicode scalar index.
///
/// This is the inclusive counterpart of [`byte_before_scalar`] for reverse slices. An index beyond the front clamps
/// after the last scalar, while one beyond the back clamps before the first scalar.
fn byte_after_scalar(s: &str, index: i64) -> usize {
    if index >= 0 {
        let Ok(position) = usize::try_from(index) else {
            return s.len();
        };
        return s
            .char_indices()
            .nth(position)
            .map_or(s.len(), |(offset, character)| offset + character.len_utf8());
    }

    let Ok(position) = usize::try_from(index.unsigned_abs() - 1) else {
        return 0;
    };
    s.char_indices()
        .rev()
        .nth(position)
        .map_or(0, |(offset, character)| offset + character.len_utf8())
}

/// Slice a string over Unicode scalars (Python-like semantics).
///
/// ## Parameters
/// - `s`: String to slice.
/// - `start`: Optional start index (inclusive).
/// - `end`: Optional end index (exclusive).
/// - `step`: Optional step; defaults to `1`. Negative steps slice backwards.
///
/// ## Returns
/// - `Ok(String)`: Sliced string.
/// - `Err(StringAccessError)`: If `step == 0`.
///
/// ## Notes
/// - Indices support Python-like negative values.
/// - Indices are clamped to bounds for slicing.
pub fn str_slice(
    s: &str,
    start: Option<i64>,
    end: Option<i64>,
    step: Option<i64>,
) -> Result<String, StringAccessError> {
    let step = step.unwrap_or(1);
    if step == 0 {
        return Err(StringAccessError::SliceStepZero);
    }

    // ---- Forward slices use boundaries before their start and end scalars ----
    let mut out = String::new();
    if step > 0 {
        let start_byte = start.map_or(0, |index| byte_before_scalar(s, index));
        let end_byte = end.map_or(s.len(), |index| byte_before_scalar(s, index));
        if start_byte >= end_byte {
            return Ok(out);
        }
        let stride = step.unsigned_abs();
        let mut distance = 0_u64;
        for character in s[start_byte..end_byte].chars() {
            if distance % stride == 0 {
                out.push(character);
            }
            distance += 1;
        }
        return Ok(out);
    }

    // ---- Reverse slices use boundaries after their inclusive start and exclusive end scalars ----
    let start_byte = start.map_or(s.len(), |index| byte_after_scalar(s, index));
    let end_byte = end.map_or(0, |index| byte_after_scalar(s, index));
    if start_byte <= end_byte {
        return Ok(out);
    }
    let stride = step.unsigned_abs();
    let mut distance = 0_u64;
    for character in s[end_byte..start_byte].chars().rev() {
        if distance % stride == 0 {
            out.push(character);
        }
        distance += 1;
    }

    Ok(out)
}

/// Slice a string between two UTF-8 byte offsets.
///
/// This is a narrow interop helper for host APIs, such as regex engines, whose spans are byte-based. Ordinary Incan
/// string indexing and slicing remain Unicode-scalar based through [`str_char_at`] and [`str_slice`].
///
/// ## Parameters
/// - `s`: String to slice.
/// - `start`: Inclusive UTF-8 byte offset.
/// - `end`: Exclusive UTF-8 byte offset.
///
/// ## Returns
/// - `Ok(String)`: The requested byte range when both offsets are valid UTF-8 boundaries.
/// - `Err(StringAccessError)`: If either offset is negative, out of range, not a character boundary, or reversed.
pub fn str_slice_byte_range(s: &str, start: i64, end: i64) -> Result<String, StringAccessError> {
    let start = usize::try_from(start).map_err(|_| StringAccessError::IndexOutOfRange)?;
    let end = usize::try_from(end).map_err(|_| StringAccessError::IndexOutOfRange)?;
    s.get(start..end)
        .map(str::to_string)
        .ok_or(StringAccessError::IndexOutOfRange)
}

/// Slice a string from one UTF-8 byte offset through the end.
///
/// ## Parameters
/// - `s`: String to slice.
/// - `start`: Inclusive UTF-8 byte offset.
///
/// ## Returns
/// - `Ok(String)`: The suffix beginning at `start` when it is a valid UTF-8 boundary.
/// - `Err(StringAccessError)`: If `start` is negative, out of range, or not a character boundary.
pub fn str_slice_from_byte_offset(s: &str, start: i64) -> Result<String, StringAccessError> {
    let start = usize::try_from(start).map_err(|_| StringAccessError::IndexOutOfRange)?;
    s.get(start..)
        .map(str::to_string)
        .ok_or(StringAccessError::IndexOutOfRange)
}

// ---- String methods (shared policy) -------------------------------------------------------------

/// Convert a string to uppercase.
///
/// ## Parameters
/// - `s`: Input string.
///
/// ## Returns
/// - `String`: Uppercased string.
pub fn str_upper(s: &str) -> String {
    s.to_uppercase()
}

/// Convert a string to lowercase.
///
/// ## Parameters
/// - `s`: Input string.
///
/// ## Returns
/// - `String`: Lowercased string.
pub fn str_lower(s: &str) -> String {
    s.to_lowercase()
}

/// Strip leading and trailing whitespace.
///
/// ## Parameters
/// - `s`: Input string.
///
/// ## Returns
/// - `String`: Stripped string.
pub fn str_strip(s: &str) -> String {
    s.trim().to_string()
}

/// Check whether a string starts with a prefix.
///
/// ## Parameters
/// - `s`: Input string.
/// - `prefix`: Prefix to test.
///
/// ## Returns
/// - `bool`: Whether `s` starts with `prefix`.
pub fn str_starts_with(s: &str, prefix: &str) -> bool {
    s.starts_with(prefix)
}

/// Check whether a string ends with a suffix.
///
/// ## Parameters
/// - `s`: Input string.
/// - `suffix`: Suffix to test.
///
/// ## Returns
/// - `bool`: Whether `s` ends with `suffix`.
pub fn str_ends_with(s: &str, suffix: &str) -> bool {
    s.ends_with(suffix)
}

/// Replace all occurrences of `from` with `to`.
///
/// ## Parameters
/// - `s`: Input string.
/// - `from`: Substring to replace.
/// - `to`: Replacement string.
///
/// ## Returns
/// - `String`: Replaced string.
pub fn str_replace(s: &str, from: &str, to: &str) -> String {
    s.replace(from, to)
}

/// Split a string by an optional separator.
///
/// ## Parameters
/// - `s`: Input string.
/// - `sep`: Optional separator; if `None`, returns a single-element vector containing `s`.
///
/// ## Returns
/// - `Vec<String>`: Split parts as owned strings.
pub fn str_split(s: &str, sep: Option<&str>) -> Vec<String> {
    match sep {
        Some(sep) => s.split(sep).map(|p| p.to_string()).collect(),
        None => vec![s.to_string()],
    }
}

/// Join items with a separator.
///
/// ## Parameters
/// - `sep`: Separator placed between items.
/// - `items`: Items to join.
///
/// ## Returns
/// - `String`: Joined string.
pub fn str_join<S: AsRef<str>>(sep: &str, items: &[S]) -> String {
    items.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(sep)
}

/// Compose an f-string from literal parts and already-formatted arguments.
///
/// ## Parameters
/// - `parts`: Literal segments (length must be `args.len() + 1`).
/// - `args`: Already-formatted argument strings.
///
/// ## Returns
/// - `String`: Assembled string.
///
/// ## Panics
/// - If `parts.len() != args.len() + 1` (mismatched part/arg lengths).
///
/// ## Notes
/// - Formatting is the caller's responsibility; this only concatenates.
pub fn fstring(parts: &[&str], args: &[String]) -> String {
    if parts.len() != args.len() + 1 {
        // Defensive: compiler should ensure lengths match.
        panic!("fstring parts/args length mismatch");
    }
    let mut out = String::new();
    for i in 0..args.len() {
        out.push_str(parts[i]);
        out.push_str(&args[i]);
    }
    out.push_str(parts.last().unwrap_or(&""));
    out
}

#[cfg(test)]
mod tests {
    use super::{StringAccessError, str_char_at, str_slice, str_slice_byte_range, str_slice_from_byte_offset};

    /// Preserve scalar indexing and Python-like bounds for ASCII text.
    #[test]
    fn scalar_access_preserves_ascii_bounds_and_steps() {
        let text = "abcdef";

        assert_eq!(str_char_at(text, 0), Ok("a".to_string()));
        assert_eq!(str_char_at(text, -1), Ok("f".to_string()));
        assert_eq!(str_char_at(text, 6), Err(StringAccessError::IndexOutOfRange));
        assert_eq!(str_char_at(text, -7), Err(StringAccessError::IndexOutOfRange));
        assert_eq!(str_slice(text, Some(0), Some(1), None), Ok("a".to_string()));
        assert_eq!(str_slice(text, None, Some(3), None), Ok("abc".to_string()));
        assert_eq!(str_slice(text, Some(3), None, None), Ok("def".to_string()));
        assert_eq!(str_slice(text, None, None, Some(2)), Ok("ace".to_string()));
        assert_eq!(str_slice(text, None, None, Some(-1)), Ok("fedcba".to_string()));
        assert_eq!(str_slice(text, Some(4), Some(1), Some(-2)), Ok("ec".to_string()));
        assert_eq!(str_slice(text, Some(-3), Some(-1), None), Ok("de".to_string()));
        assert_eq!(str_slice(text, Some(-99), Some(2), None), Ok("ab".to_string()));
        assert_eq!(str_slice(text, Some(99), Some(100), None), Ok(String::new()));
        assert_eq!(str_slice(text, Some(4), Some(2), None), Ok(String::new()));
        assert_eq!(
            str_slice(text, None, None, Some(0)),
            Err(StringAccessError::SliceStepZero)
        );
    }

    /// Count accented, astral, and combining Unicode scalars independently of UTF-8 width.
    #[test]
    fn scalar_access_preserves_multibyte_and_combining_text() {
        let text = "Aé🙂e\u{301}Z";

        assert_eq!(str_char_at(text, 2), Ok("🙂".to_string()));
        assert_eq!(str_char_at(text, -2), Ok("\u{301}".to_string()));
        assert_eq!(str_slice(text, Some(1), Some(5), None), Ok("é🙂e\u{301}".to_string()));
        assert_eq!(str_slice(text, Some(2), Some(3), None), Ok("🙂".to_string()));
        assert_eq!(str_slice(text, Some(3), Some(5), None), Ok("e\u{301}".to_string()));
        assert_eq!(str_slice(text, None, Some(-1), None), Ok("Aé🙂e\u{301}".to_string()));
        assert_eq!(str_slice(text, Some(-3), None, None), Ok("e\u{301}Z".to_string()));
        assert_eq!(str_slice(text, Some(1), None, Some(2)), Ok("éeZ".to_string()));
        assert_eq!(str_slice(text, None, None, Some(-1)), Ok("Z\u{301}e🙂éA".to_string()));
    }

    /// Keep byte-range helpers aligned to valid UTF-8 boundaries.
    #[test]
    fn byte_range_slices_preserve_utf8_boundaries() {
        let text = "beforeé;31m";

        assert_eq!(str_slice_byte_range(text, 0, 6), Ok("before".to_string()));
        assert_eq!(str_slice_byte_range(text, 6, 8), Ok("é".to_string()));
        assert_eq!(str_slice_from_byte_offset(text, 8), Ok(";31m".to_string()));
    }

    /// Refuse negative, reversed, out-of-range, and split-codepoint byte offsets.
    #[test]
    fn byte_range_slices_reject_invalid_offsets() {
        let text = "é";

        for result in [
            str_slice_byte_range(text, -1, 2),
            str_slice_byte_range(text, 0, 3),
            str_slice_byte_range(text, 0, 1),
            str_slice_byte_range(text, 2, 0),
            str_slice_from_byte_offset(text, -1),
            str_slice_from_byte_offset(text, 1),
            str_slice_from_byte_offset(text, 3),
        ] {
            assert_eq!(result, Err(StringAccessError::IndexOutOfRange));
        }
    }
}

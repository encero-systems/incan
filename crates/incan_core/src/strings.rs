//! Define shared string semantics (policy + pure helpers).
//!
//! This module holds **pure/deterministic** helpers used by both the compiler (typechecking,
//! const-eval, lowering decisions) and the runtime/stdlib to avoid semantic drift.
//!
//! ## Notes
//! - **Indexing model**: Unicode scalar indexing (Rust `char`), not bytes or grapheme clusters.
//! - **Negative indices**: supported (Python-style): `s[-1]` is the last scalar.
//! - **Slicing**: Python-like `start`, `end`, `step` (default `step = 1`), with negative indices and bounds clamping.
//! - **Error messages**: user-facing exception formatting lives in [`crate::errors::IncanError`].

use core::fmt;
use std::cmp::Ordering;

use crate::errors::IncanError;
use crate::indexing::normalize_slice_bounds;

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

pub fn str_eq(lhs: &str, rhs: &str) -> bool {
    lhs == rhs
}

pub fn str_ne(lhs: &str, rhs: &str) -> bool {
    lhs != rhs
}

pub fn str_lt(lhs: &str, rhs: &str) -> bool {
    lhs < rhs
}

pub fn str_le(lhs: &str, rhs: &str) -> bool {
    lhs <= rhs
}

pub fn str_gt(lhs: &str, rhs: &str) -> bool {
    lhs > rhs
}

pub fn str_ge(lhs: &str, rhs: &str) -> bool {
    lhs >= rhs
}

/// Return the string length in Unicode scalars (Rust `char`).
///
/// ## Parameters
/// - `s`: the string to measure.
///
/// ## Returns
/// - (`usize`): number of Unicode scalar values in `s`.
pub fn str_len(s: &str) -> usize {
    s.chars().count()
}

/// Normalize an index (supports negatives). Returns `None` if out of range.
fn normalize_index(len: usize, idx: i64) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let len_i = len as i64;
    let mut i = idx;
    if i < 0 {
        i += len_i;
    }
    if i < 0 || i >= len_i { None } else { Some(i as usize) }
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
    let len = str_len(s);
    let Some(pos) = normalize_index(len, idx) else {
        return Err(StringAccessError::IndexOutOfRange);
    };
    let ch = s.chars().nth(pos).expect("index normalized to bounds");
    Ok(ch.to_string())
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

    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;

    let (start_idx, end_idx) = normalize_slice_bounds(len, start, end, step);

    let mut out = String::new();
    let mut i = start_idx;

    if step > 0 {
        while i < end_idx {
            let idx = i as usize;
            if let Some(ch) = chars.get(idx) {
                out.push(*ch);
            }
            i += step;
        }
    } else {
        while i > end_idx {
            let idx = i as usize;
            if let Some(ch) = chars.get(idx) {
                out.push(*ch);
            }
            i += step; // negative
        }
    }

    Ok(out)
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

/// Escape `{` and `}` in f-string literal parts for safe interpolation.
///
/// ## Parameters
/// - `s`: Literal segment to escape.
///
/// ## Returns
/// - `String`: `s` with braces escaped as `{{` and `}}`.
///
/// ## Notes
/// - Preserves literal braces when lowering Incan f-strings.
pub fn escape_format_literal(s: &str) -> String {
    s.replace('{', "{{").replace('}', "}}")
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

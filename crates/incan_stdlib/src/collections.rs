//! Collection helpers for Incan-generated Rust code.
//!
//! This module exists to keep runtime behavior Python-like while avoiding Rust-default panic messages
//! (e.g. Vec/HashMap indexing panics). Instead, we raise canonical `IncanError` messages.

use core::fmt::Display;
use std::collections::HashMap;
use std::hash::Hash;

use crate::errors::raise;
use incan_core::errors::{IncanError, key_not_found_in_dict};
use incan_core::indexing::normalize_slice_bounds;

/// Get a list element by Python-style index (supports negative indices).
///
/// ## Panics
/// - `IndexError: index {index} out of range for list of length {len}` if out of range.
#[inline]
pub fn list_get<T>(list: &[T], index: i64) -> &T {
    let len = list.len();
    let len_i = len as i64;
    let mut i = index;
    if i < 0 {
        i += len_i;
    }
    if i < 0 || i >= len_i {
        raise(IncanError::index_out_of_range_for("list", index, len));
    }
    // Safe after bounds check.
    &list[i as usize]
}

/// Get a mutable list element by Python-style index (supports negative indices).
///
/// ## Panics
/// - `IndexError: index {index} out of range for list of length {len}` if out of range.
#[inline]
pub fn list_get_mut<T>(list: &mut [T], index: i64) -> &mut T {
    let len = list.len();
    let len_i = len as i64;
    let mut i = index;
    if i < 0 {
        i += len_i;
    }
    if i < 0 || i >= len_i {
        raise(IncanError::index_out_of_range_for("list", index, len));
    }
    // Safe after bounds check.
    &mut list[i as usize]
}

/// Slice a list using Python-like semantics.
///
/// - Negative indices are supported.
/// - Indices are clamped to bounds.
/// - `step` defaults to `1`.
/// - Negative steps slice backwards.
///
/// ## Panics
/// - `ValueError: slice step cannot be zero` if `step == 0`.
pub fn list_slice<T: Clone>(list: &[T], start: Option<i64>, end: Option<i64>, step: Option<i64>) -> Vec<T> {
    let step = step.unwrap_or(1);
    if step == 0 {
        raise(IncanError::slice_step_zero());
    }

    let len = list.len() as i64;

    let (start_idx, end_idx) = normalize_slice_bounds(len, start, end, step);

    let mut out = Vec::new();
    let mut i = start_idx;

    if step > 0 {
        while i < end_idx {
            if let Some(v) = list.get(i as usize) {
                out.push(v.clone());
            }
            i += step;
        }
    } else {
        while i > end_idx {
            if let Some(v) = list.get(i as usize) {
                out.push(v.clone());
            }
            i += step; // negative
        }
    }

    out
}

/// Get a dict value by key (Python-style `d[key]`).
///
/// ## Panics
/// - `KeyError: '{key}' not found in dict` if missing.
#[inline]
pub fn dict_get<'a, K, V>(map: &'a HashMap<K, V>, key: &K) -> &'a V
where
    K: Eq + Hash + Display,
{
    match map.get(key) {
        Some(v) => v,
        None => raise(key_not_found_in_dict(key)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_get_supports_negative_indices() {
        let v = vec![10, 20, 30];
        assert_eq!(*list_get(&v, -1), 30);
        assert_eq!(*list_get(&v, -3), 10);
    }

    #[test]
    #[should_panic(expected = "IndexError: index 3 out of range for list of length 3")]
    fn list_get_oob_panics_with_index_error() {
        let v = vec![10, 20, 30];
        let _ = list_get(&v, 3);
    }

    #[test]
    fn list_slice_clamps_and_steps() {
        let v = vec![1, 2, 3, 4, 5];
        assert_eq!(list_slice(&v, Some(1), Some(10), None), vec![2, 3, 4, 5]);
        assert_eq!(list_slice(&v, Some(0), Some(5), Some(2)), vec![1, 3, 5]);
        assert_eq!(list_slice(&v, Some(-1), None, Some(-1)), vec![5, 4, 3, 2, 1]);
    }

    #[test]
    #[should_panic(expected = "ValueError: slice step cannot be zero")]
    fn list_slice_zero_step_panics_with_value_error() {
        let v = vec![1, 2, 3];
        let _ = list_slice(&v, None, None, Some(0));
    }

    #[test]
    fn dict_get_returns_value_when_present() {
        let mut m: HashMap<String, i64> = HashMap::new();
        m.insert("a".to_string(), 1);
        assert_eq!(*dict_get(&m, &"a".to_string()), 1);
    }

    #[test]
    #[should_panic(expected = "KeyError: 'b' not found in dict")]
    fn dict_get_missing_panics_with_key_error() {
        let mut m: HashMap<String, i64> = HashMap::new();
        m.insert("a".to_string(), 1);
        let _ = dict_get(&m, &"b".to_string());
    }
}

//! Collection helpers for Incan-generated Rust code.
//!
//! This module exists to keep runtime behavior Python-like while avoiding Rust-default panic messages
//! (e.g. Vec/HashMap indexing panics). Instead, we raise canonical `IncanError` messages.

use core::borrow::Borrow;
use core::fmt::Display;
use std::collections::HashMap;
use std::hash::Hash;

use crate::errors::raise;
use incan_core::errors::{IncanError, key_not_found_in_dict};
use incan_core::indexing::normalize_slice_bounds;

#[inline]
fn normalize_list_index(len: usize, index: i64) -> usize {
    let len_i = len as i64;
    let mut i = index;
    if i < 0 {
        i += len_i;
    }
    if i < 0 || i >= len_i {
        raise(IncanError::index_out_of_range_for("list", index, len));
    }
    i as usize
}

/// Get a list element by Python-style index (supports negative indices).
///
/// ## Panics
/// - `IndexError: index {index} out of range for list of length {len}` if out of range.
#[inline]
pub fn list_get<T>(list: &[T], index: i64) -> &T {
    &list[normalize_list_index(list.len(), index)]
}

/// Get a mutable list element by Python-style index (supports negative indices).
///
/// ## Panics
/// - `IndexError: index {index} out of range for list of length {len}` if out of range.
#[inline]
pub fn list_get_mut<T>(list: &mut [T], index: i64) -> &mut T {
    let idx = normalize_list_index(list.len(), index);
    &mut list[idx]
}

/// Remove a list element by Python-style index (supports negative indices).
///
/// This preserves Incan's current `list.remove(index)` semantics while avoiding Rust-native `Vec::remove` panics.
///
/// ## Panics
/// - `IndexError: index {index} out of range for list of length {len}` if out of range.
#[inline]
pub fn list_remove<T>(list: &mut Vec<T>, index: i64) {
    let idx = normalize_list_index(list.len(), index);
    let _ = list.remove(idx);
}

/// Swap two list elements by Python-style indices (supports negative indices).
///
/// ## Panics
/// - `IndexError: index {index} out of range for list of length {len}` if either index is out of range.
#[inline]
pub fn list_swap<T>(list: &mut [T], left: i64, right: i64) {
    let left_idx = normalize_list_index(list.len(), left);
    let right_idx = normalize_list_index(list.len(), right);
    list.swap(left_idx, right_idx);
}

/// Concatenate two lists into a new list, preserving left-to-right order.
///
/// This borrows both inputs so generated `list + list` expressions leave the original bindings usable, matching
/// Incan's value-like list semantics.
#[inline]
#[must_use]
pub fn list_concat<T: Clone>(lhs: &[T], rhs: &[T]) -> Vec<T> {
    let mut out = Vec::with_capacity(lhs.len() + rhs.len());
    out.extend_from_slice(lhs);
    out.extend_from_slice(rhs);
    out
}

/// Append the contents of `rhs` into `lhs`, preserving the source list.
///
/// This matches Incan's `list.extend(other)` behavior: mutate the receiver in place without consuming `other`.
#[inline]
pub fn list_extend<T: Clone>(lhs: &mut Vec<T>, rhs: &[T]) {
    lhs.extend_from_slice(rhs);
}

/// Count occurrences of a value in a list.
#[inline]
#[must_use]
pub fn list_count<T>(list: &[T], value: &T) -> i64
where
    T: PartialEq,
{
    list.iter().filter(|item| *item == value).count() as i64
}

/// Return the first index of a value in a list.
///
/// ## Panics
/// - `ValueError: value not found in list` if missing.
#[inline]
#[must_use]
pub fn list_index<T>(list: &[T], value: &T) -> i64
where
    T: PartialEq,
{
    match list.iter().position(|item| item == value) {
        Some(index) => index as i64,
        None => raise(IncanError::list_value_not_found()),
    }
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
/// Mirrors `HashMap::get` by accepting borrowed probe keys (`&Q`) as long as the stored key type can borrow as `Q`.
/// This keeps generated lookups ergonomic for `Dict[str, V]`, where source-level string literals and borrowed `str`
/// probes should work without forcing owned `String` materialization at every index site.
///
/// ## Panics
/// - `KeyError: '{key}' not found in dict` if missing.
#[inline]
pub fn dict_get<'a, K, Q, V>(map: &'a HashMap<K, V>, key: &Q) -> &'a V
where
    K: Borrow<Q> + Eq + Hash,
    Q: Eq + Hash + Display + ?Sized,
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
    fn list_remove_supports_negative_indices() {
        let mut v = vec![10, 20, 30];
        list_remove(&mut v, -1);
        assert_eq!(v, vec![10, 20]);
    }

    #[test]
    #[should_panic(expected = "IndexError: index 3 out of range for list of length 3")]
    fn list_remove_oob_panics_with_index_error() {
        let mut v = vec![10, 20, 30];
        list_remove(&mut v, 3);
    }

    #[test]
    fn list_swap_supports_negative_indices() {
        let mut v = vec![10, 20, 30];
        list_swap(&mut v, 0, -1);
        assert_eq!(v, vec![30, 20, 10]);
    }

    #[test]
    #[should_panic(expected = "IndexError: index 3 out of range for list of length 3")]
    fn list_swap_oob_panics_with_index_error() {
        let mut v = vec![10, 20, 30];
        list_swap(&mut v, 0, 3);
    }

    #[test]
    fn list_concat_preserves_order() {
        let lhs = vec![1, 2];
        let rhs = vec![3, 4];
        assert_eq!(list_concat(&lhs, &rhs), vec![1, 2, 3, 4]);
        assert_eq!(lhs, vec![1, 2]);
        assert_eq!(rhs, vec![3, 4]);
    }

    #[test]
    fn list_extend_preserves_source_list() {
        let mut lhs = vec![1, 2];
        let rhs = vec![3, 4];
        list_extend(&mut lhs, &rhs);
        assert_eq!(lhs, vec![1, 2, 3, 4]);
        assert_eq!(rhs, vec![3, 4]);
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
    fn dict_get_accepts_borrowed_string_probe() {
        let mut m: HashMap<String, i64> = HashMap::new();
        m.insert("a".to_string(), 1);
        assert_eq!(*dict_get(&m, "a"), 1);
    }

    #[test]
    #[should_panic(expected = "KeyError: 'b' not found in dict")]
    fn dict_get_missing_panics_with_key_error() {
        let mut m: HashMap<String, i64> = HashMap::new();
        m.insert("a".to_string(), 1);
        let _ = dict_get(&m, &"b".to_string());
    }

    #[test]
    #[should_panic(expected = "KeyError: 'b' not found in dict")]
    fn dict_get_missing_borrowed_string_probe_panics_with_key_error() {
        let mut m: HashMap<String, i64> = HashMap::new();
        m.insert("a".to_string(), 1);
        let _ = dict_get(&m, "b");
    }

    #[test]
    fn list_count_returns_occurrence_count() {
        let v = vec![1, 2, 1, 3, 1];
        assert_eq!(list_count(&v, &1), 3);
        assert_eq!(list_count(&v, &9), 0);
    }

    #[test]
    fn list_index_returns_first_match() {
        let v = vec![4, 7, 4, 9];
        assert_eq!(list_index(&v, &4), 0);
        assert_eq!(list_index(&v, &9), 3);
    }

    #[test]
    #[should_panic(expected = "ValueError: value not found in list")]
    fn list_index_missing_panics_with_value_error() {
        let v = vec![1, 2, 3];
        let _ = list_index(&v, &9);
    }
}

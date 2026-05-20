//! Collection helpers for Incan-generated Rust code.
//!
//! This module exists to keep runtime behavior Python-like while avoiding Rust-default panic messages
//! (e.g. Vec/HashMap indexing panics). Instead, we raise canonical `IncanError` messages.

use core::borrow::Borrow;
use core::fmt::Display;
use std::collections::HashMap;
use std::hash::Hash;

use crate::errors::{raise, raise_value_error};
use incan_core::errors::{IncanError, key_not_found_in_dict};
use incan_core::indexing::normalize_slice_bounds;

pub(crate) mod ordinal_map;

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

/// Build a list containing `count` clone-derived copies of `value`.
///
/// This backs Incan's `list.repeat(value, count)` helper. Negative counts are runtime caller errors because the count
/// may be computed dynamically even when the call type-checks.
///
/// ## Panics
/// - `ValueError: list.repeat count must be non-negative, got {count}` if `count < 0`.
#[inline]
#[must_use]
pub fn list_repeat<T: Clone>(value: T, count: i64) -> Vec<T> {
    if count < 0 {
        raise_value_error(&format!("list.repeat count must be non-negative, got {count}"));
    }
    vec![value; count as usize]
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

/// Compiler-only collection helpers used by generated Rust.
#[doc(hidden)]
pub mod __private {
    use super::{IncanError, raise, raise_value_error};

    /// Hash canonical `OrdinalKey` bytes into the signed positive hash domain used by `std.collections.OrdinalMap`.
    #[inline]
    #[must_use]
    pub fn ordinal_key_hash_bytes(data: &[u8]) -> i64 {
        (::xxhash_rust::xxh3::xxh3_64(data) % 9_223_372_036_854_775_807u64) as i64
    }

    /// Return the stable encoding identifier for `str` ordinal keys.
    #[inline]
    #[must_use]
    pub fn ordinal_key_encoding_str() -> String {
        "str:utf8".to_string()
    }

    /// Return the stable encoding identifier for `bytes` ordinal keys.
    #[inline]
    #[must_use]
    pub fn ordinal_key_encoding_bytes() -> String {
        "bytes:raw".to_string()
    }

    /// Return the stable encoding identifier for `bool` ordinal keys.
    #[inline]
    #[must_use]
    pub fn ordinal_key_encoding_bool() -> String {
        "bool:u8".to_string()
    }

    /// Return the stable encoding identifier for `Decimal128` ordinal keys.
    #[inline]
    #[must_use]
    pub fn ordinal_key_encoding_decimal() -> String {
        "decimal128:coefficient-i128-le:scale-u8".to_string()
    }

    /// Return the stable encoding identifier for signed integer ordinal keys.
    #[inline]
    #[must_use]
    pub fn ordinal_key_encoding_int(bits: u16) -> String {
        format!("int:i{bits}:le")
    }

    /// Return the stable encoding identifier for unsigned integer ordinal keys.
    #[inline]
    #[must_use]
    pub fn ordinal_key_encoding_uint(bits: u16) -> String {
        format!("uint:u{bits}:le")
    }

    /// Decode an exact-width little-endian `OrdinalKey` payload.
    #[inline]
    pub fn ordinal_key_exact_bytes<const N: usize>(data: Vec<u8>, encoding: &str) -> Result<[u8; N], String> {
        if data.len() != N {
            return Err(format!("{encoding} OrdinalMap key bytes must be {N} bytes"));
        }
        <[u8; N]>::try_from(data.as_slice())
            .map_err(|_| format!("{encoding} OrdinalMap key bytes could not be decoded"))
    }

    /// Decode a UTF-8 string `OrdinalKey` payload.
    #[inline]
    pub fn ordinal_key_string_from_bytes(data: Vec<u8>) -> Result<String, String> {
        String::from_utf8(data).map_err(|err| err.to_string())
    }

    /// Decode a boolean `OrdinalKey` payload.
    #[inline]
    pub fn ordinal_key_bool_from_bytes(data: Vec<u8>) -> Result<bool, String> {
        if data.len() != 1 {
            return Err(format!(
                "{} OrdinalMap key bytes must be 1 byte",
                ordinal_key_encoding_bool()
            ));
        }
        match data[0] {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(format!(
                "{} OrdinalMap key byte must be 0 or 1",
                ordinal_key_encoding_bool()
            )),
        }
    }

    /// Encode a decimal `OrdinalKey` into its canonical coefficient/scale payload.
    #[inline]
    #[must_use]
    pub fn ordinal_key_decimal_bytes(value: &crate::num::Decimal128) -> [u8; 17] {
        let mut out = [0u8; 17];
        out[0..16].copy_from_slice(&value.coefficient().to_le_bytes());
        out[16] = value.scale();
        out
    }

    /// Decode a decimal `OrdinalKey` payload.
    #[inline]
    pub fn ordinal_key_decimal_from_bytes(data: Vec<u8>) -> Result<crate::num::Decimal128, String> {
        let encoding = ordinal_key_encoding_decimal();
        if data.len() != 17 {
            return Err(format!("{encoding} OrdinalMap key bytes must be 17 bytes"));
        }
        let coefficient_bytes = <[u8; 16]>::try_from(&data[0..16])
            .map_err(|_| format!("{encoding} OrdinalMap coefficient bytes could not be decoded"))?;
        let scale = data[16];
        if scale > 38 {
            return Err(format!("{encoding} OrdinalMap scale byte must be <= 38"));
        }
        Ok(crate::num::Decimal128::new(
            i128::from_le_bytes(coefficient_bytes),
            scale,
        ))
    }

    /// Pop the last element from a list.
    ///
    /// This preserves Incan's `list.pop()` return type (`T`, not `Option<T>`) while keeping the empty-list failure on
    /// the runtime side instead of in compiler-emitted extraction code.
    ///
    /// ## Panics
    /// - `IndexError: pop from empty list` if the list is empty.
    #[inline]
    #[must_use]
    pub fn list_pop<T>(list: &mut Vec<T>) -> T {
        match list.pop() {
            Some(value) => value,
            None => raise(IncanError::list_pop_empty()),
        }
    }

    /// Return the minimum float value in a list.
    ///
    /// ## Panics
    /// - `ValueError: min() arg is an empty sequence` if the list is empty.
    #[inline]
    #[must_use]
    pub fn list_min_f64(list: &[f64]) -> f64 {
        match list.iter().copied().reduce(f64::min) {
            Some(value) => value,
            None => raise_value_error("min() arg is an empty sequence"),
        }
    }

    /// Return the maximum float value in a list.
    ///
    /// ## Panics
    /// - `ValueError: max() arg is an empty sequence` if the list is empty.
    #[inline]
    #[must_use]
    pub fn list_max_f64(list: &[f64]) -> f64 {
        match list.iter().copied().reduce(f64::max) {
            Some(value) => value,
            None => raise_value_error("max() arg is an empty sequence"),
        }
    }

    /// Return the minimum copied value in a list.
    ///
    /// ## Panics
    /// - `ValueError: min() arg is an empty sequence` if the list is empty.
    #[inline]
    #[must_use]
    pub fn list_min_copy<T>(list: &[T]) -> T
    where
        T: Copy + Ord,
    {
        match list.iter().min() {
            Some(value) => *value,
            None => raise_value_error("min() arg is an empty sequence"),
        }
    }

    /// Return the maximum copied value in a list.
    ///
    /// ## Panics
    /// - `ValueError: max() arg is an empty sequence` if the list is empty.
    #[inline]
    #[must_use]
    pub fn list_max_copy<T>(list: &[T]) -> T
    where
        T: Copy + Ord,
    {
        match list.iter().max() {
            Some(value) => *value,
            None => raise_value_error("max() arg is an empty sequence"),
        }
    }

    /// Return the minimum cloned value in a list.
    ///
    /// ## Panics
    /// - `ValueError: min() arg is an empty sequence` if the list is empty.
    #[inline]
    #[must_use]
    pub fn list_min_clone<T>(list: &[T]) -> T
    where
        T: Clone + Ord,
    {
        match list.iter().min() {
            Some(value) => value.clone(),
            None => raise_value_error("min() arg is an empty sequence"),
        }
    }

    /// Return the maximum cloned value in a list.
    ///
    /// ## Panics
    /// - `ValueError: max() arg is an empty sequence` if the list is empty.
    #[inline]
    #[must_use]
    pub fn list_max_clone<T>(list: &[T]) -> T
    where
        T: Clone + Ord,
    {
        match list.iter().max() {
            Some(value) => value.clone(),
            None => raise_value_error("max() arg is an empty sequence"),
        }
    }
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
    fn list_pop_returns_last_value() {
        let mut v = vec![10, 20, 30];
        assert_eq!(__private::list_pop(&mut v), 30);
        assert_eq!(v, vec![10, 20]);
    }

    #[test]
    fn ordinal_key_encoding_helpers_are_stable() {
        assert_eq!(__private::ordinal_key_encoding_str(), "str:utf8");
        assert_eq!(__private::ordinal_key_encoding_bytes(), "bytes:raw");
        assert_eq!(__private::ordinal_key_encoding_bool(), "bool:u8");
        assert_eq!(
            __private::ordinal_key_encoding_decimal(),
            "decimal128:coefficient-i128-le:scale-u8"
        );
        assert_eq!(__private::ordinal_key_encoding_int(32), "int:i32:le");
        assert_eq!(__private::ordinal_key_encoding_uint(64), "uint:u64:le");
    }

    #[test]
    #[should_panic(expected = "IndexError: pop from empty list")]
    fn list_pop_empty_panics_with_index_error() {
        let mut v: Vec<i64> = Vec::new();
        let _ = __private::list_pop(&mut v);
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
    fn list_repeat_clones_values() {
        let repeated = list_repeat("seed".to_string(), 3);
        assert_eq!(
            repeated,
            vec!["seed".to_string(), "seed".to_string(), "seed".to_string()]
        );
    }

    #[test]
    fn list_repeat_zero_returns_empty_list() {
        let repeated = list_repeat(42, 0);
        assert_eq!(repeated, Vec::<i32>::new());
    }

    #[test]
    #[should_panic(expected = "ValueError: list.repeat count must be non-negative, got -2")]
    fn list_repeat_negative_count_panics_with_value_error() {
        let _ = list_repeat("x", -2);
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
    fn list_min_max_helpers_return_expected_values() {
        assert_eq!(__private::list_min_copy(&[4, 2, 8]), 2);
        assert_eq!(__private::list_max_copy(&[4, 2, 8]), 8);
        assert_eq!(
            __private::list_min_clone(&["pear".to_string(), "apple".to_string()]),
            "apple"
        );
        assert_eq!(
            __private::list_max_clone(&["pear".to_string(), "apple".to_string()]),
            "pear"
        );
        assert_eq!(__private::list_min_f64(&[4.0, 2.5, 8.0]), 2.5);
        assert_eq!(__private::list_max_f64(&[4.0, 2.5, 8.0]), 8.0);
    }

    #[test]
    #[should_panic(expected = "ValueError: min() arg is an empty sequence")]
    fn list_min_copy_empty_panics_with_value_error() {
        let empty: [i64; 0] = [];
        let _ = __private::list_min_copy(&empty);
    }

    #[test]
    #[should_panic(expected = "ValueError: max() arg is an empty sequence")]
    fn list_max_clone_empty_panics_with_value_error() {
        let empty: Vec<String> = Vec::new();
        let _ = __private::list_max_clone(&empty);
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

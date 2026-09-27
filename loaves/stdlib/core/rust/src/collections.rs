//! Collection helpers for Incan-generated Rust code.
//!
//! This module exists to keep runtime behavior Python-like while avoiding Rust-default panic messages (e.g. Vec/HashMap
//! indexing panics). Instead, we raise canonical `IncanError` messages.

use core::borrow::Borrow;
use core::fmt::Display;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use crate::errors::{raise, raise_value_error};
use crate::frozen::FrozenDict;
use incan_lang::errors::{IncanError, key_not_found_in_dict};
use incan_lang::indexing::normalize_slice_bounds;

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

/// Whether a list contains a value, by equality.
///
/// Backs Incan's `value in list`. The arguments are `(haystack, needle)`, the reverse of the source spelling, so
/// every containment helper here agrees with `str_contains` and with `contains` throughout Rust; Body IR swaps the
/// operands at the call site so the call matches the function it names.
#[inline]
#[must_use]
pub fn list_contains<T: PartialEq>(list: &[T], value: &T) -> bool {
    list.contains(value)
}

/// Whether a list does *not* contain a value, by equality.
///
/// A real function rather than a negated [`list_contains`] call, mirroring the `str_contains`/`str_not_contains`
/// pair. `value not in list` is its own operator in the source and its own operation in Body IR, so it stays one
/// call here too instead of becoming a negation a reader has to unwrap.
#[inline]
#[must_use]
pub fn list_not_contains<T: PartialEq>(list: &[T], value: &T) -> bool {
    !list.contains(value)
}

/// Whether a set contains a value.
///
/// Takes `&HashSet` rather than a slice because the point of the set is the hashed lookup; accepting a slice would
/// quietly turn `value in set` into a linear scan.
#[inline]
#[must_use]
pub fn set_contains<T: Eq + Hash>(set: &HashSet<T>, value: &T) -> bool {
    set.contains(value)
}

/// Whether a set does *not* contain a value, as its own operation for the reason given on [`list_not_contains`].
#[inline]
#[must_use]
pub fn set_not_contains<T: Eq + Hash>(set: &HashSet<T>, value: &T) -> bool {
    !set.contains(value)
}

/// Whether a dict contains a **key**.
///
/// Dict membership tests keys, not values: `key in dict` asks whether the mapping has an entry for `key`. The name
/// says so rather than leaving it to be inferred from the receiver, which is the same reason Body IR names the
/// operation `DictContainsKey`.
#[inline]
#[must_use]
pub fn dict_contains_key<K: Eq + Hash, V>(dict: &HashMap<K, V>, key: &K) -> bool {
    dict.contains_key(key)
}

/// Whether a dict lacks an entry for a key, as its own operation for the reason given on [`list_not_contains`].
#[inline]
#[must_use]
pub fn dict_not_contains_key<K: Eq + Hash, V>(dict: &HashMap<K, V>, key: &K) -> bool {
    !dict.contains_key(key)
}

/// Compiler-only collection helpers used by generated Rust.
#[doc(hidden)]
pub mod __private {
    use super::{IncanError, raise, raise_value_error};

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

/// Get a frozen dict value by key (Python-style `d[key]` on a `const` `FrozenDict`).
///
/// The probe borrows the way [`dict_get`]'s does, so string-keyed frozen dicts accept literal and runtime `str`
/// probes alike.
///
/// ## Panics
/// - `KeyError: '{key}' not found in dict` if missing.
#[inline]
pub fn frozen_dict_get<K, Q, V>(map: &FrozenDict<K, V>, key: &Q) -> &'static V
where
    K: Borrow<Q>,
    Q: PartialEq + Display + ?Sized,
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
    fn containment_helpers_answer_membership_for_each_container() {
        let list = vec![10, 20, 30];
        assert!(list_contains(&list, &20));
        assert!(!list_contains(&list, &40));
        assert!(list_not_contains(&list, &40));
        assert!(!list_not_contains(&list, &20));

        let set: HashSet<i64> = [1, 2, 3].into_iter().collect();
        assert!(set_contains(&set, &2));
        assert!(!set_contains(&set, &9));
        assert!(set_not_contains(&set, &9));
        assert!(!set_not_contains(&set, &2));

        let mut dict: HashMap<String, i64> = HashMap::new();
        dict.insert("a".to_string(), 1);
        assert!(dict_contains_key(&dict, &"a".to_string()));
        assert!(!dict_contains_key(&dict, &"b".to_string()));
        assert!(dict_not_contains_key(&dict, &"b".to_string()));
        assert!(!dict_not_contains_key(&dict, &"a".to_string()));
    }

    #[test]
    fn dict_membership_tests_keys_rather_than_values() {
        // The distinction the helper is named for: a value present in the mapping is not membership.
        let mut dict: HashMap<String, i64> = HashMap::new();
        dict.insert("key".to_string(), 42);

        assert!(dict_contains_key(&dict, &"key".to_string()));
        assert!(!dict_contains_key(&dict, &"42".to_string()));
    }

    #[test]
    fn containment_helpers_hold_for_empty_containers() {
        let list: Vec<i64> = Vec::new();
        assert!(!list_contains(&list, &1));
        assert!(list_not_contains(&list, &1));

        let set: HashSet<i64> = HashSet::new();
        assert!(!set_contains(&set, &1));
        assert!(set_not_contains(&set, &1));

        let dict: HashMap<String, i64> = HashMap::new();
        assert!(!dict_contains_key(&dict, &"a".to_string()));
        assert!(dict_not_contains_key(&dict, &"a".to_string()));
    }

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

    const TABLE: FrozenDict<&'static str, i64> = FrozenDict::new(&[("names", 2), ("empty", 0)]);

    /// `table[key]` on a `const` `FrozenDict` reads the value through a `str` probe of any lifetime (#1757).
    #[test]
    fn frozen_dict_get_reads_the_value_for_a_text_probe() {
        let probe = String::from("names");
        assert_eq!(*frozen_dict_get(&TABLE, probe.as_str()), 2);
        assert_eq!(*frozen_dict_get(&TABLE, "empty"), 0);
    }

    /// A missing key raises the same `KeyError` a dict lookup raises (#1757).
    #[test]
    #[should_panic(expected = "KeyError")]
    fn frozen_dict_get_missing_key_raises_key_error() {
        let _ = frozen_dict_get(&TABLE, "missing");
    }
}

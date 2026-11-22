//! HashSet and HashMap operations for checked collection plans, with the legacy missing-key and cursor errors.

use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::hash::Hash;

use incan_std_primitives::collections::{
    dict_contains_key, dict_get, dict_get_mut, normalize_list_index, set_contains,
};
use incan_std_primitives::errors::raise_index_error;
use incan_std_primitives::iter::nonnegative_count;

/// Compare hashed membership without copying either set.
pub fn equal_set<T: Eq + Hash>(left: &HashSet<T>, right: &HashSet<T>) -> bool {
    left == right
}

/// Unequal hashed sets.
pub fn unequal_set<T: Eq + Hash>(left: &HashSet<T>, right: &HashSet<T>) -> bool {
    left != right
}

/// Matching keys and values.
pub fn equal_dict<K: Eq + Hash, V: PartialEq>(left: &HashMap<K, V>, right: &HashMap<K, V>) -> bool {
    left == right
}

/// Unequal dictionaries.
pub fn unequal_dict<K: Eq + Hash, V: PartialEq>(left: &HashMap<K, V>, right: &HashMap<K, V>) -> bool {
    left != right
}

/// Empty hash set storage.
pub fn empty_set<T: Eq + Hash>() -> HashSet<T> {
    HashSet::new()
}

/// Insert one owned element, eliminating duplicates.
pub fn insert_set<T: Eq + Hash>(values: &mut HashSet<T>, value: T) {
    values.insert(value);
}

/// The set's size.
pub fn length_set<T>(values: &HashSet<T>) -> i64 {
    values.len() as i64
}

/// Set membership through the legacy helper.
pub fn contains_set<T: Eq + Hash>(values: &HashSet<T>, value: T) -> bool {
    set_contains(values, &value)
}

/// Copy a set when the checked read requires an owned copy.
pub fn clone_set<T: Clone>(values: &HashSet<T>) -> HashSet<T> {
    values.clone()
}

/// Empty hash map storage.
pub fn empty_dict<K: Eq + Hash, V>() -> HashMap<K, V> {
    HashMap::new()
}

/// Insert or overwrite a key.
pub fn insert_dict<K: Eq + Hash, V>(values: &mut HashMap<K, V>, key: K, value: V) {
    values.insert(key, value);
}

/// The dictionary's size.
pub fn length_dict<K, V>(values: &HashMap<K, V>) -> i64 {
    values.len() as i64
}

/// Key membership through the legacy helper.
pub fn contains_dict<K: Eq + Hash, V>(values: &HashMap<K, V>, key: K) -> bool {
    dict_contains_key(values, &key)
}

/// Copy an owned dictionary for a value-preserving read.
pub fn clone_dict<K: Clone, V: Clone>(values: &HashMap<K, V>) -> HashMap<K, V> {
    values.clone()
}

/// Index a dictionary with the legacy missing-key diagnostic.
pub fn get_dict<K: Eq + Hash + Display, V: Clone>(values: &HashMap<K, V>, key: K) -> V {
    dict_get(values, &key).clone()
}

/// Borrow a stored value in place through the legacy missing-key diagnostic.
pub fn borrow_mut_dict<K: Eq + Hash + Display, V>(values: &mut HashMap<K, V>, key: K) -> &mut V {
    dict_get_mut(values, &key)
}

/// Copy a present value or report its absence.
pub fn optional_dict<K: Eq + Hash + Display, V: Clone>(values: &HashMap<K, V>, key: K) -> Option<V> {
    values.get(&key).cloned()
}

/// Copy keys in native order.
pub fn keys_dict<K: Clone, V>(values: &HashMap<K, V>) -> Vec<K> {
    values.keys().cloned().collect()
}

/// Copy values in native order.
pub fn values_dict<K: Eq + Hash + Clone + Display, V: Clone>(values: &HashMap<K, V>) -> Vec<V> {
    values.values().cloned().collect()
}

/// Copy the stored value, or return the eagerly evaluated default.
pub fn default_dict<K: Eq + Hash + Display, V: Clone>(values: &HashMap<K, V>, key: K, default: V) -> V {
    match values.get(&key) {
        Some(value) => value.clone(),
        None => default,
    }
}

/// Copy one element in the live HashSet order; lowering proves the cursor in bounds.
pub fn nth_set<T: Clone>(values: &HashSet<T>, index: i64) -> T {
    match values.iter().nth(nonnegative_count(index)) {
        Some(value) => value.clone(),
        None => raise_index_error("set cursor outside collection"),
    }
}

/// Clone the selected key in native order, with canonical signed index bounds and errors.
pub fn nth_dict<K: Clone, V>(values: &HashMap<K, V>, index: i64) -> K {
    let position = normalize_list_index(values.len(), index);
    match values.keys().nth(position) {
        Some(key) => key.clone(),
        None => raise_index_error("dictionary cursor outside its checked collection bounds"),
    }
}

/// Insert list elements in their source order.
pub fn extend_list_set<T: Eq + Hash + Clone>(values: &mut HashSet<T>, source: &Vec<T>) {
    for value in source {
        values.insert(value.clone());
    }
}

/// Insert a source set's elements.
pub fn extend_set<T: Eq + Hash + Clone>(values: &mut HashSet<T>, source: &HashSet<T>) {
    for value in source {
        values.insert(value.clone());
    }
}

/// Apply a mapping spread, overwriting earlier destination keys.
pub fn extend_dict<K: Eq + Hash + Clone + Display, V: Clone>(values: &mut HashMap<K, V>, source: &HashMap<K, V>) {
    for (key, item) in source {
        values.insert(key.clone(), item.clone());
    }
}

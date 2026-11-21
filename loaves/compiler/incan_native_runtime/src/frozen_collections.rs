//! Read-only observations and owned snapshots of the static frozen carriers, in baked order.

use std::borrow::Borrow;
use std::fmt::Display;

use incan_std_primitives::errors::raise_key_error;
use incan_std_primitives::frozen::{FrozenDict, FrozenList, FrozenSet};

/// Keep a list computed once, a computed constant's value, as frozen storage for the rest of the program.
pub fn freeze_list<T: 'static>(values: Vec<T>) -> FrozenList<T> {
    FrozenList::leak(values)
}

/// Measure static list storage.
pub fn frozen_list_length<T: 'static>(value: FrozenList<T>) -> i64 {
    value.len() as i64
}

/// Measure borrowed static list storage.
pub fn frozen_list_length_ref<T: 'static>(value: &FrozenList<T>) -> i64 {
    value.len() as i64
}

/// Measure borrowed static set storage.
pub fn frozen_set_length_ref<T: 'static>(value: &FrozenSet<T>) -> i64 {
    value.len() as i64
}

/// Measure borrowed static dictionary storage, duplicate keys included.
pub fn frozen_dict_length_ref<K: 'static, V: 'static>(value: &FrozenDict<K, V>) -> i64 {
    value.len() as i64
}

/// Owned elements in baked order through their Clone implementations.
pub fn frozen_list_items<T: Clone + 'static>(value: FrozenList<T>) -> Vec<T> {
    value.as_slice().to_vec()
}

/// Each element's display text in baked order.
pub fn frozen_list_text_items<T: Display + 'static>(value: FrozenList<T>) -> Vec<String> {
    value.as_slice().iter().map(ToString::to_string).collect()
}

/// Owned byte buffers copied from each static byte element.
pub fn frozen_list_byte_items<Source: Clone + Into<Vec<u8>> + 'static>(value: FrozenList<Source>) -> Vec<Vec<u8>> {
    value.as_slice().iter().map(|item| item.clone().into()).collect()
}

/// Copy a static byte slice into the owned buffer a frozen element read returns.
pub fn own_static_bytes<Source: Into<Vec<u8>>>(value: Source) -> Vec<u8> {
    value.into()
}

/// Measure baked set storage.
pub fn frozen_set_length<T: 'static>(value: FrozenSet<T>) -> i64 {
    value.len() as i64
}

/// Owned set elements in baked order.
pub fn frozen_set_items<T: Clone + 'static>(value: FrozenSet<T>) -> Vec<T> {
    value.as_slice().to_vec()
}

/// Set elements' display text in baked order.
pub fn frozen_set_text_items<T: Display + 'static>(value: FrozenSet<T>) -> Vec<String> {
    value.as_slice().iter().map(ToString::to_string).collect()
}

/// Measure baked entries, including duplicate-key storage.
pub fn frozen_dict_length<K: 'static, V: 'static>(value: FrozenDict<K, V>) -> i64 {
    value.len() as i64
}

/// Copy keys in baked order without reading values.
pub fn frozen_dict_keys<K: Clone + 'static, V: 'static>(value: FrozenDict<K, V>) -> Vec<K> {
    value.as_slice().iter().map(|(key, _)| key.clone()).collect()
}

/// Keys' display text in baked order.
pub fn frozen_dict_text_keys<K: Display + 'static, V: 'static>(value: FrozenDict<K, V>) -> Vec<String> {
    value.as_slice().iter().map(|(key, _)| key.to_string()).collect()
}

/// Find a value by borrowed text identity, cloning only the selected result, or raise KeyError.
pub fn frozen_dict_text_get<K: Borrow<str> + 'static, V: Clone + 'static>(value: FrozenDict<K, V>, key: &String) -> V {
    match value.get::<str>(key.as_str()) {
        Some(item) => item.clone(),
        None => raise_key_error(&format!("'{key}' not found in dict")),
    }
}

/// Copy the first value whose scalar key equals `key`, or raise KeyError.
pub fn frozen_dict_get<K: PartialEq + Display + 'static, V: Clone + 'static>(value: FrozenDict<K, V>, key: K) -> V {
    match value.as_slice().iter().find(|(stored, _)| *stored == key) {
        Some((_, item)) => item.clone(),
        None => raise_key_error(&format!("'{key}' not found in dict")),
    }
}

/// Test a borrowed text key.
pub fn frozen_dict_text_contains<K: Borrow<str> + 'static, V: 'static>(value: FrozenDict<K, V>, key: &String) -> bool {
    value.contains_key::<str>(key.as_str())
}

/// Test a scalar key, stopping at the first equal key.
pub fn frozen_dict_contains<K: PartialEq + 'static, V: 'static>(value: FrozenDict<K, V>, key: K) -> bool {
    value.as_slice().iter().any(|(stored, _)| *stored == key)
}

//! List operations over the `Vec` representation, with Python indexing and the legacy runtime errors.

use std::cmp::Ordering;

use incan_std_primitives::collections::__private::list_pop;
use incan_std_primitives::collections::{
    list_concat, list_contains, list_count, list_extend, list_get, list_get_mut, list_index, list_remove, list_repeat,
    list_slice, normalize_list_index,
};

use crate::strings::optional_index;

/// Compare elements without cloning either list.
pub fn equal_list<T: PartialEq>(left: &Vec<T>, right: &Vec<T>) -> bool {
    left == right
}

/// Borrowed list inequality.
pub fn unequal_list<T: PartialEq>(left: &Vec<T>, right: &Vec<T>) -> bool {
    left != right
}

/// Allocate empty list storage.
pub fn empty_list<T>() -> Vec<T> {
    Vec::new()
}

/// Append to the caller's list storage.
pub fn append_list<T>(values: &mut Vec<T>, value: T) {
    values.push(value);
}

/// The list's length.
pub fn length_list<T>(values: &Vec<T>) -> i64 {
    values.len() as i64
}

/// Read a copy of an element with Python indices and the legacy IndexError.
pub fn get_list<T: Clone>(values: &Vec<T>, index: i64) -> T {
    list_get(values, index).clone()
}

/// Borrow an element in place through the negative-index and bounds checks.
pub fn borrow_list_mut<T>(values: &mut Vec<T>, index: i64) -> &mut T {
    list_get_mut(values, index)
}

/// Replace an element through a mutable list borrow.
pub fn set_list<T>(values: &mut Vec<T>, index: i64, value: T) {
    *list_get_mut(values, index) = value;
}

/// Duplicate a list for a value-preserving read.
pub fn clone_list<T: Clone>(values: &Vec<T>) -> Vec<T> {
    values.clone()
}

/// Membership with the list's element equality.
pub fn contains_list<T: PartialEq>(values: &Vec<T>, value: T) -> bool {
    list_contains(values, &value)
}

/// Count matching elements.
pub fn count_list<T: PartialEq>(values: &Vec<T>, value: T) -> i64 {
    list_count(values, &value)
}

/// The first matching position, or the legacy missing-value error.
pub fn index_list<T: PartialEq>(values: &Vec<T>, value: T) -> i64 {
    list_index(values, &value)
}

/// Append a copy of the other list's elements.
pub fn extend_list<T: Clone>(values: &mut Vec<T>, other: &Vec<T>) {
    list_extend(values, other);
}

/// Remove the last element, or raise the empty-list error.
pub fn pop_list<T>(values: &mut Vec<T>) -> T {
    list_pop(values)
}

/// Remove one element with Python index normalization.
pub fn remove_list<T>(values: &mut Vec<T>, index: i64) {
    list_remove(values, index);
}

/// Swap two elements through the canonical index normalization and bounds errors.
pub fn swap_list<T>(values: &mut Vec<T>, left: i64, right: i64) {
    let size = values.len();
    let left_index = normalize_list_index(size, left);
    let right_index = normalize_list_index(size, right);
    values.swap(left_index, right_index);
}

/// Concatenate borrowed lists.
pub fn concat_list<T: Clone>(left: &Vec<T>, right: &Vec<T>) -> Vec<T> {
    list_concat(left, right)
}

/// `count` clones of an owned value, refusing a negative count as the legacy helper does.
pub fn repeat_list<T: Clone>(value: T, count: i64) -> Vec<T> {
    list_repeat(value, count)
}

/// A stably sorted fresh snapshot.
pub fn sorted_list<T: Clone + Ord>(values: &Vec<T>) -> Vec<T> {
    let mut owned = values.clone();
    owned.sort();
    owned
}

/// A stably sorted float snapshot, treating an unordered pair as equal.
pub fn sorted_float(values: &Vec<f64>) -> Vec<f64> {
    let mut owned = values.clone();
    owned.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    owned
}

/// Copy a slice with the legacy bounds, steps and errors, retaining omitted endpoints.
pub fn slice_list<T: Clone>(
    values: &Vec<T>,
    has_start: bool,
    start: i64,
    has_end: bool,
    end: i64,
    has_step: bool,
    step: i64,
) -> Vec<T> {
    list_slice(
        values,
        optional_index(has_start, start),
        optional_index(has_end, end),
        optional_index(has_step, step),
    )
}

//! Text operations on borrowed strings, through the canonical string primitives so their semantics stay the legacy
//! ones (Unicode scalar indices, Python slices, and the retained empty pieces of a split).

use incan_std_primitives::strings::{
    str_concat, str_contains, str_ends_with, str_eq, str_ge, str_gt, str_join, str_le, str_len, str_lower, str_lt,
    str_ne, str_replace, str_slice, str_split, str_starts_with, str_strip, str_upper,
};

/// Decode a presence flag without treating an omitted index as zero.
pub(crate) fn optional_index(present: bool, value: i64) -> Option<i64> {
    present.then_some(value)
}

/// Copy a Unicode slice, preserving omitted bounds, steps and errors.
pub fn slice_text(
    value: &String,
    has_start: bool,
    start: i64,
    has_end: bool,
    end: i64,
    has_step: bool,
    step: i64,
) -> String {
    str_slice(
        value,
        optional_index(has_start, start),
        optional_index(has_end, end),
        optional_index(has_step, step),
    )
}

/// Copy a Unicode-scalar slice, preserving both signed endpoints.
pub fn slice_text_bounds(value: &String, start: i64, end: i64) -> String {
    str_slice(value, Some(start), Some(end), None)
}

/// Match a borrowed prefix.
pub fn starts_text(value: &String, prefix: &String) -> bool {
    str_starts_with(value, prefix)
}

/// Match a borrowed suffix.
pub fn ends_text(value: &String, suffix: &String) -> bool {
    str_ends_with(value, suffix)
}

/// Concatenate shared text.
pub fn concat_text(left: &String, right: &String) -> String {
    str_concat(left, right)
}

/// Compare shared text for equality.
pub fn equal_text(left: &String, right: &String) -> bool {
    str_eq(left, right)
}

/// Compare shared text for inequality.
pub fn unequal_text(left: &String, right: &String) -> bool {
    str_ne(left, right)
}

/// Compare shared text in lexical order.
pub fn less_text(left: &String, right: &String) -> bool {
    str_lt(left, right)
}

/// Compare shared text in inclusive lexical order.
pub fn less_equal_text(left: &String, right: &String) -> bool {
    str_le(left, right)
}

/// Compare shared text in reverse lexical order.
pub fn greater_text(left: &String, right: &String) -> bool {
    str_gt(left, right)
}

/// Compare shared text in inclusive reverse lexical order.
pub fn greater_equal_text(left: &String, right: &String) -> bool {
    str_ge(left, right)
}

/// Count Unicode scalars.
pub fn length_text(value: &String) -> i64 {
    str_len(value)
}

/// Uppercase shared text.
pub fn upper_text(value: &String) -> String {
    str_upper(value)
}

/// Lowercase shared text.
pub fn lower_text(value: &String) -> String {
    str_lower(value)
}

/// Strip surrounding whitespace.
pub fn strip_text(value: &String) -> String {
    str_strip(value)
}

/// Replace every occurrence of `old`.
pub fn replace_text(value: &String, old: &String, replacement: &String) -> String {
    str_replace(value, old, replacement)
}

/// Test substring containment.
pub fn contains_text(value: &String, needle: &String) -> bool {
    str_contains(value, needle)
}

/// Test substring absence.
pub fn not_contains_text(value: &String, needle: &String) -> bool {
    !str_contains(value, needle)
}

/// Split at an explicit separator, retaining empty pieces.
pub fn split_text(value: &String, separator: &String) -> Vec<String> {
    str_split(value, Some(separator))
}

/// An omitted separator retains the entire input as one piece.
pub fn split_default_text(value: &String) -> Vec<String> {
    str_split::<&String, &str>(value, None)
}

/// Materialize Rust's Unicode whitespace pieces as owned strings, without empty pieces.
pub fn split_whitespace_text(value: &String) -> Vec<String> {
    value.split_whitespace().map(str::to_owned).collect()
}

/// Join borrowed strings with the separator.
pub fn join_text(separator: &String, values: &Vec<String>) -> String {
    str_join(separator, values)
}

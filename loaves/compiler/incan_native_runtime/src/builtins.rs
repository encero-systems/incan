//! Scalar builtin contracts: truthiness, `abs`, `sum`, extrema, powers and the text file builtins.

use incan_std_primitives::collections::__private::{
    list_max_clone, list_max_copy, list_max_f64, list_min_clone, list_min_copy, list_min_f64,
};
use incan_std_primitives::errors::raise_value_error;

use crate::integer_helpers::integer_power;

/// Take an absolute value, refusing the one unrepresentable minimum with the builtin's overflow message.
pub fn absolute_int(value: i64) -> i64 {
    if value == i64::MIN {
        raise_value_error("integer overflow in builtin `abs`");
    }
    value.wrapping_abs()
}

/// Integers are false exactly at zero.
pub fn truth_int(value: i64) -> bool {
    value != 0
}

/// Floats are false at either signed zero and true for NaN.
pub fn truth_float(value: f64) -> bool {
    value != 0.0
}

/// Boolean conversion preserves its operand.
pub fn truth_bool(value: bool) -> bool {
    value
}

/// Combine already evaluated compiler-owned boolean operands.
pub fn boolean_and(left: bool, right: bool) -> bool {
    left && right
}

/// Combine already evaluated compiler-owned boolean operands.
pub fn boolean_or(left: bool, right: bool) -> bool {
    left || right
}

/// Sum integers in source order, refusing overflow at the first item that causes it.
pub fn sum_int(values: &Vec<i64>) -> i64 {
    let mut total: i64 = 0;
    for value in values {
        total = match total.checked_add(*value) {
            Some(sum) => sum,
            None => raise_value_error("integer overflow in builtin `sum`"),
        };
    }
    total
}

/// Count the true values of a borrowed list.
pub fn sum_bool(values: &Vec<bool>) -> i64 {
    values
        .iter()
        .fold(0_i64, |total, value| if *value { total.wrapping_add(1) } else { total })
}

/// Smallest integer, with the empty-sequence error.
pub fn minimum_int(values: &Vec<i64>) -> i64 {
    list_min_copy(values)
}

/// Largest integer, with the empty-sequence error.
pub fn maximum_int(values: &Vec<i64>) -> i64 {
    list_max_copy(values)
}

/// Smallest float, with the legacy NaN and empty-sequence conventions.
pub fn minimum_float(values: &Vec<f64>) -> f64 {
    list_min_f64(values)
}

/// Largest float, with the legacy NaN and empty-sequence conventions.
pub fn maximum_float(values: &Vec<f64>) -> f64 {
    list_max_f64(values)
}

/// False orders before true.
pub fn minimum_bool(values: &Vec<bool>) -> bool {
    list_min_copy(values)
}

/// True orders after false.
pub fn maximum_bool(values: &Vec<bool>) -> bool {
    list_max_copy(values)
}

/// Clone the smallest borrowed string.
pub fn minimum_text(values: &Vec<String>) -> String {
    list_min_clone(values)
}

/// Clone the largest borrowed string.
pub fn maximum_text(values: &Vec<String>) -> String {
    list_max_clone(values)
}

/// Read UTF-8 text, preserving the native error's display text.
pub fn read_file_text(path: &String) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|error| error.to_string())
}

/// Write UTF-8 text, preserving the native error's display text.
pub fn write_file_text(path: &String, content: &String) -> Result<(), String> {
    std::fs::write(path, content).map_err(|error| error.to_string())
}

/// Empty text is false; every nonempty string is true.
pub fn truth_text(value: &String) -> bool {
    !value.is_empty()
}

/// Test list emptiness without consuming its owner.
pub fn truth_list<T>(value: &Vec<T>) -> bool {
    !value.is_empty()
}

/// Integer power with the low-32-bit exponent and the release runtime's wrapping multiplication.
pub fn power_i64(left: i64, right: i64) -> i64 {
    integer_power(left, right, 1)
}

/// Floating exponentiation without changing NaN or infinity behavior.
pub fn power_f64(left: f64, right: f64) -> f64 {
    left.powf(right)
}

/// Single-precision exponentiation.
pub fn power_f32(left: f32, right: f32) -> f32 {
    left.powf(right)
}

//! Nominal operations through the same Rust derives the generated declarations carry.

use std::fmt::{Debug, Display};

use incan_std_data::json::__private::{parse_or_error, stringify_or_raise};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Render the derived Debug implementation, field order and string escaping included.
pub fn debug_model<T: Debug>(value: &T) -> String {
    format!("{value:?}")
}

/// Format an owned carrier with its Debug spelling.
pub fn debug_value<T: Debug>(value: T) -> String {
    format!("{value:?}")
}

/// Format a value through its own Display, as generic code does, integral floats included.
pub fn display_value<T: Display>(value: T) -> String {
    format!("{value}")
}

/// A type parameter's Display, without the concrete float adapter.
pub fn display_type_parameter<T: Display>(value: T) -> String {
    display_value(value)
}

/// Derived equality of borrowed models.
pub fn equal_model<T: PartialEq>(left: &T, right: &T) -> bool {
    left == right
}

/// Derived inequality of borrowed models.
pub fn unequal_model<T: PartialEq>(left: &T, right: &T) -> bool {
    left != right
}

/// Derived partial ordering: less than.
pub fn less_model<T: PartialOrd>(left: &T, right: &T) -> bool {
    left < right
}

/// Derived partial ordering: less than or equal, unordered fields included.
pub fn less_equal_model<T: PartialOrd>(left: &T, right: &T) -> bool {
    left <= right
}

/// Derived partial ordering: greater than.
pub fn greater_model<T: PartialOrd>(left: &T, right: &T) -> bool {
    left > right
}

/// Derived partial ordering: greater than or equal.
pub fn greater_equal_model<T: PartialOrd>(left: &T, right: &T) -> bool {
    left >= right
}

/// serde's stringify with the structured JSON error boundary.
pub fn serialize_model<T: Serialize>(value: &T) -> String {
    stringify_or_raise(value, std::any::type_name::<T>())
}

/// Parse through serde with the derived `from_json` error text.
pub fn deserialize_model<T: DeserializeOwned>(text: &String) -> Result<T, String> {
    parse_or_error(text)
}

/// Serialize the canonical integer.
pub fn serialize_int(value: i64) -> String {
    stringify_or_raise(&value, std::any::type_name::<i64>())
}

/// Serialize a float with serde's spelling and nonfinite-value policy.
pub fn serialize_float(value: f64) -> String {
    stringify_or_raise(&value, std::any::type_name::<f64>())
}

/// Serialize a boolean literal.
pub fn serialize_bool(value: bool) -> String {
    stringify_or_raise(&value, std::any::type_name::<bool>())
}

/// Serialize borrowed text with serde's escaping.
pub fn serialize_string(value: &String) -> String {
    stringify_or_raise(value, std::any::type_name::<String>())
}

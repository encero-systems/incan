//! JSON serialization support for Incan types.
//!
//! Provides convenient wrappers around serde_json for types that implement
//! `Serialize` and `Deserialize`.

use crate::errors::raise_json_serialization_error;
use serde::{Deserialize, Serialize};
use std::error::Error;

/// Trait for types that can be serialized to JSON.
///
/// This is automatically implemented for any type that implements `serde::Serialize`.
pub trait ToJson: Serialize {
    /// Serializes this value to a JSON string.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails.
    fn to_json(&self) -> Result<String, Box<dyn Error>> {
        serde_json::to_string(self).map_err(|e| Box::new(e) as Box<dyn Error>)
    }

    /// Serializes this value to a pretty-printed JSON string.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails.
    fn to_json_pretty(&self) -> Result<String, Box<dyn Error>> {
        serde_json::to_string_pretty(self).map_err(|e| Box::new(e) as Box<dyn Error>)
    }
}

/// Trait for types that can be deserialized from JSON.
///
/// This is automatically implemented for any type that implements `serde::Deserialize`.
pub trait FromJson: for<'de> Deserialize<'de> {
    /// Deserializes a value from a JSON string.
    ///
    /// # Errors
    ///
    /// Returns an error if deserialization fails.
    fn from_json(json: &str) -> Result<Self, Box<dyn Error>>
    where
        Self: Sized,
    {
        serde_json::from_str(json).map_err(|e| Box::new(e) as Box<dyn Error>)
    }
}

// Blanket implementations for all types that implement the required serde traits
impl<T: Serialize> ToJson for T {}
impl<T: for<'de> Deserialize<'de>> FromJson for T {}

/// Serialize a value to JSON or raise Incan's canonical runtime error.
///
/// This is used by compiler-generated code for value-returning JSON paths (`json_stringify`, synthesized `to_json`,
/// and trait-backed wrappers) so the emitted Rust does not inline fallback/panic extraction.
#[must_use]
pub fn stringify_or_raise<T>(value: &T, type_name: &str) -> String
where
    T: Serialize + ?Sized,
{
    match serde_json::to_string(value) {
        Ok(json) => json,
        Err(_) => raise_json_serialization_error(type_name),
    }
}

/// Serialize a value to pretty JSON or raise Incan's canonical runtime error.
#[must_use]
pub fn stringify_pretty_or_raise<T>(value: &T, type_name: &str) -> String
where
    T: Serialize + ?Sized,
{
    match serde_json::to_string_pretty(value) {
        Ok(json) => json,
        Err(_) => raise_json_serialization_error(type_name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serializer;
    use serde::ser::Error as _;

    struct AlwaysFails;

    impl Serialize for AlwaysFails {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            Err(S::Error::custom("forced failure"))
        }
    }

    #[test]
    fn stringify_or_raise_serializes_successfully() {
        assert_eq!(stringify_or_raise(&vec![1, 2, 3], "Vec"), "[1,2,3]");
    }

    #[test]
    #[should_panic(expected = "TypeError: Object of type AlwaysFails is not JSON serializable")]
    fn stringify_or_raise_uses_canonical_runtime_error() {
        let _ = stringify_or_raise(&AlwaysFails, "AlwaysFails");
    }
}

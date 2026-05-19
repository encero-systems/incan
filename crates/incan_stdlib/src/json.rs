//! JSON serialization support and dynamic JSON values for Incan programs.
//!
//! The typed model path (`std.serde.json`) continues to use [`ToJson`] and [`FromJson`]. The dynamic `std.json`
//! source API lives in `stdlib/json.incn`; this module provides the compiler/runtime carrier, parse/stringify
//! boundary, and serde interop needed for that source API to participate in generated JSON model flows.

use crate::errors::{json_decode_error_string, raise_json_serialization_error};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::error::Error;
use std::fmt::{self, Display};

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

// Blanket implementations for all types that implement the required serde traits.
impl<T: Serialize> ToJson for T {}
impl<T: for<'de> Deserialize<'de>> FromJson for T {}

/// Stable category for JSON-specific runtime failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonErrorKind {
    /// JSON parse failure.
    Parse,
    /// Runtime type-shape failure.
    Type,
    /// Object-key lookup failure.
    Key,
    /// Array-index lookup failure.
    Index,
    /// Numeric representation failure.
    Number,
}

impl JsonErrorKind {
    /// Return the parse kind.
    #[must_use]
    pub fn parse() -> Self {
        Self::Parse
    }

    /// Return the type kind.
    #[must_use]
    pub fn type_() -> Self {
        Self::Type
    }

    /// Return the key kind.
    #[must_use]
    pub fn key() -> Self {
        Self::Key
    }

    /// Return the index kind.
    #[must_use]
    pub fn index() -> Self {
        Self::Index
    }

    /// Return the number kind.
    #[must_use]
    pub fn number() -> Self {
        Self::Number
    }

    /// Return the stable source-facing kind spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Parse => "parse",
            Self::Type => "type",
            Self::Key => "key",
            Self::Index => "index",
            Self::Number => "number",
        }
    }
}

impl Display for JsonErrorKind {
    /// Format the stable JSON error-kind spelling.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error payload used by the dynamic `std.json` runtime bridge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonError {
    kind: JsonErrorKind,
    detail: String,
}

impl JsonError {
    /// Return a JSON parse error.
    #[must_use]
    fn parse(detail: impl Into<String>) -> Self {
        Self {
            kind: JsonErrorKind::Parse,
            detail: detail.into(),
        }
    }

    /// Return a type-shape error.
    #[must_use]
    fn type_error(detail: impl Into<String>) -> Self {
        Self {
            kind: JsonErrorKind::Type,
            detail: detail.into(),
        }
    }

    /// Return an array-index lookup error.
    #[must_use]
    fn index_error(detail: impl Into<String>) -> Self {
        Self {
            kind: JsonErrorKind::Index,
            detail: detail.into(),
        }
    }

    /// Return a numeric conversion or representation error.
    #[must_use]
    fn number_error(detail: impl Into<String>) -> Self {
        Self {
            kind: JsonErrorKind::Number,
            detail: detail.into(),
        }
    }

    /// Return the stable error category.
    #[must_use]
    pub fn kind(&self) -> JsonErrorKind {
        self.kind
    }

    /// Return the stable source-facing error category spelling.
    #[must_use]
    pub fn kind_name(&self) -> &'static str {
        self.kind.as_str()
    }

    /// Return the human-readable detail without a diagnostic prefix.
    #[must_use]
    pub fn detail(&self) -> String {
        self.detail.clone()
    }

    /// Return the canonical runtime diagnostic text for this error.
    #[must_use]
    pub fn message(&self) -> String {
        match self.kind {
            JsonErrorKind::Parse => format!("JSONDecodeError: {}", self.detail),
            JsonErrorKind::Type => format!("TypeError: {}", self.detail),
            JsonErrorKind::Key => format!("KeyError: {}", self.detail),
            JsonErrorKind::Index => format!("IndexError: {}", self.detail),
            JsonErrorKind::Number => format!("ValueError: {}", self.detail),
        }
    }
}

impl Display for JsonError {
    /// Format the canonical runtime diagnostic message.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl Error for JsonError {}

/// Dynamic JSON value carrier used by Incan's `std.json.JsonValue`.
///
/// This wrapper serializes and deserializes as its inner JSON value so it can be used directly in `@derive(json)`
/// model fields.
#[derive(Clone, Debug, PartialEq)]
pub struct JsonValue(serde_json::Value);

impl Serialize for JsonValue {
    /// Serialize as the contained JSON value, not as a wrapper object.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for JsonValue {
    /// Deserialize from any JSON value after validating Incan numeric representation rules.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        Self::from_serde(value).map_err(serde::de::Error::custom)
    }
}

impl Display for JsonValue {
    /// Format as compact JSON text when serialization succeeds.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match serde_json::to_string(&self.0) {
            Ok(value) => f.write_str(&value),
            Err(_) => f.write_str("<invalid json>"),
        }
    }
}

impl JsonValue {
    /// Construct JSON null.
    #[must_use]
    pub fn null() -> Self {
        Self(serde_json::Value::Null)
    }

    /// Construct a JSON boolean.
    #[must_use]
    pub fn bool(value: bool) -> Self {
        Self(serde_json::Value::Bool(value))
    }

    /// Construct a JSON integer.
    #[must_use]
    pub fn int(value: i64) -> Self {
        Self(serde_json::Value::Number(serde_json::Number::from(value)))
    }

    /// Construct a JSON finite floating-point number.
    pub fn float(value: f64) -> Result<Self, JsonError> {
        match serde_json::Number::from_f64(value) {
            Some(number) => Ok(Self(serde_json::Value::Number(number))),
            None => Err(JsonError::number_error("JSON float must be finite")),
        }
    }

    /// Construct a JSON string.
    #[must_use]
    pub fn string(value: String) -> Self {
        Self(serde_json::Value::String(value))
    }

    /// Construct a JSON array.
    #[must_use]
    pub fn array(values: Vec<JsonValue>) -> Self {
        Self(serde_json::Value::Array(
            values.into_iter().map(|value| value.0).collect(),
        ))
    }

    /// Construct a JSON object.
    #[must_use]
    pub fn object(entries: HashMap<String, JsonValue>) -> Self {
        let mut out = serde_json::Map::new();
        for (key, value) in entries {
            out.insert(key, value.0);
        }
        Self(serde_json::Value::Object(out))
    }

    /// Parse JSON text.
    pub fn parse(source: String) -> Result<Self, JsonError> {
        let parsed =
            serde_json::from_str::<serde_json::Value>(&source).map_err(|err| JsonError::parse(err.to_string()))?;
        Self::from_serde(parsed)
    }

    /// Convert from `serde_json::Value` into an Incan dynamic JSON value.
    pub fn from_serde(value: serde_json::Value) -> Result<Self, JsonError> {
        Ok(Self(normalize_serde_value(value)?))
    }

    /// Convert into `serde_json::Value` for serialization and runtime helpers.
    #[must_use]
    pub fn into_serde(self) -> serde_json::Value {
        self.0
    }

    /// Borrow the inner serde value.
    #[must_use]
    pub fn as_serde(&self) -> &serde_json::Value {
        &self.0
    }

    /// Serialize this value to compact JSON.
    pub fn to_json(&self) -> Result<String, JsonError> {
        serde_json::to_string(&self.0).map_err(|err| JsonError::type_error(err.to_string()))
    }

    /// Serialize this value to pretty-printed JSON.
    pub fn to_pretty_json(&self) -> Result<String, JsonError> {
        serde_json::to_string_pretty(&self.0).map_err(|err| JsonError::type_error(err.to_string()))
    }

    /// Return this value's runtime JSON shape spelling.
    #[must_use]
    pub fn kind_name(&self) -> &'static str {
        kind_name_for_serde_value(&self.0)
    }

    /// Return the boolean payload when this value is a JSON boolean.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        self.0.as_bool()
    }

    /// Return the integer payload when this value is a JSON integer.
    #[must_use]
    pub fn as_int(&self) -> Option<i64> {
        self.0.as_i64()
    }

    /// Return this value as a float when it is numeric.
    #[must_use]
    pub fn as_float(&self) -> Option<f64> {
        self.0.as_f64()
    }

    /// Return the text payload when this value is a JSON string.
    #[must_use]
    pub fn as_str(&self) -> Option<String> {
        self.0.as_str().map(ToOwned::to_owned)
    }

    /// Return a cloned list when this value is a JSON array.
    #[must_use]
    pub fn as_array(&self) -> Option<Vec<JsonValue>> {
        self.0
            .as_array()
            .map(|values| values.iter().cloned().map(Self).collect())
    }

    /// Return a cloned map when this value is a JSON object.
    #[must_use]
    pub fn as_object(&self) -> Option<HashMap<String, JsonValue>> {
        self.0.as_object().map(|entries| {
            entries
                .iter()
                .map(|(key, value)| (key.clone(), Self(value.clone())))
                .collect()
        })
    }

    /// Look up an object key without raising.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<JsonValue> {
        self.0.as_object()?.get(key).cloned().map(Self)
    }

    /// Return object keys in deterministic order.
    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        self.0
            .as_object()
            .map(|values| values.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Return object values in deterministic key order.
    #[must_use]
    pub fn values(&self) -> Vec<JsonValue> {
        self.0
            .as_object()
            .map(|values| values.values().cloned().map(Self).collect())
            .unwrap_or_default()
    }

    /// Return object key/value pairs in deterministic key order.
    #[must_use]
    pub fn items(&self) -> Vec<(String, JsonValue)> {
        self.0
            .as_object()
            .map(|values| {
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), Self(value.clone())))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Insert or replace an object key.
    pub fn set(&mut self, key: &str, value: JsonValue) -> Result<(), JsonError> {
        match self.0.as_object_mut() {
            Some(values) => {
                values.insert(key.to_owned(), value.0);
                Ok(())
            }
            None => Err(JsonError::type_error("expected JSON object")),
        }
    }

    /// Remove an object key and return the previous value when present.
    pub fn remove(&mut self, key: &str) -> Result<Option<JsonValue>, JsonError> {
        match self.0.as_object_mut() {
            Some(values) => Ok(values.remove(key).map(Self)),
            None => Err(JsonError::type_error("expected JSON object")),
        }
    }

    /// Return an array element without raising.
    #[must_use]
    pub fn at(&self, index: i64) -> Option<JsonValue> {
        let index = usize::try_from(index).ok()?;
        self.0.as_array()?.get(index).cloned().map(Self)
    }

    /// Append a value to this JSON array.
    pub fn push(&mut self, value: JsonValue) -> Result<(), JsonError> {
        match self.0.as_array_mut() {
            Some(values) => {
                values.push(value.0);
                Ok(())
            }
            None => Err(JsonError::type_error("expected JSON array")),
        }
    }

    /// Insert a value into this JSON array before `index`.
    pub fn insert(&mut self, index: i64, value: JsonValue) -> Result<(), JsonError> {
        match self.0.as_array_mut() {
            Some(values) => {
                let Ok(index_usize) = usize::try_from(index) else {
                    return Err(JsonError::index_error(format!(
                        "JSON array insert index out of range: {index}"
                    )));
                };
                if index_usize > values.len() {
                    return Err(JsonError::index_error(format!(
                        "JSON array insert index out of range: {index}"
                    )));
                }
                values.insert(index_usize, value.0);
                Ok(())
            }
            None => Err(JsonError::type_error("expected JSON array")),
        }
    }

    /// Remove an array item and return the previous value when present.
    pub fn remove_at(&mut self, index: i64) -> Result<Option<JsonValue>, JsonError> {
        match self.0.as_array_mut() {
            Some(values) => {
                let Ok(index_usize) = usize::try_from(index) else {
                    return Ok(None);
                };
                if index_usize >= values.len() {
                    return Ok(None);
                }
                Ok(Some(Self(values.remove(index_usize))))
            }
            None => Err(JsonError::type_error("expected JSON array")),
        }
    }

    /// Return deterministic structural equality.
    #[must_use]
    pub fn equals(&self, other: JsonValue) -> bool {
        self == &other
    }
}

/// Recursively normalize a serde JSON value into Incan's dynamic JSON representation contract.
fn normalize_serde_value(value: serde_json::Value) -> Result<serde_json::Value, JsonError> {
    match value {
        serde_json::Value::Number(number) => normalize_serde_number(&number),
        serde_json::Value::Array(values) => values
            .into_iter()
            .map(normalize_serde_value)
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        serde_json::Value::Object(values) => values
            .into_iter()
            .map(|(key, value)| normalize_serde_value(value).map(|value| (key, value)))
            .collect::<Result<serde_json::Map<_, _>, _>>()
            .map(serde_json::Value::Object),
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::String(_) => Ok(value),
    }
}

/// Normalize one serde JSON number into the runtime number family selected by the parser.
fn normalize_serde_number(number: &serde_json::Number) -> Result<serde_json::Value, JsonError> {
    if let Some(value) = number.as_i64() {
        return Ok(serde_json::Value::Number(serde_json::Number::from(value)));
    }

    if let Some(value) = number.as_u64() {
        let value = i64::try_from(value)
            .map_err(|_| JsonError::number_error(format!("JSON integer `{number}` does not fit in Incan int")))?;
        return Ok(serde_json::Value::Number(serde_json::Number::from(value)));
    }

    let Some(value) = number.as_f64() else {
        return Err(JsonError::number_error(format!(
            "JSON number `{number}` cannot be represented as Incan int or float"
        )));
    };
    serde_json::Number::from_f64(value)
        .map(serde_json::Value::Number)
        .ok_or_else(|| JsonError::number_error(format!("JSON float `{number}` does not fit in Incan float")))
}

/// Return the stable kind spelling for a normalized serde JSON value.
fn kind_name_for_serde_value(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(number) => {
            if number.as_i64().is_some() {
                "int"
            } else {
                "float"
            }
        }
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Compiler-only JSON helpers used by generated Rust.
#[doc(hidden)]
pub mod __private {
    use super::{Deserialize, Serialize, json_decode_error_string, raise_json_serialization_error};

    /// Serialize a value to JSON or raise Incan's canonical runtime error.
    ///
    /// This is used by compiler-generated code for value-returning JSON paths (`json_stringify`, synthesized
    /// `to_json`, and trait-backed wrappers) so the emitted Rust does not inline fallback/panic extraction.
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

    /// Decode JSON or return Incan's canonical JSON decode error string.
    ///
    /// This is used by compiler-generated `from_json` and `json_parse` paths so generated projects do not need to
    /// reference `serde_json::from_str` directly for ordinary stdlib JSON decoding.
    pub fn parse_or_error<T>(json: &str) -> Result<T, String>
    where
        T: for<'de> Deserialize<'de>,
    {
        serde_json::from_str(json).map_err(json_decode_error_string)
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
        assert_eq!(__private::stringify_or_raise(&vec![1, 2, 3], "Vec"), "[1,2,3]");
    }

    #[test]
    #[should_panic(expected = "TypeError: Object of type AlwaysFails is not JSON serializable")]
    fn stringify_or_raise_uses_canonical_runtime_error() {
        let _ = __private::stringify_or_raise(&AlwaysFails, "AlwaysFails");
    }

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct DecodePayload {
        value: i64,
    }

    #[test]
    fn parse_or_error_decodes_successfully() {
        let decoded = __private::parse_or_error::<DecodePayload>(r#"{"value":42}"#);
        assert_eq!(decoded, Ok(DecodePayload { value: 42 }));
    }

    #[test]
    fn parse_or_error_uses_canonical_error_string() {
        let decoded = __private::parse_or_error::<DecodePayload>("not json");
        assert_eq!(
            decoded,
            Err("JSONDecodeError: expected ident at line 1 column 2".to_string())
        );
    }

    #[test]
    fn parse_classifies_json_numbers_and_serializes_deterministically() -> Result<(), JsonError> {
        let value = JsonValue::parse(r#"{"float":1e3,"int":42,"null":null,"items":[true,"x"]}"#.to_string())?;
        assert_eq!(value.kind_name(), "object");
        assert_eq!(value.get("int").map(|value| value.kind_name()), Some("int"));
        assert_eq!(value.get("int").and_then(|value| value.as_int()), Some(42));
        assert_eq!(value.get("float").map(|value| value.kind_name()), Some("float"));
        assert_eq!(value.get("float").and_then(|value| value.as_float()), Some(1000.0));
        assert_eq!(value.get("null"), Some(JsonValue::null()));
        assert_eq!(value.get("missing"), None);
        assert_eq!(
            value.to_json()?,
            r#"{"float":1000.0,"int":42,"items":[true,"x"],"null":null}"#
        );
        Ok(())
    }

    #[test]
    fn constructors_and_helpers_cover_object_array_mutation() -> Result<(), JsonError> {
        let mut entries = HashMap::new();
        entries.insert("name".to_string(), JsonValue::string("incan".to_string()));
        let mut value = JsonValue::object(entries);
        value.set("ok", JsonValue::bool(true))?;
        assert_eq!(value.keys(), vec!["name".to_string(), "ok".to_string()]);
        assert_eq!(value.get("ok").and_then(|value| value.as_bool()), Some(true));

        let mut items = JsonValue::array(vec![JsonValue::int(1)]);
        items.push(JsonValue::null())?;
        items.insert(1, JsonValue::int(2))?;
        assert_eq!(items.as_array().map(|values| values.len()), Some(3));
        assert_eq!(items.at(1).and_then(|value| value.as_int()), Some(2));
        assert_eq!(items.remove_at(1)?, Some(JsonValue::int(2)));
        assert_eq!(items.at(1), Some(JsonValue::null()));
        Ok(())
    }

    #[test]
    fn raw_mutation_helpers_return_json_errors() {
        let value = JsonValue::array(vec![]);
        let mut array = value.clone();
        let missing = array.insert(-1, JsonValue::null());
        assert!(matches!(
            missing,
            Err(JsonError {
                kind: JsonErrorKind::Index,
                ..
            })
        ));

        let mut wrong_type_value = value;
        let wrong_type = wrong_type_value.set("name", JsonValue::null());
        assert!(matches!(
            wrong_type,
            Err(JsonError {
                kind: JsonErrorKind::Type,
                ..
            })
        ));
    }
}

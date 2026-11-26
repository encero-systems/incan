//! The worker's JSON documents, written and read exactly as the standard JSON value of the reference does: one
//! compact object with its keys in sorted order, and the standard library's error texts.

use std::collections::BTreeMap;

use serde_json::Value;

/// One field of a worker document.
pub(crate) enum Field {
    Text(String),
    Integer(i64),
    Boolean(bool),
    Texts(Vec<String>),
}

/// Quote one string with JSON escaping: quotes, backslashes and control characters escape; other text stays UTF-8.
fn quoted(text: &str) -> String {
    match serde_json::to_string(text) {
        Ok(quoted) => quoted,
        Err(_) => "\"\"".to_owned(),
    }
}

/// The compact text of one document, keys in sorted order.
pub(crate) fn object(fields: BTreeMap<&str, Field>) -> String {
    let members: Vec<String> = fields
        .into_iter()
        .map(|(key, value)| {
            let value = match value {
                Field::Text(text) => quoted(&text),
                Field::Integer(number) => number.to_string(),
                Field::Boolean(flag) => flag.to_string(),
                Field::Texts(items) => {
                    let items: Vec<String> = items.iter().map(|item| quoted(item)).collect();
                    format!("[{}]", items.join(","))
                }
            };
            format!("{}:{value}", quoted(key))
        })
        .collect();
    format!("{{{}}}", members.join(","))
}

/// Parse one document as the standard JSON value does: a syntax error is its `JSONDecodeError`, and an integer above
/// the int range anywhere in the document is its `ValueError`, the first one in array and sorted key order.
pub fn parse_document(text: &str) -> Result<Value, String> {
    let document = serde_json::from_str(text).map_err(|error| format!("JSONDecodeError: {error}"))?;
    within_int_range(&document)?;
    Ok(document)
}

/// Refuse the first unsigned integer that does not fit an Incan int; floats and signed integers are kept.
fn within_int_range(value: &Value) -> Result<(), String> {
    match value {
        Value::Number(number) if number.as_i64().is_none() && number.as_u64().is_some() => {
            Err(format!("ValueError: JSON integer `{number}` does not fit in Incan int"))
        }
        Value::Array(items) => items.iter().try_for_each(within_int_range),
        Value::Object(members) => {
            let mut keys: Vec<&String> = members.keys().collect();
            keys.sort();
            keys.into_iter()
                .filter_map(|key| members.get(key))
                .try_for_each(within_int_range)
        }
        _ => Ok(()),
    }
}

/// Read a required string field, keeping the standard library's `KeyError` and `TypeError` texts.
pub(crate) fn require_text(document: &Value, name: &str) -> Result<String, String> {
    let Some(object) = document.as_object() else {
        return Err("TypeError: expected JSON object".to_owned());
    };
    match object.get(name) {
        None => Err(format!("KeyError: missing JSON object key `{name}`")),
        Some(value) => value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "TypeError: expected JSON string".to_owned()),
    }
}

/// The string field `name`, when the document has one.
pub(crate) fn text(document: &Value, name: &str) -> Option<String> {
    document.get(name).and_then(Value::as_str).map(str::to_owned)
}

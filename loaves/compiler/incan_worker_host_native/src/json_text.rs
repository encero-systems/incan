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

/// Parse one document, keeping the standard library's `JSONDecodeError` text.
pub fn parse_document(text: &str) -> Result<Value, String> {
    serde_json::from_str(text).map_err(|error| format!("JSONDecodeError: {error}"))
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

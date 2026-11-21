//! Typed source-projection values and the semantic bytes supplied to checking workers.

use crate::crumb_rows::{ModelField, model_description, required_value};
use crate::graph_crumb_fields::{decode_graph_field, encode_graph_row, items, text, tuple_items};
use crate::record_scope::RecordKey;
use internal_models_bindings::{Description, Value};

pub const SOURCE_PROFILE: &str = "incan.source-projection/1";
const SOURCE_TEXT: &[u8] = b"srctxt\x00";
const DOCUMENTATION: &[u8] = b"srcdoc\x00";

fn description() -> Description {
    model_description(
        SOURCE_TEXT,
        "incan.source-text/1",
        &[ModelField::new(b"source", "source", "string")],
    )
}

/// Persist the exact semantic slice independently of coordinates and documentation.
pub fn encode_source_text(source: &str) -> Result<Vec<u8>, String> {
    encode_graph_row(
        SOURCE_TEXT,
        &description(),
        vec![required_value(b"source", Value::Text(source.to_owned()))],
    )
}

/// Project schema-checked source bytes. Earlier snapshots contain plain UTF-8 under the same logical key.
pub fn source_value(key: &RecordKey, stored: &[u8]) -> Result<Vec<u8>, String> {
    if key.identity_profile != SOURCE_PROFILE {
        return Ok(stored.to_vec());
    }
    let prefix = &stored[..stored.len().min(5)];
    if prefix == b"CRMB\x01" || prefix == b"CRMB\x02" {
        return Ok(text(&decode_graph_field(stored, &description(), b"source")?)?.into_bytes());
    }
    std::str::from_utf8(stored).map_err(|_| "legacy source projection is not UTF-8".to_owned())?;
    Ok(stored.to_vec())
}

fn documentation_description() -> Description {
    model_description(
        DOCUMENTATION,
        "incan.source-documentation/1",
        &[ModelField::new(
            b"owners",
            "documentation",
            "list<tuple<string,string>>",
        )],
    )
}

/// Each entry carries the owning declaration and the source spelling of its documentation.
pub fn encode_documentation(entries: &[(String, String)]) -> Result<Vec<u8>, String> {
    let values = entries
        .iter()
        .map(|(owner, content)| Value::Tuple(vec![Value::Text(owner.clone()), Value::Text(content.clone())]))
        .collect();
    encode_graph_row(
        DOCUMENTATION,
        &documentation_description(),
        vec![required_value(b"owners", Value::List(values))],
    )
}

/// Read every documented owner and its documentation source spelling.
pub fn read_documentation(encoded: &[u8]) -> Result<Vec<(String, String)>, String> {
    let owners = decode_graph_field(encoded, &documentation_description(), b"owners")?;
    items(&owners)?
        .iter()
        .map(|entry| {
            let pair = tuple_items(entry, 2)?;
            Ok((text(&pair[0])?, text(&pair[1])?))
        })
        .collect()
}

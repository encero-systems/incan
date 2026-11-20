//! Store a named selection and the identity it currently selects in the same authenticated row.

use crate::crumb_rows::{ModelField, model_description, required_value};
use crate::graph_crumb_fields::{decode_graph_row, encode_graph_row, text};
use internal_models_bindings::{Description, Value};

const SELECTION: &[u8] = b"stores\x00";

fn description() -> Description {
    model_description(
        SELECTION,
        "incan.store.selection/1",
        &[
            ModelField::new(b"key___", "key", "string"),
            ModelField::new(b"target", "target", "string"),
        ],
    )
}

/// Bind a target to this storage key. Unit identities may carry the sha256 scheme prefix.
pub fn encode_binding(key: &str, target: &str) -> Result<Vec<u8>, String> {
    encode_graph_row(
        SELECTION,
        &description(),
        vec![
            required_value(b"key___", Value::Text(key.to_owned())),
            required_value(b"target", Value::Text(target.to_owned())),
        ],
    )
}

/// Open either a typed selection at its own key or the unadorned text used by previous installations.
pub fn read_binding(key: &str, data: &[u8]) -> Result<String, String> {
    if &data[..data.len().min(4)] != b"CRMB" {
        return std::str::from_utf8(data)
            .map(str::to_owned)
            .map_err(|error| error.to_string());
    }
    let fields = decode_graph_row(data, SELECTION, &description(), 2)?;
    if text(&fields[0].value)? != key {
        return Err("selection names another store key".into());
    }
    text(&fields[1].value)
}

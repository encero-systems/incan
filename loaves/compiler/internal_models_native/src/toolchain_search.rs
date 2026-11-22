//! The ordered directories that one content-named toolchain generation contributes to Rust library search.

use crate::crumb_rows::{ModelField, model_description, required_value};
use crate::graph_crumb_fields::{decode_graph_row, encode_graph_row, items, text};
use internal_models_bindings::{Description, Value};

const SEARCH: &[u8] = b"tcpath\x00";

fn description() -> Description {
    model_description(
        SEARCH,
        "incan.search-index/2",
        &[ModelField::new(b"search", "search", "list<string>")],
    )
}

fn canonical(paths: &[String]) -> bool {
    paths.windows(2).all(|pair| pair[1] > pair[0])
}

/// Publish the already sorted distinct search roots without joining or normalizing any path.
pub fn encode_search_paths(paths: &[String]) -> Result<Vec<u8>, String> {
    if !canonical(paths) {
        return Err("toolchain search directories must be sorted and distinct".into());
    }
    let values = Value::List(paths.iter().cloned().map(Value::Text).collect());
    encode_graph_row(SEARCH, &description(), vec![required_value(b"search", values)])
}

/// Return the selected native search roots after checking the schema, frame and canonical path order.
pub fn decode_search_paths(encoded: &[u8]) -> Result<Vec<String>, String> {
    let fields = decode_graph_row(encoded, SEARCH, &description(), 1)?;
    let paths = items(&fields[0].value)?
        .iter()
        .map(text)
        .collect::<Result<Vec<_>, _>>()?;
    if !canonical(&paths) {
        return Err("toolchain search directories must be sorted and distinct".into());
    }
    Ok(paths)
}

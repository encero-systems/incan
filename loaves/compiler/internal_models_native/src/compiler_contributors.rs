//! Global compiler contributors for the exact installation profile and package roster admitted to a source unit.

use crate::crumb_rows::{ModelField, model_description, required_value};
use crate::graph_crumb_fields::{decode_graph_row, encode_graph_row, items, text};
use internal_models_bindings::{Description, Value};
use std::collections::HashSet;

const ROW: &[u8] = b"locaps\x00";

/// Only packages contributing parser vocabulary or provider operations. `vocabulary` and `operations` default to
/// empty.
#[derive(Clone, PartialEq, Eq)]
pub struct CompilerContributors {
    pub profile: String,
    pub scope: String,
    pub vocabulary: Vec<String>,
    pub operations: Vec<String>,
}
crate::incan_debug::incan_debug!(CompilerContributors {
    profile,
    scope,
    vocabulary,
    operations
});

fn description() -> Description {
    model_description(
        ROW,
        "incan.compiler-contributors/1",
        &[
            ModelField::new(b"ops___", "operations", "list<string>"),
            ModelField::new(b"prfile", "profile", "string"),
            ModelField::new(b"scope_", "scope", "string"),
            ModelField::new(b"vocabs", "vocabulary", "list<string>"),
        ],
    )
}

fn valid(value: &CompilerContributors) -> bool {
    if !matches!(value.profile.as_str(), "release" | "debug") || value.scope.chars().count() != 64 {
        return false;
    }
    [&value.vocabulary, &value.operations].into_iter().all(|names| {
        let distinct: HashSet<&String> = names.iter().collect();
        !names.iter().any(String::is_empty) && names.is_sorted() && distinct.len() == names.len()
    })
}

/// Encode the contributor roster of one profile and scope.
pub fn encode_compiler_contributors(value: &CompilerContributors) -> Result<Vec<u8>, String> {
    if !valid(value) {
        return Err("invalid compiler contributors".into());
    }
    let names = |names: &[String]| Value::List(names.iter().cloned().map(Value::Text).collect());
    encode_graph_row(
        ROW,
        &description(),
        vec![
            required_value(b"ops___", names(&value.operations)),
            required_value(b"prfile", Value::Text(value.profile.clone())),
            required_value(b"scope_", Value::Text(value.scope.clone())),
            required_value(b"vocabs", names(&value.vocabulary)),
        ],
    )
}

fn names(value: &Value) -> Result<Vec<String>, String> {
    items(value)?.iter().map(text).collect()
}

/// Decode and validate one contributor roster.
pub fn decode_compiler_contributors(encoded: &[u8]) -> Result<CompilerContributors, String> {
    let fields = decode_graph_row(encoded, ROW, &description(), 4)?;
    let value = CompilerContributors {
        profile: text(&fields[1].value)?,
        scope: text(&fields[2].value)?,
        vocabulary: names(&fields[3].value)?,
        operations: names(&fields[0].value)?,
    };
    if !valid(&value) {
        return Err("invalid compiler contributors".into());
    }
    Ok(value)
}

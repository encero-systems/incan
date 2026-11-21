//! The private, path-bound proof of an imported artifact's producer and verified bytes.

use crate::crumb_rows::{ModelField, model_description, required_value};
use crate::graph_crumb_fields::{decode_graph_row, encode_graph_row, items, text};
use internal_models_bindings::{Description, Value};

const ADMISSION: &[u8] = b"unitad\x00";

/// Proof transferable across a rename while the filesystem observation still matches. `facts` and `refusal` default
/// to empty.
#[derive(Clone, PartialEq, Eq)]
pub struct PayloadAdmission {
    pub identity: String,
    pub payload: String,
    pub observation: String,
    pub provenance: Vec<String>,
    pub facts: String,
    pub refusal: String,
}
crate::incan_debug::incan_debug!(PayloadAdmission {
    identity,
    payload,
    observation,
    provenance,
    facts,
    refusal
});

fn description() -> Description {
    model_description(
        ADMISSION,
        "incan.oven.unit-admission/2",
        &[
            ModelField::new(b"facts_", "facts", "string"),
            ModelField::new(b"locati", "location", "string"),
            ModelField::new(b"observ", "observation", "string"),
            ModelField::new(b"origin", "source_provenance", "list<string>"),
            ModelField::new(b"payloa", "payload", "string"),
            ModelField::new(b"refuse", "refusal", "string"),
            ModelField::new(b"unit__", "unit", "string"),
        ],
    )
}

/// Bind the proof to the location at which its generation was observed.
pub fn encode_unit_admission(location: &str, proof: &PayloadAdmission) -> Result<Vec<u8>, String> {
    let text = |value: &String| Value::Text(value.clone());
    encode_graph_row(
        ADMISSION,
        &description(),
        vec![
            required_value(b"facts_", text(&proof.facts)),
            required_value(b"locati", Value::Text(location.to_owned())),
            required_value(b"observ", text(&proof.observation)),
            required_value(b"origin", Value::List(proof.provenance.iter().map(text).collect())),
            required_value(b"payloa", text(&proof.payload)),
            required_value(b"refuse", text(&proof.refusal)),
            required_value(b"unit__", text(&proof.identity)),
        ],
    )
}

/// Require the expected admission schema and exact artifact path before returning its proof.
pub fn decode_unit_admission(encoded: &[u8], location: &str) -> Result<PayloadAdmission, String> {
    let fields = decode_graph_row(encoded, ADMISSION, &description(), 7)?;
    if text(&fields[1].value)? != location {
        return Err(format!(
            "{location}: unit admission belongs to a different artifact path"
        ));
    }
    let origins = items(&fields[3].value)?
        .iter()
        .map(text)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PayloadAdmission {
        identity: text(&fields[6].value)?,
        payload: text(&fields[4].value)?,
        observation: text(&fields[2].value)?,
        provenance: origins,
        facts: text(&fields[0].value)?,
        refusal: text(&fields[5].value)?,
    })
}

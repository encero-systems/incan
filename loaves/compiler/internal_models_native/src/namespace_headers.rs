//! The small admission record beside one checked namespace frame (DD-0015 §9): what checking and lowering admit an
//! unchanged namespace by, without reading its interface or Body IR.

use crate::checker_record_models::SettledCallRead;
use crate::checker_records::decode_owned_namespace_view;
use crate::crumb_rows::{ModelField, model_description, required_value};
use crate::graph_crumb_fields::{decode_graph_row, encode_graph_row, items, text};
use crate::settled_call_reads::{call_read_values, decoded_call_reads};
use internal_models_bindings::{Description, Value};

const SELECTOR: &[u8] = b"grnmh1\x00";
const MAX_PROOF_BYTES: usize = 65_536;
const MAX_ENTRIES: usize = 4096;

/// One checked namespace's admission facts: its module, the compiler's identity proof (module name and path bound to
/// the interface it names), the Body IR owner-to-lowering-witness pairs, the package roots its checker selected and
/// the settled call reads it consumed.
#[derive(Clone, PartialEq, Eq)]
pub struct NamespaceHeaderValue {
    pub module: String,
    pub identity_proof: Vec<u8>,
    pub lowering_inputs: Vec<Vec<String>>,
    pub package_roots: Vec<String>,
    pub call_reads: Vec<SettledCallRead>,
}
crate::incan_debug::incan_debug!(NamespaceHeaderValue {
    module,
    identity_proof,
    lowering_inputs,
    package_roots,
    call_reads
});

fn description() -> Description {
    model_description(
        SELECTOR,
        "incan.checker-graph-namespace-header/1",
        &[
            ModelField::new(b"calls_", "call_reads", "list<tuple<string,string,string>>"),
            ModelField::new(b"inputs", "lowering_inputs", "list<string>"),
            ModelField::new(b"module", "module", "string"),
            ModelField::new(b"roots_", "package_roots", "list<string>"),
            ModelField::new(b"zproof", "identity_proof", "binary"),
        ],
    )
}

/// Refuse what a namespace record would refuse: an empty module or proof, witness pairs out of owner order, and
/// package roots that are not strictly ascending.
fn validate(value: &NamespaceHeaderValue) -> Result<(), String> {
    if value.module.is_empty()
        || value.identity_proof.is_empty()
        || value.identity_proof.len() > MAX_PROOF_BYTES
        || value.lowering_inputs.len() > MAX_ENTRIES
        || value.package_roots.len() > MAX_ENTRIES
    {
        return Err("invalid checked namespace header".into());
    }
    let mut previous = "";
    for input in &value.lowering_inputs {
        let [owner, witness] = input.as_slice() else {
            return Err("checked namespace header witness has the wrong field count".into());
        };
        if owner.is_empty() || witness.is_empty() || owner.as_str() <= previous {
            return Err("checked namespace header witnesses are not in owner order".into());
        }
        previous = owner;
    }
    let mut previous = "";
    for root in &value.package_roots {
        if root.as_str() <= previous {
            return Err("checked namespace header package roots are not ascending".into());
        }
        previous = root;
    }
    Ok(())
}

/// Encode one namespace header as one Crumb row.
pub fn encode_namespace_header(value: &NamespaceHeaderValue) -> Result<Vec<u8>, String> {
    validate(value)?;
    let inputs = value
        .lowering_inputs
        .iter()
        .flatten()
        .cloned()
        .map(Value::Text)
        .collect();
    let calls = call_read_values(&value.call_reads).map_err(|fault| format!("{fault:?}"))?;
    encode_graph_row(
        SELECTOR,
        &description(),
        vec![
            required_value(b"calls_", Value::List(calls)),
            required_value(b"inputs", Value::List(inputs)),
            required_value(b"module", Value::Text(value.module.clone())),
            required_value(
                b"roots_",
                Value::List(value.package_roots.iter().cloned().map(Value::Text).collect()),
            ),
            required_value(b"zproof", Value::Binary(value.identity_proof.clone())),
        ],
    )
}

/// Decode and validate one namespace header.
pub fn decode_namespace_header(encoded: &[u8]) -> Result<NamespaceHeaderValue, String> {
    let [calls, inputs, module, roots, proof]: [_; 5] = decode_graph_row(encoded, SELECTOR, &description(), 5)?
        .try_into()
        .map_err(|_| "checked namespace header has the wrong field count".to_owned())?;
    let flattened = items(&inputs.value)?.iter().map(text).collect::<Result<Vec<_>, _>>()?;
    if flattened.len() % 2 != 0 {
        return Err("checked namespace header witness has the wrong field count".into());
    }
    let Value::Binary(identity_proof) = proof.value else {
        return Err("checked namespace header has no binary identity proof".into());
    };
    let value = NamespaceHeaderValue {
        module: text(&module.value)?,
        identity_proof,
        lowering_inputs: flattened.chunks_exact(2).map(<[String]>::to_vec).collect(),
        package_roots: items(&roots.value)?.iter().map(text).collect::<Result<Vec<_>, _>>()?,
        call_reads: decoded_call_reads(&calls.value).map_err(|fault| format!("{fault:?}"))?,
    };
    validate(&value)?;
    Ok(value)
}

/// The header of one complete namespace record: the same facts, without its interface and Body IR. Only a current
/// record (known call reads and a compiler identity proof) has a header; an older one is admitted from its frame.
pub fn namespace_header_of(record: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let view = decode_owned_namespace_view(record).map_err(|fault| format!("{fault:?}"))?;
    if !view.call_reads_known || view.identity_proof.is_empty() {
        return Ok(None);
    }
    encode_namespace_header(&NamespaceHeaderValue {
        module: view.module,
        identity_proof: view.identity_proof,
        lowering_inputs: view.lowering_inputs,
        package_roots: view.package_roots,
        call_reads: view.call_reads,
    })
    .map(Some)
}

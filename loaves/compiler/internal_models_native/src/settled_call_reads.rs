//! The settled executable argument contract read by one checked project call.

use crate::checker_field_grammar::full_digest;
use crate::checker_record_models::{CodecFault, SettledCallRead};
use crate::crumb_rows::{ModelField, model_description, required_value};
use crate::graph_crumb_fields::{decode_graph_row, encode_graph_row, text as graph_text};
use internal_models_bindings::{Description, Value};

/// Validate the canonical reader/target order before emitting each independently typed dependency.
pub fn call_read_values(reads: &[SettledCallRead]) -> Result<Vec<Value>, CodecFault> {
    let mut previous: (&str, &str) = ("", "");
    let mut result = Vec::with_capacity(reads.len());
    for read in reads {
        if read.reader.is_empty() || read.target.is_empty() || !full_digest(&read.digest) {
            return Err(CodecFault::Malformed);
        }
        if previous.0 > read.reader.as_str() || (previous.0 == read.reader && previous.1 >= read.target.as_str()) {
            return Err(CodecFault::Malformed);
        }
        result.push(Value::Tuple(vec![
            Value::Text(read.reader.clone()),
            Value::Text(read.target.clone()),
            Value::Text(read.digest.clone()),
        ]));
        previous = (&read.reader, &read.target);
    }
    Ok(result)
}

/// Refuse duplicate edges and malformed digests at the graph boundary.
pub fn decoded_call_reads(value: &Value) -> Result<Vec<SettledCallRead>, CodecFault> {
    let Value::List(values) = value else {
        return Err(CodecFault::Malformed);
    };
    let mut result = Vec::with_capacity(values.len());
    for item in values {
        let Value::Tuple(fields) = item else {
            return Err(CodecFault::Malformed);
        };
        if fields.len() != 3 {
            return Err(CodecFault::Malformed);
        }
        result.push(SettledCallRead {
            reader: text(&fields[0])?,
            target: text(&fields[1])?,
            digest: text(&fields[2])?,
        });
    }
    call_read_values(&result)?;
    Ok(result)
}

fn text(value: &Value) -> Result<String, CodecFault> {
    match value {
        Value::Text(found) => Ok(found.clone()),
        _ => Err(CodecFault::Malformed),
    }
}

const OWNER_CALLS: &[u8] = b"owncal\x00";

fn owner_description() -> Description {
    model_description(
        OWNER_CALLS,
        "incan.graph.owner-settled-calls/1",
        &[
            ModelField::new(b"ident_", "owner", "string"),
            ModelField::new(b"reader", "kernel_reader", "string"),
            ModelField::new(b"reads_", "reads", "list<tuple<string,string,string>>"),
            ModelField::new(b"source", "source", "string"),
        ],
    )
}

fn check_owner_reads(identity: &str, reader: &str, source: &str, reads: &[SettledCallRead]) -> Result<(), String> {
    if identity.is_empty() || reader.is_empty() || source.is_empty() {
        return Err("missing executable call witness owner".into());
    }
    if reads.iter().any(|read| read.reader != reader) {
        return Err("executable call witness contains a different reader".into());
    }
    Ok(())
}

/// Record the exact settled targets, including an empty set, for this physical owner.
pub fn encode_owner_call_reads(
    identity: &str,
    reader: &str,
    source: &str,
    reads: &[SettledCallRead],
) -> Result<Vec<u8>, String> {
    check_owner_reads(identity, reader, source, reads)?;
    let values = call_read_values(reads).map_err(|error| format!("invalid executable call read: {error:?}"))?;
    encode_graph_row(
        OWNER_CALLS,
        &owner_description(),
        vec![
            required_value(b"ident_", Value::Text(identity.to_owned())),
            required_value(b"reader", Value::Text(reader.to_owned())),
            required_value(b"reads_", Value::List(values)),
            required_value(b"source", Value::Text(source.to_owned())),
        ],
    )
}

/// Verify the checked and physical reader identities before admitting any call contract.
pub fn decode_owner_call_reads(
    identity: &str,
    reader: &str,
    source: &str,
    encoded: &[u8],
) -> Result<Vec<SettledCallRead>, String> {
    let fields = decode_graph_row(encoded, OWNER_CALLS, &owner_description(), 4)?;
    if graph_text(&fields[0].value)? != identity
        || graph_text(&fields[1].value)? != reader
        || graph_text(&fields[3].value)? != source
    {
        return Err("executable call witness belongs to another owner".into());
    }
    let reads =
        decoded_call_reads(&fields[2].value).map_err(|error| format!("invalid executable call read: {error:?}"))?;
    check_owner_reads(identity, reader, source, &reads)?;
    Ok(reads)
}

//! Bind a checked overload lookup to the exact signature-addressed candidates enumerated by the resolver.

use crate::crumb_rows::{ModelField, model_description, required_value};
use crate::graph_crumb_fields::{KEY_TYPE, decode_graph_row, encode_graph_row, items, key_value, read_key, text};
use crate::hex_text::hexlify;
use crate::namespace_order::ranked_output_suffix;
use crate::record_scope::{
    DEFAULT_READ_BYTES, MemberFact, MemberSelection, ReadFault, ReadScope, RecordKey, RecordRead, RecordRequest,
    RecordSelection,
};
use internal_models_bindings::{Description, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

pub const OVERLOAD_MEMBERS: &str = "overload-members";
pub const LOOKUP_FILE: &str = "method-lookups.crumb";
const INVENTORY: &[u8] = b"ovluse\x00";
const MEMBER_PAGE: &[u8] = b"ovlset\x00";

/// One method lookup a checked reader performed and the candidates it selected among.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct MethodLookup {
    pub reader: String,
    pub module: String,
    pub receiver: String,
    pub method: String,
    pub candidates: Vec<String>,
}
crate::incan_debug::incan_debug!(MethodLookup {
    reader,
    module,
    receiver,
    method,
    candidates
});

/// Negotiate the separately encoded membership evidence with a compiler that can publish it.
pub fn method_lookup_suffix() -> String {
    format!(".methods{}", ranked_output_suffix())
}

fn inventory_description() -> Description {
    model_description(
        INVENTORY,
        "incan.compiler.method-lookups/1",
        &[ModelField::new(
            b"lookup",
            "reader_and_candidates",
            "list<list<string>>",
        )],
    )
}

/// Encode the method lookups of one compilation: fresh lookups and retained owners.
pub fn encode_method_lookups(rows: &[Vec<String>]) -> Result<Vec<u8>, String> {
    let value = Value::List(
        rows.iter()
            .map(|row| Value::List(row.iter().cloned().map(Value::Text).collect()))
            .collect(),
    );
    encode_graph_row(
        INVENTORY,
        &inventory_description(),
        vec![required_value(b"lookup", value)],
    )
}

fn canonical_members(values: &[Value]) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = Vec::with_capacity(values.len());
    let mut prior = String::new();
    for value in values {
        let name = text(value)?;
        if name <= prior || name.contains('\n') || name.contains('\t') {
            return Err("invalid or unordered overload member identity".into());
        }
        prior = name.clone();
        names.push(name);
    }
    if names.is_empty() {
        return Err("successful overload selection has no candidates".into());
    }
    Ok(names)
}

/// Method lookups grouped by reader, and the readers retained without lookups. Both default to empty.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct MethodReads {
    pub groups: HashMap<String, Vec<MethodLookup>>,
    pub retained: HashSet<String>,
}
crate::incan_debug::incan_debug!(MethodReads { groups, retained });

/// Decode the method lookups of one compilation.
pub fn read_method_lookups(encoded: &[u8]) -> Result<MethodReads, String> {
    let fields = decode_graph_row(encoded, INVENTORY, &inventory_description(), 1)?;
    let mut lookups: HashMap<String, Vec<MethodLookup>> = HashMap::new();
    let mut retained = HashSet::new();
    for value in items(&fields[0].value)? {
        let cells = items(value)?;
        if cells.len() == 1 {
            let reader = text(&cells[0])?;
            if reader.is_empty() || lookups.contains_key(&reader) || retained.contains(&reader) {
                return Err("invalid retained method lookup".into());
            }
            retained.insert(reader);
            continue;
        }
        if cells.len() < 5 {
            return Err("incomplete compiler method lookup".into());
        }
        let mut header = Vec::with_capacity(4);
        for cell in &cells[..4] {
            let part = text(cell)?;
            if part.is_empty() || part.contains('\n') || part.contains('\t') {
                return Err("invalid compiler method lookup scope".into());
            }
            header.push(part);
        }
        if retained.contains(&header[0]) {
            return Err("fresh and retained lookups share one owner".into());
        }
        let candidates = canonical_members(&cells[4..])?;
        let [reader, module, receiver, method]: [String; 4] = header
            .try_into()
            .map_err(|_| "incomplete compiler method lookup".to_owned())?;
        lookups.entry(reader.clone()).or_default().push(MethodLookup {
            reader,
            module,
            receiver,
            method,
            candidates,
        });
    }
    Ok(MethodReads {
        groups: lookups,
        retained,
    })
}

fn page_description() -> Description {
    model_description(
        MEMBER_PAGE,
        "incan.compiler.overload-members/1",
        &[
            ModelField::new(b"key___", "scope", KEY_TYPE),
            ModelField::new(b"member", "candidates", "list<string>"),
        ],
    )
}

/// The canonical full-page member request for one receiver-scoped key.
pub fn method_member_request(key: &RecordKey) -> RecordRequest {
    RecordRequest {
        key: key.clone(),
        selection: RecordSelection::Members(MemberSelection {
            selector: key.facet.clone(),
            order: "canonical".into(),
            lower: String::new(),
            upper: String::new(),
            continuation: String::new(),
        }),
        byte_limit: DEFAULT_READ_BYTES,
    }
}

/// Encode one receiver group's ordered candidates under its receiver-scoped key.
pub fn encode_method_members(key: &RecordKey, candidates: &[String]) -> Result<Vec<u8>, String> {
    if key.family != OVERLOAD_MEMBERS {
        return Err("method membership requires a receiver-scoped key".into());
    }
    let members: Vec<Value> = candidates.iter().cloned().map(Value::Text).collect();
    canonical_members(&members)?;
    encode_graph_row(
        MEMBER_PAGE,
        &page_description(),
        vec![
            required_value(b"key___", key_value(key)),
            required_value(b"member", Value::List(members)),
        ],
    )
}

/// Read one authenticated receiver group, including its ordered members and terminal continuation.
pub fn read_method_members(scope: &ReadScope, request: &RecordRequest, encoded: &[u8]) -> RecordRead {
    if *request != method_member_request(&request.key) || request.key.family != OVERLOAD_MEMBERS {
        return RecordRead::Failed(ReadFault::SelectionMismatch);
    }
    let Ok(members) = member_names(&request.key, encoded) else {
        return RecordRead::Failed(ReadFault::DamagedRecord);
    };
    let value = members.join("\n").into_bytes();
    if value.len() as i64 > request.byte_limit {
        return RecordRead::Failed(ReadFault::InvalidEvidence);
    }
    let preimage = format!(
        "{}\ncanonical\n\n\nend\n{}",
        request.key.facet,
        hexlify(&Sha256::digest(&value))
    );
    RecordRead::Members(MemberFact {
        scope: scope.clone(),
        request: request.clone(),
        value,
        membership_digest: hexlify(&Sha256::digest(preimage.as_bytes())),
        continuation: "end".into(),
    })
}

fn member_names(key: &RecordKey, encoded: &[u8]) -> Result<Vec<String>, String> {
    let fields = decode_graph_row(encoded, MEMBER_PAGE, &page_description(), 2)?;
    if read_key(&fields[0].value)? != *key {
        return Err("overload member page belongs to a different receiver".into());
    }
    canonical_members(items(&fields[1].value)?)
}

/// Ask a native producer to retain bounded syntax coordinates alongside its exact physical input stamp.
pub fn coordinate_output_suffix() -> String {
    format!(".placement{}", method_lookup_suffix())
}

/// Request the checked declarations' executable parameter access digests.
pub fn contract_output_suffix() -> String {
    format!(".contracts{}", coordinate_output_suffix())
}

/// Request exact name membership for functions with a compiler-proven closed scalar scope.
pub fn scope_output_suffix() -> String {
    format!(".scope{}", contract_output_suffix())
}

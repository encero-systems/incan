//! Pinned store frame references offered by Oven to one compiler request.

use crate::checker_field_grammar::full_digest;
use crate::crumb_rows::{ModelField, model_description, required_value};
use crate::graph_crumb_fields::{
    KEY_TYPE, decode_graph_row, encode_graph_row, integer, items, key_value, number, read_key, text, tuple_items,
};
use crate::record_scope::{MAX_CHECKER_ROW_BYTES, RecordKey};
use crate::store_record_source::encode_record_key;
use internal_models_bindings::{Description, Value};
use std::collections::HashSet;

const NAMESPACE_REFERENCE_PAGE: &str = "namespace-references.crumb";
const NAMESPACE_HEADER_REFERENCE_PAGE: &str = "namespace-header-references.crumb";
pub const NAMESPACE_IDENTITY_PROFILE: &str = "incan.editable-namespace-crumb/7";
const ROW: &[u8] = b"nsrefs\x00";

/// An evaluated namespace in an immutable segment under the request's bakery anchor.
#[derive(Clone, PartialEq, Eq)]
pub struct NamespaceReference {
    pub key: RecordKey,
    pub segment: String,
    pub offset: i64,
    pub length: i64,
    pub frame_digest: Vec<u8>,
    pub value_digest: String,
}
crate::incan_debug::incan_debug!(NamespaceReference {
    key,
    segment,
    offset,
    length,
    frame_digest,
    value_digest
});

/// Name the one frame inventory in the compiler request directory.
pub fn namespace_reference_filename() -> String {
    NAMESPACE_REFERENCE_PAGE.to_owned()
}

/// Name the inventory of namespace header frames in the compiler request directory.
pub fn namespace_header_reference_filename() -> String {
    NAMESPACE_HEADER_REFERENCE_PAGE.to_owned()
}

/// A request's offered layout companions. An empty page still requests layout publication.
pub fn namespace_layout_reference_filename() -> String {
    "namespace-layout-references.crumb".to_owned()
}

/// Previously published namespaces used only to bind layout candidates for rechecked modules.
pub fn namespace_layout_candidate_filename() -> String {
    "namespace-layout-candidates.crumb".to_owned()
}

/// Directory of checked layout companion rows beside the compiler's other namespace outputs.
pub fn namespace_layout_output_directory() -> String {
    "namespace-layouts".to_owned()
}

/// Name a checked namespace's layout companion. The context versions the witness and its comparison inputs.
pub fn namespace_layout_key(origin: &str, module: &str) -> RecordKey {
    RecordKey {
        family: "namespace-layout".into(),
        identity_profile: NAMESPACE_IDENTITY_PROFILE.into(),
        origin: origin.to_owned(),
        identity: module.to_owned(),
        facet: "checked".into(),
        context: "layout-witnesses/1".into(),
    }
}

/// Name one namespace header beside its composite namespace, under the same logical source projection.
pub fn namespace_header_key(origin: &str, module: &str) -> RecordKey {
    RecordKey {
        family: "namespace-header".into(),
        identity_profile: NAMESPACE_IDENTITY_PROFILE.into(),
        origin: origin.to_owned(),
        identity: module.to_owned(),
        facet: "checked".into(),
        context: "admission-header".into(),
    }
}

/// Name one composite editable namespace under its logical source projection.
pub fn namespace_key(origin: &str, module: &str) -> RecordKey {
    RecordKey {
        family: "namespace".into(),
        identity_profile: NAMESPACE_IDENTITY_PROFILE.into(),
        origin: origin.to_owned(),
        identity: module.to_owned(),
        facet: "checked".into(),
        context: "interface-body-ir-plan".into(),
    }
}

/// One bounded inventory of frame addresses, with both the store and evaluation digests.
fn description() -> Description {
    let storage = format!("list<tuple<{KEY_TYPE},string,i64,i64,binary,string>>");
    model_description(
        ROW,
        "incan.compiler.namespace-references/1",
        &[ModelField::new(b"frames", "frames", &storage)],
    )
}

/// Admit only a canonical namespace record key and a bounded frame in a content-named segment.
fn validate(reference: &NamespaceReference) -> Result<(), String> {
    let (origin, module) = (&reference.key.origin, &reference.key.identity);
    let key = encode_record_key(&reference.key)?;
    if (reference.key != namespace_key(origin, module)
        && reference.key != namespace_header_key(origin, module)
        && reference.key != namespace_layout_key(origin, module))
        || !full_digest(&format!("sha256:{}", reference.segment))
        || !full_digest(&format!("sha256:{}", reference.value_digest))
    {
        return Err("invalid namespace frame authority".into());
    }
    let maximum = MAX_CHECKER_ROW_BYTES + key.len() as i64 + 8;
    if reference.offset < 0
        || reference.length <= maximum - MAX_CHECKER_ROW_BYTES
        || reference.length > maximum
        || reference.frame_digest.len() != 32
    {
        return Err("invalid namespace frame extent or digest".into());
    }
    Ok(())
}

/// Stage one compact Crumb page. Frame payloads remain leased in their bakery segments.
pub fn encode_namespace_references(references: &[NamespaceReference]) -> Result<Vec<u8>, String> {
    let mut rows = Vec::with_capacity(references.len());
    for reference in references {
        validate(reference)?;
        rows.push(Value::Tuple(vec![
            key_value(&reference.key),
            Value::Text(reference.segment.clone()),
            integer(reference.offset),
            integer(reference.length),
            Value::Binary(reference.frame_digest.clone()),
            Value::Text(reference.value_digest.clone()),
        ]));
    }
    encode_graph_row(ROW, &description(), vec![required_value(b"frames", Value::List(rows))])
}

/// Recover unique module addresses before any requested frame can be read.
pub fn decode_namespace_references(encoded: &[u8]) -> Result<Vec<NamespaceReference>, String> {
    let row = decode_graph_row(encoded, ROW, &description(), 1)?;
    let mut seen = HashSet::new();
    let mut references = Vec::new();
    for encoded_reference in items(&row[0].value)? {
        let fields = tuple_items(encoded_reference, 6)?;
        let Value::Binary(digest) = &fields[4] else {
            return Err("namespace frame has no binary digest".into());
        };
        let reference = NamespaceReference {
            key: read_key(&fields[0])?,
            segment: text(&fields[1])?,
            offset: number(&fields[2])?,
            length: number(&fields[3])?,
            frame_digest: digest.clone(),
            value_digest: text(&fields[5])?,
        };
        validate(&reference)?;
        if !seen.insert(reference.key.identity.clone()) {
            return Err("duplicate selected namespace frame".into());
        }
        references.push(reference);
    }
    Ok(references)
}

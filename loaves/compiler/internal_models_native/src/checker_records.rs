//! Stable free-function checker records and their projections at the native compiler boundary.

pub use crate::checker_field_grammar::CanonicalField;
use crate::checker_field_grammar::{full_digest, number, validate_field};
pub use crate::codec_fault::CodecFault;
pub use crate::compiled_body_frames::{
    CompiledBodyDependencyRead, CompiledBodyFallback, CompiledBodyOutputs, CompiledBodyValue, CompiledNamespaceValue,
};
use crate::crumb_rows::{Rows, Writer};
pub use crate::graph_records::{
    GraphBodyFields, GraphBodyValue, GraphNamespaceValue, decode_graph_body_page, decode_graph_body_values,
    decode_graph_namespace_value, encode_graph_body_values, encode_graph_namespace_value,
};
use crate::row_shapes::CHECKER;
pub use crate::semantic_digest::semantic_digest;
use CodecFault::{BodyChanged, Limit, Malformed, OwnerMismatch, ProfileMismatch, Unsupported};
use sha2::{Digest, Sha256};

pub const CHECKER_SCHEMA: &str = "incan.checker-fields/10";
pub const CHECKER_PROFILE: &str = "incan.checker-free-functions/17";
pub(crate) const MAX_FIELDS: usize = 4096;
const MAX_RECORD_BYTES: i64 = 65536;

/// Declaration/default output, pending local output and settled output have distinct contracts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckedFamily {
    Declaration,
    Local,
    Final,
}
impl CheckedFamily {
    /// Return the persistent stage tag for this record family.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Declaration => "declaration",
            Self::Local => "local",
            Self::Final => "final",
        }
    }
    /// Read a versioned stage tag and preserve the spelling of an unsupported family.
    fn parse(value: &str) -> Result<Self, CodecFault> {
        match value {
            "declaration" => Ok(Self::Declaration),
            "local" => Ok(Self::Local),
            "final" => Ok(Self::Final),
            _ => Err(Unsupported(value.into())),
        }
    }
}
/// One checked owner; owner is its checked stable source identity, root its position-free semantic syntax.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckedRecord {
    pub profile: String,
    pub owner: String,
    pub root: String,
    pub family: CheckedFamily,
    pub fields: Vec<CanonicalField>,
}

/// Validate the profile, owner, root and the strict order and grammar of all canonical fields.
pub(crate) fn validate(record: &CheckedRecord) -> Result<(), CodecFault> {
    if record.profile != CHECKER_PROFILE {
        return Err(ProfileMismatch);
    }
    if record.owner.is_empty() || !full_digest(&record.root) {
        return Err(Malformed);
    }
    if record.fields.len() > MAX_FIELDS {
        return Err(Limit);
    }
    let mut previous = ("", "", "");
    for field in &record.fields {
        if !matches!(
            field.family.as_str(),
            "type"
                | "type-identity"
                | "identity-context"
                | "binding"
                | "binding-type"
                | "reference"
                | "write"
                | "builtin"
                | "constructor"
                | "requirement"
                | "parameter"
                | "signature"
                | "diagnostic"
                | "status"
                | "default"
                | "final-type"
                | "native-call"
                | "artifact-call"
                | "artifact-protocol"
                | "target"
                | "identifier"
                | "effect"
                | "mutation"
                | "ownership"
                | "input-call"
                | "input-local"
                | "input-flow"
                | "input-copy"
        ) {
            return Err(Unsupported(field.family.to_owned()));
        }
        let key = (field.family.as_str(), field.node.as_str(), field.selector.as_str());
        if key <= previous || field.values.len() > MAX_FIELDS {
            return Err(Malformed);
        }
        validate_field(field)?;
        previous = key;
    }
    Ok(())
}

/// Encode a checked owner as one bounded Crumb row.
pub fn encode_record(record: &CheckedRecord) -> Result<Vec<u8>, CodecFault> {
    validate(record)?;
    let mut out = Writer::new(&CHECKER, MAX_RECORD_BYTES)?;
    out.record();
    out.text(b"family", record.family.name());
    let count = record.fields.iter().map(|f| 4 + f.values.len()).sum();
    out.list(b"fields", count)?;
    for f in &record.fields {
        for token in [&f.family, &f.node, &f.selector] {
            out.text_node(token.to_owned())
        }
        out.text_node(f.values.len().to_string());
        for token in &f.values {
            out.text_node(token.to_owned())
        }
    }
    out.text(b"owner_", &record.owner);
    out.text(b"profil", &record.profile);
    out.text(b"root__", &record.root);
    out.finish().map_err(|_| Malformed)
}

/// Restore canonical fields by consuming each tuple header and its counted value list.
fn decoded_fields(values: Vec<String>) -> Result<Vec<CanonicalField>, CodecFault> {
    let mut it = values.into_iter();
    let mut result = Vec::new();
    while let Some(family) = it.next() {
        let node = it.next().ok_or(Malformed)?;
        let selector = it.next().ok_or(Malformed)?;
        let count = number(&it.next().ok_or(Malformed)?)?;
        if count > MAX_FIELDS || count > it.len() {
            return Err(Malformed);
        }
        result.push(CanonicalField {
            family,
            node,
            selector,
            values: it.by_ref().take(count).collect(),
        });
    }
    if result.len() > MAX_FIELDS {
        return Err(Limit);
    }
    Ok(result)
}

/// Admit exactly one checker row and reconstruct its schema fields.
fn decoded_record(encoded: &[u8]) -> Result<CheckedRecord, CodecFault> {
    let mut rows = Rows::admit(encoded, &[&CHECKER], MAX_RECORD_BYTES)?;
    if rows.len() != 1 {
        return Err(Malformed);
    }
    let (key, mut fields) = rows.row(0)?;
    if key != CHECKER.selector {
        return Err(Malformed);
    }
    fields.count(5)?;
    let family = fields.text()?;
    let values = fields.texts()?;
    let owner = fields.text()?;
    let profile = fields.text()?;
    let root = fields.text()?;
    let family = CheckedFamily::parse(&family)?;
    let fields = decoded_fields(values)?;
    let record = CheckedRecord {
        profile,
        owner,
        root,
        family,
        fields,
    };
    validate(&record)?;
    Ok(record)
}

/// Bind the decoded record to the expected profile and owner.
pub fn decode_owned_record(encoded: &[u8], profile: &str, owner: &str) -> Result<CheckedRecord, CodecFault> {
    let record = decoded_record(encoded)?;
    if record.profile != profile {
        return Err(ProfileMismatch);
    }
    if record.owner != owner {
        return Err(OwnerMismatch);
    }
    Ok(record)
}

/// Verify the expected semantic root after the record owner is authenticated.
pub fn decode_record(encoded: &[u8], profile: &str, owner: &str, root: &str) -> Result<CheckedRecord, CodecFault> {
    let record = decode_owned_record(encoded, profile, owner)?;
    if record.root != root {
        return Err(BodyChanged);
    }
    Ok(record)
}

/// Read the validated position-independent root from a complete checker record.
pub fn record_root(encoded: &[u8]) -> Result<String, CodecFault> {
    Ok(decoded_record(encoded)?.root)
}
/// Hash every encoded record byte, including the schema reference.
pub fn content_digest(record: &CheckedRecord) -> Result<String, CodecFault> {
    Ok(format!("sha256:{:x}", Sha256::digest(encode_record(record)?)))
}

/// Read a header with an explicit checker stage and its ordered native fields.
pub fn from_native_fields(rows: &[Vec<String>]) -> Result<CheckedRecord, CodecFault> {
    from_rows(rows, None)
}
/// Attach the local stage to a fresh producer header or verify its persisted tag.
pub fn from_native_local_fields(rows: &[Vec<String>]) -> Result<CheckedRecord, CodecFault> {
    from_rows(rows, Some(CheckedFamily::Local))
}
/// Attach the final stage to a fresh producer header or verify its persisted tag.
pub fn from_native_final_fields(rows: &[Vec<String>]) -> Result<CheckedRecord, CodecFault> {
    from_rows(rows, Some(CheckedFamily::Final))
}

/// Validate the header and transfer each native column into an independently owned checker field.
fn from_rows(rows: &[Vec<String>], fixed: Option<CheckedFamily>) -> Result<CheckedRecord, CodecFault> {
    let (header, rows) = rows.split_first().ok_or(Malformed)?;
    if header.first().map(String::as_str) != Some("header") {
        return Err(Malformed);
    }
    let (owner, root, tag, profile) = match (header.as_slice(), fixed) {
        ([_, owner, root, tag, profile], None) => (owner, root, tag.as_str(), profile),
        ([_, owner, root, tag, profile], Some(f)) if tag == f.name() => (owner, root, f.name(), profile),
        ([_, owner, root, profile], Some(f)) => (owner, root, f.name(), profile),
        _ => return Err(Malformed),
    };
    let mut fields = Vec::with_capacity(rows.len());
    for row in rows {
        let [family, node, selector, values @ ..] = row.as_slice() else {
            return Err(Malformed);
        };
        if values.is_empty() {
            return Err(Malformed);
        }
        fields.push(CanonicalField {
            family: family.to_owned(),
            node: node.to_owned(),
            selector: selector.to_owned(),
            values: values.to_vec(),
        });
    }
    let record = CheckedRecord {
        profile: profile.to_owned(),
        owner: owner.to_owned(),
        root: root.to_owned(),
        family: CheckedFamily::parse(tag)?,
        fields,
    };
    validate(&record)?;
    Ok(record)
}

/// Project validated fields for the native rebinding adapter.
pub fn native_fields(record: &CheckedRecord) -> Result<Vec<Vec<String>>, CodecFault> {
    validate(record)?;
    Ok(project_fields(record))
}

/// Build the ordered native header and value rows of an already validated record.
pub(crate) fn project_fields(record: &CheckedRecord) -> Vec<Vec<String>> {
    let mut rows = Vec::with_capacity(record.fields.len() + 1);
    rows.push(vec![
        "header".into(),
        record.owner.to_owned(),
        record.root.to_owned(),
        record.family.name().into(),
        record.profile.to_owned(),
    ]);
    for field in &record.fields {
        let mut row = Vec::with_capacity(3 + field.values.len());
        row.extend([
            field.family.to_owned(),
            field.node.to_owned(),
            field.selector.to_owned(),
        ]);
        row.extend_from_slice(&field.values);
        rows.push(row);
    }
    rows
}

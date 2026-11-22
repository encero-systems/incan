//! The candidate compiler's private installation index preserves the ordered logical membership of a sealed record
//! set.

use crate::crumb_rows::{ModelField, model_description, required_value};
use crate::graph_crumb_fields::{decode_graph_row, encode_graph_row, items, text, tuple_items};
use crate::hex_text::{hexlify, unhexlify};
use internal_models_bindings::{Description, Value};
use sha2::{Digest, Sha256};

const EXPORT: &[u8] = b"cmpexp\x00";

/// One key and the lowercase SHA-256 revision naming its exact exported object.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CompilerExportMember {
    pub key: String,
    pub revision: String,
}
crate::incan_debug::incan_debug!(CompilerExportMember { key, revision });

fn description() -> Description {
    model_description(
        EXPORT,
        "incan.compiler-record-export/3",
        &[
            ModelField::new(b"check_", "membership_digest", "binary"),
            ModelField::new(b"member", "members", "list<tuple<string,binary>>"),
        ],
    )
}

fn checked_revision(member: &CompilerExportMember, previous: &str) -> Result<Vec<u8>, String> {
    if member.key.as_str() <= previous {
        return Err("compiler record export requires nonempty keys in unique ascending order".into());
    }
    let revision = unhexlify(&member.revision)?;
    if revision.len() != 32 || hexlify(&revision) != member.revision {
        return Err("compiler record export revision must be a lowercase SHA-256 digest".into());
    }
    Ok(revision)
}

fn membership_digest(members: &[CompilerExportMember]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(EXPORT);
    for member in members {
        let key = member.key.as_bytes();
        hasher.update(key.len().to_string().as_bytes());
        hasher.update(b":");
        hasher.update(key);
        hasher.update(member.revision.as_bytes());
    }
    hasher.finalize().to_vec()
}

/// Bind each logical key to the binary digest of its object in the compiler's bytewise key order.
pub fn encode_compiler_export(members: &[CompilerExportMember]) -> Result<Vec<u8>, String> {
    let mut values = Vec::with_capacity(members.len());
    let mut previous = "";
    for member in members {
        let revision = checked_revision(member, previous)?;
        values.push(Value::Tuple(vec![
            Value::Text(member.key.clone()),
            Value::Binary(revision),
        ]));
        previous = &member.key;
    }
    encode_graph_row(
        EXPORT,
        &description(),
        vec![
            required_value(b"check_", Value::Binary(membership_digest(members))),
            required_value(b"member", Value::List(values)),
        ],
    )
}

/// Verify the full unique ordered roster before any exported object can enter a sealed set.
pub fn decode_compiler_export(encoded: &[u8]) -> Result<Vec<CompilerExportMember>, String> {
    let fields = decode_graph_row(encoded, EXPORT, &description(), 2)?;
    let mut members = Vec::new();
    let mut previous = String::new();
    for item in items(&fields[1].value)? {
        let pair = tuple_items(item, 2)?;
        let key = text(&pair[0])?;
        if key <= previous {
            return Err("compiler record export keys repeat or are out of order".into());
        }
        match &pair[1] {
            Value::Binary(revision) if revision.len() == 32 => {
                members.push(CompilerExportMember {
                    key: key.clone(),
                    revision: hexlify(revision),
                });
            }
            Value::Binary(_) => return Err("compiler record export revision has the wrong length".into()),
            _ => return Err("compiler record export revision must be binary".into()),
        }
        previous = key;
    }
    match &fields[0].value {
        Value::Binary(digest) if *digest == membership_digest(&members) => Ok(members),
        Value::Binary(_) => Err("compiler record export membership digest mismatch".into()),
        _ => Err("compiler record export has no membership digest".into()),
    }
}

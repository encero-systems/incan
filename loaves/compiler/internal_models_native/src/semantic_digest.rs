//! Hash the compiler's logical fields with the public Crumb canonical value and description algorithms.
use crate::checker_records::{CheckedRecord, validate};
use crate::general_rows::{operation_budget, owned_field};
use crate::{CodecFault, row_shapes::CHECKER};
use internal_models_bindings::{Projection, Value, description_hash, value_hash};
use sha2::{Digest, Sha256};

/// Hash the validated checker record and description under one shared operation budget.
pub fn semantic_digest(record: &CheckedRecord) -> Result<String, CodecFault> {
    let mut budget = operation_budget().map_err(|_| CodecFault::Limit)?;
    validate(record)?;
    let item_count = record.fields.iter().map(|f| 4 + f.values.len()).sum();
    let mut flat = Vec::with_capacity(item_count);
    for field in &record.fields {
        // Crumb owns the strings in its logical value while validating and hashing.
        flat.extend([
            Value::Text(field.family.clone()),
            Value::Text(field.node.clone()),
            Value::Text(field.selector.clone()),
            Value::Text(field.values.len().to_string()),
        ]);
        flat.extend(field.values.iter().cloned().map(Value::Text));
    }
    let logical = Value::Record(vec![
        owned_field(b"family", Value::Text(record.family.name().into()), true),
        owned_field(b"fields", Value::List(flat), true),
        owned_field(b"owner_", Value::Text(record.owner.clone()), true),
        owned_field(b"profil", Value::Text(record.profile.clone()), true),
        owned_field(b"root__", Value::Text(record.root.clone()), true),
    ]);
    let value = value_hash(logical, &Projection::Canonical, &mut budget, false).map_err(|_| CodecFault::Malformed)?;
    let description =
        description_hash(&CHECKER.description().metadata, &mut budget, false).map_err(|_| CodecFault::Malformed)?;
    let mut preimage = Vec::with_capacity(2 * (description.digest.len() + value.digest.len()) + 1);
    append_hex(&mut preimage, &description.digest);
    preimage.push(b'\n');
    append_hex(&mut preimage, &value.digest);
    Ok(format!("sha256:{:x}", Sha256::digest(&preimage)))
}

/// Append lowercase ASCII hexadecimal bytes directly to the bounded digest preimage.
fn append_hex(preimage: &mut Vec<u8>, data: &[u8]) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    for &byte in data {
        preimage.extend([DIGITS[usize::from(byte >> 4)], DIGITS[usize::from(byte & 15)]]);
    }
}

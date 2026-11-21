//! Generation-bound optional facts from ordinary admission of a published Loaf manifest.

use crate::crumb_rows::{ModelField, encode_rows_within, model_description, operation_budget, required_value};
use crate::graph_crumb_fields::{admit_graph_row, decode_graph_row, encode_graph_row, items, text};
use internal_models_bindings::{Description, RowRecord, Value, binary_message, read_record_at};
use sha2::{Digest, Sha256};

/// The public manifest digest and its optional, independently checked, family-specific value.
#[derive(Clone, PartialEq, Eq)]
pub struct ManifestCacheValue {
    pub digest: Vec<u8>,
    pub payload: Vec<u8>,
}
crate::incan_debug::incan_debug!(ManifestCacheValue { digest, payload });

fn family(kind: &str) -> Result<(&'static [u8], i64), String> {
    match kind {
        "identity" => Ok((b"lfmdig\x00", 8192)),
        "inventory" => Ok((b"lfminv\x00", 4_194_304)),
        "facts" => Ok((b"lfmfct\x00", 134_217_728)),
        _ => Err("unknown Loaf manifest cache family".into()),
    }
}

/// Codec work allowed per byte of a cache record's bound. Writing and admitting one binary payload charges each of its
/// bytes for the copy, the digest and the framing; the default of four per byte refuses a facts record well inside its
/// bound, so the cache of a dependency with a large Rust ABI was never written.
const WORK_PER_BOUND_BYTE: i64 = 16;

fn header(key: &[u8], kind: &str) -> Description {
    model_description(
        key,
        &format!("incan.loaf-manifest-cache/{kind}/2"),
        &[
            ModelField::new(b"check_", "checksum", "binary"),
            ModelField::new(b"guard_", "observed_generation", "binary"),
            ModelField::new(b"public", "manifest_digest", "binary"),
            ModelField::new(b"value_", "admitted_value", "binary"),
        ],
    )
}

fn checksum(key: &[u8], guard: &[u8], digest: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(key);
    hasher.update(guard);
    hasher.update(digest);
    hasher.update(Sha256::digest(payload));
    hasher.finalize().to_vec()
}

/// Bind a full digest, the observed file generation and the admitted facts before publication.
pub fn encode_manifest_cache(kind: &str, guard: &[u8], digest: &[u8], payload: &[u8]) -> Result<Vec<u8>, String> {
    let (key, maximum) = family(kind)?;
    if guard.len() != 32
        || digest.len() != 32
        || payload.len() as i64 > maximum
        || ((kind == "identity") != payload.is_empty())
    {
        return Err("invalid Loaf manifest cache fields".into());
    }
    let record = RowRecord {
        key: key.to_vec(),
        value: Value::Record(vec![
            required_value(b"check_", Value::Binary(checksum(key, guard, digest, payload))),
            required_value(b"guard_", Value::Binary(guard.to_vec())),
            required_value(b"public", Value::Binary(digest.to_vec())),
            required_value(b"value_", Value::Binary(payload.to_vec())),
        ]),
    };
    encode_rows_within(
        &header(key, kind),
        vec![record],
        maximum,
        &mut operation_budget(maximum * WORK_PER_BOUND_BYTE)?,
    )
}

/// Accept exactly one matching row with an intact checksum and the independently observed generation.
pub fn decode_manifest_cache(kind: &str, guard: &[u8], encoded: &[u8]) -> Result<ManifestCacheValue, String> {
    let (key, maximum) = family(kind)?;
    if guard.len() != 32 || encoded.len() as i64 > maximum {
        return Err("invalid Loaf manifest cache size".into());
    }
    let mut budget = operation_budget(maximum * WORK_PER_BOUND_BYTE)?;
    let rows = admit_graph_row(encoded, &header(key, kind), &mut budget, maximum)?;
    let row = read_record_at(&rows, 0, &mut budget).map_err(|error| binary_message(&error))?;
    if row.key != key {
        return Err("wrong Loaf manifest cache family".into());
    }
    if let Value::Record(fields) = &row.value
        && let [check, observed, digest, payload] = fields.as_slice()
        && let (Value::Binary(check), Value::Binary(observed), Value::Binary(digest), Value::Binary(payload)) =
            (&check.value, &observed.value, &digest.value, &payload.value)
        && observed.as_slice() == guard
        && digest.len() == 32
        && (kind == "identity") == payload.is_empty()
        && *check == checksum(key, guard, digest, payload)
    {
        return Ok(ManifestCacheValue {
            digest: digest.clone(),
            payload: payload.clone(),
        });
    }
    Err("Loaf manifest cache evidence differs".into())
}

/// The exact named and global facts admitted from a manifest before demand loading.
#[derive(Clone, PartialEq, Eq)]
pub struct ManifestInventory {
    pub name: String,
    pub version: String,
    pub features: Vec<String>,
    pub vocabulary: bool,
    pub operations: bool,
    pub native_unions: bool,
    pub rust_paths: Vec<String>,
    pub declares_abi_cache: bool,
    pub has_abi_cache: bool,
    pub abi_digest: Option<String>,
}
crate::incan_debug::incan_debug!(ManifestInventory {
    name,
    version,
    features,
    vocabulary,
    operations,
    native_unions,
    rust_paths,
    declares_abi_cache,
    has_abi_cache,
    abi_digest
});

const SUMMARY: &[u8] = b"lfmsum\x00";

fn summary_description() -> Description {
    model_description(
        SUMMARY,
        "incan.loaf-manifest-inventory/2",
        &[
            ModelField::new(b"abidcl", "declares_abi_cache", "boolean"),
            ModelField::new(b"abidig", "abi_digest", "option<string>"),
            ModelField::new(b"abihas", "has_abi_cache", "boolean"),
            ModelField::new(b"featur", "features", "list<string>"),
            ModelField::new(b"name__", "name", "string"),
            ModelField::new(b"ops___", "provider_operations", "boolean"),
            ModelField::new(b"paths_", "rust_paths", "list<string>"),
            ModelField::new(b"unions", "native_unions", "boolean"),
            ModelField::new(b"versio", "version", "string"),
            ModelField::new(b"vocabu", "vocabulary", "boolean"),
        ],
    )
}

fn ordered(value: &[String]) -> bool {
    value.windows(2).all(|pair| pair[1] > pair[0])
}

/// Encode the admitted inventory of one manifest.
pub fn encode_manifest_inventory(row: &ManifestInventory) -> Result<Vec<u8>, String> {
    if !ordered(&row.features) || !ordered(&row.rust_paths) {
        return Err("manifest index sets require strictly increasing order".into());
    }
    let abi = match &row.abi_digest {
        Some(digest) => Value::Some(vec![Value::Text(digest.clone())]),
        None => Value::NoneValue,
    };
    let texts = |values: &[String]| Value::List(values.iter().cloned().map(Value::Text).collect());
    encode_graph_row(
        SUMMARY,
        &summary_description(),
        vec![
            required_value(b"abidcl", Value::Boolean(row.declares_abi_cache)),
            required_value(b"abidig", abi),
            required_value(b"abihas", Value::Boolean(row.has_abi_cache)),
            required_value(b"featur", texts(&row.features)),
            required_value(b"name__", Value::Text(row.name.clone())),
            required_value(b"ops___", Value::Boolean(row.operations)),
            required_value(b"paths_", texts(&row.rust_paths)),
            required_value(b"unions", Value::Boolean(row.native_unions)),
            required_value(b"versio", Value::Text(row.version.clone())),
            required_value(b"vocabu", Value::Boolean(row.vocabulary)),
        ],
    )
}

fn strings(value: &Value) -> Result<Vec<String>, String> {
    let result = items(value)?.iter().map(text).collect::<Result<Vec<_>, _>>()?;
    if !ordered(&result) {
        return Err("invalid manifest inventory set".into());
    }
    Ok(result)
}

fn flag(value: &Value) -> Result<bool, String> {
    match value {
        Value::Boolean(found) => Ok(*found),
        _ => Err("expected manifest inventory boolean".into()),
    }
}

fn optional_string(value: &Value) -> Result<Option<String>, String> {
    match value {
        Value::NoneValue => Ok(None),
        Value::Some(parts) if parts.len() == 1 => Ok(Some(text(&parts[0])?)),
        _ => Err("invalid optional manifest ABI digest".into()),
    }
}

/// Decode the admitted inventory of one manifest.
pub fn decode_manifest_inventory(encoded: &[u8]) -> Result<ManifestInventory, String> {
    let f = decode_graph_row(encoded, SUMMARY, &summary_description(), 10)?;
    Ok(ManifestInventory {
        name: text(&f[4].value)?,
        version: text(&f[8].value)?,
        features: strings(&f[3].value)?,
        vocabulary: flag(&f[9].value)?,
        operations: flag(&f[5].value)?,
        native_unions: flag(&f[7].value)?,
        rust_paths: strings(&f[6].value)?,
        declares_abi_cache: flag(&f[0].value)?,
        has_abi_cache: flag(&f[2].value)?,
        abi_digest: optional_string(&f[1].value)?,
    })
}

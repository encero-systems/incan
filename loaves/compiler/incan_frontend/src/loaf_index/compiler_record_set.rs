//! Installed compiler-record selection from the sealed set named by this toolchain's pin (#2946).
//!
//! Oven converts and validates the installed standard/runtime manifests once. A compiler process sees exactly the
//! resulting root and digest in its environment, verifies the sealed-set receipt, and reads only content-addressed
//! record values. Packages outside the installed set retain their ordinary Loaf artifact contract.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::library_manifest::LibraryManifest;

use super::lazy_entry::Summary;
use super::{LoafIdentity, LoafIndexError};

const ROOT_ENV: &str = "INCAN_COMPILER_RECORD_ROOT";
const DIGEST_ENV: &str = "INCAN_COMPILER_RECORD_DIGEST";
const SET_FORMAT: &str = "incan.sealed-set/2";
const RECORD_PROFILE: &str = "incan.compiler-records/1";
const RECORD_CODEC: &str = "json/1";
const RECORD_ORIGIN: &str = "installed-toolchain";

static INSTALLED: OnceLock<Result<Option<RecordSet>, String>> = OnceLock::new();

/// Whether this compiler activation selected records mode, including a malformed partial selection that must refuse.
pub(super) fn configured() -> bool {
    std::env::var_os(ROOT_ENV).is_some() || std::env::var_os(DIGEST_ENV).is_some()
}

/// One compiler-record membership from the pinned sealed-set manifest.
struct RecordEntry {
    key: String,
    revision: String,
    value: String,
}

/// The immutable installed set after its root receipt has been authenticated.
struct RecordSet {
    root: PathBuf,
    entries: Vec<RecordEntry>,
}

/// Metadata retained separately from declarations so the original Loaf identity survives conversion.
#[derive(Deserialize)]
struct Metadata {
    name: String,
    version: String,
    incan_version: String,
    manifest_format: u32,
    manifest_digest: String,
    vocabulary: bool,
    provider_operations: bool,
    native_unions: bool,
    rust_paths: BTreeSet<String>,
    rust_abi_cache_declared: bool,
}

/// Read only the small, authenticated inventory record used to index an installed Loaf lazily.
pub(super) fn summary(dir: &Path, name: &str) -> Result<Option<Summary>, LoafIndexError> {
    let Some(set) = installed().map_err(|reason| unreadable(dir, name, reason))? else {
        return Ok(None);
    };
    let profile = dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| unreadable(dir, name, "Loaf profile directory is not UTF-8".to_owned()))?;
    let prefix = format!("loaf/{profile}/{name}/metadata/");
    let entries = set
        .entries
        .iter()
        .filter(|entry| entry.key.starts_with(&prefix))
        .collect::<Vec<_>>();
    if entries.is_empty() {
        return Ok(None);
    }
    if entries.len() != 1 {
        return Err(unreadable(
            dir,
            name,
            "compiler-record metadata requires exactly one value".to_owned(),
        ));
    }
    let entry = entries[0];
    if entry.revision != entry.value {
        return Err(unreadable(
            dir,
            name,
            format!(
                "compiler-record `{}` has distinct revision and value identities",
                entry.key
            ),
        ));
    }
    let metadata: Metadata =
        serde_json::from_str(&read_value(&set.root, &entry.value).map_err(|reason| unreadable(dir, name, reason))?)
            .map_err(|error| unreadable(dir, name, format!("invalid compiler-record metadata: {error}")))?;
    if metadata.name != name {
        return Err(unreadable(
            dir,
            name,
            format!("compiler records name the Loaf `{}`", metadata.name),
        ));
    }
    valid_digest(&metadata.manifest_digest).map_err(|reason| unreadable(dir, name, reason))?;
    Ok(Some(Summary {
        identity: LoafIdentity {
            name: metadata.name,
            version: metadata.version,
            digest: metadata.manifest_digest,
            feature_projection: BTreeSet::new(),
        },
        vocabulary: metadata.vocabulary,
        provider_operations: metadata.provider_operations,
        native_unions: metadata.native_unions,
        rust_paths: metadata.rust_paths,
        rust_abi_cache_declared: metadata.rust_abi_cache_declared,
        rust_abi_cache: metadata.rust_abi_cache_declared,
    }))
}

/// Read one installed standard/runtime Loaf through its independently addressed records.
pub(super) fn read(dir: &Path, name: &str) -> Result<Option<(Arc<LibraryManifest>, String)>, LoafIndexError> {
    let Some(set) = installed().map_err(|reason| unreadable(dir, name, reason))? else {
        return Ok(None);
    };
    let profile = dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| unreadable(dir, name, "Loaf profile directory is not UTF-8".to_owned()))?;
    let prefix = format!("loaf/{profile}/{name}/");
    let entries = {
        let _selection = tracing::trace_span!(
            target: "incan::invocation",
            "incan_frontend::select_compiler_record_group",
            loaf = name
        )
        .entered();
        set.entries
            .iter()
            .filter(|entry| entry.key.starts_with(&prefix))
            .collect::<Vec<_>>()
    };
    if entries.is_empty() {
        return Ok(None);
    }
    let _invocation = tracing::trace_span!(
        target: "incan::invocation",
        "incan_frontend::read_compiler_records",
        loaf = name,
        records = entries.len()
    )
    .entered();
    let mut families: BTreeMap<&str, Vec<(usize, String)>> = BTreeMap::new();
    for entry in entries {
        let identity = entry
            .key
            .strip_prefix(&prefix)
            .ok_or_else(|| unreadable(dir, name, "compiler-record key escaped its package prefix".to_owned()))?;
        let fields = identity.split('/').collect::<Vec<_>>();
        let [family, index, name_digest] = fields.as_slice() else {
            return Err(unreadable(
                dir,
                name,
                format!("malformed compiler-record key `{identity}`"),
            ));
        };
        let index = index
            .parse::<usize>()
            .map_err(|error| unreadable(dir, name, format!("malformed compiler-record index: {error}")))?;
        valid_digest(name_digest).map_err(|reason| unreadable(dir, name, reason))?;
        if entry.revision != entry.value {
            return Err(unreadable(
                dir,
                name,
                format!(
                    "compiler-record `{}` has distinct revision and value identities",
                    entry.key
                ),
            ));
        }
        let value = read_value(&set.root, &entry.value).map_err(|reason| unreadable(dir, name, reason))?;
        families.entry(family).or_default().push((index, value));
    }
    for values in families.values_mut() {
        values.sort_by_key(|(index, _)| *index);
        if values.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(unreadable(
                dir,
                name,
                "duplicate compiler-record family index".to_owned(),
            ));
        }
    }
    let metadata: Metadata = serde_json::from_str(&single(&mut families, "metadata", name, dir)?)
        .map_err(|error| unreadable(dir, name, format!("invalid compiler-record metadata: {error}")))?;
    if metadata.name != name {
        return Err(unreadable(
            dir,
            name,
            format!("compiler records name the Loaf `{}`", metadata.name),
        ));
    }
    valid_digest(&metadata.manifest_digest).map_err(|reason| unreadable(dir, name, reason))?;
    let encoded = format!(
        concat!(
            "{{\"name\":{},\"version\":{},\"incan_version\":{},\"manifest_format\":{},",
            "\"exports\":{{\"aliases\":{},\"partials\":{},\"models\":{},\"classes\":{},",
            "\"functions\":{},\"traits\":{},\"enums\":{},\"type_aliases\":{},\"newtypes\":{},",
            "\"consts\":{},\"statics\":{}}},\"made_private_types\":{},\"vocab\":{},",
            "\"soft_keywords\":{{\"activations\":{}}},\"contract_metadata\":{},\"rust_abi\":{}}}"
        ),
        serde_json::to_string(&metadata.name).map_err(|error| unreadable(dir, name, error.to_string()))?,
        serde_json::to_string(&metadata.version).map_err(|error| unreadable(dir, name, error.to_string()))?,
        serde_json::to_string(&metadata.incan_version).map_err(|error| unreadable(dir, name, error.to_string()))?,
        metadata.manifest_format,
        array_values(&mut families, "alias"),
        array_values(&mut families, "partial"),
        array_values(&mut families, "model"),
        array_values(&mut families, "class"),
        array_values(&mut families, "function"),
        array_values(&mut families, "trait"),
        array_values(&mut families, "enum"),
        array_values(&mut families, "type-alias"),
        array_values(&mut families, "newtype"),
        array_values(&mut families, "const"),
        array_values(&mut families, "static"),
        array_values(&mut families, "made-private-type"),
        optional(&mut families, "vocabulary", name, dir)?,
        array_values(&mut families, "soft-keyword"),
        single(&mut families, "contract-metadata", name, dir)?,
        optional(&mut families, "rust-abi", name, dir)?,
    );
    if let Some((family, _)) = families.first_key_value() {
        return Err(unreadable(
            dir,
            name,
            format!("unsupported compiler-record family `{family}`"),
        ));
    }
    let manifest = LibraryManifest::from_compiler_record_json(&encoded)
        .map_err(|error| unreadable(dir, name, format!("invalid installed compiler records: {error}")))?;
    Ok(Some((Arc::new(manifest), metadata.manifest_digest)))
}

/// Return the process's one immutable installed set; the worker cannot select another root between requests.
fn installed() -> Result<Option<&'static RecordSet>, String> {
    match INSTALLED.get_or_init(load_installed) {
        Ok(Some(set)) => Ok(Some(set)),
        Ok(None) => Ok(None),
        Err(reason) => Err(reason.clone()),
    }
}

/// Load and authenticate the set receipt supplied by Oven, refusing a partial environment or digest mismatch.
fn load_installed() -> Result<Option<RecordSet>, String> {
    let _invocation = tracing::trace_span!(
        target: "incan::invocation",
        "incan_frontend::authenticate_compiler_record_set"
    )
    .entered();
    let (root, digest) = match (std::env::var_os(ROOT_ENV), std::env::var_os(DIGEST_ENV)) {
        (None, None) => return Ok(None),
        (Some(root), Some(digest)) => (root, digest),
        _ => return Err("compiler-record root and digest must be supplied together".to_owned()),
    };
    let root = PathBuf::from(root);
    let digest = digest
        .into_string()
        .map_err(|_| "compiler-record digest is not UTF-8".to_owned())?;
    valid_digest(&digest)?;
    let path = root.join("manifest.json");
    let bytes = fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if !digest_matches(&bytes, &digest) {
        return Err("compiler-record manifest differs from the installed toolchain pin".to_owned());
    }
    let document: Value =
        serde_json::from_slice(&bytes).map_err(|error| format!("invalid compiler-record set: {error}"))?;
    let fields = array(&document, 10, "compiler-record set")?;
    if text(&fields[0], "set format")? != SET_FORMAT {
        return Err("unsupported compiler-record set format".to_owned());
    }
    let profile = array(&fields[1], 3, "compiler-record profile")?;
    if text(&profile[0], "record identity")? != RECORD_PROFILE
        || text(&profile[1], "record schema")? != RECORD_PROFILE
        || text(&profile[2], "record codec")? != RECORD_CODEC
        || text(&fields[2], "record origin")? != RECORD_ORIGIN
    {
        return Err("installed compiler-record profile does not match this compiler".to_owned());
    }
    let members = fields[5]
        .as_array()
        .ok_or_else(|| "compiler-record membership is not an array".to_owned())?;
    let mut entries = Vec::with_capacity(members.len());
    for member in members {
        let member = array(member, 6, "compiler-record member")?;
        let key = text(&member[0], "record key")?.to_owned();
        let revision = text(&member[1], "record revision")?.to_owned();
        let value = text(&member[2], "record value")?.to_owned();
        valid_digest(&revision)?;
        valid_digest(&value)?;
        entries.push(RecordEntry { key, revision, value });
    }
    Ok(Some(RecordSet { root, entries }))
}

/// Compare a payload with its canonical SHA-256 spelling without allocating another hexadecimal string.
fn digest_matches(bytes: &[u8], expected: &str) -> bool {
    let actual = Sha256::digest(bytes);
    expected.as_bytes().chunks_exact(2).zip(actual).all(|(encoded, byte)| {
        let decode = |digit: u8| match digit {
            b'0'..=b'9' => digit - b'0',
            b'a'..=b'f' => digit - b'a' + 10,
            _ => u8::MAX,
        };
        decode(encoded[0]) << 4 | decode(encoded[1]) == byte
    })
}

/// Read and authenticate one immutable value file from the sealed transport.
fn read_value(root: &Path, digest: &str) -> Result<String, String> {
    valid_digest(digest)?;
    let directory = root.join("payloads");
    let path = directory.join(digest);
    let bytes = {
        if fs::symlink_metadata(&directory)
            .map_err(|error| error.to_string())?
            .file_type()
            .is_symlink()
            || fs::symlink_metadata(&path)
                .map_err(|error| error.to_string())?
                .file_type()
                .is_symlink()
        {
            return Err(format!("compiler-record object is a symbolic link: {}", path.display()));
        }
        fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?
    };
    if !digest_matches(&bytes, digest) {
        return Err(format!(
            "compiler-record object differs from its content name: {}",
            path.display()
        ));
    }
    String::from_utf8(bytes).map_err(|error| format!("compiler-record object is not UTF-8 JSON: {error}"))
}

/// Remove all ordered values for one repeatable record family.
fn values(families: &mut BTreeMap<&str, Vec<(usize, String)>>, family: &str) -> Vec<String> {
    families
        .remove(family)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, value)| value)
        .collect()
}

/// Assemble authenticated JSON values as one array without decoding and re-encoding each member.
fn array_values(families: &mut BTreeMap<&str, Vec<(usize, String)>>, family: &str) -> String {
    format!("[{}]", values(families, family).join(","))
}

/// Remove exactly one required value for a singleton family.
fn single(
    families: &mut BTreeMap<&str, Vec<(usize, String)>>,
    family: &str,
    name: &str,
    dir: &Path,
) -> Result<String, LoafIndexError> {
    let mut selected = values(families, family);
    if selected.len() != 1 {
        return Err(unreadable(
            dir,
            name,
            format!("compiler-record family `{family}` requires exactly one value"),
        ));
    }
    selected
        .pop()
        .ok_or_else(|| unreadable(dir, name, format!("compiler-record family `{family}` is absent")))
}

/// Remove zero or one value for an optional singleton family.
fn optional(
    families: &mut BTreeMap<&str, Vec<(usize, String)>>,
    family: &str,
    name: &str,
    dir: &Path,
) -> Result<String, LoafIndexError> {
    let mut selected = values(families, family);
    if selected.len() > 1 {
        return Err(unreadable(
            dir,
            name,
            format!("compiler-record family `{family}` has multiple values"),
        ));
    }
    Ok(selected.pop().unwrap_or_else(|| "null".to_owned()))
}

/// Require a JSON array of one exact arity.
fn array<'a>(value: &'a Value, length: usize, label: &str) -> Result<&'a [Value], String> {
    let values = value.as_array().ok_or_else(|| format!("{label} is not an array"))?;
    if values.len() != length {
        return Err(format!("{label} has {} fields, expected {length}", values.len()));
    }
    Ok(values)
}

/// Require one JSON string field.
fn text<'a>(value: &'a Value, label: &str) -> Result<&'a str, String> {
    value.as_str().ok_or_else(|| format!("{label} is not a string"))
}

/// Require a canonical full SHA-256 spelling before using it as identity or path component.
fn valid_digest(value: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("invalid compiler-record digest `{value}`"));
    }
    Ok(())
}

/// Attach record failures to the corresponding ordinary Loaf artifact path.
fn unreadable(dir: &Path, name: &str, reason: String) -> LoafIndexError {
    LoafIndexError::Unreadable {
        path: dir.join(format!("{name}.incnlib")),
        reason,
    }
}

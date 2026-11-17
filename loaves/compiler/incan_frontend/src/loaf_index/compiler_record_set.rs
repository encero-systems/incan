//! Installed compiler-record selection from the sealed set named by this toolchain's pin (#2946).
//!
//! Oven converts and validates the installed standard/runtime manifests once. A compiler process sees exactly the
//! resulting root and digest in its environment, verifies the sealed-set receipt, and reads only content-addressed
//! record values. Each Loaf's record group is read and authenticated once per process; later demands in the same
//! process, including later compilations in one worker, share the decoded group. Packages outside the installed set
//! retain their ordinary Loaf artifact contract.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use serde::de::DeserializeOwned;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::library_manifest::{
    AliasExport, ClassExport, ConstExport, EnumExport, FunctionExport, LibraryContractMetadata, LibraryExports,
    LibraryManifest, LibraryRustAbi, MadePrivateTypeExport, ModelExport, NewtypeExport, PartialExport,
    SoftKeywordActivation, SoftKeywordExports, StaticExport, TraitExport, TypeAliasExport, VocabExports,
};

use super::LoafIndexError;
use super::lazy_entry::Summary;
use crate::api_metadata::ApiDocumentation;
use crate::library_manifest::executable_surface::ExecutableSurface;
use incan_semantics_core::body_ir::RuntimeCallableBinding;

mod declaration_records;
mod inventory;
mod runtime_bindings;
#[cfg(test)]
mod thread_selection;
use declaration_records::NAMED_FAMILIES;
pub(super) use declaration_records::{names_record, read_declaration};
pub(super) use inventory::summary;
use inventory::{Metadata, read_summary};
pub(super) use runtime_bindings::{read_runtime_bindings, runtime_binding_selectors};

/// The record family holding a Loaf's checked API documentation, which compiles never open (RFC 179).
const DOCUMENTATION: &str = "documentation";
/// The record families of a Loaf's executable surface besides its metadata, and its runtime callable bindings; a
/// manifest projection never selects them.
const EXECUTABLE_DESCRIPTOR: &str = "executable-descriptor";
const PUBLIC_IDENTITIES: &str = "public-identities";
const RUNTIME_BINDINGS: &str = "runtime-bindings";
const ROOT_ENV: &str = "INCAN_COMPILER_RECORD_ROOT";
const DIGEST_ENV: &str = "INCAN_COMPILER_RECORD_DIGEST";
const SET_FORMAT: &str = "incan.sealed-set/2";
const RECORD_PROFILE: &str = "incan.compiler-records/2";
const RECORD_CODEC: &str = "incan.compiler-record-value/1";
const RECORD_ORIGIN: &str = "installed-toolchain";

static INSTALLED: OnceLock<Result<Option<RecordSet>, String>> = OnceLock::new();

/// Whether this compiler activation selected records mode, including a malformed partial selection that must refuse.
pub(super) fn configured() -> bool {
    #[cfg(test)]
    if thread_selection::selected().is_some() {
        return true;
    }
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
    /// Records this process has read from the payload directory.
    records_read: AtomicUsize,
    /// Payload bytes this process has read from the payload directory.
    bytes_read: AtomicUsize,
    /// Record groups this process already read, authenticated and decoded, by package prefix and projection.
    admitted: Mutex<BTreeMap<(String, Projection), AdmittedGroup>>,
    /// Inventory summaries this process already read and authenticated, by metadata key prefix.
    summaries: Mutex<BTreeMap<String, Summary>>,
    /// Executable surfaces this process already read and authenticated, by package prefix.
    surfaces: Mutex<BTreeMap<String, (Arc<ExecutableSurface>, String)>>,
    /// Runtime callable bindings this process already read, by record key prefix.
    bindings: Mutex<BTreeMap<String, (Arc<Vec<RuntimeCallableBinding>>, String)>>,
}

/// One authenticated, decoded record group and the manifest digest its metadata names.
type AdmittedGroup = (Arc<LibraryManifest>, String);

/// Records and payload bytes one installed set has read from its payload directory.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RecordReads {
    records: usize,
    bytes: usize,
}

/// The part of one Loaf's record group a reader asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Projection {
    /// Every record: the complete library manifest.
    Complete,
    /// The records executable-resolution admission consults.
    ///
    /// Top-level callable identities and the executable descriptor live in contract metadata. Nominal records add the
    /// public members that representation coverage must admit. Callable, alias, constant, static, vocabulary and Rust
    /// ABI values are unrelated to this demand and remain unopened.
    Execution,
    /// The complete library manifest with its checked API documentation, for readers that show documentation.
    Documented,
}

impl Projection {
    /// Whether records of `family` belong to this projection.
    fn admits(self, family: &str) -> bool {
        let surface = matches!(family, EXECUTABLE_DESCRIPTOR | PUBLIC_IDENTITIES | RUNTIME_BINDINGS)
            || NAMED_FAMILIES.contains(&family);
        match self {
            Self::Complete => family != DOCUMENTATION && !surface,
            Self::Execution => matches!(
                family,
                "metadata" | "contract-metadata" | "model" | "class" | "trait" | "enum" | "newtype"
            ),
            Self::Documented => !surface,
        }
    }

    /// The admitted projections that can answer a demand for this one, nearest first. The complete group answers an
    /// executable demand; only a documented group answers a documented demand, and none answers a compile's.
    fn answered_by(self) -> &'static [Projection] {
        match self {
            Self::Complete => &[Self::Complete],
            Self::Execution => &[Self::Execution, Self::Complete],
            Self::Documented => &[Self::Documented],
        }
    }
}

impl RecordSet {
    /// An installed set whose receipt the caller authenticated, before any payload is read.
    fn new(root: PathBuf, entries: Vec<RecordEntry>) -> Self {
        Self {
            root,
            entries,
            records_read: AtomicUsize::new(0),
            bytes_read: AtomicUsize::new(0),
            admitted: Mutex::new(BTreeMap::new()),
            summaries: Mutex::new(BTreeMap::new()),
            surfaces: Mutex::new(BTreeMap::new()),
            bindings: Mutex::new(BTreeMap::new()),
        }
    }

    /// A group this process already authenticated that answers `projection`.
    fn admitted(&self, prefix: &str, projection: Projection) -> Option<AdmittedGroup> {
        let admitted = self.admitted.lock().unwrap_or_else(PoisonError::into_inner);
        projection
            .answered_by()
            .iter()
            .find_map(|answer| admitted.get(&(prefix.to_owned(), *answer)))
            .map(|(manifest, digest)| (Arc::clone(manifest), digest.clone()))
    }

    /// Keep one authenticated group for the rest of the process: its values are content-named under a receipt that
    /// cannot change while the process runs, so reading and hashing them again could only return the same bytes.
    fn admit(&self, prefix: String, projection: Projection, group: &AdmittedGroup) {
        self.admitted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((prefix, projection), (Arc::clone(&group.0), group.1.clone()));
    }

    /// Records and payload bytes read from the payload directory so far.
    fn reads(&self) -> RecordReads {
        RecordReads {
            records: self.records_read.load(Ordering::Relaxed),
            bytes: self.bytes_read.load(Ordering::Relaxed),
        }
    }

    /// Count one payload read from the payload directory, the value of the record `key`.
    fn count_read(&self, key: &str, bytes: usize) {
        self.records_read.fetch_add(1, Ordering::Relaxed);
        self.bytes_read.fetch_add(bytes, Ordering::Relaxed);
        #[cfg(test)]
        thread_selection::note_read(key);
        tracing::trace!(
            target: "incan::invocation",
            phase = "record_read",
            record = key,
            bytes
        );
    }
}

/// Read one installed standard/runtime Loaf through its independently addressed records.
pub(super) fn read(dir: &Path, name: &str) -> Result<Option<(Arc<LibraryManifest>, String)>, LoafIndexError> {
    read_selected(dir, name, Projection::Complete)
}

/// Read only the manifest records executable-resolution admission actually consults.
pub(super) fn read_execution(dir: &Path, name: &str) -> Result<Option<(Arc<LibraryManifest>, String)>, LoafIndexError> {
    read_selected(dir, name, Projection::Execution)
}

/// Read one installed Loaf with its checked API documentation, which compiles never read.
pub(super) fn read_documented(
    dir: &Path,
    name: &str,
) -> Result<Option<(Arc<LibraryManifest>, String)>, LoafIndexError> {
    read_selected(dir, name, Projection::Documented)
}

/// Read what executable resolution needs of one installed Loaf: its name, version, descriptor and public identities.
pub(super) fn read_surface(dir: &Path, name: &str) -> Result<Option<(Arc<ExecutableSurface>, String)>, LoafIndexError> {
    let Some(set) = installed().map_err(|reason| unreadable(dir, name, reason))? else {
        return Ok(None);
    };
    read_surface_in(set, dir, name)
}

/// Read one Loaf group's executable surface from `set`: its metadata, descriptor and public identities records only.
///
/// A set converted before surfaces were published has no public-identities record; the caller then derives the
/// surface from the executable projection.
fn read_surface_in(
    set: &RecordSet,
    dir: &Path,
    name: &str,
) -> Result<Option<(Arc<ExecutableSurface>, String)>, LoafIndexError> {
    let profile = dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| unreadable(dir, name, "Loaf profile directory is not UTF-8".to_owned()))?;
    let prefix = format!("loaf/{profile}/{name}/");
    if let Some(found) = set.surfaces.lock().unwrap_or_else(PoisonError::into_inner).get(&prefix) {
        return Ok(Some(found.clone()));
    }
    let entries = set
        .entries
        .iter()
        .filter(|entry| entry.key.starts_with(&prefix))
        .collect::<Vec<_>>();
    let selected = select_records(entries, &prefix, dir, name)?
        .into_iter()
        .filter(|(family, _, _)| matches!(*family, "metadata" | EXECUTABLE_DESCRIPTOR | PUBLIC_IDENTITIES))
        .collect::<Vec<_>>();
    if !selected.iter().any(|(family, _, _)| *family == PUBLIC_IDENTITIES) {
        return Ok(None);
    }
    validate_payload_directory(&set.root).map_err(|reason| unreadable(dir, name, reason))?;
    let mut metadata: Option<Metadata> = None;
    let mut surface = ExecutableSurface {
        name: name.to_owned(),
        version: String::new(),
        descriptor: None,
        public_identities: BTreeSet::new(),
    };
    for (family, _, entry) in selected {
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
        let bytes = open_value(&set.root, &entry.value)
            .map_err(|reason| unreadable(dir, name, record_failure(&entry.key, reason)))?;
        set.count_read(&entry.key, bytes.len());
        let invalid = |error| unreadable(dir, name, format!("invalid compiler-record family `{family}`: {error}"));
        match family {
            "metadata" => metadata = Some(LibraryManifest::decode_compiler_record_value(&bytes).map_err(invalid)?),
            EXECUTABLE_DESCRIPTOR => {
                surface.descriptor = Some(LibraryManifest::decode_compiler_record_value(&bytes).map_err(invalid)?)
            }
            _ => surface.public_identities = LibraryManifest::decode_compiler_record_value(&bytes).map_err(invalid)?,
        }
    }
    let metadata = metadata.ok_or_else(|| {
        unreadable(
            dir,
            name,
            "compiler-record metadata requires exactly one value".to_owned(),
        )
    })?;
    if metadata.name != name {
        return Err(unreadable(
            dir,
            name,
            format!("compiler records name the Loaf `{}`", metadata.name),
        ));
    }
    surface.version = metadata.version;
    let found = (Arc::new(surface), metadata.manifest_digest);
    set.surfaces
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(prefix, found.clone());
    Ok(Some(found))
}

/// Read one semantic projection from the process's installed set without opening unselected values.
fn read_selected(
    dir: &Path,
    name: &str,
    projection: Projection,
) -> Result<Option<(Arc<LibraryManifest>, String)>, LoafIndexError> {
    let Some(set) = installed().map_err(|reason| unreadable(dir, name, reason))? else {
        return Ok(None);
    };
    read_group(set, dir, name, projection)
}

/// Read one semantic projection from a sealed Loaf group of `set` without opening unselected values.
fn read_group(
    set: &RecordSet,
    dir: &Path,
    name: &str,
    projection: Projection,
) -> Result<Option<(Arc<LibraryManifest>, String)>, LoafIndexError> {
    let profile = dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| unreadable(dir, name, "Loaf profile directory is not UTF-8".to_owned()))?;
    let prefix = format!("loaf/{profile}/{name}/");
    if let Some(group) = set.admitted(&prefix, projection) {
        tracing::trace!(target: "incan::invocation", phase = "reuse", loaf = name);
        return Ok(Some(group));
    }
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
    let selected = select_records(entries, &prefix, dir, name)?
        .into_iter()
        .filter(|(family, _, _)| projection.admits(family))
        .collect::<Vec<_>>();
    let records = selected.len();
    let _invocation = tracing::trace_span!(
        target: "incan::invocation",
        "incan_frontend::read_compiler_records",
        loaf = name,
        records
    )
    .entered();
    let opened = {
        let _open = tracing::trace_span!(
            target: "incan::invocation",
            "incan_frontend::open_compiler_record_group",
            loaf = name,
            records
        )
        .entered();
        validate_payload_directory(&set.root).map_err(|reason| unreadable(dir, name, reason))?;
        let opened = selected
            .into_iter()
            .map(|(family, index, entry)| {
                open_value(&set.root, &entry.value)
                    .map(|bytes| {
                        set.count_read(&entry.key, bytes.len());
                        (family, index, entry, bytes)
                    })
                    .map_err(|reason| unreadable(dir, name, record_failure(&entry.key, reason)))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let bytes = opened.iter().map(|(_, _, _, value)| value.len()).sum::<usize>();
        let process = set.reads();
        tracing::trace!(
            target: "incan::invocation",
            phase = "open",
            loaf = name,
            records,
            bytes,
            process_records = process.records,
            process_bytes = process.bytes
        );
        opened
    };
    let bytes = opened.iter().map(|(_, _, _, value)| value.len()).sum::<usize>();
    let verified = {
        let _verification = tracing::trace_span!(
            target: "incan::invocation",
            "incan_frontend::verify_compiler_record_group",
            loaf = name,
            records,
            bytes
        )
        .entered();
        let verified = opened
            .into_iter()
            .map(|(family, index, entry, bytes)| {
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
                Ok((family, index, bytes))
            })
            .collect::<Result<Vec<_>, _>>()?;
        tracing::trace!(target: "incan::invocation", phase = "verification", loaf = name, records, bytes);
        verified
    };
    let families = {
        let _materialization = tracing::trace_span!(
            target: "incan::invocation",
            "incan_frontend::materialize_compiler_record_group",
            loaf = name,
            records,
            bytes
        )
        .entered();
        let materialized = materialize(verified, dir, name)?;
        tracing::trace!(target: "incan::invocation", phase = "materialization", loaf = name, records, bytes);
        materialized
    };
    let (manifest, manifest_digest) = {
        let _decode = tracing::trace_span!(
            target: "incan::invocation",
            "incan_frontend::decode_compiler_record_group",
            loaf = name,
            records,
            bytes
        )
        .entered();
        let manifest = decode_manifest(families, dir, name)?;
        tracing::trace!(target: "incan::invocation", phase = "decode", loaf = name, records, bytes);
        manifest
    };
    let group = (Arc::new(manifest), manifest_digest);
    set.admit(prefix, projection, &group);
    Ok(Some(group))
}

/// Validate and select one Loaf group's record identities without opening their independently addressed values.
fn select_records<'a>(
    entries: Vec<&'a RecordEntry>,
    prefix: &str,
    dir: &Path,
    name: &str,
) -> Result<Vec<(&'a str, usize, &'a RecordEntry)>, LoafIndexError> {
    let _selection = tracing::trace_span!(
        target: "incan::invocation",
        "incan_frontend::classify_compiler_record_group",
        loaf = name,
        records = entries.len()
    )
    .entered();
    entries
        .into_iter()
        .map(|entry| {
            let identity = entry
                .key
                .strip_prefix(prefix)
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
            Ok((*family, index, entry))
        })
        .collect()
}

/// Independently addressed values grouped by the family and ordinal already validated from their record identities.
type RecordFamilies<'a> = BTreeMap<&'a str, Vec<(usize, Vec<u8>)>>;

/// Order one authenticated group by its selected family identities without decoding its values.
fn materialize<'a>(
    records: Vec<(&'a str, usize, Vec<u8>)>,
    dir: &Path,
    name: &str,
) -> Result<RecordFamilies<'a>, LoafIndexError> {
    let mut families: RecordFamilies<'a> = BTreeMap::new();
    for (family, index, value) in records {
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
    Ok(families)
}

/// Decode each selected record directly into its family type after installation admitted the complete manifest.
fn decode_manifest(
    mut families: RecordFamilies<'_>,
    dir: &Path,
    name: &str,
) -> Result<(LibraryManifest, String), LoafIndexError> {
    let metadata: Metadata = decode_single(&mut families, "metadata", name, dir)?;
    if metadata.name != name {
        return Err(unreadable(
            dir,
            name,
            format!("compiler records name the Loaf `{}`", metadata.name),
        ));
    }
    valid_digest(&metadata.manifest_digest).map_err(|reason| unreadable(dir, name, reason))?;
    let vocab = optional_value(&mut families, "vocabulary", name, dir)?
        .map(|value| VocabExports::decode_compiler_record_value(&value))
        .transpose()
        .map_err(|error| unreadable(dir, name, format!("invalid compiler-record vocabulary: {error}")))?;
    let mut contract_metadata =
        decode_single::<LibraryContractMetadata>(&mut families, "contract-metadata", name, dir)?;
    // Only a documented read selects the documentation record; a compile's projections never open it.
    if let Some(documentation) = decode_optional::<Vec<ApiDocumentation>>(&mut families, DOCUMENTATION, name, dir)? {
        contract_metadata
            .restore_documentation(documentation)
            .map_err(|reason| unreadable(dir, name, format!("invalid compiler-record documentation: {reason}")))?;
    }
    let manifest = LibraryManifest {
        name: metadata.name,
        version: metadata.version,
        incan_version: metadata.incan_version,
        manifest_format: metadata.manifest_format,
        exports: LibraryExports {
            aliases: decode_values::<AliasExport>(&mut families, "alias", name, dir)?,
            partials: decode_values::<PartialExport>(&mut families, "partial", name, dir)?,
            models: decode_values::<ModelExport>(&mut families, "model", name, dir)?,
            classes: decode_values::<ClassExport>(&mut families, "class", name, dir)?,
            functions: decode_values::<FunctionExport>(&mut families, "function", name, dir)?,
            traits: decode_values::<TraitExport>(&mut families, "trait", name, dir)?,
            enums: decode_values::<EnumExport>(&mut families, "enum", name, dir)?,
            type_aliases: decode_values::<TypeAliasExport>(&mut families, "type-alias", name, dir)?,
            newtypes: decode_values::<NewtypeExport>(&mut families, "newtype", name, dir)?,
            consts: decode_values::<ConstExport>(&mut families, "const", name, dir)?,
            statics: decode_values::<StaticExport>(&mut families, "static", name, dir)?,
        },
        made_private_types: decode_values::<MadePrivateTypeExport>(&mut families, "made-private-type", name, dir)?,
        vocab,
        soft_keywords: SoftKeywordExports {
            activations: decode_values::<SoftKeywordActivation>(&mut families, "soft-keyword", name, dir)?,
        },
        contract_metadata,
        rust_abi: decode_optional::<LibraryRustAbi>(&mut families, "rust-abi", name, dir)?,
    };
    if let Some((family, _)) = families.first_key_value() {
        return Err(unreadable(
            dir,
            name,
            format!("unsupported compiler-record family `{family}`"),
        ));
    }
    Ok((manifest, metadata.manifest_digest))
}

/// Return the process's one immutable installed set; the worker cannot select another root between requests.
fn installed() -> Result<Option<&'static RecordSet>, String> {
    #[cfg(test)]
    if let Some(set) = thread_selection::selected() {
        return Ok(Some(set));
    }
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
    Ok(Some(RecordSet::new(root, entries)))
}

/// Compare a payload with its canonical SHA-256 spelling without allocating another hexadecimal string.
fn digest_matches(bytes: &[u8], expected: &str) -> bool {
    let actual = Sha256::digest(bytes);
    expected
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .zip(actual)
        .all(|(encoded, byte)| {
            let decode = |digit: u8| match digit {
                b'0'..=b'9' => digit - b'0',
                b'a'..=b'f' => digit - b'a' + 10,
                _ => u8::MAX,
            };
            decode(encoded[0]) << 4 | decode(encoded[1]) == byte
        })
}

/// Read one immutable value after the installed set receipt named it; the value itself is authenticated on open.
fn read_value(root: &Path, digest: &str) -> Result<Vec<u8>, String> {
    validate_payload_directory(root)?;
    open_value(root, digest)
}

/// Refuse replacement of the immutable payload directory once before opening a selected record group.
fn validate_payload_directory(root: &Path) -> Result<(), String> {
    let directory = root.join("payloads");
    if fs::symlink_metadata(&directory)
        .map_err(|error| {
            format!(
                "cannot inspect compiler-record directory {}: {error}",
                directory.display()
            )
        })?
        .file_type()
        .is_symlink()
    {
        return Err(format!(
            "compiler-record payload directory is a symbolic link: {}",
            directory.display()
        ));
    }
    Ok(())
}

/// Open one canonical value selected by the authenticated set receipt, refusing symbolic-link substitution and any
/// bytes that are not the content their name promises; a payload that still decodes is no exception.
fn open_value(root: &Path, digest: &str) -> Result<Vec<u8>, String> {
    valid_digest(digest)?;
    let directory = root.join("payloads");
    let path = directory.join(digest);
    if fs::symlink_metadata(&path)
        .map_err(|error| format!("cannot inspect compiler-record object {}: {error}", path.display()))?
        .file_type()
        .is_symlink()
    {
        return Err(format!("compiler-record object is a symbolic link: {}", path.display()));
    }
    let bytes = fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if !digest_matches(&bytes, digest) {
        return Err(format!(
            "compiler-record payload {digest} does not match its content name: {}",
            path.display()
        ));
    }
    Ok(bytes)
}

/// Remove all ordered values for one repeatable record family.
fn values(families: &mut RecordFamilies<'_>, family: &str) -> Vec<Vec<u8>> {
    families
        .remove(family)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, value)| value)
        .collect()
}

/// Decode every ordered value of one repeatable family through the profile's exact compact codec.
fn decode_values<T: DeserializeOwned>(
    families: &mut RecordFamilies<'_>,
    family: &str,
    name: &str,
    dir: &Path,
) -> Result<Vec<T>, LoafIndexError> {
    values(families, family)
        .into_iter()
        .map(|value| {
            LibraryManifest::decode_compiler_record_value(&value)
                .map_err(|error| unreadable(dir, name, format!("invalid compiler-record family `{family}`: {error}")))
        })
        .collect()
}

/// Remove exactly one required value for a singleton family.
fn single_value(
    families: &mut RecordFamilies<'_>,
    family: &str,
    name: &str,
    dir: &Path,
) -> Result<Vec<u8>, LoafIndexError> {
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
fn optional_value(
    families: &mut RecordFamilies<'_>,
    family: &str,
    name: &str,
    dir: &Path,
) -> Result<Option<Vec<u8>>, LoafIndexError> {
    let mut selected = values(families, family);
    if selected.len() > 1 {
        return Err(unreadable(
            dir,
            name,
            format!("compiler-record family `{family}` has multiple values"),
        ));
    }
    Ok(selected.pop())
}

/// Decode exactly one required family value after enforcing its singleton cardinality.
fn decode_single<T: DeserializeOwned>(
    families: &mut RecordFamilies<'_>,
    family: &str,
    name: &str,
    dir: &Path,
) -> Result<T, LoafIndexError> {
    let value = single_value(families, family, name, dir)?;
    LibraryManifest::decode_compiler_record_value(&value)
        .map_err(|error| unreadable(dir, name, format!("invalid compiler-record family `{family}`: {error}")))
}

/// Decode an optional singleton family while preserving absence independently from a malformed value.
fn decode_optional<T: DeserializeOwned>(
    families: &mut RecordFamilies<'_>,
    family: &str,
    name: &str,
    dir: &Path,
) -> Result<Option<T>, LoafIndexError> {
    optional_value(families, family, name, dir)?
        .map(|value| {
            LibraryManifest::decode_compiler_record_value(&value)
                .map_err(|error| unreadable(dir, name, format!("invalid compiler-record family `{family}`: {error}")))
        })
        .transpose()
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

/// Name the compiler record whose payload could not be read or authenticated.
fn record_failure(key: &str, reason: String) -> String {
    format!("compiler record `{key}`: {reason}")
}

/// Attach record failures to the corresponding ordinary Loaf artifact path.
fn unreadable(dir: &Path, name: &str, reason: String) -> LoafIndexError {
    LoafIndexError::Unreadable {
        path: dir.join(format!("{name}.incnlib")),
        reason,
    }
}

#[cfg(test)]
mod tests {
    mod member_families;
    mod namespace_records;
    mod type_scopes;
    mod typechecker_reads;

    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;

    use sha2::{Digest, Sha256};

    use super::declaration_records::read_declaration_in;
    use super::runtime_bindings::read_runtime_bindings_in;
    use super::{
        Metadata, Projection, RecordEntry, RecordReads, RecordSet, open_value, read_group, read_summary,
        read_surface_in,
    };
    use crate::api_metadata::{
        CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
    };
    use crate::library_manifest::executable_surface::{ExecutableSurface, runtime_bindings};
    use crate::library_manifest::{ExecutableRepresentationExport, LibraryContractMetadata, LibraryManifest};

    /// The canonical lowercase SHA-256 spelling of `bytes`.
    fn content_name(bytes: &[u8]) -> String {
        Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// Install `values` as the records of one Loaf group named `name` in a fresh payload directory under `root`.
    fn record_set(
        root: &Path,
        name: &str,
        values: &[(&str, Vec<u8>)],
    ) -> Result<RecordSet, Box<dyn std::error::Error>> {
        fs::create_dir(root.join("payloads"))?;
        let mut entries = Vec::new();
        for (family, value) in values {
            let digest = content_name(value);
            fs::write(root.join("payloads").join(&digest), value)?;
            entries.push(RecordEntry {
                key: format!(
                    "loaf/debug/{name}/{family}/00000000/{}",
                    content_name(family.as_bytes())
                ),
                revision: digest.clone(),
                value: digest,
            });
        }
        Ok(RecordSet::new(root.to_path_buf(), entries))
    }

    #[test]
    fn a_record_group_is_read_and_authenticated_once_per_process() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let metadata = LibraryManifest::encode_compiler_record_value(&Metadata {
            name: "probe_loaf".to_owned(),
            version: "0.1.0".to_owned(),
            incan_version: "0.6.0".to_owned(),
            manifest_format: 1,
            manifest_digest: content_name(b"probe_loaf manifest"),
            vocabulary: false,
            provider_operations: false,
            native_unions: false,
            rust_paths: BTreeSet::new(),
            rust_abi_cache_declared: false,
            rust_abi_cache_digest: None,
        })?;
        let contract = LibraryManifest::encode_compiler_record_value(&LibraryContractMetadata::default())?;
        let expected = RecordReads {
            records: 2,
            bytes: metadata.len() + contract.len(),
        };
        let set = record_set(
            root.path(),
            "probe_loaf",
            &[("metadata", metadata), ("contract-metadata", contract)],
        )?;
        let dir = root.path().join("debug");

        let (first, first_digest) =
            read_group(&set, &dir, "probe_loaf", Projection::Complete)?.ok_or("the group is installed")?;
        assert_eq!(set.reads(), expected, "the first read opens each record once");
        let (second, second_digest) =
            read_group(&set, &dir, "probe_loaf", Projection::Complete)?.ok_or("the group is installed")?;
        assert_eq!(first_digest, second_digest);
        assert_eq!(first.name, second.name);
        assert_eq!(
            set.reads(),
            expected,
            "a group this process already authenticated is not read or hashed again"
        );
        Ok(())
    }

    #[test]
    fn a_payload_that_still_parses_but_differs_from_its_content_name_is_refused()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::create_dir(root.path().join("payloads"))?;
        let original = br#"{"name":"BindingDeclaration"}"#;
        let digest = content_name(original);
        let payload = root.path().join("payloads").join(&digest);
        fs::write(&payload, original)?;
        assert_eq!(open_value(root.path(), &digest)?, original);

        // One byte changed in place: the value still decodes, but it is no longer the content its name promises.
        fs::write(&payload, br#"{"name":"BindingXeclaration"}"#)?;
        let refusal = match open_value(root.path(), &digest) {
            Ok(_) => return Err("a damaged compiler-record payload was accepted".into()),
            Err(reason) => reason,
        };
        assert!(refusal.contains(&digest), "the refusal names the payload: {refusal}");
        assert!(
            refusal.contains("does not match its content name"),
            "the refusal says why: {refusal}"
        );
        Ok(())
    }

    /// The metadata record of the probe Loaf.
    fn probe_metadata() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        Ok(LibraryManifest::encode_compiler_record_value(&Metadata {
            name: "probe_loaf".to_owned(),
            version: "0.1.0".to_owned(),
            incan_version: "0.6.0".to_owned(),
            manifest_format: 1,
            manifest_digest: content_name(b"probe_loaf manifest"),
            vocabulary: false,
            provider_operations: false,
            native_unions: false,
            rust_paths: BTreeSet::new(),
            rust_abi_cache_declared: false,
            rust_abi_cache_digest: None,
        })?)
    }

    /// Contract metadata whose checked API documents one function.
    fn documented_contract() -> Result<LibraryContractMetadata, Box<dyn std::error::Error>> {
        let source = "pub def price() -> int:\n    \"\"\"The list price.\"\"\"\n    return 1999\n";
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = crate::typechecker::TypeChecker::new();
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        let module = collect_checked_api_metadata(&program, &checker, vec!["prices".to_owned()]);
        Ok(LibraryContractMetadata {
            api: Some(CheckedApiMetadataPackage {
                schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
                package: None,
                modules: vec![module],
                public_namespaces: Vec::new(),
            }),
            ..LibraryContractMetadata::default()
        })
    }

    /// The probe Loaf installed with its contract facts and their documentation as separate records, the contract
    /// metadata it was split from, and what reading only the facts reads.
    fn documented_set(
        root: &Path,
    ) -> Result<(RecordSet, LibraryContractMetadata, RecordReads), Box<dyn std::error::Error>> {
        let original = documented_contract()?;
        let (facts, documentation) = original.split_documentation();
        let metadata = probe_metadata()?;
        let contract = LibraryManifest::encode_compiler_record_value(&facts)?;
        let documentation = LibraryManifest::encode_compiler_record_value(&documentation)?;
        let facts_read = RecordReads {
            records: 2,
            bytes: metadata.len() + contract.len(),
        };
        let set = record_set(
            root,
            "probe_loaf",
            &[
                ("metadata", metadata),
                ("contract-metadata", contract),
                ("documentation", documentation),
            ],
        )?;
        Ok((set, original, facts_read))
    }

    #[test]
    fn a_compile_reads_no_documentation_records() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let (set, _, facts_read) = documented_set(root.path())?;
        let dir = root.path().join("debug");
        for projection in [Projection::Complete, Projection::Execution] {
            read_group(&set, &dir, "probe_loaf", projection)?.ok_or("the group is installed")?;
        }
        assert_eq!(
            set.reads(),
            facts_read,
            "compiles open the metadata and contract facts, never the documentation"
        );
        Ok(())
    }

    #[test]
    fn a_documented_read_restores_the_documentation_records() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let (set, original, _) = documented_set(root.path())?;
        let dir = root.path().join("debug");
        let (compiled, _) =
            read_group(&set, &dir, "probe_loaf", Projection::Complete)?.ok_or("the group is installed")?;
        let (documented, _) =
            read_group(&set, &dir, "probe_loaf", Projection::Documented)?.ok_or("the group is installed")?;
        assert_ne!(
            compiled.contract_metadata, original,
            "the compile's manifest carries no documentation"
        );
        assert_eq!(
            documented.contract_metadata, original,
            "a documented read gives back the checked API with its documentation"
        );
        Ok(())
    }

    #[test]
    fn an_inventory_summary_is_read_once_per_process() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let metadata = probe_metadata()?;
        let expected = RecordReads {
            records: 1,
            bytes: metadata.len(),
        };
        let set = record_set(root.path(), "probe_loaf", &[("metadata", metadata)])?;
        let dir = root.path().join("debug");
        for _ in 0..2 {
            let summary = read_summary(&set, &dir, "probe_loaf")?.ok_or("the group is installed")?;
            assert_eq!(summary.identity.name, "probe_loaf");
        }
        assert_eq!(
            set.reads(),
            expected,
            "a summary this process already authenticated is not read or hashed again"
        );
        Ok(())
    }

    /// What executable resolution may read of a reached Loaf: its metadata, descriptor and public identities.
    const SURFACE_BOUND: usize = 4096;

    /// A library whose executable surface is small beside its contract metadata: a function, a model with a method,
    /// a descriptor, and a checked API repeated until the contract metadata is well over the bound.
    fn surfaced_library() -> Result<LibraryManifest, Box<dyn std::error::Error>> {
        let source = "pub def price() -> int:\n    return 1999\n\npub model Money:\n    cents: int\n\n    pub def doubled(self) -> int:\n        return self.cents * 2\n";
        surfaced_library_for("prices", source)
    }

    fn surfaced_library_for(name: &str, source: &str) -> Result<LibraryManifest, Box<dyn std::error::Error>> {
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = crate::typechecker::TypeChecker::new();
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        let exports = crate::library_exports::collect_checked_public_exports(&program, &checker);
        let mut manifest = LibraryManifest::from_checked_exports(name, "0.1.0", &exports);
        let module = collect_checked_api_metadata(&program, &checker, vec![name.to_owned()]);
        manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            package: None,
            modules: vec![module; 200],
            public_namespaces: Vec::new(),
        });
        manifest.contract_metadata.executable_representation = Some(ExecutableRepresentationExport {
            representation_version: 1,
            content_digest: content_name(b"prices bodies"),
        });
        Ok(manifest)
    }

    /// Install every compiler record of `manifest` as its Loaf group in a fresh payload directory under `root`.
    fn installed_library(root: &Path, manifest: &LibraryManifest) -> Result<RecordSet, Box<dyn std::error::Error>> {
        fs::create_dir(root.join("payloads"))?;
        let mut entries = Vec::new();
        for record in manifest.compiler_records(&content_name(b"prices manifest"))?.records {
            let digest = content_name(&record.value);
            fs::write(root.join("payloads").join(&digest), &record.value)?;
            entries.push(RecordEntry {
                key: format!("loaf/debug/{}/{}", manifest.name, record.key()),
                revision: digest.clone(),
                value: digest,
            });
        }
        Ok(RecordSet::new(root.to_path_buf(), entries))
    }

    #[test]
    fn executable_resolution_reads_only_a_reached_loafs_executable_surface() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let manifest = surfaced_library()?;
        let set = installed_library(root.path(), &manifest)?;
        let (surface, _) =
            read_surface_in(&set, &root.path().join("debug"), "prices")?.ok_or("the Loaf is installed")?;
        assert_eq!(
            *surface,
            ExecutableSurface::of(&manifest),
            "the surface the manifest describes"
        );
        let read = set.reads();
        assert!(
            read.bytes <= SURFACE_BOUND,
            "executable resolution read {} records, {} bytes; the bound is {SURFACE_BOUND}",
            read.records,
            read.bytes
        );
        Ok(())
    }

    #[test]
    fn native_lowering_reads_a_loafs_runtime_bindings_by_name() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let manifest = surfaced_library()?;
        let set = installed_library(root.path(), &manifest)?;
        let dir = root.path().join("debug");
        // The index admits an installed Loaf by its inventory summary before anything demands its records.
        read_summary(&set, &dir, "prices")?.ok_or("the Loaf is installed")?;
        let indexed = set.reads();
        let (bindings, _) = read_runtime_bindings_in(&set, &dir, "prices")?.ok_or("the Loaf is installed")?;
        let expected = runtime_bindings(&manifest);
        assert!(!expected.is_empty(), "`prices::price` is a runtime callable");
        assert_eq!(*bindings, expected, "the bindings the manifest describes");
        let (records, bytes) = (set.reads().records - indexed.records, set.reads().bytes - indexed.bytes);
        assert_eq!(
            records, 1,
            "native lowering read {records} records, {bytes} bytes; it needs the runtime bindings record alone"
        );
        Ok(())
    }

    #[test]
    fn runtime_planning_reads_only_the_individually_requested_facade() -> Result<(), Box<dyn std::error::Error>> {
        let name = "incan_native_runtime";
        let root = tempfile::tempdir()?;
        let manifest = surfaced_library_for(
            name,
            "pub def used() -> int:\n    return 1\n\npub def unused() -> int:\n    return 2\n",
        )?;
        let selected = thread_selection::select(installed_library(root.path(), &manifest)?);
        let dir = root.path().join("debug");
        let index = crate::loaf_index::LoafIndex::deferred([(name.to_owned(), name.to_owned(), dir, false)])?;
        let baseline = selected.0.reads();
        let names = index.runtime_binding_selectors(name)?;
        let used = format!("{name}::used");
        let unused = format!("{name}::unused");
        assert!(names.contains(&content_name(used.as_bytes())));
        assert!(names.contains(&content_name(unused.as_bytes())));
        assert_eq!(selected.0.reads(), baseline);
        let binding = index.runtime_binding(name, &used)?.ok_or("expected the named facade")?;
        assert_eq!(binding.path, used);
        assert_eq!(index.runtime_binding(name, &used)?, Some(binding.clone()));
        assert!(index.runtime_binding(name, "incan_native_runtime::missing")?.is_none());
        let read = selected.0.reads();
        assert_eq!(read.records - baseline.records, 1);
        assert_eq!(
            read.bytes - baseline.bytes,
            LibraryManifest::encode_compiler_record_value(&binding)?.len()
        );
        assert!(
            !thread_selection::read_keys()
                .iter()
                .any(|key| key.contains("/runtime-bindings/") || key.ends_with(&content_name(unused.as_bytes())))
        );
        Ok(())
    }

    #[test]
    fn a_consumer_reads_one_declarations_signature_record_alone() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        // One copy of the checked API module, so `prices::price` names one declaration.
        let mut manifest = surfaced_library()?;
        if let Some(api) = manifest.contract_metadata.api.as_mut() {
            api.modules.truncate(1);
        }
        let set = installed_library(root.path(), &manifest)?;
        let dir = root.path().join("debug");
        // The index admits an installed Loaf by its inventory summary before anything demands its records.
        read_summary(&set, &dir, "prices")?.ok_or("the Loaf is installed")?;
        let indexed = set.reads();
        let (signatures, _) = read_declaration_in(&set, &dir, "prices", "declaration-signature", "prices::price")?
            .ok_or("the signature record is installed")?;
        assert_eq!(signatures.len(), 1, "one declaration named `prices::price`");
        assert_eq!(signatures[0]["name"], "price", "the signature of `price`");
        let (records, bytes) = (set.reads().records - indexed.records, set.reads().bytes - indexed.bytes);
        assert_eq!(
            records, 1,
            "the consumer read {records} records, {bytes} bytes; it needs the signature record alone"
        );
        Ok(())
    }
}

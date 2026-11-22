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
use crate::library_manifest::executable_ranges::{HEAD, RANGE, record_key};
use crate::library_manifest::executable_surface::ExecutableSurface;
use incan_semantics_core::body_ir::RuntimeCallableBinding;

mod declaration_records;
mod directory;
pub use directory::install_directory_reader;
mod inventory;
mod runtime_bindings;
#[cfg(test)]
mod thread_selection;
use declaration_records::NAMED_FAMILIES;
pub(super) use declaration_records::{names_record, read_declaration};
use inventory::{Metadata, read_summary};
pub(super) use inventory::{admit_capabilities, summary};
pub(super) use runtime_bindings::read_runtime_bindings;

/// The record family holding a Loaf's checked API documentation, which compiles never open (RFC 179).
const DOCUMENTATION: &str = "documentation";
/// The record families of a Loaf's executable surface besides its metadata, and its runtime callable bindings; a
/// manifest projection never selects them.
const EXECUTABLE_DESCRIPTOR: &str = "executable-descriptor";
const PUBLIC_IDENTITIES: &str = "public-identities";
const RUNTIME_BINDINGS: &str = "runtime-bindings";
const ROOT_ENV: &str = "INCAN_COMPILER_RECORD_ROOT";
const DIGEST_ENV: &str = "INCAN_COMPILER_RECORD_DIGEST";
const DIRECTORY_ENV: &str = "INCAN_COMPILER_RECORD_DIRECTORY";
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
    [ROOT_ENV, DIGEST_ENV, DIRECTORY_ENV]
        .iter()
        .any(|key| std::env::var_os(key).is_some())
}

/// One compiler-record membership from the pinned sealed-set manifest.
#[derive(Clone)]
struct RecordEntry {
    key: String,
    revision: String,
    value: String,
}

/// The immutable installed set after its root receipt has been authenticated.
struct RecordSet {
    root: PathBuf,
    entries: Vec<RecordEntry>,
    directory: Option<(String, String)>,
    /// Authenticated values for demanded executable range heads and memberships.
    range_values: Mutex<BTreeMap<String, Vec<u8>>>,
    /// Records this process has read from the payload directory.
    records_read: AtomicUsize,
    /// Payload bytes this process has read from the payload directory.
    bytes_read: AtomicUsize,
    /// Record groups this process already read, authenticated and decoded, by package prefix and projection.
    admitted: Mutex<BTreeMap<(String, Projection), AdmittedGroup>>,
    /// Inventory summaries this process already read and authenticated, by metadata key prefix.
    summaries: Mutex<BTreeMap<String, Summary>>,
    /// Profiles whose global contributor inventory was checked, including older installations without one.
    capability_profiles:
        Mutex<BTreeMap<String, BTreeMap<String, crate::library_manifest::capabilities::Contributions>>>,
    /// Parser registrations already authenticated under this installed set and Loaf profile.
    vocabularies: Mutex<BTreeMap<String, (Arc<VocabExports>, String)>>,
    /// Executable surfaces this process already read and authenticated, by package prefix.
    surfaces: Mutex<BTreeMap<String, (Arc<ExecutableSurface>, String)>>,
    /// A successful exact declaration read already proves that its family exists in this immutable set.
    present_named_families: Mutex<BTreeSet<String>>,
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
        let surface = matches!(
            family,
            EXECUTABLE_DESCRIPTOR | PUBLIC_IDENTITIES | RUNTIME_BINDINGS | HEAD | RANGE
        ) || NAMED_FAMILIES.contains(&family);
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
    fn new(root: PathBuf, mut entries: Vec<RecordEntry>) -> Self {
        entries.sort_unstable_by(|a, b| a.key.cmp(&b.key));
        Self {
            range_values: Mutex::default(),
            present_named_families: Mutex::default(),
            directory: None,
            root,
            entries,
            records_read: AtomicUsize::new(0),
            bytes_read: AtomicUsize::new(0),
            admitted: Mutex::new(BTreeMap::new()),
            summaries: Mutex::new(BTreeMap::new()),
            capability_profiles: Mutex::default(),
            vocabularies: Mutex::new(BTreeMap::new()),
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

    /// Keep one authenticated group for the rest of the request: its values are content-named under a receipt that
    /// cannot change while the process runs, so reading and hashing them again inside the request could only return
    /// the same bytes. [`release_admitted_groups`] drops it when the request ends.
    fn admit(&self, prefix: String, projection: Projection, group: &AdmittedGroup) {
        self.admitted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((prefix, projection), (Arc::clone(&group.0), group.1.clone()));
    }

    /// Drop every decoded group, keeping only the small named records, summaries and vocabularies already read.
    fn release_groups(&self) {
        self.admitted.lock().unwrap_or_else(PoisonError::into_inner).clear();
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

/// Select an exact range fact by binary search over the authenticated directory. Old pins have no range head.
pub(super) fn range_value<T: DeserializeOwned>(
    dir: &Path,
    name: &str,
    family: &str,
    member: &str,
) -> Result<Option<T>, LoafIndexError> {
    let Some(set) = installed().map_err(|reason| unreadable(dir, name, reason))? else {
        return Ok(None);
    };
    let profile = dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| unreadable(dir, name, "Loaf profile is not UTF-8".into()))?;
    let key = format!("loaf/{profile}/{name}/{}", record_key(family, member));
    let prefix = format!("loaf/{profile}/{name}/{family}/");
    let hash = format!("{:x}", Sha256::digest(member.as_bytes()));
    let rows = set
        .members(&prefix, Some(&hash), 0)
        .map_err(|reason| unreadable(dir, name, reason))?;
    let fail = |reason| unreadable(dir, name, record_failure(&key, reason));
    let Some(entry) = rows.first() else { return Ok(None) };
    if rows.len() != 1 || entry.key != key {
        return Err(fail("ambiguous or invalid installed executable member".into()));
    }
    if entry.revision != entry.value {
        return Err(fail("distinct revision and content identities".into()));
    }
    let mut cached = set.range_values.lock().unwrap_or_else(PoisonError::into_inner);
    let bytes = match cached.entry(key.clone()) {
        std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
        std::collections::btree_map::Entry::Vacant(slot) => {
            validate_payload_directory(&set.root).map_err(fail)?;
            let bytes = open_value(&set.root, &entry.value).map_err(fail)?;
            set.count_read(&key, bytes.len());
            slot.insert(bytes)
        }
    };
    Ok(Some(
        LibraryManifest::decode_compiler_record_value(bytes).map_err(|error| fail(error.to_string()))?,
    ))
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
    let mut entries = Vec::new();
    for family in ["metadata", EXECUTABLE_DESCRIPTOR, PUBLIC_IDENTITIES] {
        entries.extend(
            set.members(&format!("{prefix}{family}/"), None, 0)
                .map_err(|reason| unreadable(dir, name, reason))?,
        );
    }
    let selected = select_records(entries.iter().collect(), &prefix, dir, name)?
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
        set.members(&prefix, None, 0)
            .map_err(|reason| unreadable(dir, name, reason))?
    };
    if entries.is_empty() {
        return Ok(None);
    }
    let selected = select_records(entries.iter().collect(), &prefix, dir, name)?
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

/// Release the record groups the request that just ended decoded: a resident worker keeps no decoded copy of an
/// installed Loaf between requests. A set this process never loaded stays unloaded.
pub(super) fn release_admitted_groups() {
    #[cfg(test)]
    if let Some(set) = thread_selection::selected() {
        set.release_groups();
    }
    if let Some(Ok(Some(set))) = INSTALLED.get() {
        set.release_groups();
    }
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
        (None, None) if std::env::var_os(DIRECTORY_ENV).is_none() => return Ok(None),
        (Some(root), Some(digest)) => (root, digest),
        _ => return Err("compiler-record root and digest must be supplied together".to_owned()),
    };
    let root = PathBuf::from(root);
    let digest = digest
        .into_string()
        .map_err(|_| "compiler-record digest is not UTF-8".to_owned())?;
    valid_digest(&digest)?;
    if let Some(top) = std::env::var_os(DIRECTORY_ENV) {
        let top = top
            .into_string()
            .map_err(|_| "non-UTF-8 compiler directory root".to_owned())?;
        if top.is_empty() {
            return Err("empty compiler directory root".into());
        }
        let mut set = RecordSet::new(root, Vec::new());
        set.directory = Some((digest, top));
        return Ok(Some(set));
    }
    let path = root.join("manifest.json");
    let bytes = fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if !digest_matches(&bytes, &digest) {
        return Err("compiler-record manifest differs from the installed toolchain pin".to_owned());
    }
    tracing::trace!(target: "incan::invocation", phase = "record_directory_read", digest = %digest, bytes = bytes.len());
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

/// The parser needs the complete vocabulary registration of an admitted contributor, independently of the
/// contributor's type-checking contracts. Older or external stores still provide their complete validated manifest.
pub(super) fn read_vocabulary(dir: &Path, name: &str) -> Result<Option<(Arc<VocabExports>, String)>, LoafIndexError> {
    let Some(set) = installed().map_err(|reason| unreadable(dir, name, reason))? else {
        return Ok(None);
    };
    let profile = dir
        .file_name()
        .and_then(|part| part.to_str())
        .ok_or_else(|| unreadable(dir, name, "Loaf profile directory is not UTF-8".to_owned()))?;
    let prefix = format!("loaf/{profile}/{name}/vocabulary/");
    if let Some(found) = set
        .vocabularies
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&prefix)
        .cloned()
    {
        return Ok(Some(found));
    }
    let rows = set
        .members(&prefix, None, 2)
        .map_err(|reason| unreadable(dir, name, reason))?;
    let mut matching = rows.iter();
    let Some(entry) = matching.next() else {
        return Ok(None);
    };
    if matching.next().is_some() || entry.revision != entry.value {
        return Err(unreadable(
            dir,
            name,
            "vocabulary publication must select one content-named record".to_owned(),
        ));
    }
    let Some(summary) = read_summary(set, dir, name)? else {
        return Ok(None);
    };
    let bytes = read_value(&set.root, &entry.value)
        .map_err(|reason| unreadable(dir, name, record_failure(&entry.key, reason)))?;
    set.count_read(&entry.key, bytes.len());
    let vocab =
        VocabExports::decode_compiler_record_value(&bytes).map_err(|error| unreadable(dir, name, error.to_string()))?;
    if !entry
        .key
        .ends_with(&format!("/{:x}", Sha256::digest(vocab.package_name.as_bytes())))
    {
        return Err(unreadable(
            dir,
            name,
            "vocabulary registration disagrees with its record key".to_owned(),
        ));
    }
    let found = (Arc::new(vocab), summary.identity.digest);
    set.vocabularies
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(prefix, found.clone());
    Ok(Some(found))
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
mod tests;

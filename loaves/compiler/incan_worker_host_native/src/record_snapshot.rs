//! Resolve content-named compiler snapshot descriptors through the worker owner's home bakery binding.

use std::collections::BTreeMap;
use std::path::Path;

use incan_worker_host_bindings::{
    Lookup, Snapshot, StoreError, begin, commit, disk_root, lease_token, lookup, owned, pin_lease, put, read_manifest,
    read_store_identity, store_error_text, store_snapshot,
};
use internal_models_bindings::Value;
use internal_models_native::crumb_rows::{ModelField, model_description, required_value};
use internal_models_native::graph_crumb_fields::{decode_graph_row, encode_graph_row, integer, items, number, text};
use internal_models_native::records::{ReadScope, StoreRecordSource};
use internal_models_native::{PREPARED_FAMILY, WORK_FAMILY, envelope_key};
use sha2::{Digest, Sha256};

use crate::json_text::{Field, object, parse_document};

pub const SNAPSHOT_FORMAT_PROFILE: &str = "incan.compiler-records/1";
pub const STORE_VIEW_SCHEMA: &str = "incan.store-view/1";
const BINDING_ROW: &[u8] = b"stview\x00";
const SNAPSHOT_ROW: &[u8] = b"cmpsnp\x00";
const SNAPSHOT_SCHEMA: &str = "incan.compiler-snapshot/1";
const SNAPSHOT_FAMILY: &str = "incan.compiler.snapshot";
/// `put` with no expectation about the key's current state.
const EXPECT_ANY: i64 = 0;

/// Closed worker snapshot faults; none authorizes a fallback or partial read receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotFaultKind {
    MissingBinding,
    InvalidBinding,
    StoreMismatch,
    MissingDescriptor,
    InvalidDescriptor,
    ProfileMismatch,
    SnapshotMismatch,
    RootOutside,
    Store,
}

impl SnapshotFaultKind {
    /// The fault's wire spelling.
    pub fn value(self) -> &'static str {
        match self {
            SnapshotFaultKind::MissingBinding => "missing-binding",
            SnapshotFaultKind::InvalidBinding => "invalid-binding",
            SnapshotFaultKind::StoreMismatch => "store-mismatch",
            SnapshotFaultKind::MissingDescriptor => "missing-descriptor",
            SnapshotFaultKind::InvalidDescriptor => "invalid-descriptor",
            SnapshotFaultKind::ProfileMismatch => "profile-mismatch",
            SnapshotFaultKind::SnapshotMismatch => "snapshot-mismatch",
            SnapshotFaultKind::RootOutside => "root-outside",
            SnapshotFaultKind::Store => "store",
        }
    }
}

/// One typed failure to resolve or read a staged compiler snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotFault {
    pub kind: SnapshotFaultKind,
    pub detail: String,
}

impl SnapshotFault {
    /// The bounded compiler-fault detail.
    pub fn message(&self) -> String {
        format!("{}: {}", self.kind.value(), self.detail)
    }
}

impl std::fmt::Display for SnapshotFault {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message())
    }
}

impl std::error::Error for SnapshotFault {}

fn fault(kind: SnapshotFaultKind, detail: impl Into<String>) -> SnapshotFault {
    SnapshotFault {
        kind,
        detail: detail.into(),
    }
}

/// Keep a typed store failure inside the compiler-fault boundary.
fn store_fault(error: StoreError) -> SnapshotFault {
    fault(SnapshotFaultKind::Store, store_error_text(&error))
}

/// The only home-local locator for a physical bakery and its home view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreView {
    pub store_id: String,
    pub locator: String,
    pub home_view: String,
}

/// A verified binding and whether a writer can backfill the current private projection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedStoreView {
    pub binding: StoreView,
    pub backfill: bool,
}

/// One leased immutable store view and the record roots a compiler activation may read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotHandle {
    pub store_id: String,
    pub generation: i64,
    pub manifest: String,
    pub view: String,
    pub view_revision: String,
    pub allowed_roots: Vec<String>,
    pub lease: String,
    pub format_profile: String,
}

/// Content roots for one value, receipt, diagnostic page and counter page staged in one store generation.
pub struct StagedCompilerOutputs {
    pub snapshot: Snapshot,
    pub value: String,
    pub receipt: String,
    pub diagnostic: String,
    pub counter: String,
}

/// An exact pinned view whose reads enforce the descriptor's allowed-root invariant.
pub struct ResolvedSnapshot {
    snapshot: Snapshot,
    allowed_roots: Vec<String>,
}

impl ResolvedSnapshot {
    /// A resolved reader, created only after descriptor and lease validation succeeds.
    pub fn open(snapshot: Snapshot, allowed_roots: Vec<String>) -> Self {
        Self {
            snapshot,
            allowed_roots,
        }
    }

    /// Read one selected record only when its exact key or family root is inside the staged envelope.
    pub fn read(&self, key: &str) -> Result<Lookup, SnapshotFault> {
        if !allowed(&self.allowed_roots, key) {
            return Err(fault(SnapshotFaultKind::RootOutside, key));
        }
        lookup(&self.snapshot, key.to_owned()).map_err(store_fault)
    }

    /// This resolved snapshot and its allowed roots, as the compiler's authenticated read boundary.
    pub fn record_source(&self, scope: &ReadScope) -> StoreRecordSource {
        StoreRecordSource::open(&self.snapshot, scope, &self.allowed_roots)
    }

    /// Publish content-named transport pages without replacing the pinned inputs of the pending evaluation.
    pub fn stage_checking_envelopes(&self, prepared: &[u8], work: &[u8]) -> Result<String, SnapshotFault> {
        let prepared_root = digest(prepared);
        let invalid = |detail: String| fault(SnapshotFaultKind::InvalidDescriptor, detail);
        let prepared_key = envelope_key(PREPARED_FAMILY, &prepared_root).map_err(invalid)?;
        let work_key = envelope_key(WORK_FAMILY, &digest(work)).map_err(invalid)?;
        if !allowed(&self.allowed_roots, &prepared_key) || !allowed(&self.allowed_roots, &work_key) {
            return Err(fault(SnapshotFaultKind::RootOutside, "checking envelopes"));
        }
        let current = store_snapshot(self.snapshot.root.clone(), self.snapshot.view.clone()).map_err(store_fault)?;
        let transaction = put(begin(&current), prepared_key, &prepared.to_vec(), EXPECT_ANY);
        commit(put(transaction, work_key, &work.to_vec(), EXPECT_ANY)).map_err(store_fault)?;
        Ok(prepared_root)
    }

    /// Resolve only immutable, hash-verified work descriptors through the latest staging view.
    pub fn checking_envelope(&self, family: &str, root: &str) -> Result<Vec<u8>, SnapshotFault> {
        if family != WORK_FAMILY && family != PREPARED_FAMILY {
            return Err(fault(SnapshotFaultKind::InvalidDescriptor, family));
        }
        let key = envelope_key(family, root).map_err(|detail| fault(SnapshotFaultKind::InvalidDescriptor, detail))?;
        if !allowed(&self.allowed_roots, &key) {
            return Err(fault(SnapshotFaultKind::RootOutside, key));
        }
        let current = store_snapshot(self.snapshot.root.clone(), self.snapshot.view.clone()).map_err(store_fault)?;
        match lookup(&current, key.clone()).map_err(store_fault)? {
            Lookup::Found(found) => {
                let value = owned(&found);
                if digest(&value) != root {
                    return Err(fault(SnapshotFaultKind::InvalidDescriptor, key));
                }
                Ok(value)
            }
            _ => Err(fault(SnapshotFaultKind::MissingDescriptor, key)),
        }
    }

    /// Stage one activation's four immutable output pages atomically against its resolved bakery view.
    pub fn stage_compiler_outputs(
        &self,
        value: &[u8],
        receipt: &[u8],
        diagnostic: &[u8],
        counter: &[u8],
    ) -> Result<StagedCompilerOutputs, SnapshotFault> {
        let roots = [digest(value), digest(receipt), digest(diagnostic), digest(counter)];
        let mut transaction = begin(&self.snapshot);
        for ((kind, root), page) in ["value", "receipt", "diagnostic", "counter"]
            .iter()
            .zip(&roots)
            .zip([value, receipt, diagnostic, counter])
        {
            transaction = put(transaction, staged_key(kind, root), &page.to_vec(), EXPECT_ANY);
        }
        let published = commit(transaction).map_err(store_fault)?;
        let [value, receipt, diagnostic, counter] = roots;
        Ok(StagedCompilerOutputs {
            snapshot: published,
            value,
            receipt,
            diagnostic,
            counter,
        })
    }
}

/// The canonical home bakery binding.
pub fn encode_store_view(binding: &StoreView) -> Result<String, String> {
    let mut fields = BTreeMap::new();
    fields.insert("schema", Field::Text(STORE_VIEW_SCHEMA.to_owned()));
    fields.insert("store_id", Field::Text(binding.store_id.clone()));
    fields.insert("locator", Field::Text(binding.locator.clone()));
    fields.insert("home_view", Field::Text(binding.home_view.clone()));
    Ok(object(fields))
}

/// The exact portable configuration accepted by independently installed older workers.
pub fn portable_store_view(binding: &StoreView) -> Result<Vec<u8>, String> {
    Ok(format!("{}\n", encode_store_view(binding)?).into_bytes())
}

fn binding_description() -> internal_models_bindings::Description {
    model_description(
        BINDING_ROW,
        "incan.store-view/2",
        &[
            ModelField::new(b"homevw", "home_view", "string"),
            ModelField::new(b"locatr", "locator", "string"),
            ModelField::new(b"store_", "store_id", "string"),
        ],
    )
}

/// Persist the view, absolute locator and stable store identity as independent Crumb fields.
pub fn encode_store_binding(binding: &StoreView) -> Result<Vec<u8>, String> {
    encode_graph_row(
        BINDING_ROW,
        &binding_description(),
        vec![
            required_value(b"homevw", Value::Text(binding.home_view.clone())),
            required_value(b"locatr", Value::Text(binding.locator.clone())),
            required_value(b"store_", Value::Text(binding.store_id.clone())),
        ],
    )
}

fn decode_store_binding(encoded: &[u8]) -> Result<StoreView, String> {
    let fields = decode_graph_row(encoded, BINDING_ROW, &binding_description(), 3)?;
    validate_store_view(StoreView {
        store_id: text(&fields[2].value)?,
        locator: text(&fields[1].value)?,
        home_view: text(&fields[0].value)?,
    })
}

/// Accept the typed binding only when it reproduces the portable descriptor's exact observed bytes; otherwise decode
/// the portable descriptor, whose whitespace and member order do not change the selected view.
pub fn select_store_view(path: &str) -> Result<SelectedStoreView, String> {
    let path = Path::new(path);
    let portable = std::fs::read(path).map_err(|error| error.to_string())?;
    let mut projection = path.as_os_str().to_owned();
    projection.push(".crumb");
    let projection = Path::new(&projection);
    if projection.is_file()
        && let Ok(stored) = std::fs::read(projection)
        && let Ok(binding) = decode_store_binding(&stored)
        && portable_store_view(&binding)? == portable
    {
        return Ok(SelectedStoreView {
            binding,
            backfill: false,
        });
    }
    let source = std::str::from_utf8(&portable).map_err(|_| "invalid portable bakery view encoding".to_owned())?;
    let binding = decode_store_view(source)?;
    let backfill = portable_store_view(&binding)? == portable;
    Ok(SelectedStoreView { binding, backfill })
}

/// The canonical first-generation JSON spelling for clients reopening a previous snapshot.
pub fn encode_snapshot_handle(handle: &SnapshotHandle) -> Result<String, String> {
    let mut fields = BTreeMap::new();
    fields.insert("schema", Field::Text(SNAPSHOT_SCHEMA.to_owned()));
    fields.insert("store_id", Field::Text(handle.store_id.clone()));
    fields.insert("generation", Field::Integer(handle.generation));
    fields.insert("manifest", Field::Text(handle.manifest.clone()));
    fields.insert("view", Field::Text(handle.view.clone()));
    fields.insert("view_revision", Field::Text(handle.view_revision.clone()));
    fields.insert("allowed_roots", Field::Texts(handle.allowed_roots.clone()));
    fields.insert("lease", Field::Text(handle.lease.clone()));
    fields.insert("format_profile", Field::Text(handle.format_profile.clone()));
    Ok(object(fields))
}

/// The immutable source view, its lease and the exact capabilities granted to one activation.
fn snapshot_description() -> internal_models_bindings::Description {
    model_description(
        SNAPSHOT_ROW,
        "incan.compiler-snapshot/2",
        &[
            ModelField::new(b"format", "format_profile", "string"),
            ModelField::new(b"genera", "generation", "i64"),
            ModelField::new(b"lease_", "lease", "string"),
            ModelField::new(b"manife", "manifest", "string"),
            ModelField::new(b"revisi", "view_revision", "string"),
            ModelField::new(b"roots_", "allowed_roots", "list<string>"),
            ModelField::new(b"store_", "store_id", "string"),
            ModelField::new(b"view__", "view", "string"),
        ],
    )
}

/// Write the field-addressable descriptor a new worker activation selects.
pub fn encode_snapshot_record(handle: &SnapshotHandle) -> Result<Vec<u8>, String> {
    if handle.generation < 1 || canonical_roots(&handle.allowed_roots)? != handle.allowed_roots {
        return Err("snapshot descriptor requires a positive generation and canonical allowed roots".to_owned());
    }
    let roots = handle
        .allowed_roots
        .iter()
        .map(|root| Value::Text(root.clone()))
        .collect();
    encode_graph_row(
        SNAPSHOT_ROW,
        &snapshot_description(),
        vec![
            required_value(b"format", Value::Text(handle.format_profile.clone())),
            required_value(b"genera", integer(handle.generation)),
            required_value(b"lease_", Value::Text(handle.lease.clone())),
            required_value(b"manife", Value::Text(handle.manifest.clone())),
            required_value(b"revisi", Value::Text(handle.view_revision.clone())),
            required_value(b"roots_", Value::List(roots)),
            required_value(b"store_", Value::Text(handle.store_id.clone())),
            required_value(b"view__", Value::Text(handle.view.clone())),
        ],
    )
}

/// The content identity used when staging a new snapshot.
pub fn snapshot_handle_digest(handle: &SnapshotHandle) -> Result<String, String> {
    Ok(digest(&encode_snapshot_record(handle)?))
}

/// Authenticate the stored bytes before granting any authority; earlier snapshots keep their original digests.
pub fn read_snapshot_record(data: &[u8], expected: &str) -> Result<SnapshotHandle, String> {
    if digest(data) != expected {
        return Err("descriptor digest mismatch".to_owned());
    }
    if !data.starts_with(b"CRMB") {
        let legacy = std::str::from_utf8(data).map_err(|_| "invalid compiler snapshot encoding".to_owned())?;
        let handle = decode_snapshot_handle(legacy)?;
        if encode_snapshot_handle(&handle)? != legacy {
            return Err("noncanonical legacy compiler snapshot".to_owned());
        }
        return Ok(handle);
    }
    let fields = decode_graph_row(data, SNAPSHOT_ROW, &snapshot_description(), 8)?;
    let allowed_roots = items(&fields[5].value)?
        .iter()
        .map(text)
        .collect::<Result<Vec<_>, _>>()?;
    let generation = number(&fields[1].value)?;
    if generation < 1 {
        return Err("compiler snapshot generation must be positive".to_owned());
    }
    Ok(SnapshotHandle {
        store_id: text(&fields[6].value)?,
        generation,
        manifest: text(&fields[3].value)?,
        view: text(&fields[7].value)?,
        view_revision: text(&fields[4].value)?,
        allowed_roots,
        lease: text(&fields[2].value)?,
        format_profile: text(&fields[0].value)?,
    })
}

/// The engine-owned store key of one content-named descriptor.
pub fn snapshot_record_key(digest: &str) -> String {
    format!("{SNAPSHOT_FAMILY}\t{digest}")
}

/// The worker staging key of one content-named output page.
pub fn staged_key(kind: &str, root: &str) -> String {
    format!("incan.compiler.staged-{kind}\t{root}")
}

/// One lowercase SHA-256 content identity.
pub fn digest(value: &[u8]) -> String {
    Sha256::digest(value).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Sort and deduplicate nonempty record roots so descriptor identity has one spelling.
pub fn canonical_roots(roots: &[String]) -> Result<Vec<String>, String> {
    let mut sorted = roots.to_vec();
    sorted.sort();
    let mut canonical: Vec<String> = Vec::new();
    for root in sorted {
        if root.is_empty() || root.contains('\t') || root.trim() != root {
            return Err("snapshot allowed roots must be nonempty record-family names".to_owned());
        }
        if !canonical.contains(&root) {
            canonical.push(root);
        }
    }
    if canonical.is_empty() {
        return Err("snapshot descriptor requires at least one allowed root".to_owned());
    }
    Ok(canonical)
}

/// Resolve one activation digest through `<home>/bakery.view`, reopen its exact view revision and acquire its lease.
/// The protocol contributes no locator or capability: a missing descriptor, wrong store, profile or revision, or an
/// expired lease is a compiler fault before any producer read begins.
pub fn resolve_snapshot(home: &str, digest: &str) -> Result<ResolvedSnapshot, SnapshotFault> {
    if !valid_digest(digest) {
        return Err(fault(SnapshotFaultKind::MissingDescriptor, digest));
    }
    let binding_path = Path::new(home).join("bakery.view");
    if !binding_path.is_file() {
        return Err(fault(SnapshotFaultKind::MissingBinding, binding_path.to_string_lossy()));
    }
    let selected = select_store_view(&binding_path.to_string_lossy())
        .map_err(|detail| fault(SnapshotFaultKind::InvalidBinding, detail))?;
    let binding = selected.binding;
    let root = disk_root(binding.locator.clone());
    if read_store_identity(&root).map_err(store_fault)? != binding.store_id {
        return Err(fault(SnapshotFaultKind::StoreMismatch, binding.store_id));
    }
    let home_view = store_snapshot(root.clone(), binding.home_view.clone()).map_err(store_fault)?;
    let encoded = match lookup(&home_view, snapshot_record_key(digest)).map_err(store_fault)? {
        Lookup::Found(value) => owned(&value),
        Lookup::Deleted | Lookup::Absent => return Err(fault(SnapshotFaultKind::MissingDescriptor, digest)),
    };
    let handle =
        read_snapshot_record(&encoded, digest).map_err(|detail| fault(SnapshotFaultKind::InvalidDescriptor, detail))?;
    if handle.store_id != binding.store_id {
        return Err(fault(SnapshotFaultKind::StoreMismatch, handle.store_id));
    }
    if handle.format_profile != SNAPSHOT_FORMAT_PROFILE {
        return Err(fault(SnapshotFaultKind::ProfileMismatch, handle.format_profile));
    }
    let roots =
        canonical_roots(&handle.allowed_roots).map_err(|detail| fault(SnapshotFaultKind::InvalidDescriptor, detail))?;
    if roots != handle.allowed_roots {
        return Err(fault(
            SnapshotFaultKind::InvalidDescriptor,
            "allowed roots are not canonical",
        ));
    }
    let mut exact =
        read_manifest(&root, handle.generation, &handle.manifest, handle.view.clone()).map_err(store_fault)?;
    if exact.index != handle.view_revision || lease_token(&exact) != handle.lease {
        return Err(fault(SnapshotFaultKind::SnapshotMismatch, digest));
    }
    pin_lease(&mut exact).map_err(store_fault)?;
    Ok(ResolvedSnapshot::open(exact, roots))
}

/// Decode one exact home bakery binding.
fn decode_store_view(source: &str) -> Result<StoreView, String> {
    let document = parse_document(source)?;
    if required_text(&document, "schema")? != STORE_VIEW_SCHEMA {
        return Err("unsupported bakery view schema".to_owned());
    }
    validate_store_view(StoreView {
        store_id: required_text(&document, "store_id")?,
        locator: required_text(&document, "locator")?,
        home_view: required_text(&document, "home_view")?,
    })
}

fn validate_store_view(binding: StoreView) -> Result<StoreView, String> {
    if binding.store_id.is_empty() || !binding.locator.starts_with('/') || binding.home_view.is_empty() {
        return Err("bakery view requires an absolute locator and a home view".to_owned());
    }
    Ok(binding)
}

/// Decode one exact compiler snapshot descriptor.
fn decode_snapshot_handle(source: &str) -> Result<SnapshotHandle, String> {
    let document = parse_document(source)?;
    if required_text(&document, "schema")? != SNAPSHOT_SCHEMA {
        return Err("unsupported compiler snapshot schema".to_owned());
    }
    let generation = match document.get("generation") {
        Some(value) => value
            .as_i64()
            .ok_or_else(|| "`generation` must be an integer".to_owned())?,
        None => return Err("missing `generation`".to_owned()),
    };
    if generation < 1 {
        return Err("compiler snapshot generation must be positive".to_owned());
    }
    Ok(SnapshotHandle {
        store_id: required_text(&document, "store_id")?,
        generation,
        manifest: required_text(&document, "manifest")?,
        view: required_text(&document, "view")?,
        view_revision: required_text(&document, "view_revision")?,
        allowed_roots: required_strings(&document, "allowed_roots")?,
        lease: required_text(&document, "lease")?,
        format_profile: required_text(&document, "format_profile")?,
    })
}

/// Read one required string field without accepting another JSON kind.
fn required_text(document: &serde_json::Value, name: &str) -> Result<String, String> {
    match document.get(name) {
        Some(value) => value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("`{name}` must be text")),
        None => Err(format!("missing `{name}`")),
    }
}

/// Read one required array containing only strings.
fn required_strings(document: &serde_json::Value, name: &str) -> Result<Vec<String>, String> {
    let Some(value) = document.get(name) else {
        return Err(format!("missing `{name}`"));
    };
    let invalid = || format!("`{name}` must be an array of strings");
    value
        .as_array()
        .ok_or_else(invalid)?
        .iter()
        .map(|item| item.as_str().map(str::to_owned).ok_or_else(invalid))
        .collect()
}

/// Whether an exact key belongs to one allowed family root.
fn allowed(roots: &[String], key: &str) -> bool {
    roots.iter().any(|root| {
        key == root
            || key
                .strip_prefix(root.as_str())
                .is_some_and(|tail| tail.starts_with('\t'))
    })
}

/// Whether a descriptor identity is one lowercase SHA-256 digest.
pub fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

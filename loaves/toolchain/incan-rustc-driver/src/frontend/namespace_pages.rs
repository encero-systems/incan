//! Restore editable namespaces from request-local pages or the pinned store selected by Oven.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use incan_frontend::checker_records::{GraphNamespaceHeader, GraphNamespaceRecord, RecordAdapterFault};
use incan_semantics_core::SymbolOrigin;
use incan_semantics_core::body_ir::BodyIrModule;
use internal_models::{GraphNamespaceView, NamespaceReference};
use sha2::{Digest, Sha256};

use super::namespace_crumb;

const RETAINED_BODY_BYTES: usize = 128 * 1024 * 1024;
const RETAINED_NAMESPACES: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProgramScope {
    bakery: PathBuf,
    project: PathBuf,
}

/// The graph selected these exact immutable producer bytes. Both values survive a fresh request's staging directory.
#[derive(Debug, PartialEq, Eq)]
struct NamespaceDigest {
    frame: Option<Vec<u8>>,
    value: String,
}

impl NamespaceDigest {
    fn from_reference(reference: &NamespaceReference) -> Self {
        Self {
            frame: Some(reference.frame_digest.clone()),
            value: reference.value_digest.clone(),
        }
    }

    fn matches(&self, reference: &NamespaceReference) -> bool {
        self.value == reference.value_digest && self.frame.as_ref().is_none_or(|frame| frame == &reference.frame_digest)
    }
}

#[derive(Debug)]
struct SelectedBody {
    digest: NamespaceDigest,
    header: Arc<GraphNamespaceHeader>,
    interface_digest: Option<[u8; 32]>,
    wire_bytes: usize,
    /// Exact authored bytes held only until the backend returns a reusable decoded body.
    authored: Option<Vec<u8>>,
}

struct ResidentBody {
    selected: SelectedBody,
    body: ResidentBodyIr,
}

/// Own one representation. A worker can decode the bytes it already published when native preparation moved
/// parts of the decoded representation into an invocation-specific context.
enum ResidentBodyIr {
    Decoded(BodyIrModule),
    Authored(Vec<u8>),
}

struct ResidentBodies {
    scope: ProgramScope,
    values: HashMap<String, ResidentBody>,
    wire_bytes: usize,
}

static RESIDENT: Mutex<Option<ResidentBodies>> = Mutex::new(None);

/// Sorted frame addresses owned by one request. Oven keeps the producing snapshot leased until the request ends.
#[derive(Debug, Default)]
pub(super) struct NamespacePages {
    references: Vec<NamespaceReference>,
    reader: Option<Mutex<incan_worker_host_native::NamespaceFrameReader>>,
    scope: Option<ProgramScope>,
    checked: Mutex<HashMap<String, SelectedBody>>,
}

impl NamespacePages {
    /// Read the bounded transport once. Older installations write ordinary keyed namespace files.
    pub(super) fn open(
        directory: &Path,
        bakery: Option<&Path>,
        project: &Path,
        retain_bodies: bool,
    ) -> Result<Self, String> {
        let path = directory.join(internal_models::namespace_reference_filename());
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::with_selection(Vec::new(), None, bakery, project, retain_bodies));
            }
            Err(error) => return Err(format!("cannot inspect namespace frame references: {error}")),
        };
        if !metadata.is_file() || metadata.len() > 64 * 1024 * 1024 {
            return Err("namespace frame references require a bakery and a bounded regular page".into());
        }
        let bakery = bakery
            .and_then(Path::to_str)
            .ok_or("namespace frame references require a UTF-8 Bakery anchor")?;
        let reader = Some(Mutex::new(incan_worker_host_native::namespace_frame_reader(bakery)));
        let bytes = std::fs::read(path).map_err(|error| format!("cannot read namespace frame references: {error}"))?;
        let mut references = internal_models::decode_namespace_references(&bytes)?;
        references.sort_unstable_by(|left, right| left.key.identity.cmp(&right.key.identity));
        Ok(Self::with_selection(
            references,
            reader,
            Some(Path::new(bakery)),
            project,
            retain_bodies,
        ))
    }

    fn with_selection(
        references: Vec<NamespaceReference>,
        reader: Option<Mutex<incan_worker_host_native::NamespaceFrameReader>>,
        bakery: Option<&Path>,
        project: &Path,
        retain_bodies: bool,
    ) -> Self {
        let scope = bakery.filter(|_| retain_bodies).map(|bakery| ProgramScope {
            bakery: bakery.to_owned(),
            project: project.to_owned(),
        });
        if let Some(scope) = &scope
            && let Ok(mut resident) = RESIDENT.lock()
        {
            let contents = resident.get_or_insert_with(|| ResidentBodies {
                scope: scope.clone(),
                values: HashMap::new(),
                wire_bytes: 0,
            });
            if &contents.scope != scope {
                contents.scope = scope.clone();
                contents.values.clear();
                contents.wire_bytes = 0;
            }
            contents.values.retain(|source, cached| {
                references
                    .binary_search_by(|reference| reference.key.identity.as_str().cmp(source))
                    .is_ok_and(|index| cached.selected.digest.matches(&references[index]))
            });
            contents.wire_bytes = contents.values.values().map(|body| body.selected.wire_bytes).sum();
        }
        Self {
            references,
            reader,
            scope,
            checked: Mutex::default(),
        }
    }

    /// Recover an admitted namespace through the selected address, verifying both frame and evaluation digests.
    pub(super) fn read(
        &self,
        directory: &Path,
        source: &str,
    ) -> Result<Option<GraphNamespaceRecord>, RecordAdapterFault> {
        let Ok(index) = self
            .references
            .binary_search_by(|reference| reference.key.identity.as_str().cmp(source))
        else {
            return read_legacy_namespace(directory, source);
        };
        let reference = &self.references[index];
        if let Some(mut retained) = self.take(source, reference) {
            if retained.selected.digest.frame.is_none() {
                retained.selected.digest.frame = Some(reference.frame_digest.clone());
            }
            let body_ir = match retained.body {
                ResidentBodyIr::Decoded(body) => {
                    tracing::trace!(target: "incan::invocation", module = source, wire_bytes = retained.selected.wire_bytes,
                        "reused resident namespace Body IR");
                    body
                }
                ResidentBodyIr::Authored(bytes) => {
                    let _decode = tracing::trace_span!(
                        target: "incan::invocation", "incan_rustc_driver::decode_resident_namespace_body_ir",
                        module = source, bytes = bytes.len(),
                    )
                    .entered();
                    let body = decode_namespace_body(&bytes)?;
                    retained.selected.authored = Some(bytes);
                    body
                }
            };
            let record = GraphNamespaceRecord {
                header: Arc::clone(&retained.selected.header),
                body_ir,
            };
            self.note(source, retained.selected);
            return Ok(Some(record));
        }
        let reader = self
            .reader
            .as_ref()
            .ok_or_else(|| RecordAdapterFault::InvalidField("namespace frame has no Bakery reader".into()))?;
        let bytes = reference.length
            - reference.key.family.len() as i64
            - reference.key.identity_profile.len() as i64
            - reference.key.origin.len() as i64
            - reference.key.identity.len() as i64
            - reference.key.facet.len() as i64
            - reference.key.context.len() as i64
            - 8 // frame key length prefix
            - 5; // separators in the six-field record key
        let _page = tracing::trace_span!(
            target: "incan::invocation", "incan_rustc_driver::restore_namespace",
            module = source, bytes, backing = "bakery"
        )
        .entered();
        let view = {
            let _frame = tracing::trace_span!(
                target: "incan::invocation", "incan_rustc_driver::open_namespace_frame",
                module = source, segment = reference.segment.as_str(), bytes,
            )
            .entered();
            let mut reader = reader
                .lock()
                .map_err(|_| RecordAdapterFault::InvalidField("Bakery namespace reader is poisoned".into()))?;
            incan_worker_host_native::read_namespace_frame(&mut reader, &frame_reference(reference))
                .map_err(RecordAdapterFault::InvalidField)?
        };
        let record = {
            let _row = tracing::trace_span!(
                target: "incan::invocation", "incan_rustc_driver::admit_namespace_envelope",
                module = source, bytes,
            )
            .entered();
            internal_models::decode_graph_namespace_view(&view).map_err(namespace_codec_error)?
        };
        let Some((record, interface_digest, wire_bytes)) = admit_namespace(source, record)? else {
            return Ok(None);
        };
        self.note(
            source,
            SelectedBody {
                digest: NamespaceDigest::from_reference(reference),
                header: Arc::clone(&record.header),
                interface_digest: Some(interface_digest),
                wire_bytes,
                authored: None,
            },
        );
        Ok(Some(record))
    }

    /// Take the one decoded owner. The cache never keeps a second copy alongside an active compiler.
    fn take(&self, source: &str, reference: &NamespaceReference) -> Option<ResidentBody> {
        let scope = self.scope.as_ref()?;
        let mut resident = RESIDENT.lock().ok()?;
        let held = resident.as_mut().filter(|held| &held.scope == scope)?;
        let entry = held.values.remove(source)?;
        held.wire_bytes -= entry.selected.wire_bytes;
        entry.selected.digest.matches(reference).then_some(entry)
    }

    fn note(&self, source: &str, selected: SelectedBody) {
        if self.scope.is_some()
            && let Ok(mut checked) = self.checked.lock()
        {
            checked.insert(source.to_owned(), selected);
        }
    }

    /// Record the exact value produced by today's compiler. The next selected reference supplies its frame seal.
    pub(super) fn publish(&self, header: Arc<GraphNamespaceHeader>, value: String, body_ir: Vec<u8>) {
        let source = header.source.clone();
        self.note(
            &source,
            SelectedBody {
                digest: NamespaceDigest { frame: None, value },
                header,
                interface_digest: None,
                wire_bytes: body_ir.len(),
                authored: Some(body_ir),
            },
        );
    }

    pub(super) fn retains(&self, source: &str) -> bool {
        self.scope.is_some() && self.checked.lock().is_ok_and(|checked| checked.contains_key(source))
    }

    /// Move the authoritative Body IR back after the backend has borrowed it. Retain only owned header state.
    pub(super) fn retain(&self, source: &str, body: &mut BodyIrModule) {
        let Some(mut selected) = self.checked.lock().ok().and_then(|mut checked| checked.remove(source)) else {
            return;
        };
        if let Some(digest) = selected.interface_digest {
            let Some(interface) = namespace_crumb::resident_interface(&digest) else {
                return;
            };
            Arc::make_mut(&mut selected.header).interface = interface;
        }
        selected.authored = None;
        self.keep(source.to_owned(), selected, || {
            ResidentBodyIr::Decoded(std::mem::replace(body, BodyIrModule::declarations_only(String::new())))
        });
    }

    /// Preserve the available authored bytes under the same bound, after giving decoded bodies first admission.
    pub(super) fn settle(&self) {
        let pending = self
            .checked
            .lock()
            .ok()
            .map(|mut checked| std::mem::take(&mut *checked));
        for (source, mut selected) in pending.into_iter().flatten() {
            if let Some(body) = selected.authored.take() {
                self.keep(source, selected, || ResidentBodyIr::Authored(body));
            }
        }
    }

    fn keep(&self, source: String, selected: SelectedBody, body: impl FnOnce() -> ResidentBodyIr) {
        let Some(scope) = &self.scope else { return };
        let Ok(mut resident) = RESIDENT.lock() else { return };
        let Some(held) = resident.as_mut().filter(|held| &held.scope == scope) else {
            return;
        };
        if held.values.len() >= RETAINED_NAMESPACES || held.wire_bytes + selected.wire_bytes > RETAINED_BODY_BYTES {
            return;
        }
        let body = body();
        held.wire_bytes += selected.wire_bytes;
        match &body {
            ResidentBodyIr::Decoded(_) => tracing::trace!(target: "incan::invocation",
                module = source.as_str(), wire_bytes = selected.wire_bytes, "kept decoded namespace Body IR"),
            ResidentBodyIr::Authored(_) => tracing::trace!(target: "incan::invocation",
                module = source.as_str(), wire_bytes = selected.wire_bytes, "kept authored namespace encoding"),
        }
        held.values.insert(source, ResidentBody { selected, body });
    }
}

/// Report a schema refusal identically for both namespace transports.
fn namespace_codec_error(error: internal_models::CodecFault) -> RecordAdapterFault {
    RecordAdapterFault::InvalidField(format!("graph namespace page refused: {error:?}"))
}

fn decode_namespace_body(bytes: &[u8]) -> Result<BodyIrModule, RecordAdapterFault> {
    incan_semantics_core::executable_representation::decode_namespace_module(bytes)
        .map_err(|error| RecordAdapterFault::InvalidField(format!("graph namespace Body IR refused: {error}")))
}

/// Read a complete namespace page staged by an Oven that did not offer store references.
fn read_legacy_namespace(root: &Path, source: &str) -> Result<Option<GraphNamespaceRecord>, RecordAdapterFault> {
    const MAX_PAGE_BYTES: u64 = 64 * 1024 * 1024;
    let key = format!("namespace-{}", hex::encode(Sha256::digest(source.as_bytes())));
    let path = root.join(key);
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(incan_frontend::checker_records::RecordAdapterFault::InvalidField(
                format!("cannot inspect graph namespace page {}: {error}", path.display()),
            ));
        }
    };
    if !metadata.is_file() || metadata.len() > MAX_PAGE_BYTES {
        return Err(incan_frontend::checker_records::RecordAdapterFault::InvalidField(
            "graph namespace page is not a bounded regular file".into(),
        ));
    }
    let _page = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::restore_namespace", module = source, bytes = metadata.len()).entered();
    let encoded = std::fs::read(&path).map_err(|error| {
        incan_frontend::checker_records::RecordAdapterFault::InvalidField(format!(
            "cannot read graph namespace page {}: {error}",
            path.display()
        ))
    })?;
    let record = internal_models::decode_owned_namespace_view(&encoded).map_err(namespace_codec_error)?;
    admit_namespace(source, record).map(|selected| selected.map(|(record, _, _)| record))
}

/// Bind compiler-owned namespace metadata and Body IR to the source whose graph record was opened.
fn admit_namespace(
    source: &str,
    record: GraphNamespaceView,
) -> Result<Option<(GraphNamespaceRecord, [u8; 32], usize)>, RecordAdapterFault> {
    if record.module != source {
        return Err(incan_frontend::checker_records::RecordAdapterFault::InvalidField(
            "graph namespace page disagrees with its requested source".into(),
        ));
    }
    if !record.call_reads_known {
        return Ok(None);
    }
    let interface_span = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::decode_namespace_interface", module = source, bytes = record.interface.0.as_bytes().len()).entered();
    let (checked, interface_digest) = namespace_crumb::admit(source, &record.identity_proof, record.interface)
        .map_err(incan_frontend::checker_records::RecordAdapterFault::InvalidField)?;
    drop(interface_span);
    let wire_bytes = record.body_ir.0.as_bytes().len();
    let body_span = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::decode_namespace_body_ir", module = source, bytes = record.body_ir.0.as_bytes().len()).entered();
    let body_ir = decode_namespace_body(record.body_ir.0.as_bytes())?;
    drop(body_span);
    // Join witnesses by exact kernel owner path once per namespace. Duplicate declarations stay ambiguous.
    let mut owners = std::collections::HashMap::new();
    if !record.lowering_inputs.is_empty() {
        for body in &body_ir.bodies {
            owners
                .entry(body.direct_call_id.path())
                .and_modify(|found| *found = None)
                .or_insert(Some(&body.direct_call_id));
        }
    }
    let mut lowering_inputs = Vec::with_capacity(record.lowering_inputs.len());
    for input in record.lowering_inputs {
        let [owner, witness]: [String; 2] = input.try_into().map_err(|_| {
            incan_frontend::checker_records::RecordAdapterFault::InvalidField(
                "graph namespace lowering witness has the wrong field count".into(),
            )
        })?;
        let id = match owners.get(owner.as_str()) {
            Some(Some(id)) => *id,
            Some(None) => {
                return Err(incan_frontend::checker_records::RecordAdapterFault::InvalidField(
                    "graph namespace lowering witness names multiple Body IR owners".into(),
                ));
            }
            None => {
                return Err(incan_frontend::checker_records::RecordAdapterFault::InvalidField(
                    "graph namespace lowering witness names no Body IR owner".into(),
                ));
            }
        };
        lowering_inputs.push((id.clone(), witness));
    }
    let mut package_roots = Vec::with_capacity(record.package_roots.len());
    for encoded in record.package_roots {
        let identity = incan_semantics_core::decode_incan_symbol_identity(&encoded)
            .ok()
            .flatten()
            .filter(|identity| matches!(identity.origin, SymbolOrigin::Package { .. }))
            .ok_or_else(|| {
                incan_frontend::checker_records::RecordAdapterFault::InvalidField(
                    "graph namespace has an invalid package declaration root".into(),
                )
            })?;
        package_roots.push(identity);
    }
    let call_reads = record
        .call_reads
        .into_iter()
        .map(|r| incan_frontend::body_ir::SettledCallRead {
            reader: r.reader,
            target: r.target,
            digest: r.digest,
        })
        .collect::<Arc<[_]>>();
    let settled = super::checked_program::settle_lowering_witnesses(&mut lowering_inputs, call_reads.iter())
        .map_err(RecordAdapterFault::InvalidField)?;
    tracing::trace!(target: "incan::invocation", module = source, settled, "prepared namespace lowering witnesses");
    let header = Arc::new(GraphNamespaceHeader {
        source: record.module,
        call_reads,
        interface: checked,
        package_roots: package_roots.into(),
        lowering_inputs: lowering_inputs.into(),
    });
    Ok(Some((
        GraphNamespaceRecord { header, body_ir },
        interface_digest,
        wire_bytes,
    )))
}

/// The worker host's frame reference for one admitted namespace reference: the same fields in the Rust record model,
/// until the record models are one Rust type.
fn frame_reference(reference: &NamespaceReference) -> internal_models_native::namespace_references::NamespaceReference {
    let key = &reference.key;
    internal_models_native::namespace_references::NamespaceReference {
        key: internal_models_native::records::RecordKey {
            family: key.family.clone(),
            identity: key.identity.clone(),
            identity_profile: key.identity_profile.clone(),
            origin: key.origin.clone(),
            facet: key.facet.clone(),
            context: key.context.clone(),
        },
        segment: reference.segment.clone(),
        offset: reference.offset,
        length: reference.length,
        frame_digest: reference.frame_digest.clone(),
        value_digest: reference.value_digest.clone(),
    }
}

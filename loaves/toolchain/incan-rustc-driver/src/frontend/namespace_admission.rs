//! Admit an unchanged editable namespace by the small header Oven offers beside its frame (DD-0015 §9): checking reads
//! the header, and opens the namespace frame only for an interface a checked module installs.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use incan_frontend::checker_records::{GraphNamespaceAdmission, GraphNamespaceInterface, RecordAdapterFault};
use incan_frontend::typechecker::ModuleDependencyInterface;
use incan_semantics_core::CanonicalSymbolId;
use internal_models::NamespaceReference;
use internal_models_native::MappedView;
use internal_models_native::records::RecordKey;

use super::namespace_crumb;

/// The identity of an authenticated namespace frame. Compaction can change its address without changing its bytes.
#[derive(Debug, PartialEq, Eq, Hash)]
struct FrameRevision {
    key: RecordKey,
    frame_digest: Vec<u8>,
    value_digest: String,
}

const MAX_INSTALLED_INTERFACES: usize = 512;

/// Verified frame revisions in the existing resident program's scope. The decoded interface owns its tables.
#[derive(Debug, Default)]
pub(super) struct ResidentInterfaces {
    installed: Mutex<HashMap<FrameRevision, Weak<ModuleDependencyInterface>>>,
}

impl ResidentInterfaces {
    /// Borrow tables only from the exact authenticated revision currently selected in this program.
    fn get(&self, revision: &FrameRevision) -> Option<Arc<ModuleDependencyInterface>> {
        self.installed.lock().ok()?.get(revision)?.upgrade()
    }

    /// Retain the frame witness without extending the lifetime of the immutable tables it identifies.
    fn remember(&self, revision: FrameRevision, interface: &Arc<ModuleDependencyInterface>) {
        let Ok(mut interfaces) = self.installed.lock() else {
            return;
        };
        if interfaces.len() >= MAX_INSTALLED_INTERFACES {
            interfaces.retain(|_, interface| interface.strong_count() > 0);
        }
        if interfaces.len() < MAX_INSTALLED_INTERFACES {
            interfaces.insert(revision, Arc::downgrade(interface));
        }
    }
}

/// The request's Bakery frame reader and the frames it has verified, shared by the namespace pages and the interfaces
/// admitted from headers, so each frame a request reads is opened and verified once.
#[derive(Debug, Clone)]
pub(super) struct FrameAccess {
    reader: Arc<Mutex<incan_worker_host_native::NamespaceFrameReader>>,
    verified: Arc<Mutex<HashMap<String, MappedView>>>,
}

impl FrameAccess {
    pub(super) fn new(reader: incan_worker_host_native::NamespaceFrameReader) -> Self {
        Self {
            reader: Arc::new(Mutex::new(reader)),
            verified: Arc::default(),
        }
    }

    /// The verified view of `source`'s frame at `reference`, read and authenticated the first time it is asked for.
    pub(super) fn frame(&self, source: &str, reference: &NamespaceReference) -> Result<MappedView, RecordAdapterFault> {
        let poisoned = |_| RecordAdapterFault::InvalidField("Bakery namespace frames are poisoned".into());
        if let Some(view) = self.verified.lock().map_err(poisoned)?.get(source) {
            return Ok(view.clone());
        }
        let view = self.read(reference)?;
        self.verified
            .lock()
            .map_err(poisoned)?
            .insert(source.to_owned(), view.clone());
        Ok(view)
    }

    /// Read and authenticate one frame through the request's segment mappings.
    pub(super) fn read(&self, reference: &NamespaceReference) -> Result<MappedView, RecordAdapterFault> {
        let mut reader = self
            .reader
            .lock()
            .map_err(|_| RecordAdapterFault::InvalidField("Bakery namespace reader is poisoned".into()))?;
        incan_worker_host_native::read_namespace_frame(&mut reader, &super::namespace_pages::frame_reference(reference))
            .map_err(RecordAdapterFault::InvalidField)
    }
}

/// Admit `source` by its header frame at `header`, with its declaration surface opened from its namespace frame at
/// `frame` only when installed.
pub(super) fn admit_header(
    access: &FrameAccess,
    source: &str,
    header: &NamespaceReference,
    frame: &NamespaceReference,
    resident: Option<Arc<ResidentInterfaces>>,
) -> Result<GraphNamespaceAdmission, RecordAdapterFault> {
    let _span = tracing::trace_span!(
        target: "incan::invocation", "incan_rustc_driver::admit_namespace_header",
        module = source, bytes = header.length,
    )
    .entered();
    let view = access.read(header)?;
    let value =
        internal_models_native::decode_namespace_header(view.0.as_bytes()).map_err(RecordAdapterFault::InvalidField)?;
    if value.module != source {
        return Err(RecordAdapterFault::InvalidField(
            "graph namespace header disagrees with its requested source".into(),
        ));
    }
    let identity = internal_models_native::namespace_proof_identity(&value.identity_proof)
        .map_err(RecordAdapterFault::InvalidField)?;
    let call_reads = value
        .call_reads
        .into_iter()
        .map(|read| incan_frontend::body_ir::SettledCallRead {
            reader: read.reader,
            target: read.target,
            digest: read.digest,
        })
        .collect();
    Ok(GraphNamespaceAdmission {
        call_reads,
        interface: Arc::new(FrameInterface {
            source: source.to_owned(),
            name: identity.name,
            path: identity.path,
            frame: frame.clone(),
            access: access.clone(),
            resident,
            admitted: OnceLock::new(),
        }),
    })
}

/// A namespace's declaration surface known by its header's identity, opened from its frame on installation.
#[derive(Debug)]
struct FrameInterface {
    source: String,
    name: String,
    path: Vec<String>,
    frame: NamespaceReference,
    access: FrameAccess,
    resident: Option<Arc<ResidentInterfaces>>,
    admitted: OnceLock<Result<Arc<ModuleDependencyInterface>, String>>,
}

impl FrameInterface {
    /// Share the installed tables when the request selects their authenticated frame and matching header identity.
    fn install(&self) -> Result<Arc<ModuleDependencyInterface>, String> {
        let selection = self.resident.as_ref().map(|interfaces| {
            let frame = super::namespace_pages::frame_reference(&self.frame);
            let revision = FrameRevision {
                key: frame.key,
                frame_digest: frame.frame_digest,
                value_digest: frame.value_digest,
            };
            (interfaces, revision)
        });
        if let Some((interfaces, revision)) = &selection
            && let Some(interface) = interfaces.get(revision)
        {
            if interface.module_name != self.name || interface.module_path != self.path {
                return Err("checked namespace identity disagrees with its admitted header".into());
            }
            return Ok(interface);
        }
        let interface = self.open()?.installable()?;
        if let Some((interfaces, revision)) = selection {
            interfaces.remember(revision, &interface);
        }
        Ok(interface)
    }

    /// Open the frame and admit its interface exactly as a complete namespace read admits it.
    fn open(&self) -> Result<Arc<dyn GraphNamespaceInterface>, String> {
        let _span = tracing::trace_span!(
            target: "incan::invocation", "incan_rustc_driver::open_installed_namespace_interface",
            module = self.source.as_str(),
        )
        .entered();
        let view = self
            .access
            .frame(&self.source, &self.frame)
            .map_err(|fault| format!("{fault:?}"))?;
        let record = internal_models::decode_graph_namespace_view(&view).map_err(|error| format!("{error:?}"))?;
        if record.module != self.source {
            return Err("graph namespace page disagrees with its requested source".into());
        }
        let (interface, _) = namespace_crumb::admit(&self.source, &record.identity_proof, record.interface)?;
        if !interface.belongs_to(&self.name, &self.path) {
            return Err("checked namespace identity disagrees with its admitted header".into());
        }
        Ok(interface)
    }
}

impl GraphNamespaceInterface for FrameInterface {
    fn belongs_to(&self, name: &str, path: &[String]) -> bool {
        self.name == name && self.path == path
    }

    fn installable(&self) -> Result<Arc<ModuleDependencyInterface>, String> {
        self.admitted.get_or_init(|| self.install()).clone()
    }

    fn lowering_member_owners(&self) -> Result<BTreeMap<CanonicalSymbolId, String>, String> {
        self.open()?.lowering_member_owners()
    }
}

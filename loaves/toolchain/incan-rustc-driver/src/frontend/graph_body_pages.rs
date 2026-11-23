//! Read authenticated owner pages and admit each against the selected namespace context.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use incan_semantics_core::body_ir::BodyIrModule;
use sha2::{Digest, Sha256};

use super::{decoded_pages, namespace_pages};

/// Filesystem-backed source of independently bounded graph body pages.
#[derive(Debug)]
struct GraphBodyRecordPages {
    root: PathBuf,
    bakery: Option<PathBuf>,
    namespaces: Arc<namespace_pages::NamespacePages>,
    reopened: std::sync::OnceLock<Result<tempfile::TempDir, String>>,
}

impl GraphBodyRecordPages {
    /// Address one selected owner, preferring a newly reopened sibling page.
    fn owner_page(&self, identity: &str) -> PathBuf {
        let key = hex::encode(Sha256::digest(identity.as_bytes()));
        if let Some(Ok(directory)) = self.reopened.get() {
            let supplied = directory.path().join(&key);
            if supplied.is_file() {
                return supplied;
            }
        }
        self.root.join(key)
    }

    /// Attach the scope only after the requested snapshot authenticates the referenced module and digest.
    fn bind_executable_scope(
        &self,
        page: decoded_pages::DecodedBodyPage,
    ) -> Result<incan_frontend::checker_records::GraphBodyRecord, incan_frontend::checker_records::RecordAdapterFault>
    {
        use incan_frontend::checker_records::RecordAdapterFault;
        let mut record = page.record;
        if let Some(digest) = page.scope_digest {
            let frame = record
                .body_ir
                .as_mut()
                .ok_or_else(|| RecordAdapterFault::InvalidField("executable scope has no selected body".into()))?;
            frame.body.named_type_identities = self
                .namespaces
                .scope_access
                .resolve(&frame.source, &digest)
                .map_err(RecordAdapterFault::InvalidField)?;
        }
        Ok(record)
    }
}

impl incan_frontend::checker_records::GraphBodyRecordSource for GraphBodyRecordPages {
    fn has_owner(&self, identity: &str) -> bool {
        self.owner_page(identity).try_exists().unwrap_or(true)
    }

    /// Read only the page addressed by this stable owner; absence keeps that owner on the ordinary checking path.
    fn read(
        &self,
        identity: &str,
    ) -> Result<
        Option<incan_frontend::checker_records::GraphBodyRecord>,
        incan_frontend::checker_records::RecordAdapterFault,
    > {
        const MAX_PAGE_BYTES: u64 = 64 * 1024 * 1024;
        let path = self.owner_page(identity);
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(incan_frontend::checker_records::RecordAdapterFault::InvalidField(
                    format!("cannot inspect graph body page {}: {error}", path.display()),
                ));
            }
        };
        if !metadata.is_file() || metadata.len() > MAX_PAGE_BYTES {
            return Err(incan_frontend::checker_records::RecordAdapterFault::InvalidField(
                "graph body page is not a bounded regular file".into(),
            ));
        }
        let generation = crate::native_search::FileGeneration::observe(&metadata);
        let typed = self.namespaces.typed_executable;
        if let Some(generation) = generation
            && let Some(record) = decoded_pages::observed(generation, identity, typed)
        {
            tracing::trace!(target: "incan::invocation", phase = "checker_page_retained", owner = identity);
            return self.bind_executable_scope(record).map(Some);
        }
        let _input = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::read_checker_page",
            owner = identity, bytes = metadata.len())
        .entered();
        let (encoded, generation) = read_checker_page(&path, generation, MAX_PAGE_BYTES)?;
        let digest: [u8; 32] = Sha256::digest(&encoded).into();
        if let Some(record) = decoded_pages::decoded(digest, identity, generation, typed) {
            return self.bind_executable_scope(record).map(Some);
        }
        // Oven writes each page as one owner's Crumb row (`encode_graph_body_values`); the shared codec decodes and
        // authenticates it, both checker records included.
        let mut records = internal_models::decode_graph_checking_inputs(&encoded).map_err(|error| {
            incan_frontend::checker_records::RecordAdapterFault::InvalidField(format!(
                "graph body page refused: {error:?}"
            ))
        })?;
        let record = match (records.pop(), records.is_empty()) {
            (Some(record), true) if record.identity == identity => record,
            _ => {
                return Err(incan_frontend::checker_records::RecordAdapterFault::InvalidField(
                    "graph body page disagrees with its requested identity".into(),
                ));
            }
        };
        let (frame, scope_digest) = if record.body_ir.is_empty() {
            (None, None)
        } else {
            let (frame, scope_digest) = if typed {
                let opened =
                    incan_frontend::typechecker::executable_frames::open_graph_executable_owner(&record.body_ir)
                        .map_err(incan_frontend::checker_records::RecordAdapterFault::InvalidField)?;
                (opened.frame, opened.scope_digest)
            } else {
                let frame = incan_semantics_core::executable_representation::decode_owner(&record.body_ir).map_err(
                    |error| {
                        incan_frontend::checker_records::RecordAdapterFault::InvalidField(format!(
                            "graph Body IR owner frame refused: {error}"
                        ))
                    },
                )?;
                (frame, None)
            };
            if frame.source.is_empty() || frame.identity != record.identity {
                return Err(incan_frontend::checker_records::RecordAdapterFault::InvalidField(
                    "graph Body IR owner frame disagrees with its selected identity".into(),
                ));
            }
            (Some(frame), scope_digest)
        };
        let record = incan_frontend::checker_records::GraphBodyRecord {
            identity: record.identity,
            local: record.local.into(),
            final_value: record.final_fields.into(),
            body_ir: frame,
        };
        let page = decoded_pages::DecodedBodyPage { record, scope_digest };
        decoded_pages::remember(digest, generation, &page, encoded.len(), typed);
        self.bind_executable_scope(page).map(Some)
    }

    fn reopen_namespace_peers(&self, source: &str, changed: &std::collections::BTreeSet<String>) -> Result<(), String> {
        let Some(bakery) = &self.bakery else { return Ok(()) };
        if !self
            .root
            .join("reopen-snapshot.crumb")
            .try_exists()
            .map_err(|e| e.to_string())?
        {
            return Ok(());
        }
        let Some(oven) = std::env::var_os("INCAN_GRAPH_PEER_READER") else {
            return Ok(());
        };
        let temporary = self
            .reopened
            .get_or_init(|| {
                tempfile::Builder::new()
                    .prefix("incan-reopened-peers-")
                    .tempdir()
                    .map_err(|e| e.to_string())
            })
            .as_ref()
            .map_err(Clone::clone)?;
        let _trace = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::reopen_namespace_peers", module = source).entered();
        let status = std::process::Command::new(oven)
            .arg("--internal-reopened-peers")
            .arg(bakery)
            .arg(&self.root)
            .arg(temporary.path())
            .arg(source)
            .args(changed)
            .status()
            .map_err(|error| format!("cannot start the selected graph reader: {error}"))?;
        if !status.success() {
            return Err(format!("graph reader refused reopened namespace {source} ({status})"));
        }
        Ok(())
    }

    /// Read the request's exact namespace selection on demand.
    fn read_namespace(
        &self,
        source: &str,
    ) -> Result<
        Option<incan_frontend::checker_records::GraphNamespaceRecord>,
        incan_frontend::checker_records::RecordAdapterFault,
    > {
        self.namespaces.read(&self.root, source)
    }

    /// Admit an unchanged namespace by the header the request offers beside its frame.
    fn read_namespace_header(
        &self,
        source: &str,
    ) -> Result<
        Option<incan_frontend::checker_records::GraphNamespaceAdmission>,
        incan_frontend::checker_records::RecordAdapterFault,
    > {
        self.namespaces.read_header(source)
    }

    fn publish_namespace(
        &self,
        header: std::sync::Arc<incan_frontend::checker_records::GraphNamespaceHeader>,
        value: String,
        body_ir: Vec<u8>,
        executable_scopes: Vec<String>,
    ) {
        self.namespaces.publish(header, value, body_ir, executable_scopes);
    }

    fn retains_namespace(&self, source: &str) -> bool {
        self.namespaces.retains(source)
    }

    fn retain_namespace(&self, source: &str, body: &mut BodyIrModule) {
        self.namespaces.retain(source, body);
    }

    fn settle_namespace_retention(&self) {
        self.namespaces.settle();
    }
}

/// Read an exact bounded file through one descriptor. A changing read is refused before the codec sees its bytes.
fn read_checker_page(
    path: &Path,
    generation: Option<crate::native_search::FileGeneration>,
    limit: u64,
) -> Result<(Vec<u8>, Option<crate::native_search::FileGeneration>), incan_frontend::checker_records::RecordAdapterFault>
{
    use crate::native_search::FileGeneration;
    use incan_frontend::checker_records::RecordAdapterFault;
    use std::io::Read;
    let read_failed =
        |error| RecordAdapterFault::InvalidField(format!("cannot read graph body page {}: {error}", path.display()));
    let changed = || RecordAdapterFault::InvalidField("graph body page changed during its read".into());
    let mut file = std::fs::File::open(path).map_err(read_failed)?;
    let metadata = file.metadata().map_err(read_failed)?;
    if !metadata.is_file() || metadata.len() > limit || FileGeneration::observe(&metadata) != generation {
        return Err(changed());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(read_failed)?;
    if bytes.len() as u64 != metadata.len()
        || FileGeneration::observe(&file.metadata().map_err(read_failed)?) != generation
    {
        return Err(changed());
    }
    Ok((bytes, generation))
}

/// Bind the engine's keyed graph-record page directory without reading an aggregate inventory.
pub(super) fn read_graph_body_records(
    path: &Path,
    bakery: Option<PathBuf>,
    project_root: &Path,
    retain_namespaces: bool,
) -> Result<
    (
        incan_frontend::checker_records::GraphBodyRecords,
        Arc<namespace_pages::NamespacePages>,
    ),
    String,
> {
    if !path.is_dir() {
        return Err(format!(
            "graph body record pages are not a directory: {}",
            path.display()
        ));
    }
    let namespaces = Arc::new(namespace_pages::NamespacePages::open(
        path,
        bakery.as_deref(),
        project_root,
        retain_namespaces,
    )?);
    let records =
        incan_frontend::checker_records::GraphBodyRecords::from_source(std::sync::Arc::new(GraphBodyRecordPages {
            root: path.to_path_buf(),
            namespaces: Arc::clone(&namespaces),
            bakery,
            reopened: std::sync::OnceLock::new(),
        }));
    if path
        .join("ranked-source-edit.crumb")
        .try_exists()
        .map_err(|e| e.to_string())?
    {
        return bind_contextual_rank(path, records).map(|records| (records, namespaces));
    }
    Ok((records, namespaces))
}

/// Bind a body-only edit and the exact namespace order the engine selected for it.
fn bind_contextual_rank(
    directory: &Path,
    records: incan_frontend::checker_records::GraphBodyRecords,
) -> Result<incan_frontend::checker_records::GraphBodyRecords, String> {
    let bounded = |name: &str| -> Result<Vec<u8>, String> {
        use std::io::Read;
        const LIMIT: u64 = 65_536;
        let mut bytes = Vec::new();
        std::fs::File::open(directory.join(name))
            .map_err(|error| error.to_string())?
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 > LIMIT {
            return Err(format!("contextual ranked page {name} exceeds its bound"));
        }
        Ok(bytes)
    };
    let demand = internal_models::decode_demand_page(&bounded("ranked-source-edit.crumb")?)
        .map_err(|error| error.to_string())?;
    let [owner, source]: [_; 2] = demand
        .keys
        .try_into()
        .map_err(|_| "contextual ranked edit requires one owner and its changed source")?;
    let name = owner
        .identity
        .split_once('|')
        .and_then(|(identity, _)| identity.strip_prefix("function:"))
        .and_then(|identity| identity.rsplit_once("::"))
        .map(|(_, name)| name);
    if owner.family != "ranked-source-edit"
        || owner.identity_profile != "incan.stable-declaration/1"
        || source.family != "owner-source"
        || source.identity_profile != "incan.source-projection/1"
        || source.origin != owner.origin
        || source.identity != owner.facet
        || name != Some(source.facet.as_str())
        || !owner.facet.ends_with(".incn")
        || Path::new(&owner.facet)
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err("contextual ranked edit does not bind a project source owner".into());
    }
    let order = internal_models::decode_namespace_order(&bounded("ranked-source-order.crumb")?, &owner.origin)
        .map_err(|error| error.to_string())?;
    if order.evidence != owner.context || order.members.as_slice() != [owner.facet.as_str()] {
        return Err("contextual ranked edit does not match its namespace certificate".into());
    }
    Ok(records.for_contextual_ranked_owner(owner.identity, owner.facet, order.rank, order.evidence))
}

#[cfg(test)]
#[path = "graph_body_page_tests.rs"]
mod graph_body_page_tests;

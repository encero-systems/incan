//! Open only the nominal scopes selected by executable restoration in one pinned request.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, mpsc};
use std::thread::{self, JoinHandle};

use incan_frontend::checker_records::ExecutableScopeWitness;
use incan_frontend::typechecker::executable_frames::open_mapped_graph_executable_scope;
use incan_semantics_core::body_ir::SharedIdentities;
use internal_models::ExecutableSnapshot;
use sha2::{Digest, Sha256};

/// Freeze the authored scope dictionary for all mapped reads in this compiler process.
static EXECUTABLE_SCOPES: LazyLock<Result<internal_models_native::PreparedWireReader, String>> =
    LazyLock::new(internal_models::prepare_executable_scope_reader);

/// Request-owned scope authority and the immutable checked bindings already restored under it.
#[derive(Default)]
pub(super) struct GraphScopeAccess {
    request: PathBuf,
    bakery: Option<PathBuf>,
    state: Mutex<ScopeState>,
}

#[derive(Default)]
struct ScopeState {
    reader: Option<Result<ScopeReader, String>>,
    decoded: HashMap<String, HashMap<String, ScopeValue>>,
}

struct ScopeAddress {
    module: String,
    digest: String,
}

struct RetainedScope {
    revision: Option<Vec<u8>>,
    bindings: SharedIdentities,
}

/// The immutable bindings and the authenticated frame that supplied them in a pinned snapshot.
#[derive(Clone)]
struct ScopeValue {
    revision: Vec<u8>,
    bindings: SharedIdentities,
}

/// Keep the thread-bound Incan lease with its owner. Compiler workers share only the resulting immutable bindings.
struct ScopeReader {
    requests: Option<mpsc::SyncSender<(ScopeAddress, Option<RetainedScope>)>>,
    replies: mpsc::Receiver<(ScopeAddress, Result<ScopeValue, String>)>,
    owner: Option<JoinHandle<()>>,
}

impl ScopeReader {
    /// Start one lazy reader for the request's exact graph. Opening the snapshot here keeps its lease on this thread.
    fn start(request: PathBuf, bakery: Option<PathBuf>) -> Result<Self, String> {
        let (requests, queries) = mpsc::sync_channel::<(ScopeAddress, Option<RetainedScope>)>(0);
        // One caller holds the request lock at a time. An unbounded reply channel lets the lease close during unwind.
        let (results, replies) = mpsc::channel();
        let parent = tracing::Span::current();
        let owner = thread::Builder::new()
            .name("incan-executable-scopes".to_owned())
            .spawn(move || {
                let snapshot = parent.in_scope(|| open_snapshot(&request, bakery.as_deref()));
                drop(parent);
                for (address, retained) in queries {
                    let result = snapshot
                        .as_ref()
                        .map_err(Clone::clone)
                        .and_then(|selected| Self::open_scope(selected, &address, retained));
                    if results.send((address, result)).is_err() {
                        break;
                    }
                }
            })
            .map_err(|error| format!("cannot start executable scope reader: {error}"))?;
        Ok(Self {
            requests: Some(requests),
            replies,
            owner: Some(owner),
        })
    }

    /// Bind retained data to the current index selection, opening scope bytes only when their proof is unavailable.
    fn open_scope(
        snapshot: &ExecutableSnapshot,
        address: &ScopeAddress,
        mut retained: Option<RetainedScope>,
    ) -> Result<ScopeValue, String> {
        let prior = retained.as_mut().and_then(|scope| scope.revision.take());
        let selected = internal_models::select_executable_scope(snapshot, &address.module, &address.digest, prior)?;
        let bindings = match (selected.value, retained) {
            (None, Some(retained)) => retained.bindings,
            (Some(view), Some(retained)) => {
                authenticate_retained_scope(view.0.as_bytes(), &address.digest, retained.bindings)?
            }
            (Some(view), None) => {
                let reader = EXECUTABLE_SCOPES.as_ref().map_err(Clone::clone)?;
                open_mapped_graph_executable_scope(reader, &view, &address.module, &address.digest)?
            }
            (None, None) => return Err("scope selection omitted its opened value".into()),
        };
        Ok(ScopeValue {
            revision: selected.revision,
            bindings,
        })
    }

    /// Return the address with its checked binding so the cache owns the inputs already transferred to the reader.
    fn read(
        &self,
        module: &str,
        digest: &str,
        retained: Option<RetainedScope>,
    ) -> Result<(ScopeAddress, ScopeValue), String> {
        let address = ScopeAddress {
            module: module.to_owned(),
            digest: digest.to_owned(),
        };
        self.requests
            .as_ref()
            .ok_or("the executable scope reader is closed")?
            .send((address, retained))
            .map_err(|_| "the executable scope reader stopped")?;
        let (address, scope) = self.replies.recv().map_err(|_| "the executable scope reader stopped")?;
        Ok((address, scope?))
    }
}

impl Drop for ScopeReader {
    /// Disconnect the request channel and release the lease on its owning thread before leaving the request.
    fn drop(&mut self) {
        drop(self.requests.take());
        if let Some(owner) = self.owner.take() {
            let _ = owner.join();
        }
    }
}

impl std::fmt::Debug for GraphScopeAccess {
    /// Describe the authority without opening the snapshot or formatting its cached bindings.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GraphScopeAccess")
            .field("request", &self.request)
            .finish_non_exhaustive()
    }
}

impl GraphScopeAccess {
    /// Admit the typed executable transport selected by Oven before consulting any resident record.
    pub(super) fn requested(request: &Path) -> Result<bool, String> {
        const FORMAT: &[u8] = b"incan.graph.checked-executable/1";
        let path = request.join("executable-graph-format");
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(format!(
                    "cannot open executable graph format {}: {error}",
                    path.display()
                ));
            }
        };
        let mut bytes = Vec::new();
        file.take(FORMAT.len() as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes != FORMAT {
            return Err("unsupported selected executable graph format".into());
        }
        Ok(true)
    }

    /// Bind the request's graph anchor lazily. A metadata-only route never opens a scope snapshot.
    pub(super) fn new(request: &Path, bakery: Option<&Path>) -> Self {
        Self {
            request: request.to_owned(),
            bakery: bakery.map(Path::to_owned),
            ..Self::default()
        }
    }

    /// Share one checked scope per module and complete digest for the lifetime of this request.
    pub(super) fn resolve(&self, source: &str, digest: &str) -> Result<SharedIdentities, String> {
        self.select(source, digest, None).map(|scope| scope.bindings)
    }

    /// Keep the scope's checked bindings and exact selected frame for subsequent resident namespace admissions.
    pub(super) fn resolve_witness(&self, source: &str, digest: &str) -> Result<ExecutableScopeWitness, String> {
        let scope = self.select(source, digest, None)?;
        Ok(ExecutableScopeWitness {
            digest: digest.to_owned(),
            revision: Some(scope.revision),
            bindings: scope.bindings,
        })
    }

    /// Authenticate resident bindings under this request's snapshot and share them with any other selected owner.
    pub(super) fn reuse(&self, source: &str, witness: &mut ExecutableScopeWitness) -> Result<(), String> {
        let retained = RetainedScope {
            revision: witness.revision.take(),
            bindings: Arc::clone(&witness.bindings),
        };
        let scope = self.select(source, &witness.digest, Some(retained))?;
        witness.revision = Some(scope.revision);
        Ok(())
    }

    /// Open and authenticate an immutable scope only once per request. A resident value already has its typed bindings.
    fn select(&self, source: &str, digest: &str, retained: Option<RetainedScope>) -> Result<ScopeValue, String> {
        let mut state = self.state.lock().map_err(|_| "executable scope cache is poisoned")?;
        if let Some(value) = state.decoded.get(source).and_then(|scopes| scopes.get(digest)) {
            return Ok(value.clone());
        }
        let _restore = tracing::trace_span!(
            target: "incan::invocation", "incan_rustc_driver::restore_executable_scope",
            module = source, digest, resident = retained.is_some(),
        )
        .entered();
        let reader = state
            .reader
            .get_or_insert_with(|| ScopeReader::start(self.request.clone(), self.bakery.clone()));
        let (address, scope) = reader.as_ref().map_err(Clone::clone)?.read(source, digest, retained)?;
        state
            .decoded
            .entry(address.module)
            .or_default()
            .insert(address.digest, scope.clone());
        Ok(scope)
    }
}

/// Verify exactly the bytes that originally produced these bindings, after selecting their key in the pinned graph.
fn authenticate_retained_scope(
    encoded: &[u8],
    digest: &str,
    bindings: SharedIdentities,
) -> Result<SharedIdentities, String> {
    let _verify = tracing::trace_span!(
        target: "incan::invocation", "incan_rustc_driver::authenticate_resident_executable_scope", bytes = encoded.len(),
    )
    .entered();
    if hex::encode(Sha256::digest(encoded)) != digest {
        return Err("checked executable scope disagrees with its selected digest".into());
    }
    Ok(bindings)
}

/// Open the manifest and lease once on the scope reader thread, using the request's authenticated view.
fn open_snapshot(request: &Path, bakery: Option<&Path>) -> Result<ExecutableSnapshot, String> {
    let _open = tracing::trace_span!(
        target: "incan::invocation", "incan_rustc_driver::open_executable_graph_snapshot",
    )
    .entered();
    const MAX_DESCRIPTOR: u64 = 65_536;
    let bakery = bakery
        .and_then(Path::to_str)
        .ok_or("executable scopes require a UTF-8 Bakery anchor")?;
    let path = request.join("reopen-snapshot.crumb");
    let input = std::fs::File::open(&path)
        .map_err(|error| format!("cannot open executable scope snapshot {}: {error}", path.display()))?;
    if !input.metadata().map_err(|error| error.to_string())?.is_file() {
        return Err("executable scope snapshot is not a regular file".into());
    }
    let mut encoded = Vec::new();
    input
        .take(MAX_DESCRIPTOR + 1)
        .read_to_end(&mut encoded)
        .map_err(|error| error.to_string())?;
    if encoded.len() as u64 > MAX_DESCRIPTOR {
        return Err("executable scope snapshot exceeds its byte limit".into());
    }
    internal_models::reopen_executable_snapshot(&bakery.to_owned(), &encoded)
}

#[cfg(test)]
mod tests {
    use super::authenticate_retained_scope;
    use incan_semantics_core::body_ir::SharedIdentities;
    use sha2::{Digest, Sha256};
    use std::sync::Arc;

    /// The resident route keeps the existing bindings only when the selected bytes match their record digest.
    #[test]
    fn resident_scope_authentication_reuses_the_original_map() -> Result<(), String> {
        let bytes = b"exact admitted scope";
        let digest = hex::encode(Sha256::digest(bytes));
        let bindings = SharedIdentities::default();
        let restored = authenticate_retained_scope(bytes, &digest, Arc::clone(&bindings))?;
        assert!(Arc::ptr_eq(&restored, &bindings));
        assert!(authenticate_retained_scope(b"replaced scope frame", &digest, bindings).is_err());
        Ok(())
    }
}

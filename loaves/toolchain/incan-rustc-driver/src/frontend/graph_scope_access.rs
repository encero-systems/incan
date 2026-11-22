//! Open only the nominal scopes selected by executable restoration in one pinned request.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

use incan_frontend::typechecker::executable_frames::open_mapped_graph_executable_scope;
use incan_semantics_core::body_ir::SharedIdentities;
use internal_models::ExecutableSnapshot;

/// Freeze the authored scope dictionary for all mapped reads in this compiler process.
static EXECUTABLE_SCOPES: LazyLock<Result<internal_models_native::PreparedWireReader, String>> =
    LazyLock::new(internal_models::prepare_executable_scope_reader);

/// Request-owned scope authority and the immutable checked bindings already restored under it.
#[derive(Default)]
pub(super) struct GraphScopeAccess {
    request: PathBuf,
    bakery: Option<PathBuf>,
    snapshot: OnceLock<Result<ExecutableSnapshot, String>>,
    decoded: Mutex<HashMap<String, HashMap<String, SharedIdentities>>>,
}

impl std::fmt::Debug for GraphScopeAccess {
    /// Describe the authority without opening it or formatting the cached maps.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GraphScopeAccess")
            .field("request", &self.request)
            .field("snapshot_open", &self.snapshot.get().is_some())
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

    /// Open the exact generation named in the request, retaining its owned store lease.
    fn snapshot(&self) -> Result<&ExecutableSnapshot, String> {
        self.snapshot
            .get_or_init(|| {
                const MAX_DESCRIPTOR: u64 = 65_536;
                let bakery = self
                    .bakery
                    .as_deref()
                    .and_then(Path::to_str)
                    .ok_or("executable scopes require a UTF-8 Bakery anchor")?;
                let path = self.request.join("reopen-snapshot.crumb");
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
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// Share one restored scope per module and complete digest for the lifetime of this request.
    pub(super) fn resolve(&self, source: &str, digest: &str) -> Result<SharedIdentities, String> {
        let mut decoded = self.decoded.lock().map_err(|_| "executable scope cache is poisoned")?;
        if let Some(value) = decoded.get(source).and_then(|scopes| scopes.get(digest)) {
            return Ok(Arc::clone(value));
        }
        let view =
            internal_models::selected_executable_scope(self.snapshot()?, &source.to_owned(), &digest.to_owned())?;
        let reader = EXECUTABLE_SCOPES.as_ref().map_err(Clone::clone)?;
        let scope = open_mapped_graph_executable_scope(reader, &view, source, digest)?;
        decoded
            .entry(source.to_owned())
            .or_default()
            .insert(digest.to_owned(), Arc::clone(&scope));
        Ok(scope)
    }
}

//! Explicit declared producer inputs, separate from physical consumer hints (#1337, #1698).
//!
//! This is generated resolver input, not metadata authority. Only the ordinary complete producer can grant a
//! request observation after compiling and checking this entire document. The original broader candidate world
//! remains observed until its caller finishes publication; ordinary demand coverage is still checked separately.

use super::{Current, NativeLoafConsumerReport, NativeLoafConsumerRequest, projection};
use crate::native_loaf::request_observation::RequestFile;
use crate::native_loaf::{Result, failed, refused};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Original selection and persisted complete request inputs retained through explicit toolchain preparation.
pub struct NativeLoafProducerProjection {
    original: Current,
    sources: String,
    graph: RequestFile,
    graph_path: PathBuf,
    lock: RequestFile,
}

impl NativeLoafProducerProjection {
    /// Borrow the generated normal producer document; this path grants no checked metadata or native authority.
    pub fn graph(&self) -> &Path {
        &self.graph_path
    }

    /// Reapply canonical selection and require original inputs and generated documents to remain unchanged.
    /// A changed or changed-then-restored observed source refuses. Platforms without retained observations still
    /// reproject actual source digests, declarations, graph, lock and compiler identity through the existing reader.
    pub fn verify(&mut self, request: &NativeLoafConsumerRequest<'_>) -> Result<()> {
        if !self.original.documents_current() {
            return Err(refused(
                "original native projection documents changed during preparation",
            ));
        }
        self.original.finish_index()?;
        let mut current = Current::read(request, &mut NativeLoafConsumerReport::default())?;
        let (lock, facets, sources) = projection::selected_inputs(request, &mut current)?;
        let graph = graph_bytes(&current, &facets, self.lock.owner()?.join("lock.json"))?;
        if current.key != self.original.key
            || sources != self.sources
            || lock != self.lock.bytes()
            || graph != self.graph.bytes()
        {
            return Err(refused("original native projection inputs changed during preparation"));
        }
        current.finish_index()?;
        if !current.documents_current()
            || RequestFile::read(self.graph())? != self.graph
            || RequestFile::read(&self.lock.owner()?.join("lock.json"))? != self.lock
        {
            return Err(refused("persisted native projection changed during preparation"));
        }
        Ok(())
    }
}

/// Persist a declared forward closure as a new explicit complete producer request, using the canonical selector.
///
/// The output graph is an explicit mutable coordinate; its digest-named lock is never overwritten. Local facets
/// retain original absolute paths and index revision/codegen selections. This does not compile, bless physical
/// hints, or change the complete observation rules of either producer or consumer. Callers must retain and verify
/// the returned guard around preparation and publication. Later consumers may select this normal graph explicitly
/// or use another genuine complete request that covers their own declared native demands.
pub fn project_declared_native_loaf_request(
    request: &NativeLoafConsumerRequest<'_>,
    graph: &Path,
) -> Result<NativeLoafProducerProjection> {
    let requested = std::path::absolute(graph).map_err(failed)?;
    let parent = requested
        .parent()
        .ok_or_else(|| refused("native projection graph has no parent"))?;
    std::fs::create_dir_all(parent).map_err(failed)?;
    let parent = parent.canonicalize().map_err(failed)?;
    let graph = parent.join(
        requested
            .file_name()
            .ok_or_else(|| refused("native projection graph has no filename"))?,
    );
    if graph == std::path::absolute(request.graph).map_err(failed)? {
        return Err(refused("native projection cannot replace its original graph"));
    }
    let mut original = Current::read(request, &mut NativeLoafConsumerReport::default())?;
    if graph
        .canonicalize()
        .ok()
        .is_some_and(|path| path == original.graph_coordinate || path == original.lock_coordinate)
    {
        return Err(refused(
            "native projection cannot replace original resolution documents",
        ));
    }
    let (lock_bytes, facets, sources) = projection::selected_inputs(request, &mut original)?;
    original.finish_index()?;
    if !original.documents_current() {
        return Err(refused("native projection inputs changed during selection"));
    }
    let lock_root = parent
        .join("native-requests")
        .join(oven_store::digest_bytes(&lock_bytes).replace(':', "-"));
    std::fs::create_dir_all(&lock_root).map_err(failed)?;
    let lock = lock_root.join("lock.json");
    write_document(&lock, &lock_bytes, false)?;
    let graph_bytes = graph_bytes(&original, &facets, lock.clone())?;
    write_document(&graph, &graph_bytes, true)?;
    let mut result = NativeLoafProducerProjection {
        original,
        sources,
        graph: RequestFile::read(&graph)?,
        graph_path: graph,
        lock: RequestFile::read(&lock)?,
    };
    result.verify(request)?;
    Ok(result)
}

/// Preserve exact facet fields while rebasing their coordinates to the original resolved document's owner.
fn graph_bytes(current: &Current, facets: &[crate::native_loaf::NativeLoafFacet], lock: PathBuf) -> Result<Vec<u8>> {
    let facets = facets
        .iter()
        .map(|facet| {
            let mut selected = facet.clone();
            selected.project = current.owner.join(&facet.project);
            selected
        })
        .collect::<Vec<_>>();
    serde_json::to_vec(
        &serde_json::json!({"index_commit":current.graph.index_commit,"registry_lock":lock,"facets":facets}),
    )
    .map_err(failed)
}

/// Publish generated inputs atomically; preserve identical bytes and refuse corrupted digest-addressed locks.
fn write_document(path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
    match std::fs::read(path) {
        Ok(existing) if existing == bytes => return Ok(()),
        Ok(_) if !replace => return Err(refused("persisted native projection lock differs from its identity")),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(failed(error)),
    }
    let parent = path
        .parent()
        .ok_or_else(|| refused("native projection document has no parent"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(failed)?;
    temporary.write_all(bytes).map_err(failed)?;
    if replace {
        temporary.persist(path).map_err(failed)?;
    } else if let Err(error) = temporary.persist_noclobber(path)
        && std::fs::read(path).map_err(failed)? != bytes
    {
        return Err(failed(error));
    }
    Ok(())
}

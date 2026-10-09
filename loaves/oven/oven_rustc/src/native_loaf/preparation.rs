//! Explicit native-only preparation over the existing resolver output and local native producer.
//!
//! No SDK inventory, stdlib component publisher, semantic checker, or alternate resolver participates here.

use std::path::{Path, PathBuf};
use std::time::Instant;

use oven_store::store::{OvenStore, OvenStoreLimits};
use serde::{Deserialize, Serialize};

use super::{NativeLoafError, NativeLoafGraph, Result, failed, refused};
use crate::sdk_closure::{ClosureCompileRequest, LocalFacetSelection, compile_local_sdk_facets, prepare_closure};

/// One explicitly selected local Rust facet; dependency and feature resolution remain the caller's authority.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeLoafFacet {
    /// Loaf project directory relative to the request's facet owner.
    pub project: PathBuf,
    /// Complete selected local features, including expanded aliases.
    pub features: Vec<String>,
    /// Exact selected host or target compilation domain.
    pub domain: String,
}

/// Explicit physical inputs for native-only preparation; no ambient SDK discovery supplies missing authority.
pub struct NativeLoafPreparationRequest<'a> {
    /// Already resolved Loaf lock/projection consumed by the existing native producer.
    pub lock: &'a Path,
    /// Digest-addressed source archive directory.
    pub blobs: &'a Path,
    /// Native staging and ordinary Store parent directory.
    pub output: &'a Path,
    /// Explicit compiler executable.
    pub rustc: &'a Path,
    /// Exact adopted index repository.
    pub index: &'a Path,
    /// Pinned index commit, without a working-tree fallback.
    pub index_commit: &'a str,
    /// Requested target triple.
    pub target: &'a str,
    /// Debug or release profile for registry units.
    pub profile: &'a str,
    /// Owner directory for relative local facet project paths.
    pub facet_owner: &'a Path,
    /// Already selected local facets, compiled in the existing producer's dependency order.
    pub facets: &'a [NativeLoafFacet],
}

/// Actual native producer outcomes, independent of a stdlib inventory or aggregate SDK identity.
#[derive(Debug, Serialize)]
pub struct NativeLoafPreparationReport {
    /// Exact labels compiled by the existing native producer.
    pub compiled: Vec<String>,
    /// Exact labels reused by the existing native producer.
    pub reused: Vec<String>,
    /// Whole native-only preparation and durable per-unit publication time, measured at this boundary.
    pub seconds: f64,
}

/// Retained per-unit physical owners and measured outcomes of one explicit native-only preparation.
pub struct NativeLoafPreparation {
    graph: NativeLoafGraph,
    report: NativeLoafPreparationReport,
}

/// Explicit resolved graph wire format; parsing supplies paths, never a second dependency resolver.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolvedNativeGraph {
    index_commit: String,
    registry_lock: PathBuf,
    facets: Vec<NativeLoafFacet>,
}

/// Prepare an explicit resolved graph with lock and local facets relative to its canonical document owner.
///
/// The index, archives, output, compiler, target and profile are supplied by the caller. This entry point has no
/// SDK discovery, environment-derived authority or standard-library companion publication.
pub fn prepare_resolved_native_loafs(
    graph: &Path,
    index: &Path,
    blobs: &Path,
    output: &Path,
    rustc: &Path,
    target: &str,
    profile: &str,
) -> Result<NativeLoafPreparation> {
    let graph = graph.canonicalize().map_err(failed)?;
    let owner = graph
        .parent()
        .ok_or_else(|| refused("resolved native graph has no owner"))?;
    let selection: ResolvedNativeGraph =
        serde_json::from_slice(&std::fs::read(&graph).map_err(failed)?).map_err(failed)?;
    prepare_native_loafs(&NativeLoafPreparationRequest {
        lock: &owner.join(selection.registry_lock),
        blobs,
        output,
        rustc,
        index,
        index_commit: &selection.index_commit,
        target,
        profile,
        facet_owner: owner,
        facets: &selection.facets,
    })
}

impl NativeLoafPreparation {
    /// Borrow complete producer-authenticated ordinary records and original native leases.
    pub fn graph(&self) -> &NativeLoafGraph {
        &self.graph
    }

    /// Borrow actual compile/reuse outcomes and the measured boundary duration.
    pub fn report(&self) -> &NativeLoafPreparationReport {
        &self.report
    }
}

/// Prepare native registry/local units and durable per-unit records without publishing or checking stdlib components.
///
/// The existing local operational helper currently supports only debug and the compiler's host target. Requests
/// outside that envelope refuse explicitly until the local producer carries those policies, rather than silently
/// using a different target or profile. Cross-Store distribution and semantic/macro completeness remain separate.
pub fn prepare_native_loafs(request: &NativeLoafPreparationRequest<'_>) -> Result<NativeLoafPreparation> {
    let started = Instant::now();
    if !request.facets.is_empty() {
        if request.profile != "debug"
            || crate::rustc::rustc_host_target(request.rustc).map_err(failed)? != request.target
        {
            return Err(refused(
                "local native preparation currently requires debug and the explicit compiler host target",
            ));
        }
    }
    let mut closure = prepare_closure(&ClosureCompileRequest {
        primary: &[],
        lock: request.lock,
        blobs: request.blobs,
        output: request.output,
        rustc: request.rustc,
        index: request.index,
        index_commit: request.index_commit,
        target: request.target,
        profile: request.profile,
    })
    .map_err(NativeLoafError::Failed)?;
    let facets = request
        .facets
        .iter()
        .map(|facet| LocalFacetSelection {
            project: facet.project.clone(),
            features: facet.features.clone(),
            domain: facet.domain.clone(),
        })
        .collect::<Vec<_>>();
    compile_local_sdk_facets(
        &mut closure,
        &facets,
        request.facet_owner,
        request.output,
        request.rustc,
    )
    .map_err(NativeLoafError::Failed)?;
    closure.require_complete().map_err(NativeLoafError::Failed)?;
    let compiled = closure.report().compiled.clone();
    let reused = closure.report().reused.clone();
    let store = OvenStore::new(
        request.output.join("store"),
        OvenStoreLimits::new(4 * 1024 * 1024 * 1024, 4 * 1024 * 1024 * 1024, 4 * 1024 * 1024 * 1024),
    );
    let graph = closure.into_native_loafs(&store).map_err(NativeLoafError::Failed)?;
    Ok(NativeLoafPreparation {
        graph,
        report: NativeLoafPreparationReport {
            compiled,
            reused,
            seconds: started.elapsed().as_secs_f64(),
        },
    })
}

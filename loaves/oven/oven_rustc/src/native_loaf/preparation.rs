//! Explicit native-only preparation over the existing resolver output and local native producer.
//!
//! No SDK inventory, stdlib component publisher, semantic checker, or alternate resolver participates here.

use std::path::{Path, PathBuf};
use std::time::Instant;

use oven_store::store::OvenStore;
use serde::{Deserialize, Serialize};

use super::prepared::native_store;
use super::request_observation::{NativeLoafRequestObservation, ProducerRequest, RequestFile};
use super::{NativeLoafError, NativeLoafGraph, Result, failed, refused};
use crate::sdk_closure::{
    ClosureCompileRequest, LocalFacetSelection, compile_local_native_facets_for_profile, prepare_closure_in_store,
};

/// One explicitly selected local Rust facet; dependency and feature resolution remain the caller's authority.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
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
    /// Debug or release profile for registry and local units, including host procedural macros.
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
    pub(super) graph: NativeLoafGraph,
    pub(super) report: NativeLoafPreparationReport,
    pub(super) observation: Option<NativeLoafRequestObservation>,
}

/// Explicit resolved graph wire format; parsing supplies paths, never a second dependency resolver.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResolvedNativeGraph {
    /// Exact adopted index authority.
    pub(super) index_commit: String,
    /// Owner-relative already resolved registry bindings.
    pub(super) registry_lock: PathBuf,
    /// Explicit local source/feature/domain selections.
    pub(super) facets: Vec<NativeLoafFacet>,
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
    let store = native_store(output);
    prepare_resolved_native_loafs_in_store(graph, index, blobs, output, rustc, target, profile, &store)
}

/// Preserve the original producer request while publishing every native unit and record in one supplied Store.
#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_resolved_native_loafs_in_store(
    graph: &Path,
    index: &Path,
    blobs: &Path,
    output: &Path,
    rustc: &Path,
    target: &str,
    profile: &str,
    store: &OvenStore,
) -> Result<NativeLoafPreparation> {
    let (document, owner, selection) = read_resolved_native_graph_inputs(graph)?;
    prepare_native_loafs_with_document(
        &NativeLoafPreparationRequest {
            lock: &owner.join(selection.registry_lock),
            blobs,
            output,
            rustc,
            index,
            index_commit: &selection.index_commit,
            target,
            profile,
            facet_owner: &owner,
            facets: &selection.facets,
        },
        Some(document),
        store,
    )
}

/// Read one graph snapshot and its exact digest, retaining its canonical relative-path owner.
pub(super) fn read_resolved_native_graph(graph: &Path) -> Result<(PathBuf, ResolvedNativeGraph, String)> {
    let (document, owner, selection) = read_resolved_native_graph_inputs(graph)?;
    Ok((owner, selection, document.digest().to_string()))
}

/// Parse the retained actual graph bytes once, preserving its original relative-path owner for producer observation.
fn read_resolved_native_graph_inputs(graph: &Path) -> Result<(RequestFile, PathBuf, ResolvedNativeGraph)> {
    let document = RequestFile::read(graph)?;
    let owner = document.owner()?.to_path_buf();
    let selection = serde_json::from_slice(document.bytes()).map_err(failed)?;
    Ok((document, owner, selection))
}

impl NativeLoafPreparation {
    /// Retain the complete supplied producer request, including original units outside physical consumer roots.
    ///
    /// This does not establish that the request covers all compiler semantic demands. Synthetic preparations and
    /// rooted consumer hints have no such producer association and refuse rather than manufacturing one.
    pub fn request_observation(&self) -> Result<NativeLoafRequestObservation> {
        let observation = self
            .observation
            .as_ref()
            .ok_or_else(|| refused("complete native producer request observation is unavailable"))?;
        observation.verify()?;
        Ok(observation.clone())
    }

    /// Project the already retained ordinary set, using the same durable source authority as installed admission.
    pub fn inspection_inputs(&self) -> Result<super::NativeLoafInspectionInputs> {
        self.graph.inspection_inputs()
    }

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
/// Registry and local units use the requested debug or release profile. Local facets still require the compiler's
/// host target; unsupported targets refuse rather than silently using another triple. Cross-Store distribution and
/// semantic/macro completeness remain separate.
pub fn prepare_native_loafs(request: &NativeLoafPreparationRequest<'_>) -> Result<NativeLoafPreparation> {
    let store = native_store(request.output);
    prepare_native_loafs_in_store(request, &store)
}

/// Preserve full supplied-request observation while publishing its exact selected units in the caller's Store.
pub(super) fn prepare_native_loafs_in_store(
    request: &NativeLoafPreparationRequest<'_>,
    store: &OvenStore,
) -> Result<NativeLoafPreparation> {
    prepare_native_loafs_with_document(request, None, store)
}

/// Preserve the optional original resolved document while running the same ordinary producer for either entrypoint.
fn prepare_native_loafs_with_document(
    request: &NativeLoafPreparationRequest<'_>,
    document: Option<RequestFile>,
    store: &OvenStore,
) -> Result<NativeLoafPreparation> {
    let started = Instant::now();
    if !request.facets.is_empty() {
        if crate::rustc::rustc_host_target(request.rustc).map_err(failed)? != request.target {
            return Err(refused(
                "local native preparation currently requires the explicit compiler host target",
            ));
        }
    }
    let actual_request = ProducerRequest::capture(request, document)?;
    let mut closure = prepare_closure_in_store(
        &ClosureCompileRequest {
            primary: &[],
            lock: request.lock,
            blobs: request.blobs,
            output: request.output,
            rustc: request.rustc,
            index: request.index,
            index_commit: request.index_commit,
            target: request.target,
            profile: request.profile,
        },
        store,
    )
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
    compile_local_native_facets_for_profile(
        &mut closure,
        &facets,
        request.facet_owner,
        request.output,
        request.rustc,
        request.profile,
        store,
    )
    .map_err(NativeLoafError::Failed)?;
    closure.require_complete().map_err(NativeLoafError::Failed)?;
    let compiled = closure.report().compiled.clone();
    let reused = closure.report().reused.clone();
    let graph = closure.into_native_loafs(store).map_err(NativeLoafError::Failed)?;
    let observation = NativeLoafRequestObservation::from_producer(actual_request, &graph)?;
    Ok(NativeLoafPreparation {
        graph,
        observation: Some(observation),
        report: NativeLoafPreparationReport {
            compiled,
            reused,
            seconds: started.elapsed().as_secs_f64(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::{NativeLoafFacet, NativeLoafPreparationRequest, prepare_native_loafs};
    use std::collections::BTreeSet;
    use std::process::Command;

    /// Real local host macros and target libraries compile under each profile, then reuse their exact receipts.
    #[test]
    fn dev7_native_local_profiles_publish_and_reuse_actual_optimization() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let output = root.path().join("native");
        let lock = root.path().join("lock.json");
        std::fs::write(&lock, r#"{"schema":"incan.oven.loaf-resolution/2","units":[]}"#)?;
        for (name, kind, dependencies, source) in [
            (
                "consumer",
                "lib",
                "[dependencies]\nleaf={loaf='leaf',path='../leaf'}\nmarker={loaf='marker',path='../marker'}\n",
                "pub fn value() -> u8 { leaf::value() + marker::profile!() }",
            ),
            (
                "leaf",
                "lib",
                "",
                "pub fn value() -> u8 { if cfg!(debug_assertions) { 1 } else { 2 } }",
            ),
            (
                "marker",
                "proc-macro",
                "",
                "use proc_macro::TokenStream; #[proc_macro] pub fn profile(_: TokenStream) -> TokenStream { match (if cfg!(debug_assertions) { \"11\" } else { \"23\" }).parse() { Ok(tokens) => tokens, Err(_) => TokenStream::new() } }",
            ),
        ] {
            let project = root.path().join(name);
            std::fs::create_dir_all(project.join("src"))?;
            std::fs::write(
                project.join("loaf.toml"),
                format!(
                    "[project]\nname='{name}'\nversion='1.0.0'\n[rust]\nname='{name}'\ntype='{kind}'\nedition='2024'\n{dependencies}"
                ),
            )?;
            std::fs::write(project.join("src/lib.rs"), source)?;
        }
        let facets = ["consumer", "leaf", "marker"].map(|name| NativeLoafFacet {
            project: name.into(),
            features: Vec::new(),
            domain: if name == "marker" {
                "host".into()
            } else {
                "target".into()
            },
        });
        let rustc = crate::rustc::resolve_active_rustc()?;
        let target = crate::rustc::rustc_host_target(&rustc)?;
        let mut first_identities = Vec::new();
        for (profile, expected) in [("debug", 12), ("release", 25)] {
            let request = NativeLoafPreparationRequest {
                lock: &lock,
                blobs: root.path(),
                output: &output,
                rustc: &rustc,
                index: root.path(),
                index_commit: "0000000000000000000000000000000000000000",
                target: &target,
                profile,
                facet_owner: root.path(),
                facets: &facets,
            };
            let unsupported_target = NativeLoafPreparationRequest {
                target: "unsupported-local-target",
                ..request
            };
            let error = prepare_native_loafs(&unsupported_target)
                .err()
                .ok_or("unsupported local target was accepted")?;
            assert!(error.to_string().contains("explicit compiler host target"), "{error}");
            let first = prepare_native_loafs(&request)?;
            assert_eq!(first.report().compiled.len(), 3, "{profile}: {:?}", first.report());
            assert!(first.report().reused.is_empty());
            let consumer = first
                .graph()
                .units()
                .values()
                .find(|unit| unit.record().source.loaf == "consumer")
                .ok_or("consumer record missing")?;
            let original = consumer.record().native.clone();
            let identities = first.graph().units().keys().cloned().collect::<BTreeSet<_>>();
            for unit in first.graph().units().values() {
                assert_eq!(unit.record().recipe.intent.profile, profile);
                assert_eq!(unit.record().recipe.intent.target, target);
                assert_eq!(
                    unit.record().source.domain,
                    if unit.record().source.loaf == "marker" {
                        "host"
                    } else {
                        "target"
                    }
                );
                unit.verify()?;
            }
            let source = root.path().join(format!("{profile}-caller.rs"));
            std::fs::write(
                &source,
                format!("fn main() {{ assert_eq!(consumer::value(), {expected}); }}"),
            )?;
            let executable = root
                .path()
                .join(format!("{profile}-caller{}", std::env::consts::EXE_SUFFIX));
            let mut command = Command::new(&rustc);
            command
                .arg(&source)
                .args(["--crate-name", "profile_caller", "--edition", "2024"])
                .arg("--extern")
                .arg(format!("consumer={}", consumer.output()?.display()))
                .arg("-o")
                .arg(&executable);
            let searches = first
                .graph()
                .units()
                .values()
                .map(|unit| {
                    let path = unit.output()?;
                    Ok(path.parent().ok_or("native output parent missing")?.to_path_buf())
                })
                .collect::<Result<BTreeSet<_>, Box<dyn std::error::Error>>>()?;
            for search in searches {
                command.arg("-L").arg(format!("dependency={}", search.display()));
            }
            let compiled = command.output()?;
            assert!(
                compiled.status.success(),
                "{profile}: {}",
                String::from_utf8_lossy(&compiled.stderr)
            );
            let ran = Command::new(&executable).output()?;
            assert!(
                ran.status.success(),
                "{profile}: {}",
                String::from_utf8_lossy(&ran.stderr)
            );
            let repeated = prepare_native_loafs(&request)?;
            assert!(
                repeated.report().compiled.is_empty(),
                "{profile}: {:?}",
                repeated.report()
            );
            assert_eq!(repeated.report().reused.len(), 3);
            assert_eq!(
                identities,
                repeated.graph().units().keys().cloned().collect::<BTreeSet<_>>()
            );
            let repeated_consumer = repeated
                .graph()
                .units()
                .values()
                .find(|unit| unit.record().source.loaf == "consumer")
                .ok_or("repeated consumer missing")?;
            assert_eq!(original, repeated_consumer.record().native);
            first_identities.push((identities, original));
        }
        assert!(first_identities[0].0.is_disjoint(&first_identities[1].0));
        assert_ne!(
            first_identities[0].1.receipt_identity,
            first_identities[1].1.receipt_identity
        );
        assert_ne!(first_identities[0].1.digest, first_identities[1].1.digest);
        Ok(())
    }
}

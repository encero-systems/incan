//! Conservative observation of one complete explicit native producer request (#1337, #1698).
//!
//! This capability comes only from a completed producer. It retains every original record/native owner, including
//! units outside a consumer's physical roots. It proves this supplied request is current; the compiler must separately
//! establish that the supplied request covers all checked semantic demands. It grants no macros or namespaces.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use super::preparation::{NativeLoafFacet, NativeLoafPreparationRequest};
use super::selection::{LocalSources, source_manifest};
use super::{NativeLoafGraph, NativeLoafOrigin, Result, failed, refused, verify_child_bindings};
use crate::sdk_closure::current_inputs;

/// An original explicit document and its observed coordinate; mutable source files need not become immutable.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub(super) struct RequestFile {
    path: PathBuf,
    canonical: PathBuf,
    digest: String,
    #[serde(skip)]
    bytes: Vec<u8>,
}

impl RequestFile {
    /// Read exact actual input bytes while preserving its requested and canonical coordinates.
    pub(super) fn read(path: &Path) -> Result<Self> {
        let path = std::path::absolute(path).map_err(failed)?;
        let canonical = path.canonicalize().map_err(failed)?;
        if !std::fs::metadata(&canonical).map_err(failed)?.is_file() {
            return Err(refused("native producer request document is not a regular file"));
        }
        let bytes = std::fs::read(&canonical).map_err(failed)?;
        if path.canonicalize().map_err(failed)? != canonical {
            return Err(refused(
                "native producer request document changed coordinate during observation",
            ));
        }
        Ok(Self {
            path,
            canonical,
            digest: oven_store::digest_bytes(&bytes),
            bytes,
        })
    }

    /// Original canonical owner for resolving the existing graph document's relative paths.
    pub(super) fn owner(&self) -> Result<&Path> {
        self.canonical
            .parent()
            .ok_or_else(|| refused("native producer request document has no owner"))
    }

    /// Borrow the exact retained producer input, rather than rereading an unbound document for parsing.
    pub(super) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Borrow the observed input identity for existing coordinate-hint callers.
    pub(super) fn digest(&self) -> &str {
        &self.digest
    }
}

/// Current actual compiler generation; metadata probe results are reused only under equal executable bytes.
#[derive(Clone, PartialEq, Eq, Serialize)]
struct Compiler {
    executable: String,
    standard_library: String,
    identity: String,
    host: String,
    commit: String,
}

/// Actual mapped local source generation, using the original producer's source and feature projector.
#[derive(Clone, PartialEq, Serialize)]
struct Local {
    path: PathBuf,
    canonical: PathBuf,
    domain: String,
    features: Vec<String>,
    source: String,
    manifest: toml::Value,
}

/// Private complete request snapshot. Public data, arbitrary records and generic Engine owners cannot construct it.
#[derive(Clone, PartialEq, Serialize)]
pub(super) struct ProducerRequest {
    lock: RequestFile,
    document: Option<RequestFile>,
    index: PathBuf,
    index_coordinate: Option<PathBuf>,
    blobs: PathBuf,
    output: PathBuf,
    rustc: PathBuf,
    compiler_coordinate: PathBuf,
    compiler: Compiler,
    index_commit: String,
    target: String,
    profile: String,
    facet_owner: PathBuf,
    facets: Vec<NativeLoafFacet>,
    local: Vec<Local>,
    archives: BTreeMap<PathBuf, (PathBuf, String)>,
    native_environment: BTreeMap<String, Option<String>>,
}

/// Opaque complete actual producer-request observation with its original full per-unit owner set.
///
/// A rooted consumer hint cannot construct this. The digest is deliberately conservative and includes all supplied
/// units/facts/edges/compiler inputs. It does not prove the compiler supplied every source or macro demand needed by
/// a checked program, authorize macro execution, or issue namespace ownership. Revalidate at every metadata handoff.
#[derive(Clone)]
pub struct NativeLoafRequestObservation {
    inner: Arc<Observation>,
}

/// Private association between a producer-captured request and its actual completed original graph.
struct Observation {
    request: ProducerRequest,
    graph: NativeLoafGraph,
    digest: String,
}

impl ProducerRequest {
    /// Capture explicit actual producer inputs before native preparation, without discovering ambient source trees.
    pub(super) fn capture(request: &NativeLoafPreparationRequest<'_>, document: Option<RequestFile>) -> Result<Self> {
        Self::observe(request, document, None)
    }

    /// Observe current files/compiler/source mapping through existing authority projectors, never a new resolver.
    fn observe(
        request: &NativeLoafPreparationRequest<'_>,
        document: Option<RequestFile>,
        previous: Option<&Compiler>,
    ) -> Result<Self> {
        let lock = RequestFile::read(request.lock)?;
        let seed = current_inputs::read_resolution_bytes(lock.bytes()).map_err(super::NativeLoafError::Failed)?;
        let mut archives = BTreeMap::new();
        for binding in &seed.units {
            let hex = binding
                .archive_digest
                .strip_prefix("sha256:")
                .filter(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
                .ok_or_else(|| refused("complete native request has an invalid archive identity"))?;
            let path = std::path::absolute(request.blobs)
                .map_err(failed)?
                .join(format!("{hex}.tar"));
            if !archives.contains_key(&path) {
                let canonical = path.canonicalize().map_err(failed)?;
                let digest = oven_store::store::digest_regular_file(&canonical).map_err(failed)?.1;
                if digest != binding.archive_digest {
                    return Err(refused("complete native request archive changed"));
                }
                archives.insert(path, (canonical, digest));
            }
        }
        let rustc = std::path::absolute(request.rustc).map_err(failed)?;
        let compiler_coordinate = rustc.canonicalize().map_err(failed)?;
        let executable = oven_store::store::digest_regular_file(&compiler_coordinate)
            .map_err(failed)?
            .1;
        let compiler = Compiler {
            standard_library: crate::sdk_closure::compiler_closure_digest(&compiler_coordinate, request.target)
                .map_err(super::NativeLoafError::Failed)?,
            identity: match previous.filter(|old| old.executable == executable) {
                Some(old) => old.identity.clone(),
                None => crate::rustc::rustc_identity(&compiler_coordinate).map_err(failed)?,
            },
            host: match previous.filter(|old| old.executable == executable) {
                Some(old) => old.host.clone(),
                None => crate::rustc::rustc_host_target(&compiler_coordinate).map_err(failed)?,
            },
            commit: match previous.filter(|old| old.executable == executable) {
                Some(old) => old.commit.clone(),
                None => crate::rustc::rustc_commit_hash(&compiler_coordinate)
                    .ok_or_else(|| refused("native producer compiler has no commit hash"))?,
            },
            executable,
        };
        let facet_owner = std::path::absolute(request.facet_owner).map_err(failed)?;
        let mut sources = LocalSources::default();
        let mut local = Vec::new();
        for facet in request.facets {
            let path = facet_owner.join(&facet.project);
            let canonical = path.canonicalize().map_err(failed)?;
            let (manifest, source) = sources.get(&path)?;
            let features = crate::sdk_closure::native_required_features(manifest, &facet.features, false)
                .map_err(super::NativeLoafError::Failed)?;
            local.push(Local {
                path,
                canonical,
                domain: facet.domain.clone(),
                features: features.into_iter().collect(),
                source: source.clone(),
                manifest: manifest.clone(),
            });
        }
        let native_environment = [
            "INCAN_OVEN_LINK_OWNERS",
            "INCAN_OVEN_LINK_SDK",
            "INCAN_OVEN_LINK_SYSROOT",
        ]
        .into_iter()
        .map(|name| {
            let value = std::env::var_os(name)
                .map(|value| {
                    value
                        .into_string()
                        .map_err(|_| refused("native producer tool selection is not UTF-8"))
                })
                .transpose()?;
            Ok((name.to_string(), value))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
        Ok(Self {
            lock,
            document,
            index: std::path::absolute(request.index).map_err(failed)?,
            index_coordinate: if seed.units.is_empty() {
                None
            } else {
                Some(request.index.canonicalize().map_err(failed)?)
            },
            blobs: std::path::absolute(request.blobs).map_err(failed)?,
            output: std::path::absolute(request.output).map_err(failed)?,
            rustc,
            compiler_coordinate,
            compiler,
            index_commit: request.index_commit.to_string(),
            target: request.target.to_string(),
            profile: request.profile.to_string(),
            facet_owner,
            facets: request.facets.to_vec(),
            local,
            archives,
            native_environment,
        })
    }

    /// Reobserve every supplied source/lock/graph/compiler input, without turning mutable source into a Store owner.
    fn verify(&self) -> Result<()> {
        let document = self
            .document
            .as_ref()
            .map(|file| RequestFile::read(&file.path))
            .transpose()?;
        let current = Self::observe(
            &NativeLoafPreparationRequest {
                lock: &self.lock.path,
                blobs: &self.blobs,
                output: &self.output,
                rustc: &self.rustc,
                index: &self.index,
                index_commit: &self.index_commit,
                target: &self.target,
                profile: &self.profile,
                facet_owner: &self.facet_owner,
                facets: &self.facets,
            },
            document,
            Some(&self.compiler),
        )?;
        if &current != self {
            return Err(refused(
                "complete native producer request inputs changed after preparation",
            ));
        }
        Ok(())
    }
}

impl NativeLoafRequestObservation {
    /// Bind only the original completed producer graph to the request captured before its preparation began.
    pub(super) fn from_producer(request: ProducerRequest, graph: &NativeLoafGraph) -> Result<Self> {
        let graph = NativeLoafGraph {
            units: graph
                .units
                .iter()
                .map(|(key, unit)| (key.clone(), Arc::clone(unit)))
                .collect(),
        };
        let records = graph
            .units
            .iter()
            .map(|(identity, unit)| {
                (
                    identity,
                    (
                        unit.record(),
                        &unit.record_owner.artifact_root,
                        &unit.native_owner.artifact_root,
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let digest = oven_store::digest_bytes(
            &serde_json::to_vec(&("incan.oven.native-producer-request/1", &request, records)).map_err(failed)?,
        );
        let observation = Self {
            inner: Arc::new(Observation { request, graph, digest }),
        };
        // Physical preparation captures source consistency and original owners without granting semantic replay.
        // Unsupported semantic shapes must refuse at observation handoff, not disable their native producer.
        observation.inner.request.verify()?;
        let manifests = verify_original_records(&observation.inner.graph)?;
        verify_request_bindings(&observation.inner.request, &observation.inner.graph, &manifests)?;
        Ok(observation)
    }

    /// Borrow complete actual producer records and original leases; consumers must verify at each metadata handoff.
    pub fn graph(&self) -> &NativeLoafGraph {
        &self.inner.graph
    }

    /// Revalidate the complete supplied request and every original owner before returning its deterministic digest.
    ///
    /// This never reacquires Store owners, prunes semantic inputs to physical roots or constructs macro grants.
    pub fn verified_digest(&self) -> Result<&str> {
        self.verify()?;
        Ok(&self.inner.digest)
    }

    /// Verify actual compiler/request intent and return the complete digest after one full handoff verification.
    ///
    /// Features belong to individual selected units, so this validates target/profile/toolchain without inventing
    /// an aggregate feature set. Exact canonical executable coordinates and bytes must match the original producer.
    pub fn verify_intent(&self, rustc: &Path, intent: &oven_store::OvenBuildIntent) -> Result<&str> {
        self.verify()?;
        let request = &self.inner.request;
        if rustc.canonicalize().map_err(failed)? != request.compiler_coordinate
            || intent.target != request.target
            || intent.profile != request.profile
            || intent.toolchain != request.compiler.identity
        {
            return Err(refused(
                "complete native producer request differs from the requested compiler intent",
            ));
        }
        Ok(&self.inner.digest)
    }

    /// Verify current explicit inputs, complete original records and current native facts without Store re-admission.
    pub fn verify(&self) -> Result<()> {
        let request = &self.inner.request;
        request.verify()?;
        verify_records(request, &self.inner.graph)
    }
}

/// Reuse the producer's current fact/environment/tool projectors over every originally produced unit.
fn verify_records(request: &ProducerRequest, graph: &NativeLoafGraph) -> Result<()> {
    let seed = current_inputs::read_resolution_bytes(request.lock.bytes()).map_err(super::NativeLoafError::Failed)?;
    let mut index = if seed.units.is_empty() {
        None
    } else {
        Some(
            current_inputs::PinnedIndexBatch::open(&request.index, &request.index_commit)
                .map_err(super::NativeLoafError::Failed)?,
        )
    };
    let about = match &mut index {
        Some(index) => current_inputs::selected_about(&seed.units, index).map_err(super::NativeLoafError::Failed)?,
        None => BTreeMap::new(),
    };
    let mut tools = current_inputs::NativeToolOwners::default();
    let mut link = None;
    let manifests = verify_original_records(graph)?;
    for (identity, unit) in &graph.units {
        let manifest = manifests
            .get(identity)
            .ok_or_else(|| refused("native source declaration is missing"))?;
        let inputs = &unit.record.recipe.sources.build_unit_inputs;
        if unit.record.source.domain == "host" && request.compiler.host != request.target {
            return Err(refused(
                "complete native observation requires independently bound host standard-library authority",
            ));
        }
        if unit.record.recipe.intent.target != request.target
            || unit.record.recipe.intent.profile != request.profile
            || unit.record.recipe.intent.toolchain != request.compiler.identity
            || inputs.get("native-compiler-executable") != Some(&request.compiler.executable)
            || inputs.get("compiler-binary") != Some(&request.compiler.standard_library)
            || inputs.get("compiler-host") != Some(&request.compiler.host)
            || inputs.get("compiler-commit") != Some(&request.compiler.commit)
            || inputs.get("sdk-compile-policy").map(String::as_str) != Some("source-sealed-portable-v2")
        {
            return Err(refused(
                "complete native producer record differs from its compiler/profile authority",
            ));
        }
        if inputs.contains_key("link-closure") {
            if link.is_none() {
                link = Some(
                    crate::rustc::linking::pinned_link_closure_identity(&request.rustc, &request.target)
                        .map_err(failed)?,
                );
            }
            if link.as_ref().and_then(Option::as_ref) != inputs.get("link-closure") {
                return Err(refused("complete native producer linker closure changed"));
            }
        }
        let origin = super::source_origin(&unit.record.recipe)?;
        let metadata = about
            .get(&unit.record.source.archive_digest)
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        if !current_inputs::matches_current_recipe(
            &unit.record,
            manifest,
            origin,
            &seed,
            &metadata,
            &mut tools,
            index.as_mut(),
        )
        .map_err(super::NativeLoafError::Failed)?
        {
            return Err(refused("complete native producer fact or environment changed"));
        }
    }
    verify_request_bindings(request, graph, &manifests)?;
    if let Some(index) = &mut index {
        index.finish().map_err(super::NativeLoafError::Failed)?;
    }
    Ok(())
}

/// Verify retained original owners and their actual declarations, with no selector or semantic-shape restriction.
fn verify_original_records(graph: &NativeLoafGraph) -> Result<BTreeMap<String, toml::Value>> {
    graph
        .units
        .iter()
        .map(|(identity, unit)| {
            super::verify_record(&unit.record, &unit.record_owner, &unit.native_owner, true)?;
            verify_child_bindings(&unit.record, &graph.units)?;
            Ok((identity.clone(), source_manifest(unit)?))
        })
        .collect()
}

/// Bind the complete actual producer set to the captured local generations and explicit registry lock/aliases.
fn verify_request_bindings(
    request: &ProducerRequest,
    graph: &NativeLoafGraph,
    manifests: &BTreeMap<String, toml::Value>,
) -> Result<()> {
    let seed = current_inputs::read_resolution_bytes(request.lock.bytes()).map_err(super::NativeLoafError::Failed)?;
    let mut selected = BTreeSet::new();
    for binding in &seed.units {
        let expected = serde_json::to_value(binding.identity_binding()).map_err(failed)?;
        let matching = graph
            .units
            .iter()
            .filter(|(_, unit)| {
                super::source_origin(&unit.record.recipe).is_ok_and(|origin| origin == NativeLoafOrigin::Registry)
                    && serde_json::to_value(&unit.record.source).is_ok_and(|source| source == expected)
            })
            .collect::<Vec<_>>();
        let [(identity, _)] = matching.as_slice() else {
            return Err(refused(
                "complete native producer registry binding is missing or ambiguous",
            ));
        };
        let unit = graph
            .units
            .get(identity.as_str())
            .ok_or_else(|| refused("complete native producer registry record is missing"))?;
        if let Some(edges) = &binding.edges {
            if edges.len() != unit.record.dependencies.len() {
                return Err(refused("complete native producer edges differ from the actual lock"));
            }
            for edge in edges {
                let mut matches = 0;
                for dependency in &unit.record.dependencies {
                    let child_manifest = manifests
                        .get(&dependency.record_identity)
                        .ok_or_else(|| refused("complete native producer child declaration is missing"))?;
                    if dependency.source.loaf == edge.loaf
                        && dependency.source.version == edge.version
                        && dependency.source.domain == edge.domain
                        && dependency.alias
                            == crate::sdk_closure::native_extern_alias(&edge.dependency_key, &edge.loaf, child_manifest)
                    {
                        matches += 1;
                    }
                }
                if matches != 1 {
                    return Err(refused(
                        "complete native producer extern alias differs from the actual lock",
                    ));
                }
            }
        }
        selected.insert(identity.as_str());
    }
    for local in &request.local {
        let matching = graph
            .units
            .iter()
            .filter(|(identity, unit)| {
                super::source_origin(&unit.record.recipe).is_ok_and(|origin| origin == NativeLoafOrigin::Local)
                    && unit.record.source.archive_digest == local.source
                    && unit.record.source.domain == local.domain
                    && unit.record.source.features == local.features
                    && manifests.get(identity.as_str()) == Some(&local.manifest)
            })
            .collect::<Vec<_>>();
        let [(identity, _)] = matching.as_slice() else {
            return Err(refused(
                "complete native producer local binding is missing or ambiguous",
            ));
        };
        selected.insert(identity.as_str());
    }
    if selected != graph.units.keys().map(String::as_str).collect() {
        return Err(refused(
            "complete native producer contains units outside its actual supplied request",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;

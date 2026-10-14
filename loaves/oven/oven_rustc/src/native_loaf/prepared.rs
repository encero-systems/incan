//! Generic prepared-consumer coordinates with source-current rooted admission (#1337, #1698).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use oven_model::manifest::{DependencySource, DependencySpec};
use oven_model::observed_inputs::InputObservations;
use oven_store::store::{OvenStore, OvenStoreLimits};
use serde::{Deserialize, Serialize};

use super::preparation::{NativeLoafPreparation, ResolvedNativeGraph, read_resolved_native_graph};
use super::selection::{LocalSources, select_roots_with_sources, source_manifest};
use super::{
    NativeLoafClosure, NativeLoafError, NativeLoafGraph, NativeLoafOrigin, NativeLoafRoot, NativeLoafVerificationWork,
    Result, failed, refused,
};
use crate::sdk_closure::{Seed, current_inputs};

const SCHEMA: &str = "incan.oven.prepared-native-consumer-hint/1";

mod projection;

/// Explicit current physical selection inputs; dependency activation remains the declaration resolver's authority.
pub struct NativeLoafConsumerRequest<'a> {
    /// Resolved graph document, with owner-relative lock and local facets.
    pub graph: &'a Path,
    /// Adopted index repository at the graph's immutable revision.
    pub index: &'a Path,
    /// Archive directory used only on a genuine preparation miss.
    pub blobs: &'a Path,
    /// Mutable hints and staging; also the default Store parent when no Store is supplied explicitly.
    pub output: &'a Path,
    /// Explicit selected native compiler executable.
    pub rustc: &'a Path,
    /// Explicit selected target triple.
    pub target: &'a str,
    /// Exact selected debug or release profile.
    pub profile: &'a str,
    /// Already active normal/dev/cfg/optional Rust declarations.
    pub dependencies: &'a [DependencySpec],
    /// Canonical manifest owner directory; parsed paths must come from a canonical declaration.
    pub declaration_owner: &'a Path,
    /// Host or target domain for ordinary roots; proc macros retain their authored host domain.
    pub domain: &'a str,
}

/// Actual rooted work at the ordinary preparation/reuse boundary, without invented phase attribution.
#[derive(Default, Debug, Serialize)]
pub struct NativeLoafConsumerReport {
    /// Actual registry preparation index work; zero on warm/empty requests and distinct from current-input replay.
    pub preparation_index_reads: crate::sdk_closure::NativeIndexReadWork,
    /// Complete record and edge checks during graph admission/selection, excluding subsequent current-input replay.
    pub graph_verification: NativeLoafVerificationWork,
    /// Whether an optional coordinate hint survived complete current-input and per-unit admission checks.
    pub prepared_reuse: bool,
    /// Whether this request reconciled the original retained closure without Store readmission.
    pub retained_reuse: bool,
    /// Actual calls into complete native preparation; an unchanged admitted command reports zero.
    pub preparation_calls: usize,
    /// Distinct per-unit records returned by a genuine complete preparation; zero on warm/empty commands.
    pub prepared_units: usize,
    /// Labels actually compiled on a preparation miss.
    pub compiled: Vec<String>,
    /// Labels reused by the existing producer on a preparation miss.
    pub reused: Vec<String>,
    /// Rooted per-unit records retained for this consumer.
    pub selected_units: usize,
    /// Distinct original native owners retained, independently of the number of declared aliases.
    pub selected_native_owners: usize,
    /// Actual current local mapping/hash demands in this command, including a failed stale-hint validation.
    pub current_local_sources: usize,
    /// Actual registry binding/edge replays; unchanged privately retained lock authority requires none.
    pub current_registry_bindings: usize,
    /// Distinct source candidate facts retained for dependency selection, without acquiring their native owners.
    pub current_local_dependency_candidates: usize,
    /// Demanded package candidate maps actually reconstructed during this request.
    pub dependency_candidate_projections: usize,
    /// Demanded candidate lookups served by the original observed declarations without reconstruction.
    pub dependency_candidate_reuses: usize,
    /// Native executable-owner selections during current-input replay; cached reuse requires none.
    pub native_tool_owner_checks: usize,
    /// Actual current compiler standard-library closure observations.
    pub compiler_closure_checks: usize,
    /// Actual current proc-macro linker-closure observations.
    pub linker_closure_checks: usize,
    /// Git processes started by current recipe replay, excluding declaration root selection and independent cold
    /// preparation.
    pub index_git_processes: usize,
    /// Complete batch requests submitted, including the initial commit-type check.
    pub index_batch_requests: usize,
    /// Current pinned file demands, including repeated command-local reads.
    pub index_file_requests: usize,
    /// Distinct pinned blobs read through the selected Git transport.
    pub index_blob_reads: usize,
    /// File demands served by this command's already admitted pinned bytes.
    pub index_blob_cache_hits: usize,
    /// Actual pinned blob payload bytes read, excluding protocol headers and commit metadata.
    pub index_blob_bytes: usize,
    /// Best-effort coordinate-hint persistence failure; immutable native authority remains usable without the hint.
    pub hint_persistence_error: Option<String>,
    /// Measured whole boundary time, including current-input validation and genuine miss preparation.
    pub seconds: f64,
}

/// A current declared physical closure retaining only its original rooted record/native owners.
pub struct NativeLoafConsumerPreparation {
    closure: NativeLoafClosure,
    report: NativeLoafConsumerReport,
    /// Present only after complete current-input validation; persisted hints cannot supply this proof.
    current_key: Option<String>,
    local: LocalSources,
    local_declarations: Option<ObservedDeclarations>,
}

impl NativeLoafConsumerPreparation {
    /// Borrow the source-current declared closure and original leases for direct-plan publication.
    pub fn closure(&self) -> &NativeLoafClosure {
        &self.closure
    }

    /// Borrow actual preparation/admission work counts and the measured whole boundary time.
    pub fn report(&self) -> &NativeLoafConsumerReport {
        &self.report
    }

    /// Reconcile a completed selection against current inputs, retaining its original graph and leases on success.
    ///
    /// A changed request returns `None` for the existing producer to handle. Corrupt original owners return an error.
    /// This consumes the previous selection so a failed reconciliation cannot leave a supposedly current capability
    /// with its caller. It does not discover Store entries or acquire replacement owners.
    pub fn reconcile(mut self, request: &NativeLoafConsumerRequest<'_>) -> Result<Option<Self>> {
        let _invocation = tracing::trace_span!(target: "incan::invocation", "oven_rustc::reconcile_native_loafs", input = %request.output.display()).entered();
        let started = Instant::now();
        let mut report = NativeLoafConsumerReport::default();
        if request.dependencies.is_empty() {
            if self.current_key.is_some() {
                return Ok(None);
            }
            report.prepared_reuse = true;
            report.retained_reuse = true;
            report.seconds = started.elapsed().as_secs_f64();
            self.report = report;
            return Ok(Some(self));
        }
        let mut current = Current::read(request, &mut report)?;
        if self.current_key.as_ref() != Some(&current.key) {
            return Ok(None);
        }
        current.local = std::mem::take(&mut self.local);
        current.local.begin_request();
        current.local_declarations = self.local_declarations.take();
        for unit in self.closure.graph.units.values() {
            report.graph_verification.record_checks += 1;
            unit.verify()?;
        }
        let roots = self
            .closure
            .roots
            .iter()
            .map(|(alias, identity)| {
                let unit = self
                    .closure
                    .graph
                    .units
                    .get(identity)
                    .ok_or_else(|| refused("retained native root is missing"))?;
                Ok(NativeLoafRoot {
                    alias: alias.clone(),
                    record_identity: identity.clone(),
                    source: unit.record.source.clone(),
                    intent: unit.record.recipe.intent.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        if !current.matches_recipe(request, &self.closure, &roots, &mut report, false)? {
            return Ok(None);
        }
        current.finish_index()?;
        report.prepared_reuse = true;
        report.retained_reuse = true;
        finish_report(&self.closure, &current, &mut report, started)?;
        self.report = report;
        self.local = current.local;
        self.local_declarations = current.local_declarations;
        Ok(Some(self))
    }
}

/// A mutable optional hint contains coordinates only; every referenced immutable record is independently checked.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Hint {
    schema: String,
    key: String,
    roots: Vec<NativeLoafRoot>,
}

/// Command-owned current compiler facts shared by input keys and every selected per-unit recipe check.
#[derive(Serialize)]
struct CompilerInputs {
    executable: String,
    std: String,
    identity: String,
    host: String,
    commit: String,
}

/// Current source/lock snapshots and command-local observation contexts; never a process-global admission cache.
struct Current {
    owner: PathBuf,
    graph: ResolvedNativeGraph,
    seed: Seed,
    key: String,
    compiler: CompilerInputs,
    local: LocalSources,
    /// Original authored declarations in graph facet order, guarded by their consulted paths.
    local_declarations: Option<ObservedDeclarations>,
    declarations_checked: bool,
    link: Option<Option<String>>,
    candidate_projections: usize,
    candidate_reuses: usize,
    index: Option<current_inputs::PinnedIndexBatch>,
}

/// Parsed facet declarations in original graph order and the metadata observed before their reads.
struct ObservedDeclarations {
    manifests: Vec<toml::Value>,
    inputs: InputObservations,
    /// Projection of these declarations and the same graph/lock key, moved with their original observations.
    candidates: BTreeMap<String, Vec<CurrentCandidate>>,
}

/// Current source-only candidate facts; this descriptor confers no physical or semantic execution authority.
struct CurrentCandidate {
    loaf: String,
    version: String,
    domain: String,
    features: Vec<String>,
    proc_macro: bool,
    origin: NativeLoafOrigin,
}

/// Reuse only source-current declared roots, otherwise prepare through the existing ordinary native producer once.
///
/// Unchanged reuse never calls complete closure preparation or admits unrelated native owners. Hints are optional
/// coordinates, not authority: current declarations, lock edges, source generations, compiler, facts, environments
/// and tool owners are checked against exact immutable per-unit recipes. Graph/lock-wide invalidation is currently
/// conservative; cold registry projection, cross-Store distribution and code/receipt decoupling remain separate work.
/// Cross-target closures containing host units refuse until their recipes independently bind the host standard library.
pub fn prepare_declared_native_loafs(request: &NativeLoafConsumerRequest<'_>) -> Result<NativeLoafConsumerPreparation> {
    let store = native_store(request.output);
    prepare_declared_native_loafs_in_store(request, &store)
}

/// Retain declared dependencies in the command's Store while keeping mutable hints and staging at its output.
///
/// Relocating a checkout or output never creates another dependency Store. Every warm selection still reproduces
/// current source/compiler facts and retains the original native owners; explicit Store limits remain authoritative.
pub fn prepare_declared_native_loafs_in_store(
    request: &NativeLoafConsumerRequest<'_>,
    store: &OvenStore,
) -> Result<NativeLoafConsumerPreparation> {
    prepare_with_store(request, store, |current, report| {
        let prepared = projection::prepare(request, store, current)?;
        if Current::read(request, report)?.key != current.key {
            return Err(refused(
                "native consumer graph/lock/compiler inputs changed during preparation",
            ));
        }
        Ok(prepared)
    })
}

/// Run the real reuse decision with one injectable preparation boundary for work-count/refusal controls.
#[cfg(test)]
fn prepare_with(
    request: &NativeLoafConsumerRequest<'_>,
    prepare: impl FnOnce() -> Result<NativeLoafPreparation>,
) -> Result<NativeLoafConsumerPreparation> {
    let store = native_store(request.output);
    prepare_with_store(request, &store, |_, _| prepare())
}

/// Admit current native inputs from one explicit Store before invoking its existing producer on a genuine miss.
#[track_caller]
fn prepare_with_store(
    request: &NativeLoafConsumerRequest<'_>,
    store: &OvenStore,
    prepare: impl FnOnce(&mut Current, &mut NativeLoafConsumerReport) -> Result<NativeLoafPreparation>,
) -> Result<NativeLoafConsumerPreparation> {
    let caller = std::panic::Location::caller();
    let _invocation = tracing::trace_span!(target: "incan::invocation", "oven_rustc::prepare_native_loafs", input = %request.output.display(), caller_file = caller.file(), caller_line = u64::from(caller.line())).entered();
    let started = Instant::now();
    let mut report = NativeLoafConsumerReport::default();
    if request.dependencies.is_empty() {
        report.seconds = started.elapsed().as_secs_f64();
        return Ok(NativeLoafConsumerPreparation {
            closure: NativeLoafGraph::default().select(&[])?,
            report,
            current_key: None,
            local: LocalSources::default(),
            local_declarations: None,
        });
    }
    let mut current = Current::read(request, &mut report)?;
    let hint_path = request
        .output
        .join("consumer-hints")
        .join(format!("{}.json", current.key.replace(':', "-")));
    if let Some(hint) = read_hint(&hint_path, &current.key)? {
        let admitted = NativeLoafClosure::admit_with_work(store, &hint.roots, &mut report.graph_verification);
        match admitted {
            Ok(closure) => {
                if current.matches(request, &closure, &hint.roots, &mut report)? {
                    tracing::trace!(target: "incan::invocation", decision = "reuse", input = %current.key, preparation_calls = 0_u64);
                    report.prepared_reuse = true;
                    current.finish_index()?;
                    finish_report(&closure, &current, &mut report, started)?;
                    return Ok(NativeLoafConsumerPreparation {
                        closure,
                        report,
                        current_key: Some(current.key),
                        local: current.local,
                        local_declarations: current.local_declarations,
                    });
                }
            }
            Err(NativeLoafError::Unavailable { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    tracing::trace!(target: "incan::invocation", decision = "prepare", input = %current.key);
    report.preparation_calls += 1;
    let prepared = prepare(&mut current, &mut report)?;
    report.prepared_units = prepared.graph.units.len();
    report.compiled = prepared.report.compiled.clone();
    report.reused = prepared.report.reused.clone();
    report.preparation_index_reads = prepared.report.index_reads;
    let roots = select_roots_with_sources(
        &prepared.graph,
        request.dependencies,
        request.declaration_owner,
        request.domain,
        &mut current.local,
    )?;
    let closure = prepared
        .graph
        .select_with_work(&roots, &mut report.graph_verification)?;
    // A source mutation during preparation must not publish a hint for a generation different from this snapshot.
    if !current.matches(request, &closure, &roots, &mut report)? {
        return Err(refused(
            "new native preparation differs from the command's current source/lock/compiler inputs",
        ));
    }
    current.finish_index()?;
    if let Err(error) = write_hint(
        &hint_path,
        &Hint {
            schema: SCHEMA.to_string(),
            key: current.key.clone(),
            roots,
        },
    ) {
        report.hint_persistence_error = Some(error.to_string());
    }
    finish_report(&closure, &current, &mut report, started)?;
    Ok(NativeLoafConsumerPreparation {
        closure,
        report,
        current_key: Some(current.key),
        local: current.local,
        local_declarations: current.local_declarations,
    })
}

impl Current {
    /// Capture exact graph/lock/declaration/compiler inputs without source staging or native owner selection.
    #[track_caller]
    fn read(request: &NativeLoafConsumerRequest<'_>, report: &mut NativeLoafConsumerReport) -> Result<Self> {
        let caller = std::panic::Location::caller();
        let _invocation = tracing::trace_span!(target: "incan::invocation", "oven_rustc::read_current_native_inputs", input = %request.output.display(), caller_file = caller.file(), caller_line = u64::from(caller.line())).entered();
        if !matches!(request.profile, "debug" | "release") || !matches!(request.domain, "host" | "target") {
            return Err(refused(
                "ordinary native consumer requires an explicit profile and host/target domain",
            ));
        }
        let (owner, graph, graph_digest) = read_resolved_native_graph(request.graph)?;
        let lock_bytes = std::fs::read(owner.join(&graph.registry_lock)).map_err(failed)?;
        let seed = current_inputs::read_resolution_bytes(&lock_bytes).map_err(NativeLoafError::Failed)?;
        let rustc = request.rustc.canonicalize().map_err(failed)?;
        let compiler = CompilerInputs {
            executable: oven_store::store::digest_regular_file(&rustc).map_err(failed)?.1,
            std: crate::sdk_closure::compiler_closure_digest(&rustc, request.target)
                .map_err(NativeLoafError::Failed)?,
            identity: crate::rustc::rustc_identity(&rustc).map_err(failed)?,
            host: crate::rustc::rustc_host_target(&rustc).map_err(failed)?,
            commit: crate::rustc::rustc_commit_hash(&rustc).ok_or_else(|| refused("compiler has no commit hash"))?,
        };
        report.compiler_closure_checks += 1;
        let declarations = declaration_key(request.dependencies, request.declaration_owner)?;
        let key = oven_store::digest_bytes(
            &serde_json::to_vec(&(
                SCHEMA,
                &owner,
                graph_digest,
                oven_store::digest_bytes(&lock_bytes),
                request.index.canonicalize().map_err(failed)?,
                request.blobs,
                &rustc,
                &compiler,
                request.target,
                request.profile,
                request.domain,
                declarations,
            ))
            .map_err(failed)?,
        );
        Ok(Self {
            owner,
            graph,
            seed,
            key,
            compiler,
            local: LocalSources::default(),
            local_declarations: None,
            declarations_checked: false,
            link: None,
            candidate_projections: 0,
            candidate_reuses: 0,
            index: None,
        })
    }

    /// Compare a freshly admitted closure against independently current declaration, lock and source inputs.
    /// Both callers select or admit the complete graph immediately before comparison. This operation reads sealed
    /// records and digest-checked source declarations, not native outputs; repeating every owner inventory here
    /// adds no authority. Declared-root projection and subsequent physical consumers retain fresh owner checks.
    fn matches(
        &mut self,
        request: &NativeLoafConsumerRequest<'_>,
        closure: &NativeLoafClosure,
        hinted_roots: &[NativeLoafRoot],
        report: &mut NativeLoafConsumerReport,
    ) -> Result<bool> {
        self.matches_recipe(request, closure, hinted_roots, report, true)
    }

    /// Reconcile mutable sources and tools through the original selectors. Immutable recipe replay may be omitted
    /// only for the same privately retained closure after its complete graph/lock/compiler key matches again.
    /// Disk hints and newly admitted records always require the pinned recipe replay.
    fn matches_recipe(
        &mut self,
        request: &NativeLoafConsumerRequest<'_>,
        closure: &NativeLoafClosure,
        hinted_roots: &[NativeLoafRoot],
        report: &mut NativeLoafConsumerReport,
        replay_recipe: bool,
    ) -> Result<bool> {
        let _invocation = tracing::trace_span!(target: "incan::invocation", "oven_rustc::validate_current_native_inputs", input = %self.key).entered();
        if self.seed.schema != "incan.oven.loaf-resolution/2" {
            return Err(refused(
                "prepared native reuse requires explicit authenticated resolution edges",
            ));
        }
        let bindings = closure
            .graph
            .units
            .values()
            .filter(|unit| {
                replay_recipe
                    && super::source_origin(&unit.record.recipe)
                        .is_ok_and(|origin| origin == NativeLoafOrigin::Registry)
            })
            .map(|unit| {
                serde_json::from_value(serde_json::to_value(&unit.record.source).map_err(failed)?).map_err(failed)
            })
            .collect::<Result<Vec<crate::sdk_closure::SdkLockedUnit>>>()?;
        let about = if bindings.is_empty() {
            BTreeMap::new()
        } else {
            if self.index.is_none() {
                self.index = Some(
                    current_inputs::PinnedIndexBatch::open(request.index, &self.graph.index_commit)
                        .map_err(NativeLoafError::Failed)?,
                );
            }
            current_inputs::selected_about(
                &bindings,
                self.index
                    .as_mut()
                    .ok_or_else(|| refused("current pinned index reader missing"))?,
            )
            .map_err(NativeLoafError::Failed)?
        };
        for unit in closure.graph.units.values() {
            let record = &unit.record;
            let source = &record.source;
            let inputs = &record.recipe.sources.build_unit_inputs;
            if source.domain == "host" && self.compiler.host != request.target {
                return Err(refused(
                    "cross-target host native reuse requires independently bound host standard-library authority",
                ));
            }
            let target = if source.domain == "host" {
                self.compiler.host.as_str()
            } else {
                request.target
            };
            if record.recipe.intent.target != target
                || record.recipe.intent.profile != request.profile
                || record.recipe.intent.toolchain != self.compiler.identity
                || inputs.get("compiler-binary") != Some(&self.compiler.std)
                || inputs.get("native-compiler-executable") != Some(&self.compiler.executable)
                || inputs.get("compiler-host") != Some(&self.compiler.host)
                || inputs.get("compiler-commit") != Some(&self.compiler.commit)
                || inputs.get("sdk-compile-policy").map(String::as_str) != Some("source-sealed-portable-v2")
            {
                return Ok(false);
            }
            let origin = super::source_origin(&record.recipe)?;
            if origin == NativeLoafOrigin::Registry && !replay_recipe {
                let manifest = unit
                    .source_declaration
                    .get()
                    .ok_or_else(|| refused("retained registry declaration was not admitted"))?;
                if !self.matches_link_closure(request, inputs, manifest, report)? {
                    return Ok(false);
                }
                continue;
            }
            let manifest = source_manifest(unit)?;
            if origin == NativeLoafOrigin::Registry {
                report.current_registry_bindings += 1;
                let candidates = self
                    .seed
                    .units
                    .iter()
                    .filter(|binding| {
                        binding.loaf == source.loaf
                            && binding.version == source.version
                            && binding.domain == source.domain
                    })
                    .collect::<Vec<_>>();
                let [binding] = candidates.as_slice() else {
                    return Ok(false);
                };
                if serde_json::to_value(binding.identity_binding()).map_err(failed)?
                    != serde_json::to_value(source).map_err(failed)?
                {
                    return Ok(false);
                }
                let edges = binding
                    .edges
                    .as_ref()
                    .ok_or_else(|| refused("current native resolution has no edges"))?;
                if edges.len() != record.dependencies.len() {
                    return Ok(false);
                }
                for edge in edges {
                    let candidates = record.dependencies.iter().filter(|selected| {
                        selected.source.loaf == edge.loaf
                            && selected.source.version == edge.version
                            && selected.source.domain == edge.domain
                    });
                    let mut matches = 0;
                    for selected in candidates {
                        let child = closure
                            .graph
                            .units
                            .get(&selected.record_identity)
                            .ok_or_else(|| refused("current physical child record is missing"))?;
                        let child_manifest = source_manifest(child)?;
                        if selected.alias
                            == crate::sdk_closure::native_extern_alias(
                                &edge.dependency_key,
                                &edge.loaf,
                                &child_manifest,
                            )
                        {
                            matches += 1;
                        }
                    }
                    if matches != 1 {
                        return Ok(false);
                    }
                }
            } else {
                if !self.matches_local(source, &manifest)?
                    || !self.matches_local_dependencies(request, closure, record, &manifest)?
                {
                    return Ok(false);
                }
            }
            let metadata = if origin == NativeLoafOrigin::Registry {
                about
                    .get(&source.archive_digest)
                    .cloned()
                    .unwrap_or(serde_json::Value::Null)
            } else {
                serde_json::Value::Null
            };
            if replay_recipe
                && !current_inputs::matches_current_recipe(
                    record,
                    &manifest,
                    origin,
                    &self.seed,
                    &metadata,
                    self.index.as_mut(),
                )
                .map_err(NativeLoafError::Failed)?
            {
                return Ok(false);
            }
            if !self.matches_link_closure(request, inputs, &manifest, report)? {
                return Ok(false);
            }
        }
        let expected = select_roots_with_sources(
            &closure.graph,
            request.dependencies,
            request.declaration_owner,
            request.domain,
            &mut self.local,
        )?;
        Ok(serde_json::to_value(expected).map_err(failed)? == serde_json::to_value(hinted_roots).map_err(failed)?)
    }

    /// Reconcile mutable linker inputs even when the authenticated registry recipe and declaration are retained.
    /// The request-local observation is shared by all selected macro units; no previous request supplies its identity.
    fn matches_link_closure(
        &mut self,
        request: &NativeLoafConsumerRequest<'_>,
        inputs: &BTreeMap<String, String>,
        manifest: &toml::Value,
        report: &mut NativeLoafConsumerReport,
    ) -> Result<bool> {
        let _trace =
            tracing::trace_span!(target: "incan::invocation", "oven_rustc::reconcile_native_link_closure").entered();
        let proc_macro = manifest
            .get("rust")
            .and_then(|rust| rust.get("type"))
            .and_then(toml::Value::as_str)
            == Some("proc-macro");
        if proc_macro || inputs.contains_key("link-closure") {
            if self.link.is_none() {
                self.link = Some(
                    crate::rustc::linking::pinned_link_closure_identity(request.rustc, request.target)
                        .map_err(failed)?,
                );
                report.linker_closure_checks += 1;
            }
            return Ok(self.link.as_ref().and_then(Option::as_ref) == inputs.get("link-closure"));
        }
        Ok(true)
    }

    /// Require the command-owned Git child to terminate successfully before publishing or handing off current facts.
    fn finish_index(&mut self) -> Result<()> {
        self.local.finish_request()?;
        if let Some(declarations) = &self.local_declarations
            && declarations.inputs.supports_retention()
            && !declarations.inputs.current()
        {
            return Err(refused("local declarations changed during native selection"));
        }
        if let Some(index) = &mut self.index {
            index.finish().map_err(NativeLoafError::Failed)?;
        }
        Ok(())
    }

    /// Reapply the same producer selector to the current declared candidate world, rather than trusting a hint's
    /// subset.
    fn matches_local_dependencies(
        &mut self,
        request: &NativeLoafConsumerRequest<'_>,
        closure: &NativeLoafClosure,
        record: &super::NativeLoafRecord,
        manifest: &toml::Value,
    ) -> Result<bool> {
        let _trace = tracing::trace_span!(target: "incan::invocation", "oven_rustc::reconcile_local_native_dependencies", input = %record.source.loaf).entered();
        let demands = current_inputs::local_dependency_demands(manifest, &record.source.features)
            .map_err(NativeLoafError::Failed)?;
        if demands.len() != record.dependencies.len() {
            return Ok(false);
        }
        for demand in demands {
            let candidates = self.current_candidates(request, closure, &demand.loaf)?;
            let bindings = candidates
                .iter()
                .map(|candidate| current_inputs::LocalDependencyCandidate {
                    loaf: &candidate.loaf,
                    version: &candidate.version,
                    domain: &candidate.domain,
                    features: &candidate.features,
                    proc_macro: candidate.proc_macro,
                })
                .collect::<Vec<_>>();
            let selected = current_inputs::select_local_dependency(&demand, &record.source.domain, &bindings)
                .map_err(NativeLoafError::Failed)?;
            let candidate = &candidates[selected];
            let matched = record
                .dependencies
                .iter()
                .filter(|edge| {
                    edge.alias == demand.alias
                        && edge.source.loaf == candidate.loaf
                        && edge.source.version == candidate.version
                        && edge.source.domain == candidate.domain
                        && edge.source.features == candidate.features
                })
                .collect::<Vec<_>>();
            let [edge] = matched.as_slice() else {
                return Ok(false);
            };
            let child = closure
                .graph
                .units
                .get(&edge.record_identity)
                .ok_or_else(|| refused("local native dependency has no admitted child record"))?;
            if super::source_origin(&child.record.recipe)? != candidate.origin {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Project demanded candidates once per observed declaration generation, never acquiring unrelated native owners.
    ///
    /// The containing preparation moves these facts only after the graph/lock/compiler request key matches. Every
    /// local declaration, including unselected facets, remains observed. Unselected registry archives join the same
    /// observations before reading; selected declarations remain authenticated by their original owner handoffs.
    fn current_candidates(
        &mut self,
        request: &NativeLoafConsumerRequest<'_>,
        closure: &NativeLoafClosure,
        loaf: &str,
    ) -> Result<&[CurrentCandidate]> {
        self.read_local_declarations()?;
        let declarations = self
            .local_declarations
            .as_mut()
            .ok_or_else(|| refused("current local declarations missing"))?;
        if !declarations.candidates.contains_key(loaf) {
            let _projection = tracing::trace_span!(target: "incan::invocation", "oven_rustc::project_native_dependency_candidates", input = loaf).entered();
            let mut candidates = Vec::new();
            for binding in self.seed.units.iter().filter(|binding| binding.loaf == loaf) {
                let selected = closure.graph.units.values().find(|unit| {
                    unit.record.source.loaf == binding.loaf
                        && unit.record.source.version == binding.version
                        && unit.record.source.archive_digest == binding.archive_digest
                        && super::source_origin(&unit.record.recipe)
                            .is_ok_and(|origin| origin == NativeLoafOrigin::Registry)
                });
                let manifest = match selected {
                    Some(unit) => source_manifest(unit)?,
                    None => current_inputs::registry_source_manifest(request.blobs, binding, |archive| {
                        declarations.inputs.observe(archive);
                    })
                    .map_err(NativeLoafError::Failed)?,
                };
                candidates.push(CurrentCandidate {
                    loaf: binding.loaf.clone(),
                    version: binding.version.clone(),
                    domain: binding.domain.clone(),
                    features: binding.features.clone(),
                    proc_macro: manifest
                        .get("rust")
                        .and_then(|rust| rust.get("type"))
                        .and_then(toml::Value::as_str)
                        == Some("proc-macro"),
                    origin: NativeLoafOrigin::Registry,
                });
            }
            for (facet, manifest) in self.graph.facets.iter().zip(&declarations.manifests) {
                let project = manifest
                    .get("project")
                    .ok_or_else(|| refused("current local candidate has no project"))?;
                if project.get("name").and_then(toml::Value::as_str) != Some(loaf) {
                    continue;
                }
                candidates.push(CurrentCandidate {
                    loaf: loaf.to_string(),
                    version: project
                        .get("version")
                        .and_then(toml::Value::as_str)
                        .ok_or_else(|| refused("current local candidate has no version"))?
                        .to_string(),
                    domain: facet.domain.clone(),
                    features: crate::sdk_closure::native_required_features(manifest, &facet.features, false)
                        .map_err(NativeLoafError::Failed)?
                        .into_iter()
                        .collect(),
                    proc_macro: manifest
                        .get("rust")
                        .and_then(|rust| rust.get("type"))
                        .and_then(toml::Value::as_str)
                        == Some("proc-macro"),
                    origin: NativeLoafOrigin::Local,
                });
            }
            declarations.candidates.insert(loaf.to_string(), candidates);
            self.candidate_projections += 1;
        } else {
            self.candidate_reuses += 1;
            tracing::trace!(target: "incan::invocation", decision = "retained_dependency_candidates", input = loaf, projection_calls = 0_u64);
        }
        declarations
            .candidates
            .get(loaf)
            .map(Vec::as_slice)
            .ok_or_else(|| refused("current local dependency candidates missing"))
    }

    /// Compare a selected local unit to its current explicit graph facet using the existing producer mapping.
    fn matches_local(&mut self, source: &super::NativeLoafSource, manifest: &toml::Value) -> Result<bool> {
        let _trace = tracing::trace_span!(target: "incan::invocation", "oven_rustc::reconcile_local_native_source", input = %source.loaf).entered();
        self.read_local_declarations()?;
        let mut found = 0;
        for (facet, declaration) in self.graph.facets.iter().zip(
            self.local_declarations
                .as_ref()
                .ok_or_else(|| refused("current local declarations missing"))?
                .manifests
                .iter(),
        ) {
            if facet.domain != source.domain {
                continue;
            }
            let path = self.owner.join(&facet.project);
            if declaration
                .get("project")
                .and_then(|project| project.get("name"))
                .and_then(toml::Value::as_str)
                != Some(&source.loaf)
                || declaration
                    .get("project")
                    .and_then(|project| project.get("version"))
                    .and_then(toml::Value::as_str)
                    != Some(&source.version)
            {
                continue;
            }
            let features = crate::sdk_closure::native_required_features(declaration, &facet.features, false)
                .map_err(NativeLoafError::Failed)?;
            if features != source.features.iter().cloned().collect::<BTreeSet<_>>() {
                continue;
            }
            let (current, digest) = self.local.get(&path)?;
            if current == manifest && digest == &source.archive_digest {
                found += 1;
            }
        }
        Ok(found == 1)
    }

    /// Reconcile existing facet declarations once per command, parsing only after relevant metadata changes.
    /// Graph order and candidate multiplicity remain authoritative; unselected declarations are also observed so new
    /// compatible candidates cannot hide behind retained selected units. Full source mapping is guarded separately.
    fn read_local_declarations(&mut self) -> Result<()> {
        let _reconciliation = tracing::trace_span!(target: "incan::invocation", "oven_rustc::reconcile_local_declarations", already_checked = self.declarations_checked).entered();
        if self.declarations_checked {
            return Ok(());
        }
        let retained = self
            .local_declarations
            .as_ref()
            .is_some_and(|declarations| declarations.inputs.supports_retention() && declarations.inputs.current());
        if !retained {
            let mut inputs = InputObservations::default();
            let manifests = self
                .graph
                .facets
                .iter()
                .map(|facet| {
                    let project = self.owner.join(&facet.project);
                    inputs.observe(&project.join("loaf.toml"));
                    read_local_declaration(&project)
                })
                .collect::<Result<Vec<_>>>()?;
            if inputs.supports_retention() && !inputs.current() {
                return Err(refused("local declarations changed during parsing"));
            }
            self.local_declarations = Some(ObservedDeclarations {
                manifests,
                inputs,
                candidates: BTreeMap::new(),
            });
        }
        self.declarations_checked = true;
        Ok(())
    }
}

/// Observe an authored facet declaration independently of its admitted native owner's sealed source manifest.
fn read_local_declaration(project: &Path) -> Result<toml::Value> {
    let path = project.join("loaf.toml");
    let _invocation = tracing::trace_span!(target: "incan::invocation", "oven_rustc::read_current_local_declaration", input = %path.display()).entered();
    toml::from_str(&std::fs::read_to_string(path).map_err(failed)?).map_err(failed)
}

/// Canonicalize the complete declaration set by normalized alias, independent of map traversal order.
fn declaration_key(dependencies: &[DependencySpec], owner: &Path) -> Result<serde_json::Value> {
    let owner = owner.canonicalize().map_err(failed)?;
    let mut values = BTreeMap::new();
    for dependency in dependencies {
        let alias = dependency.crate_name.replace('-', "_");
        if alias.is_empty() || values.contains_key(&alias) {
            return Err(refused("declared native roots repeat an empty or normalized alias"));
        }
        let source = match &dependency.source {
            DependencySource::Registry => serde_json::json!({"registry": true}),
            DependencySource::Path { path } => {
                serde_json::json!({"path": owner.join(path).canonicalize().map_err(failed)?})
            }
            DependencySource::Git { .. } => {
                return Err(refused(
                    "git native declarations require an authenticated commit producer",
                ));
            }
        };
        let features = dependency.features.iter().collect::<BTreeSet<_>>();
        values.insert(
            alias,
            serde_json::json!({"alias": dependency.crate_name, "version": dependency.version,
            "features": features, "defaults": dependency.default_features, "optional": dependency.optional,
            "package": dependency.package, "source": source}),
        );
    }
    Ok(serde_json::json!({"owner": owner, "declarations": values.into_values().collect::<Vec<_>>()}))
}

/// Read only optional coordinates; an invalid hint is a miss, while referenced owner corruption remains an error.
fn read_hint(path: &Path, key: &str) -> Result<Option<Hint>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(failed(error)),
    };
    let Ok(hint) = serde_json::from_slice::<Hint>(&bytes) else {
        return Ok(None);
    };
    Ok((hint.schema == SCHEMA && hint.key == key).then_some(hint))
}

/// Atomically replace an optional coordinate hint only after complete rooted source-current validation.
fn write_hint(path: &Path, hint: &Hint) -> Result<()> {
    let owner = path
        .parent()
        .ok_or_else(|| refused("native consumer hint has no owner"))?;
    std::fs::create_dir_all(owner).map_err(failed)?;
    let mut temporary = tempfile::NamedTempFile::new_in(owner).map_err(failed)?;
    temporary
        .write_all(&serde_json::to_vec(hint).map_err(failed)?)
        .map_err(failed)?;
    temporary.as_file().sync_all().map_err(failed)?;
    temporary.persist(path).map_err(failed)?;
    Ok(())
}

/// Construct the same bounded ordinary Store as the existing native-only preparation boundary.
pub(super) fn native_store(output: &Path) -> OvenStore {
    OvenStore::new(
        output.join("store"),
        OvenStoreLimits::new(4 * 1024 * 1024 * 1024, 4 * 1024 * 1024 * 1024, 4 * 1024 * 1024 * 1024),
    )
}

/// Record actual retained selections and command-local boundary work without attributing unrelated producer costs.
fn finish_report(
    closure: &NativeLoafClosure,
    current: &Current,
    report: &mut NativeLoafConsumerReport,
    started: Instant,
) -> Result<()> {
    if let Some(declarations) = &current.local_declarations
        && declarations.inputs.supports_retention()
        && !declarations.inputs.current()
    {
        return Err(refused("native candidate inputs changed during reconciliation"));
    }
    report.selected_units = closure.graph.units.len();
    report.selected_native_owners = closure
        .graph
        .units
        .values()
        .map(|unit| &unit.record.native.identity)
        .collect::<BTreeSet<_>>()
        .len();
    report.current_local_sources = current.local.loads;
    // Cached native products need no installed build tools.
    report.native_tool_owner_checks = 0;
    report.current_local_dependency_candidates = current
        .local_declarations
        .as_ref()
        .map(|declarations| declarations.candidates.values().map(Vec::len).sum())
        .unwrap_or_default();
    report.dependency_candidate_projections = current.candidate_projections;
    report.dependency_candidate_reuses = current.candidate_reuses;
    if let Some(index) = &current.index {
        let work = index.work();
        report.index_git_processes = work.processes;
        report.index_batch_requests = work.requests;
        report.index_file_requests = work.file_requests;
        report.index_blob_reads = work.blob_reads;
        report.index_blob_cache_hits = work.cache_hits;
        report.index_blob_bytes = work.blob_bytes;
    }
    report.seconds = started.elapsed().as_secs_f64();
    Ok(())
}

#[cfg(all(test, unix))]
mod tests;

#[cfg(test)]
mod projection_tests;

#[cfg(test)]
mod projection_registry_tests;

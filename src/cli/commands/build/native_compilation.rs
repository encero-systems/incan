//! Host admission for the Incan-owned native-compilation projection.
//!
//! Incan decides which effective inputs form a lookup index and, once observations are complete, a reusable
//! compilation key. The Rust host keeps physical owners and execution authority. This boundary admits only the exact
//! versioned response, checks its command binding, and independently recomputes every returned content identity before
//! any key may be used to select native output.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::cli::{CliError, CliResult};
use crate::library_manifest::NativeSourceUnitDefinition;
use crate::oven::digest_bytes;
use crate::oven::rustc::{
    OvenBoundDirectRustcLibrary, OvenCallerOwnedRustcLibrary, OvenDirectRustcBake, OvenDirectRustcCompilerRetention,
    OvenDirectRustcJecBindings, OvenDirectRustcJecCompilation, OvenOwnedDirectRustcCompiler,
    OvenPreparedDirectRustcLibrary, OvenRustcArtifactManifest, OvenRustcDepInfoObservation, OvenRustcDepInfoOutcome,
};
use crate::oven::store::{
    OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore, OvenStoreExecutionPayload,
};

const SCHEMA: &str = "incan.oven.native-compilation/2";
const INDEX_DOMAIN: &str = "incan.oven.native-compilation-candidates/2";
const KEY_DOMAIN: &str = "incan.oven.native-compilation-inputs/2";
const RECORD_SCHEMA_VERSION: u32 = 1;
const RECORD_LIMIT: u64 = 1024 * 1024;
const MAX_RETAINED_KEYS_PER_CANDIDATE: usize = 64;
const MAX_ENGINE_EXCHANGES_PER_BATCH: u64 = 128;
const ENGINE_BATCH_DEADLINE: Duration = Duration::from_secs(5 * 60);
const NATIVE_CACHE_OUTPUT_DIRECTORY: &str = "native";
const MISSING_DEP_INFO_REASON: &str = "missing_rustc_dep_info";

/// Host-issued coordinates that keep a policy answer attached to its exact command and original evidence owners.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeCompilationBinding {
    pub(super) request_id: String,
    pub(super) current_receipt_id: String,
    pub(super) original_receipt_id: Option<String>,
    pub(super) unit_owner_id: String,
    pub(super) observation_id: String,
}

/// A content-addressed candidate lookup returned before observed effects are necessarily complete.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct NativeCompilationCandidate {
    pub(super) index: String,
    pub(super) material: Value,
}

/// A fully observed compilation identity admitted for immutable output lookup or publication.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct NativeCompilationKey {
    pub(super) candidate: NativeCompilationCandidate,
    pub(super) key: String,
    pub(super) material: Value,
}

/// Admitted policy result. Refusal is returned as an error and can never look like a cache miss.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum NativeCompilationProjection {
    Uncacheable {
        binding: NativeCompilationBinding,
        reasons: Vec<String>,
        candidate: Option<NativeCompilationCandidate>,
    },
    Cacheable {
        binding: NativeCompilationBinding,
        compilation: NativeCompilationKey,
    },
}

/// One canonical logical source or generated member and its exact bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct NativeCompilationContentRow(pub(super) String, pub(super) String);

/// One logical member rebound to an already admitted physical file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct NativeCompilationPhysicalRow(pub(super) String, pub(super) String, pub(super) String);

/// One generated logical member, its byte digest and producer-provenance digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct NativeCompilationGeneratedRow(pub(super) String, pub(super) String, pub(super) String);

/// One native input, its optional direct extern alias, linkage role and exact bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct NativeCompilationNativeRow(
    pub(super) String,
    pub(super) Option<String>,
    pub(super) String,
    pub(super) String,
);

/// One logical search slot and the complete native-member set reachable through it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct NativeCompilationSearchRow(pub(super) String, pub(super) String, pub(super) Vec<String>);

/// One physical search directory bound to a logical slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct NativeCompilationSearchBinding(pub(super) String, pub(super) String);

/// One complete environment observation. `None` proves that a consumed name was absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct NativeCompilationEnvironmentRow(pub(super) String, pub(super) Option<String>);

/// Compiler-owned code bytes and the target they execute for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeCompilationCompiler {
    pub(super) binary_digest: String,
    pub(super) closure_digest: String,
    pub(super) host: String,
    pub(super) target: String,
}

/// Canonical invocation vocabulary paired with the physical direct-Rustc plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeCompilationCommand {
    pub(super) cwd: String,
    pub(super) crate_name: String,
    pub(super) crate_kind: String,
    pub(super) edition: String,
    pub(super) arguments: Vec<Vec<String>>,
}

/// Checked code members produced by the declaring compilation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeCompilationSource {
    pub(super) projection: String,
    pub(super) logical_root: String,
    pub(super) entrypoint: String,
    pub(super) files: Vec<NativeCompilationContentRow>,
}

/// Observable output identity excluding its mutable attempt directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeCompilationOutput {
    pub(super) policy: String,
    pub(super) filename: String,
}

/// Effective logical inputs projected from the admitted native/source-unit plan and physical Rustc preparation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeCompilationInputs {
    pub(super) compiler: Option<NativeCompilationCompiler>,
    pub(super) command: Option<NativeCompilationCommand>,
    pub(super) source: Option<NativeCompilationSource>,
    pub(super) native: Option<Vec<NativeCompilationNativeRow>>,
    pub(super) search: Option<Vec<NativeCompilationSearchRow>>,
    pub(super) generated: Option<Vec<NativeCompilationGeneratedRow>>,
    pub(super) output: Option<NativeCompilationOutput>,
}

/// Physical bindings retained by the host and excluded from reusable content identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeCompilationExecution {
    pub(super) invocation_id: String,
    pub(super) compiler: String,
    pub(super) cwd: String,
    pub(super) source_root: String,
    pub(super) source_files: Vec<NativeCompilationPhysicalRow>,
    pub(super) native_files: Vec<NativeCompilationPhysicalRow>,
    pub(super) search_paths: Vec<NativeCompilationSearchBinding>,
    pub(super) generated_files: Vec<NativeCompilationPhysicalRow>,
    pub(super) output_root: String,
    pub(super) environment: Vec<NativeCompilationEnvironmentRow>,
}

/// Rustc dependency evidence supplied before or after one physical compilation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum NativeCompilationDepInfo {
    Missing,
    Current {
        candidate_index: String,
        receipt_id: String,
        original_compilation_key: Option<String>,
        files: Vec<NativeCompilationContentRow>,
        environment: Vec<NativeCompilationEnvironmentRow>,
    },
    Retained {
        candidate_index: String,
        receipt_id: String,
        original_compilation_key: String,
        files: Vec<NativeCompilationContentRow>,
        environment: Vec<NativeCompilationEnvironmentRow>,
    },
}

/// Explicit host-side effect classification for a prepared compilation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum NativeCompilationHostEffects {
    None,
    Unknown { reasons: Vec<String> },
}

/// Path-observability proof bound to the prepared invocation and compiler closure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum NativeCompilationPathEffects {
    Bound {
        profile: String,
        compiler_closure_digest: String,
        invocation_id: String,
        effects_digest: String,
    },
    Unknown {
        reasons: Vec<String>,
    },
}

/// Observations required before the Incan projection may issue a reusable compilation key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeCompilationEvidence {
    pub(super) dep_info: NativeCompilationDepInfo,
    pub(super) host_effects: NativeCompilationHostEffects,
    pub(super) path_effects: NativeCompilationPathEffects,
}

/// One exact bounded request to the installed Incan native-compilation policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeCompilationRequest {
    schema: &'static str,
    pub(super) binding: NativeCompilationBinding,
    pub(super) execution: NativeCompilationExecution,
    pub(super) inputs: NativeCompilationInputs,
    pub(super) evidence: NativeCompilationEvidence,
}

impl NativeCompilationRequest {
    pub(super) fn new(
        binding: NativeCompilationBinding,
        execution: NativeCompilationExecution,
        inputs: NativeCompilationInputs,
        evidence: NativeCompilationEvidence,
    ) -> Self {
        Self {
            schema: SCHEMA,
            binding,
            execution,
            inputs,
            evidence,
        }
    }

    pub(super) fn to_bytes(&self) -> CliResult<Vec<u8>> {
        serde_json::to_vec(self).map_err(|error| invalid(format!("cannot encode native-compilation request: {error}")))
    }
}

/// Encode only requests that fit the fixed Engine exchange ceiling; larger valid units remain compilable without JEC.
fn bounded_engine_request(request: &NativeCompilationRequest) -> CliResult<Option<Vec<u8>>> {
    let request = request.to_bytes()?;
    Ok((request.len() <= super::engine_exchange::FILE_LIMIT).then_some(request))
}

fn projection_exchange_is_unavailable(outcome: super::engine_exchange::ExchangeOutcome) -> bool {
    matches!(
        outcome,
        super::engine_exchange::ExchangeOutcome::Failed | super::engine_exchange::ExchangeOutcome::TimedOut
    )
}

/// Retained admitted Engine and command filesystem scope for one provider batch.
pub(super) struct NativeCompilationHost<'host, 'engine> {
    admitted: &'host super::engine::AdmittedEngineArtifact<'engine>,
    store: OvenStore,
    output_root: &'host Path,
    host_target: &'host str,
    cancelled: &'host AtomicBool,
    compiler: Option<Arc<OvenOwnedDirectRustcCompiler>>,
    exchanges: AtomicU64,
    deadline: Instant,
}

/// Already admitted physical and semantic facts for one regular provider library.
pub(super) struct NativeCompilationUnit<'unit> {
    pub(super) receipt: &'unit crate::oven::OvenReceipt,
    pub(super) unit_owner_id: &'unit str,
    pub(super) definition: &'unit NativeSourceUnitDefinition,
    pub(super) crate_root: &'unit Path,
    pub(super) artifacts: &'unit OvenRustcArtifactManifest,
    pub(super) artifact_root: &'unit Path,
    /// Private caller-owned snapshot whose members require a mutation guard around an actual compiler process.
    /// Store and Loaf members remain protected by their retained immutable owners instead.
    pub(super) private_snapshot_root: Option<&'unit Path>,
    pub(super) dependencies: &'unit [OvenCallerOwnedRustcLibrary],
    pub(super) prepared: &'unit OvenPreparedDirectRustcLibrary,
    pub(super) output: &'unit Path,
}

impl<'host, 'engine> NativeCompilationHost<'host, 'engine> {
    pub(super) fn new(
        admitted: &'host super::engine::AdmittedEngineArtifact<'engine>,
        receipt: &crate::oven::OvenReceipt,
        rustc: &Path,
        output_root: &'host Path,
        host_target: &'host str,
        cancelled: &'host AtomicBool,
    ) -> CliResult<Self> {
        let store = super::open_default_oven_store()?;
        let compiler =
            match crate::oven::rustc::retain_direct_rustc_compiler(&store, receipt, rustc, &receipt.intent.target)
                .map_err(oven_rustc_error)?
            {
                OvenDirectRustcCompilerRetention::Retained(compiler) => Some(Arc::new(compiler)),
                OvenDirectRustcCompilerRetention::Unavailable { reason } => {
                    eprintln!("warning: JEC is unavailable for this provider batch: {reason}");
                    None
                }
            };
        Ok(Self {
            admitted,
            store,
            output_root,
            host_target,
            cancelled,
            compiler,
            exchanges: AtomicU64::new(0),
            deadline: Instant::now() + ENGINE_BATCH_DEADLINE,
        })
    }

    pub(super) fn compiler(&self) -> Option<&Arc<OvenOwnedDirectRustcCompiler>> {
        self.compiler.as_ref()
    }

    /// Run one independently permitted projection and admit its response before returning any cache coordinate.
    pub(super) fn project(
        &self,
        receipt: &crate::oven::OvenReceipt,
        request: &NativeCompilationRequest,
    ) -> CliResult<Option<NativeCompilationProjection>> {
        let binding = request.binding.clone();
        let Some(request) = bounded_engine_request(request)? else {
            return Ok(None);
        };
        let sequence = self.exchanges.fetch_add(1, Ordering::Relaxed);
        let now = Instant::now();
        if sequence >= MAX_ENGINE_EXCHANGES_PER_BATCH || now >= self.deadline {
            return Ok(None);
        }
        let authority = super::EngineCommandAuthority {
            operation: super::EngineKernelOperation::ProjectNativeCompilation,
            ceiling: super::EngineKernelCeiling::BoundedNativeCompilation,
        };
        let permit = authority.issue(
            self.admitted,
            receipt,
            &request,
            self.output_root,
            self.host_target,
            (now + Duration::from_secs(30)).min(self.deadline),
            self.cancelled,
        )?;
        let report = super::engine_exchange::exchange(self.admitted, permit, &request, self.cancelled);
        super::persist_engine_exchange_observation(self.output_root, &report)?;
        if projection_exchange_is_unavailable(report.outcome) {
            return Ok(None);
        }
        if report.outcome != super::engine_exchange::ExchangeOutcome::Completed {
            return Err(CliError::failure(format!(
                "native-compilation Engine exchange did not complete under its command authority: {:?}",
                report.outcome
            )));
        }
        let response = report
            .response
            .as_deref()
            .ok_or_else(|| CliError::failure("completed native-compilation exchange has no response"))?;
        admit_response(response, &binding).map(Some)
    }

    /// Reuse or compile one regular provider library through the Incan-owned byte projection.
    pub(super) fn materialize(&self, unit: &NativeCompilationUnit<'_>) -> CliResult<OvenDirectRustcBake> {
        let bindings = logical_bindings(unit.prepared);
        let bound = unit.prepared.bind(unit.output, &bindings).map_err(oven_rustc_error)?;
        let Some(projection) = PreparedNativeCompilation::new(unit, &bound, bindings)? else {
            return bound
                .compile_uncacheable()
                .map(|compiled| compiled.bake)
                .map_err(oven_rustc_error);
        };
        let cold_binding = next_binding(unit, None, "missing");
        let cold = projection.request(cold_binding, NativeCompilationDepInfo::Missing);
        let Some(cold) = self.project(unit.receipt, &cold)? else {
            return projection.compile_uncacheable(&bound).map(|compiled| compiled.bake);
        };
        let candidate = reusable_cold_candidate(cold)?;
        let Some(candidate) = candidate else {
            return projection.compile_uncacheable(&bound).map(|compiled| compiled.bake);
        };
        let Some(retained) = read_retained_candidates(&self.store, &candidate.index, &projection.output_filename)?
        else {
            return projection.compile(&bound).map(|compiled| compiled.bake);
        };
        for retained in retained {
            if !projection.retained_observation_can_match(&retained.observation)? {
                continue;
            }
            let binding = next_binding(
                unit,
                Some(retained.original_receipt_id.clone()),
                &retained.observation_id,
            );
            let request = projection.request(
                binding,
                NativeCompilationDepInfo::Retained {
                    candidate_index: retained.candidate_index.clone(),
                    receipt_id: retained.original_receipt_id.clone(),
                    original_compilation_key: retained.compilation_key.clone(),
                    files: retained.observation.files.clone(),
                    environment: retained.observation.environment.clone(),
                },
            );
            let Some(result) = self.project(unit.receipt, &request)? else {
                break;
            };
            let NativeCompilationProjection::Cacheable { compilation, .. } = result else {
                continue;
            };
            if compilation.key != retained.compilation_key {
                continue;
            }
            if let Some(reused) = projection.reuse_from(&bound, &retained.output, &retained.output_digest)? {
                return Ok(reused);
            }
        }

        let compiled = projection.compile(&bound)?;
        let OvenRustcDepInfoOutcome::Observed(observed) = &compiled.observation else {
            return Ok(compiled.bake);
        };
        let Some(observation) = projection.logical_observation(observed)? else {
            return Ok(compiled.bake);
        };
        let observation_id = digest_observation(&observation)?;
        let binding = next_binding(unit, None, &observation_id);
        let request = projection.request(
            binding,
            NativeCompilationDepInfo::Current {
                candidate_index: candidate.index.clone(),
                receipt_id: unit.receipt.identity.clone(),
                original_compilation_key: None,
                files: observation.files.clone(),
                environment: observation.environment.clone(),
            },
        );
        let Some(current) = self.project(unit.receipt, &request)? else {
            return Ok(compiled.bake);
        };
        match current {
            NativeCompilationProjection::Uncacheable { .. } => Ok(compiled.bake),
            NativeCompilationProjection::Cacheable { compilation, .. } => {
                if compilation.candidate.index != candidate.index {
                    return Err(invalid("current observation changed the cold candidate index"));
                }
                publish(
                    &self.store,
                    &projection.output_filename,
                    &compiled.bake.output,
                    unit.receipt,
                    &compilation,
                    &observation,
                    &compiled.bake.output_digest,
                )?;
                Ok(compiled.bake)
            }
        }
    }
}

/// Continue toward reuse only when missing dep-info is the sole reason the complete cold projection is uncacheable.
fn reusable_cold_candidate(projection: NativeCompilationProjection) -> CliResult<Option<NativeCompilationCandidate>> {
    match projection {
        NativeCompilationProjection::Uncacheable { reasons, candidate, .. }
            if reasons.as_slice() == [MISSING_DEP_INFO_REASON] =>
        {
            Ok(candidate)
        }
        NativeCompilationProjection::Uncacheable { .. } => Ok(None),
        NativeCompilationProjection::Cacheable { .. } => {
            Err(invalid("missing dep-info unexpectedly produced a compilation key"))
        }
    }
}

fn oven_rustc_error(error: crate::oven::rustc::OvenRustcError) -> CliError {
    CliError::failure(error.to_string())
}

/// Physical facts paired with the logical projection of one already admitted native/source-unit compilation plan.
struct PreparedNativeCompilation {
    inputs: NativeCompilationInputs,
    execution: NativeCompilationExecution,
    host_effects: NativeCompilationHostEffects,
    path_effects: NativeCompilationPathEffects,
    source_root: PathBuf,
    source_paths: BTreeMap<PathBuf, NativeCompilationContentRow>,
    source_logical_paths: BTreeMap<String, (PathBuf, NativeCompilationContentRow)>,
    private_snapshot_root: Option<PathBuf>,
    logical_source_root: String,
    output_filename: String,
}

struct NativeSearchProjection {
    label: String,
    role: String,
    path: PathBuf,
    members: Vec<String>,
}

impl PreparedNativeCompilation {
    fn new(
        unit: &NativeCompilationUnit<'_>,
        bound: &OvenBoundDirectRustcLibrary<'_>,
        bindings: OvenDirectRustcJecBindings,
    ) -> CliResult<Option<Self>> {
        if unit.prepared.compiler().is_none() {
            return Ok(None);
        }
        if unit.definition.crate_name != unit.prepared.crate_name()
            || unit.definition.edition != unit.prepared.edition()
            || unit.definition.entrypoint.digest != unit.prepared.source_digest()
            || unit.artifacts.intent.target != unit.prepared.target()
            || unit.artifacts.intent.profile != unit.prepared.profile()
        {
            return Err(host_invalid(
                "prepared invocation no longer matches its admitted source-unit and artifact facts",
            ));
        }
        let crate_root = canonical_path(unit.crate_root, "source-unit root")?;
        let source_root = canonical_path(unit.prepared.source_root(), "prepared source root")?;
        let artifact_root = canonical_path(unit.artifact_root, "native artifact root")?;
        if crate_root != source_root || artifact_root != unit.prepared.artifact_root() {
            return Err(host_invalid(
                "prepared invocation is detached from its admitted source or native owner",
            ));
        }
        let private_snapshot_root = unit
            .private_snapshot_root
            .map(|root| canonical_path(root, "private JEC snapshot root"))
            .transpose()?;
        if private_snapshot_root
            .as_ref()
            .is_some_and(|root| !source_root.starts_with(root))
        {
            return Err(host_invalid(
                "prepared source root is detached from its private JEC snapshot owner",
            ));
        }

        let Some(compiler_path) = path_text(unit.prepared.rustc()) else {
            return Ok(None);
        };
        let Some(cwd) = path_text(&source_root) else {
            return Ok(None);
        };
        let Some(output_root) = bound.output().parent().and_then(path_text) else {
            return Ok(None);
        };
        let Some(output_filename) = bound
            .output()
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
        else {
            return Ok(None);
        };

        let mut source_paths = BTreeMap::new();
        let mut source_logical_paths = BTreeMap::new();
        let mut source_files = Vec::new();
        let source = if let Some(members) = &unit.definition.source_members {
            let mut logical = Vec::with_capacity(members.len());
            for member in members {
                let physical = canonical_file(&crate_root.join(&member.path), "source member")?;
                if !physical.starts_with(&crate_root) {
                    return Err(host_invalid("source member escaped its admitted source-unit root"));
                }
                let Some(physical_text) = path_text(&physical) else {
                    return Ok(None);
                };
                let row = NativeCompilationContentRow(member.path.clone(), member.digest.clone());
                if source_paths.insert(physical.clone(), row.clone()).is_some() {
                    return Err(host_invalid(
                        "source-unit members resolve to one physical file more than once",
                    ));
                }
                if source_logical_paths
                    .insert(member.path.clone(), (physical, row.clone()))
                    .is_some()
                {
                    return Err(host_invalid("source-unit members repeat one logical file"));
                }
                source_files.push(NativeCompilationPhysicalRow(
                    member.path.clone(),
                    physical_text,
                    member.digest.clone(),
                ));
                logical.push(row);
            }
            Some(NativeCompilationSource {
                projection: "checked_code_members_v1".to_string(),
                logical_root: bindings.logical_source_root.clone(),
                entrypoint: bindings.logical_entrypoint.clone(),
                files: logical,
            })
        } else {
            None
        };

        let catalog = admitted_native_catalog(unit)?;
        let mut search = unit
            .prepared
            .artifact_plan()
            .dependency_search_paths
            .iter()
            .zip(&bindings.dependency_searches)
            .map(|(path, label)| NativeSearchProjection {
                label: label.clone(),
                role: "dependency".to_string(),
                path: path.clone(),
                members: Vec::new(),
            })
            .chain(
                unit.prepared
                    .artifact_plan()
                    .native_search_paths
                    .iter()
                    .zip(&bindings.native_searches)
                    .map(|(path, label)| NativeSearchProjection {
                        label: label.clone(),
                        role: "native".to_string(),
                        path: path.clone(),
                        members: Vec::new(),
                    }),
            )
            .collect::<Vec<_>>();
        search.sort_by(|left, right| left.label.cmp(&right.label));

        let mut native = Vec::new();
        let mut direct_labels = BTreeMap::<PathBuf, Vec<String>>::new();
        for ((crate_name, path), logical) in unit.prepared.artifact_plan().externs.iter().zip(&bindings.externs) {
            let physical = canonical_file(path, "prepared extern")?;
            let digest = catalog
                .get(&physical)
                .ok_or_else(|| host_invalid("prepared extern has no admitted byte identity"))?
                .clone();
            let Some(physical_text) = path_text(&physical) else {
                return Ok(None);
            };
            direct_labels.entry(physical).or_default().push(logical.clone());
            native.push((
                NativeCompilationNativeRow(
                    logical.clone(),
                    Some(crate_name.clone()),
                    "extern".to_string(),
                    digest.clone(),
                ),
                NativeCompilationPhysicalRow(logical.clone(), physical_text, digest),
            ));
        }

        let mut search_member_labels = BTreeMap::<PathBuf, String>::new();
        for slot in &mut search {
            let search_path = canonical_path(&slot.path, "prepared search path")?;
            for (physical, digest) in &catalog {
                if physical.parent() != Some(search_path.as_path()) {
                    continue;
                }
                if let Some(labels) = direct_labels.get(physical) {
                    slot.members.extend(labels.iter().cloned());
                    continue;
                }
                let logical = if let Some(existing) = search_member_labels.get(physical) {
                    existing.clone()
                } else {
                    let Some(filename) = physical.file_name().and_then(|name| name.to_str()) else {
                        return Ok(None);
                    };
                    if !safe_logical_component(filename) {
                        return Ok(None);
                    }
                    let logical = format!("member/{}/{filename}", slot.label);
                    let Some(physical_text) = path_text(physical) else {
                        return Ok(None);
                    };
                    native.push((
                        NativeCompilationNativeRow(logical.clone(), None, slot.role.clone(), digest.clone()),
                        NativeCompilationPhysicalRow(logical.clone(), physical_text, digest.clone()),
                    ));
                    search_member_labels.insert(physical.clone(), logical.clone());
                    logical
                };
                slot.members.push(logical);
            }
            slot.members.sort();
            slot.members.dedup();
        }
        native.sort_by(|(left, _), (right, _)| left.0.cmp(&right.0));
        let (native_rows, native_files): (Vec<_>, Vec<_>) = native.into_iter().unzip();
        let search_rows = search
            .iter()
            .map(|slot| NativeCompilationSearchRow(slot.label.clone(), slot.role.clone(), slot.members.clone()))
            .collect::<Vec<_>>();
        let mut search_paths = Vec::with_capacity(search.len());
        for slot in &search {
            let Some(path) = path_text(&slot.path) else {
                return Ok(None);
            };
            search_paths.push(NativeCompilationSearchBinding(slot.label.clone(), path));
        }

        let Some(frozen_environment) = unit.prepared.environment() else {
            return Ok(None);
        };
        let environment = frozen_environment
            .iter()
            .map(|(name, value)| {
                NativeCompilationEnvironmentRow(name.clone(), Some(digest_bytes(value.as_os_str().as_encoded_bytes())))
            })
            .collect::<Vec<_>>();
        let compiler = unit.prepared.compiler().map(|evidence| NativeCompilationCompiler {
            binary_digest: evidence.binary_digest().to_string(),
            closure_digest: evidence.closure_digest().to_string(),
            host: evidence.host().to_string(),
            target: evidence.target().to_string(),
        });
        let inputs = NativeCompilationInputs {
            compiler,
            command: Some(NativeCompilationCommand {
                cwd: "unit".to_string(),
                crate_name: unit.prepared.crate_name().to_string(),
                crate_kind: "rlib".to_string(),
                edition: unit.prepared.edition().to_string(),
                arguments: bound.logical_arguments().to_vec(),
            }),
            source,
            native: Some(native_rows.clone()),
            search: Some(search_rows),
            generated: Some(Vec::new()),
            output: Some(NativeCompilationOutput {
                policy: "fixed_rlib_filename_v1".to_string(),
                filename: output_filename.clone(),
            }),
        };
        let invocation_material = serde_json::to_vec(&(
            "incan.oven.native-compilation-execution/1",
            &compiler_path,
            &cwd,
            &source_files,
            &native_files,
            &search_paths,
            &output_root,
            &environment,
        ))
        .map_err(|error| host_invalid(format!("cannot encode physical invocation binding: {error}")))?;
        let invocation_id = digest_bytes(&invocation_material);
        let host_effects = classify_host_effects(&native_rows, &native_files, &search);
        let execution = NativeCompilationExecution {
            invocation_id: invocation_id.clone(),
            compiler: compiler_path,
            cwd,
            source_root: path_text(&source_root).ok_or_else(|| host_invalid("source root is not UTF-8"))?,
            source_files,
            native_files,
            search_paths,
            generated_files: Vec::new(),
            output_root,
            environment,
        };
        let path_effects = match (unit.prepared.compiler(), bound.path_effects_digest()) {
            (Some(compiler), Some(effects_digest)) => NativeCompilationPathEffects::Bound {
                profile: "checked_logical_paths_v1".to_string(),
                compiler_closure_digest: compiler.closure_digest().to_string(),
                invocation_id,
                effects_digest: effects_digest.to_string(),
            },
            _ => NativeCompilationPathEffects::Unknown {
                reasons: vec!["compiler closure or logical path binding is unavailable".to_string()],
            },
        };
        Ok(Some(Self {
            inputs,
            execution,
            host_effects,
            path_effects,
            source_root,
            source_paths,
            source_logical_paths,
            private_snapshot_root,
            logical_source_root: bindings.logical_source_root,
            output_filename,
        }))
    }

    fn request(
        &self,
        binding: NativeCompilationBinding,
        dep_info: NativeCompilationDepInfo,
    ) -> NativeCompilationRequest {
        NativeCompilationRequest::new(
            binding,
            self.execution.clone(),
            self.inputs.clone(),
            NativeCompilationEvidence {
                dep_info,
                host_effects: self.host_effects.clone(),
                path_effects: self.path_effects.clone(),
            },
        )
    }

    /// Rehash caller-owned snapshot members immediately around one compiler activation.
    ///
    /// Engine projection is not an OS sandbox. The request deliberately exposes physical bindings so Incan can
    /// project their admitted identities, but no response may authorize bytes that changed after that projection.
    /// Store and Loaf members were fully verified when their owners and leases were acquired. The private snapshot
    /// is the only remaining mutable carrier, while the bound Rustc path is rehashed inside the executor immediately
    /// before every process launch. A legacy caller without a private owner stays conservative and verifies all rows.
    fn verify_current_inputs(&self) -> CliResult<()> {
        let mut expected = BTreeMap::new();
        for NativeCompilationPhysicalRow(_, physical, digest) in self
            .execution
            .source_files
            .iter()
            .chain(&self.execution.native_files)
            .chain(&self.execution.generated_files)
        {
            let physical = PathBuf::from(physical);
            if self
                .private_snapshot_root
                .as_ref()
                .is_some_and(|root| !physical.starts_with(root))
            {
                continue;
            }
            match expected.insert(physical, digest.as_str()) {
                Some(previous) if previous != digest => {
                    return Err(host_invalid(
                        "one physical compiler input has conflicting byte identities",
                    ));
                }
                _ => {}
            }
        }
        for (physical, expected) in expected {
            let actual = digest_file(&physical)?;
            if actual != expected {
                return Err(host_invalid(format!(
                    "compiler input {} changed after projection: expected {expected}, found {actual}",
                    physical.display()
                )));
            }
        }
        Ok(())
    }

    fn compile(&self, bound: &OvenBoundDirectRustcLibrary<'_>) -> CliResult<OvenDirectRustcJecCompilation> {
        self.verify_current_inputs()?;
        let compiled = bound.compile().map_err(oven_rustc_error);
        self.verify_current_inputs()?;
        compiled
    }

    fn compile_uncacheable(&self, bound: &OvenBoundDirectRustcLibrary<'_>) -> CliResult<OvenDirectRustcJecCompilation> {
        self.verify_current_inputs()?;
        let compiled = bound.compile_uncacheable().map_err(oven_rustc_error);
        self.verify_current_inputs()?;
        compiled
    }

    fn reuse_from(
        &self,
        bound: &OvenBoundDirectRustcLibrary<'_>,
        retained_output: &Path,
        expected_digest: &str,
    ) -> CliResult<Option<OvenDirectRustcBake>> {
        bound
            .reuse_from(retained_output, expected_digest)
            .map_err(oven_rustc_error)
    }

    /// Reject retained observations that cannot match the already frozen source and environment before spending an
    /// Engine exchange. This is only a negative filter: a possible match still needs the Incan policy to recompute
    /// and return the exact retained compilation key.
    fn retained_observation_can_match(&self, observation: &NativeCompilationObservation) -> CliResult<bool> {
        let environment_matches = observation.environment.iter().all(|retained| {
            let current = self
                .execution
                .environment
                .iter()
                .find(|current| current.0 == retained.0);
            match (&retained.1, current) {
                (None, None) => true,
                (Some(expected), Some(NativeCompilationEnvironmentRow(_, Some(actual)))) => expected == actual,
                _ => false,
            }
        });
        if !environment_matches {
            return Ok(false);
        }
        for retained in &observation.files {
            let Some((physical, current)) = self.source_logical_paths.get(&retained.0) else {
                return Ok(false);
            };
            if current.1 != retained.1 || digest_file(physical)? != retained.1 {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn logical_observation(
        &self,
        observed: &OvenRustcDepInfoObservation,
    ) -> CliResult<Option<NativeCompilationObservation>> {
        let mut files = Vec::with_capacity(observed.files.len());
        for path in &observed.files {
            let logical = path
                .strip_prefix(Path::new(&self.logical_source_root))
                .ok()
                .and_then(portable_path_text)
                .or_else(|| (!path.is_absolute()).then(|| portable_path_text(path)).flatten());
            if let Some((physical, row)) = logical
                .as_deref()
                .and_then(|logical| self.source_logical_paths.get(logical))
            {
                if digest_file(physical)? != row.1 {
                    return Ok(None);
                }
                files.push(row.clone());
                continue;
            }
            let candidate = if path.is_absolute() {
                path.clone()
            } else {
                self.source_root.join(path)
            };
            let physical = match fs::canonicalize(&candidate) {
                Ok(path) => path,
                Err(_) => return Ok(None),
            };
            let Some(row) = self.source_paths.get(&physical) else {
                return Ok(None);
            };
            if digest_file(&physical)? != row.1 {
                return Ok(None);
            }
            files.push(row.clone());
        }
        files.sort_by(|left, right| left.0.cmp(&right.0));
        files.dedup_by(|left, right| left.0 == right.0 && left.1 == right.1);
        let environment = observed
            .environment
            .iter()
            .map(|row| NativeCompilationEnvironmentRow(row.name.clone(), row.value_digest.clone()))
            .collect();
        Ok(Some(NativeCompilationObservation { files, environment }))
    }
}

fn logical_bindings(prepared: &OvenPreparedDirectRustcLibrary) -> OvenDirectRustcJecBindings {
    OvenDirectRustcJecBindings {
        logical_source_root: "incan-source".to_string(),
        logical_entrypoint: "src/lib.rs".to_string(),
        externs: (0..prepared.artifact_plan().externs.len())
            .map(|index| format!("extern/{index:04}"))
            .collect(),
        dependency_searches: (0..prepared.artifact_plan().dependency_search_paths.len())
            .map(|index| format!("dependency/{index:04}"))
            .collect(),
        native_searches: (0..prepared.artifact_plan().native_search_paths.len())
            .map(|index| format!("native/{index:04}"))
            .collect(),
    }
}

fn admitted_native_catalog(unit: &NativeCompilationUnit<'_>) -> CliResult<BTreeMap<PathBuf, String>> {
    let mut catalog = BTreeMap::new();
    for (relative, digest) in unit
        .prepared
        .selected_artifacts()
        .declared_artifact_digests()
        .map_err(oven_rustc_error)?
    {
        validate_digest(&digest, "admitted native artifact")?;
        let physical = canonical_file(
            &unit.prepared.artifact_root().join(relative),
            "admitted native artifact",
        )?;
        insert_catalog_member(&mut catalog, physical, digest)?;
    }
    for library in unit.dependencies {
        validate_digest(&library.digest, "caller-owned native artifact")?;
        let physical = canonical_file(&library.output, "caller-owned native artifact")?;
        insert_catalog_member(&mut catalog, physical, library.digest.clone())?;
    }
    Ok(catalog)
}

fn insert_catalog_member(catalog: &mut BTreeMap<PathBuf, String>, path: PathBuf, digest: String) -> CliResult<()> {
    if let Some(existing) = catalog.insert(path, digest.clone())
        && existing != digest
    {
        return Err(host_invalid(
            "one admitted native path carries conflicting byte identities",
        ));
    }
    Ok(())
}

fn classify_host_effects(
    native: &[NativeCompilationNativeRow],
    physical: &[NativeCompilationPhysicalRow],
    search: &[NativeSearchProjection],
) -> NativeCompilationHostEffects {
    let dependency_members = search
        .iter()
        .filter(|slot| slot.role == "dependency")
        .flat_map(|slot| slot.members.iter().map(String::as_str))
        .collect::<BTreeSet<_>>();
    let reasons = native
        .iter()
        .zip(physical)
        .filter(|(row, physical)| {
            let extension = Path::new(&physical.1).extension().and_then(|value| value.to_str());
            let dynamic = matches!(extension, Some("dylib" | "so" | "dll"));
            dynamic && (row.1.is_some() || dependency_members.contains(row.0.as_str()))
        })
        .map(|(row, _)| format!("host-executable native member {} has no bounded effect trace", row.0))
        .collect::<Vec<_>>();
    if reasons.is_empty() {
        NativeCompilationHostEffects::None
    } else {
        NativeCompilationHostEffects::Unknown { reasons }
    }
}

fn next_binding(
    unit: &NativeCompilationUnit<'_>,
    original_receipt_id: Option<String>,
    observation_id: &str,
) -> NativeCompilationBinding {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    NativeCompilationBinding {
        request_id: format!("native-compilation:{sequence}"),
        current_receipt_id: unit.receipt.identity.clone(),
        original_receipt_id,
        unit_owner_id: unit.unit_owner_id.to_string(),
        observation_id: observation_id.to_string(),
    }
}

fn digest_observation(observation: &NativeCompilationObservation) -> CliResult<String> {
    let material = serde_json::to_vec(&(
        "incan.oven.native-compilation-observation/1",
        &observation.files,
        &observation.environment,
    ))
    .map_err(|error| host_invalid(format!("cannot encode native-compilation observation: {error}")))?;
    Ok(digest_bytes(&material))
}

fn canonical_path(path: &Path, label: &str) -> CliResult<PathBuf> {
    fs::canonicalize(path).map_err(|error| host_invalid(format!("cannot resolve {label}: {error}")))
}

fn canonical_file(path: &Path, label: &str) -> CliResult<PathBuf> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| host_invalid(format!("cannot inspect {label}: {error}")))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(host_invalid(format!("{label} is not a non-symlink regular file")));
    }
    canonical_path(path, label)
}

fn path_text(path: &Path) -> Option<String> {
    path.to_str().map(str::to_string)
}

fn portable_path_text(path: &Path) -> Option<String> {
    let mut values = Vec::new();
    for component in path.components() {
        let std::path::Component::Normal(value) = component else {
            return None;
        };
        values.push(value.to_str()?.to_string());
    }
    (!values.is_empty()).then(|| values.join("/"))
}

fn safe_logical_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains(':')
}

fn host_invalid(message: impl Into<String>) -> CliError {
    CliError::failure(format!("invalid native-compilation host binding: {}", message.into()))
}

/// Logical observations retained from Rustc without persisting source or environment values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NativeCompilationObservation {
    pub(super) files: Vec<NativeCompilationContentRow>,
    pub(super) environment: Vec<NativeCompilationEnvironmentRow>,
}

/// A Store-owned reusable output and the compiler observations retained with its active lease.
pub(super) struct RetainedNativeCompilation {
    pub(super) original_receipt_id: String,
    pub(super) observation_id: String,
    pub(super) candidate_index: String,
    pub(super) compilation_key: String,
    pub(super) observation: NativeCompilationObservation,
    pub(super) output_digest: String,
    pub(super) output: PathBuf,
    cache_identity: String,
    _owner: OvenStoreExecutionPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeCompilationRecord {
    schema_version: u32,
    original_receipt_id: String,
    candidate_index: String,
    compilation_key: String,
    files: Vec<(String, String)>,
    environment: Vec<(String, Option<String>)>,
    output_digest: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RefusedResponse {
    schema: String,
    status: String,
    fields: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncacheableResponse {
    schema: String,
    status: String,
    binding: NativeCompilationBinding,
    reasons: Vec<String>,
    candidate_index: Option<String>,
    index_material: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheableResponse {
    schema: String,
    status: String,
    binding: NativeCompilationBinding,
    candidate_index: String,
    index_material: Value,
    compilation_key: String,
    key_material: Value,
}

/// Admit one exact Incan projection response and independently verify all reusable identities it carries.
pub(super) fn admit_response(
    response: &[u8],
    expected_binding: &NativeCompilationBinding,
) -> CliResult<NativeCompilationProjection> {
    validate_binding(expected_binding, expected_binding)?;
    let document: Value =
        serde_json::from_slice(response).map_err(|error| invalid(format!("response is not valid JSON: {error}")))?;
    let object = document
        .as_object()
        .ok_or_else(|| invalid("response is not a JSON object"))?;
    let schema = object.get("schema").and_then(Value::as_str);
    let status = object.get("status").and_then(Value::as_str);
    if schema != Some(SCHEMA) {
        return Err(invalid("response schema is unsupported"));
    }
    match status {
        Some("refused") => {
            let response: RefusedResponse = serde_json::from_value(document)
                .map_err(|error| invalid(format!("refusal shape is invalid: {error}")))?;
            require_header(&response.schema, &response.status, "refused")?;
            if response.fields.iter().any(|field| field.is_empty()) {
                return Err(invalid("refusal contains an empty field coordinate"));
            }
            let location = if response.fields.is_empty() {
                "request".to_string()
            } else {
                response.fields.join(".")
            };
            Err(CliError::failure(format!(
                "Incan native-compilation projection refused at {location}"
            )))
        }
        Some("uncacheable") => {
            let candidate_present = object.contains_key("candidate_index");
            let material_present = object.contains_key("index_material");
            if candidate_present != material_present {
                return Err(invalid("uncacheable candidate identity and material are not paired"));
            }
            let response: UncacheableResponse = serde_json::from_value(document)
                .map_err(|error| invalid(format!("uncacheable shape is invalid: {error}")))?;
            require_header(&response.schema, &response.status, "uncacheable")?;
            validate_binding(&response.binding, expected_binding)?;
            validate_reasons(&response.reasons)?;
            let candidate = match (response.candidate_index, response.index_material) {
                (Some(index), Some(material)) if candidate_present => Some(validate_candidate(index, material)?),
                (None, None) if !candidate_present => None,
                _ => return Err(invalid("uncacheable candidate identity or material is null")),
            };
            Ok(NativeCompilationProjection::Uncacheable {
                binding: response.binding,
                reasons: response.reasons,
                candidate,
            })
        }
        Some("cacheable") => {
            let response: CacheableResponse = serde_json::from_value(document)
                .map_err(|error| invalid(format!("cacheable shape is invalid: {error}")))?;
            require_header(&response.schema, &response.status, "cacheable")?;
            validate_binding(&response.binding, expected_binding)?;
            let candidate = validate_candidate(response.candidate_index, response.index_material)?;
            validate_digest(&response.compilation_key, "compilation key")?;
            validate_material(&response.key_material, KEY_DOMAIN, 5, "key material")?;
            let key_parts = response
                .key_material
                .as_array()
                .ok_or_else(|| invalid("key material is not an array"))?;
            if key_parts.get(1) != Some(&candidate.material) {
                return Err(invalid("key material does not embed the admitted candidate material"));
            }
            if digest_material(&response.key_material)? != response.compilation_key {
                return Err(invalid("compilation key does not match its material"));
            }
            Ok(NativeCompilationProjection::Cacheable {
                binding: response.binding,
                compilation: NativeCompilationKey {
                    candidate,
                    key: response.compilation_key,
                    material: response.key_material,
                },
            })
        }
        Some(other) => Err(invalid(format!("response status `{other}` is unsupported"))),
        None => Err(invalid("response has no text status")),
    }
}

/// Select one bounded immutable Store partition and retain every admitted owner through candidate evaluation.
///
/// `None` closes reuse and publication for a saturated or structurally ambiguous partition. An ordinary empty
/// partition remains `Some([])` so its first successful compilation may be published.
pub(super) fn read_retained_candidates(
    store: &OvenStore,
    expected_candidate: &str,
    output_filename: &str,
) -> CliResult<Option<Vec<RetainedNativeCompilation>>> {
    validate_digest(expected_candidate, "expected candidate index")?;
    let output_relative_path = cache_output_relative_path(output_filename)?;
    let domain = native_compilation_domain(expected_candidate)?;
    let owners = match store.select_payloads_matching_for_execution(|manifest| {
        manifest.kind == OvenArtifactKind::NativeCompilationOutput && manifest.domain == domain
    }) {
        Ok(owners) => owners,
        Err(_) => return Ok(None),
    };
    if owners.len() > MAX_RETAINED_KEYS_PER_CANDIDATE {
        return Ok(None);
    }
    let mut retained = Vec::new();
    for owner in owners {
        if let Some(candidate) = read_retained(owner, expected_candidate, None, &output_relative_path)? {
            retained.push(candidate);
        }
    }
    retained.sort_by(|left, right| left.cache_identity.cmp(&right.cache_identity));
    let mut outputs_by_key = BTreeMap::new();
    for candidate in &retained {
        if let Some(existing) = outputs_by_key.insert(&candidate.compilation_key, &candidate.output_digest)
            && existing != &candidate.output_digest
        {
            return Err(invalid(
                "one native-compilation key has multiple immutable Store output identities",
            ));
        }
    }
    let observation_shapes = retained
        .iter()
        .map(|candidate| {
            (
                candidate
                    .observation
                    .files
                    .iter()
                    .map(|row| row.0.as_str())
                    .collect::<Vec<_>>(),
                candidate
                    .observation
                    .environment
                    .iter()
                    .map(|row| row.0.as_str())
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<BTreeSet<_>>();
    if observation_shapes.len() > 1 {
        return Ok(None);
    }
    Ok(Some(retained))
}

/// Admit one Store payload, its original publisher receipt and its materialized output under the retained lease.
fn read_retained(
    owner: OvenStoreExecutionPayload,
    expected_candidate: &str,
    expected_key: Option<&str>,
    output_relative_path: &str,
) -> CliResult<Option<RetainedNativeCompilation>> {
    validate_digest(expected_candidate, "expected candidate index")?;
    if let Some(expected_key) = expected_key {
        validate_digest(expected_key, "expected compilation key")?;
    }
    if owner.manifest.kind != OvenArtifactKind::NativeCompilationOutput
        || owner.manifest.domain != native_compilation_domain(expected_candidate)?
        || owner.payload.len() as u64 > RECORD_LIMIT
    {
        return Ok(None);
    }
    let Ok(record) = serde_json::from_slice::<NativeCompilationRecord>(&owner.payload) else {
        return Ok(None);
    };
    if validate_record(&record).is_err()
        || record.original_receipt_id != owner.manifest.receipt_identity
        || record.candidate_index != expected_candidate
        || expected_key.is_some_and(|expected| record.compilation_key != expected)
    {
        return Ok(None);
    }
    let [materialized] = owner.manifest.materialized_files.as_slice() else {
        return Ok(None);
    };
    if materialized.relative_path != output_relative_path || materialized.digest != record.output_digest {
        return Ok(None);
    }
    if owner.verify_materialized_files().is_err() {
        return Ok(None);
    }
    let output = owner.artifact_root.join(output_relative_path);
    let observation_id = digest_bytes(&owner.payload);
    let cache_identity = owner.manifest.identity.clone();
    Ok(Some(RetainedNativeCompilation {
        original_receipt_id: record.original_receipt_id,
        observation_id,
        candidate_index: record.candidate_index,
        compilation_key: record.compilation_key,
        observation: NativeCompilationObservation {
            files: record
                .files
                .into_iter()
                .map(|(logical, digest)| NativeCompilationContentRow(logical, digest))
                .collect(),
            environment: record
                .environment
                .into_iter()
                .map(|(name, digest)| NativeCompilationEnvironmentRow(name, digest))
                .collect(),
        },
        output_digest: record.output_digest,
        output,
        cache_identity,
        _owner: owner,
    }))
}

fn digest_file(path: &Path) -> CliResult<String> {
    let mut file =
        fs::File::open(path).map_err(|error| invalid(format!("cannot read retained native output: {error}")))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| invalid(format!("cannot read retained native output: {error}")))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

/// Derive the Store compatibility domain without placing package or receipt provenance in it.
fn native_compilation_domain(candidate_index: &str) -> CliResult<String> {
    validate_digest(candidate_index, "candidate index")?;
    let hex = candidate_index
        .strip_prefix("sha256:")
        .ok_or_else(|| invalid("candidate index lost its digest prefix"))?;
    Ok(format!("native-compilation.{hex}"))
}

fn cache_output_relative_path(output_filename: &str) -> CliResult<String> {
    if output_filename.is_empty()
        || Path::new(output_filename).file_name().and_then(|name| name.to_str()) != Some(output_filename)
    {
        return Err(invalid("native output filename is not one safe path component"));
    }
    Ok(format!("{NATIVE_CACHE_OUTPUT_DIRECTORY}/{output_filename}"))
}

/// Publish one compiler-produced result through the immutable Store and immediately reacquire its owner.
pub(super) fn publish(
    store: &OvenStore,
    output_filename: &str,
    compiled_output: &Path,
    receipt: &crate::oven::OvenReceipt,
    compilation: &NativeCompilationKey,
    observation: &NativeCompilationObservation,
    output_digest: &str,
) -> CliResult<Option<RetainedNativeCompilation>> {
    validate_digest(&compilation.candidate.index, "candidate index")?;
    validate_digest(&compilation.key, "compilation key")?;
    validate_digest(output_digest, "compiled output digest")?;
    let output_relative_path = cache_output_relative_path(output_filename)?;
    if digest_file(compiled_output)? != output_digest {
        return Err(invalid("compiled native output changed before cache publication"));
    }
    let record = NativeCompilationRecord {
        schema_version: RECORD_SCHEMA_VERSION,
        original_receipt_id: receipt.identity.clone(),
        candidate_index: compilation.candidate.index.clone(),
        compilation_key: compilation.key.clone(),
        files: observation
            .files
            .iter()
            .map(|row| (row.0.clone(), row.1.clone()))
            .collect(),
        environment: observation
            .environment
            .iter()
            .map(|row| (row.0.clone(), row.1.clone()))
            .collect(),
        output_digest: output_digest.to_string(),
    };
    validate_record(&record)?;
    let payload = serde_json::to_vec(&record)
        .map_err(|error| invalid(format!("cannot encode native-compilation record: {error}")))?;
    if payload.len() as u64 > RECORD_LIMIT {
        return Ok(None);
    }
    let Some(existing) = read_retained_candidates(store, &compilation.candidate.index, output_filename)? else {
        return Ok(None);
    };
    let existing_count = existing.len();
    for retained in existing {
        if retained.compilation_key != compilation.key {
            continue;
        }
        if retained.output_digest != output_digest {
            return Err(invalid(
                "the same native-compilation key produced different immutable output bytes",
            ));
        }
        return Ok(Some(retained));
    }
    if existing_count >= MAX_RETAINED_KEYS_PER_CANDIDATE {
        return Ok(None);
    }
    let manifest = match store.publish(&OvenArtifactPublishRequest {
        receipt: receipt.clone(),
        domain: native_compilation_domain(&compilation.candidate.index)?,
        kind: OvenArtifactKind::NativeCompilationOutput,
        payload,
        materialized_files: vec![OvenArtifactMaterializedFile {
            source_path: compiled_output.to_path_buf(),
            relative_path: output_relative_path.clone(),
        }],
    }) {
        Ok(manifest) => manifest,
        Err(_) => return Ok(None),
    };
    let owner = match store.select_payloads_for_execution(std::slice::from_ref(&manifest.identity)) {
        Ok(mut owners) => match owners.pop() {
            Some(owner) => owner,
            None => return Ok(None),
        },
        Err(_) => return Ok(None),
    };
    let Some(retained) = read_retained(
        owner,
        &compilation.candidate.index,
        Some(&compilation.key),
        &output_relative_path,
    )?
    else {
        return Ok(None);
    };
    if retained.output_digest != output_digest {
        return Err(invalid(
            "the same native-compilation key produced different output bytes",
        ));
    }
    if let Some(existing) = read_retained_candidates(store, &compilation.candidate.index, output_filename)? {
        for existing in existing {
            if existing.compilation_key == compilation.key && existing.output_digest != output_digest {
                return Err(invalid(
                    "the same native-compilation key produced different immutable output bytes",
                ));
            }
        }
    }
    Ok(Some(retained))
}

fn validate_record(record: &NativeCompilationRecord) -> CliResult<()> {
    if record.schema_version != RECORD_SCHEMA_VERSION || record.original_receipt_id.is_empty() {
        return Err(invalid(
            "native-compilation record has an unsupported or incomplete header",
        ));
    }
    validate_digest(&record.candidate_index, "record candidate index")?;
    validate_digest(&record.compilation_key, "record compilation key")?;
    validate_digest(&record.output_digest, "record output digest")?;
    validate_sorted_rows(&record.files, "record source observations")?;
    let mut previous = None;
    for (name, value) in &record.environment {
        if name.is_empty() || previous.is_some_and(|previous| previous >= name.as_str()) {
            return Err(invalid("record environment observations are not sorted and unique"));
        }
        if let Some(value) = value {
            validate_digest(value, "record environment digest")?;
        }
        previous = Some(name.as_str());
    }
    Ok(())
}

fn validate_sorted_rows(rows: &[(String, String)], label: &str) -> CliResult<()> {
    if rows.is_empty() {
        return Err(invalid(format!("{label} are empty")));
    }
    let mut previous = None;
    for (name, digest) in rows {
        if name.is_empty() || previous.is_some_and(|previous| previous >= name.as_str()) {
            return Err(invalid(format!("{label} are not sorted and unique")));
        }
        validate_digest(digest, label)?;
        previous = Some(name.as_str());
    }
    Ok(())
}

fn require_header(schema: &str, status: &str, expected_status: &str) -> CliResult<()> {
    if schema != SCHEMA || status != expected_status {
        return Err(invalid("decoded response header changed during admission"));
    }
    Ok(())
}

fn validate_binding(actual: &NativeCompilationBinding, expected: &NativeCompilationBinding) -> CliResult<()> {
    let required = [
        actual.request_id.as_str(),
        actual.current_receipt_id.as_str(),
        actual.unit_owner_id.as_str(),
        actual.observation_id.as_str(),
    ];
    if required.into_iter().any(str::is_empty)
        || actual.original_receipt_id.as_deref().is_some_and(str::is_empty)
        || actual != expected
    {
        return Err(invalid(
            "response binding does not match the host-issued command binding",
        ));
    }
    Ok(())
}

fn validate_reasons(reasons: &[String]) -> CliResult<()> {
    if reasons.is_empty() || reasons.iter().any(String::is_empty) {
        return Err(invalid("uncacheable response requires nonempty reasons"));
    }
    let unique = reasons.iter().collect::<BTreeSet<_>>();
    if unique.len() != reasons.len() {
        return Err(invalid("uncacheable response repeats a reason"));
    }
    Ok(())
}

fn validate_candidate(index: String, material: Value) -> CliResult<NativeCompilationCandidate> {
    validate_digest(&index, "candidate index")?;
    validate_material(&material, INDEX_DOMAIN, 8, "candidate material")?;
    if digest_material(&material)? != index {
        return Err(invalid("candidate index does not match its material"));
    }
    Ok(NativeCompilationCandidate { index, material })
}

fn validate_material(value: &Value, domain: &str, width: usize, label: &str) -> CliResult<()> {
    let values = value
        .as_array()
        .ok_or_else(|| invalid(format!("{label} is not an array")))?;
    if values.len() != width || values.first().and_then(Value::as_str) != Some(domain) {
        return Err(invalid(format!("{label} has an incompatible domain or shape")));
    }
    if !uses_canonical_scalar_arrays(value) {
        return Err(invalid(format!("{label} contains an unordered or unsupported value")));
    }
    Ok(())
}

fn uses_canonical_scalar_arrays(value: &Value) -> bool {
    match value {
        Value::Null | Value::String(_) => true,
        Value::Array(values) => values.iter().all(uses_canonical_scalar_arrays),
        Value::Bool(_) | Value::Number(_) | Value::Object(_) => false,
    }
}

fn validate_digest(value: &str, label: &str) -> CliResult<()> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(invalid(format!("{label} is not a SHA-256 identity")));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid(format!("{label} is not lowercase hexadecimal")));
    }
    Ok(())
}

fn digest_material(value: &Value) -> CliResult<String> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| invalid(format!("cannot encode canonical projection material: {error}")))?;
    Ok(digest_bytes(&bytes))
}

fn invalid(message: impl Into<String>) -> CliError {
    CliError::failure(format!("invalid Incan native-compilation response: {}", message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn hash(character: char) -> String {
        format!("sha256:{}", character.to_string().repeat(64))
    }

    fn binding() -> NativeCompilationBinding {
        NativeCompilationBinding {
            request_id: "request-v0.1".into(),
            current_receipt_id: "provider@0.1#receipt".into(),
            original_receipt_id: None,
            unit_owner_id: "owner-v0.1".into(),
            observation_id: "observation-v0.1".into(),
        }
    }

    fn request_with_invocation_id(invocation_id: String) -> NativeCompilationRequest {
        NativeCompilationRequest::new(
            binding(),
            NativeCompilationExecution {
                invocation_id,
                compiler: "rustc".into(),
                cwd: "cwd".into(),
                source_root: "source".into(),
                source_files: Vec::new(),
                native_files: Vec::new(),
                search_paths: Vec::new(),
                generated_files: Vec::new(),
                output_root: "output".into(),
                environment: Vec::new(),
            },
            NativeCompilationInputs {
                compiler: None,
                command: None,
                source: None,
                native: None,
                search: None,
                generated: None,
                output: None,
            },
            NativeCompilationEvidence {
                dep_info: NativeCompilationDepInfo::Missing,
                host_effects: NativeCompilationHostEffects::None,
                path_effects: NativeCompilationPathEffects::Unknown {
                    reasons: vec!["test".into()],
                },
            },
        )
    }

    fn index_material() -> Value {
        let arguments = serde_json::json!([
            ["option", "crate-name", "provider"],
            ["option", "crate-type", "rlib"],
            ["option", "edition", "2021"],
            ["option", "target", "target"],
            ["switch", "dep-info-stdout"],
            ["switch", "json-diagnostics"],
            ["remap-source", "incan-source"],
            ["remap-toolchain", "incan-toolchain"],
            ["remap-search", "dependencies"],
            ["search", "dependencies"],
            ["extern", "stdlib"],
            ["source", "src/lib.rs"],
            ["output", "native"]
        ]);
        serde_json::json!([
            INDEX_DOMAIN,
            [hash('1'), hash('2'), "host", "target"],
            ["unit", "provider", "rlib", "2021", arguments],
            [
                "checked_code_members_v1",
                "incan-source",
                "src/lib.rs",
                [["src/defs.rs", hash('4')], ["src/lib.rs", hash('5')]]
            ],
            [["stdlib", "incan_stdlib", "target_library", hash('6')]],
            [["dependencies", "dependency", ["stdlib"]]],
            [],
            ["fixed_rlib_filename_v1", "libprovider.rlib"]
        ])
    }

    fn key_material(index: &Value) -> Value {
        serde_json::json!([KEY_DOMAIN, index, [], ["none"], ["checked_logical_paths_v1", hash('d')]])
    }

    fn cache_fixture(root: &Path) -> Result<(OvenStore, crate::oven::OvenReceipt), Box<dyn std::error::Error>> {
        let source = root.join("receipt.rs");
        fs::write(&source, "pub fn receipt_source() {}\n")?;
        let receipt = crate::oven::receipt_generated_project(
            &crate::oven::OvenGeneratedProjectRequest::new(
                root,
                "native_cache_fixture",
                "0.1.0",
                "test-target",
                "test-toolchain",
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", source),
        )?;
        let store = OvenStore::new(
            root.join("store"),
            crate::oven::store::OvenStoreLimits::new(16 * 1024 * 1024, 16 * 1024 * 1024, 16 * 1024 * 1024),
        );
        Ok((store, receipt))
    }

    #[test]
    fn maps_physical_and_remapped_dep_info_to_the_same_logical_members() -> TestResult {
        let root = tempfile::tempdir()?;
        fs::create_dir(root.path().join("src"))?;
        let source = root.path().join("src/lib.rs");
        fs::write(&source, "pub fn marker() {}\n")?;
        let source = source.canonicalize()?;
        let source_digest = digest_file(&source)?;
        let row = NativeCompilationContentRow("src/lib.rs".to_string(), source_digest.clone());
        let projection = PreparedNativeCompilation {
            inputs: NativeCompilationInputs {
                compiler: None,
                command: None,
                source: None,
                native: None,
                search: None,
                generated: None,
                output: None,
            },
            execution: NativeCompilationExecution {
                invocation_id: "invocation".to_string(),
                compiler: "rustc".to_string(),
                cwd: "cwd".to_string(),
                source_root: "source".to_string(),
                source_files: Vec::new(),
                native_files: Vec::new(),
                search_paths: Vec::new(),
                generated_files: Vec::new(),
                output_root: "output".to_string(),
                environment: vec![NativeCompilationEnvironmentRow("BUILD_ID".to_string(), Some(hash('a')))],
            },
            host_effects: NativeCompilationHostEffects::None,
            path_effects: NativeCompilationPathEffects::Unknown {
                reasons: vec!["test".to_string()],
            },
            source_root: root.path().canonicalize()?,
            source_paths: BTreeMap::from([(source.clone(), row.clone())]),
            source_logical_paths: BTreeMap::from([("src/lib.rs".to_string(), (source.clone(), row.clone()))]),
            private_snapshot_root: None,
            logical_source_root: "incan-source".to_string(),
            output_filename: "libfixture.rlib".to_string(),
        };
        let environment = vec![crate::oven::rustc::OvenRustcObservedEnvironment {
            name: "BUILD_ID".to_string(),
            value_digest: Some(hash('a')),
        }];
        let physical = projection
            .logical_observation(&OvenRustcDepInfoObservation {
                files: vec![source],
                environment: environment.clone(),
            })?
            .ok_or("physical observation did not map")?;
        let remapped = projection
            .logical_observation(&OvenRustcDepInfoObservation {
                files: vec![PathBuf::from("incan-source/src/lib.rs")],
                environment,
            })?
            .ok_or("remapped observation did not map")?;
        assert_eq!(physical, remapped);
        assert_eq!(physical.files, [row]);
        assert_eq!(physical.environment[0].0, "BUILD_ID");
        let expected_environment_digest = hash('a');
        assert_eq!(
            physical.environment[0].1.as_deref(),
            Some(expected_environment_digest.as_str())
        );
        assert!(projection.retained_observation_can_match(&physical)?);
        assert!(
            !projection.retained_observation_can_match(&NativeCompilationObservation {
                files: vec![NativeCompilationContentRow("src/lib.rs".to_string(), hash('b'))],
                environment: physical.environment.clone(),
            })?
        );
        assert!(
            !projection.retained_observation_can_match(&NativeCompilationObservation {
                files: physical.files.clone(),
                environment: vec![NativeCompilationEnvironmentRow("BUILD_ID".to_string(), Some(hash('b')))],
            })?
        );
        assert!(
            projection.retained_observation_can_match(&NativeCompilationObservation {
                files: physical.files.clone(),
                environment: vec![NativeCompilationEnvironmentRow("UNSET".to_string(), None)],
            })?
        );
        fs::write(root.path().join("src/lib.rs"), "pub fn changed_after_prepare() {}\n")?;
        assert!(!projection.retained_observation_can_match(&physical)?);
        Ok(())
    }

    #[test]
    fn compiler_activation_guards_only_private_snapshot_members() -> TestResult {
        let root = tempfile::tempdir()?;
        let snapshot = root.path().join("snapshot");
        let store = root.path().join("store");
        fs::create_dir_all(&snapshot)?;
        fs::create_dir_all(&store)?;
        let source = snapshot.join("lib.rs");
        let sibling = snapshot.join("sibling.rs");
        let native = snapshot.join("libdependency.rlib");
        let leased_native = store.join("libfoundation.rlib");
        fs::write(&source, "mod sibling;\npub use sibling::answer;\n")?;
        fs::write(&sibling, "pub fn answer() -> u32 { 42 }\n")?;
        fs::write(&native, b"native dependency bytes")?;
        fs::write(&leased_native, b"leased Store bytes")?;
        let source = source.canonicalize()?;
        let sibling = sibling.canonicalize()?;
        let native = native.canonicalize()?;
        let leased_native = leased_native.canonicalize()?;
        let source_digest = digest_file(&source)?;
        let sibling_digest = digest_file(&sibling)?;
        let native_digest = digest_file(&native)?;
        let leased_native_digest = digest_file(&leased_native)?;
        let projection = PreparedNativeCompilation {
            inputs: NativeCompilationInputs {
                compiler: None,
                command: None,
                source: None,
                native: None,
                search: None,
                generated: None,
                output: None,
            },
            execution: NativeCompilationExecution {
                invocation_id: "invocation".to_string(),
                compiler: "rustc".to_string(),
                cwd: root.path().display().to_string(),
                source_root: root.path().display().to_string(),
                source_files: vec![
                    NativeCompilationPhysicalRow("src/lib.rs".to_string(), source.display().to_string(), source_digest),
                    NativeCompilationPhysicalRow(
                        "src/sibling.rs".to_string(),
                        sibling.display().to_string(),
                        sibling_digest.clone(),
                    ),
                ],
                native_files: vec![
                    NativeCompilationPhysicalRow(
                        "extern/dependency".to_string(),
                        native.display().to_string(),
                        native_digest.clone(),
                    ),
                    NativeCompilationPhysicalRow(
                        "extern/foundation".to_string(),
                        leased_native.display().to_string(),
                        leased_native_digest,
                    ),
                ],
                search_paths: Vec::new(),
                generated_files: Vec::new(),
                output_root: root.path().display().to_string(),
                environment: Vec::new(),
            },
            host_effects: NativeCompilationHostEffects::None,
            path_effects: NativeCompilationPathEffects::Unknown {
                reasons: vec!["test".to_string()],
            },
            source_root: snapshot.canonicalize()?,
            source_paths: BTreeMap::new(),
            source_logical_paths: BTreeMap::new(),
            private_snapshot_root: Some(snapshot.canonicalize()?),
            logical_source_root: "incan-source".to_string(),
            output_filename: "libfixture.rlib".to_string(),
        };

        projection.verify_current_inputs()?;
        fs::write(&leased_native, b"changed outside the private snapshot")?;
        projection.verify_current_inputs()?;
        fs::write(&sibling, "pub fn answer() -> u32 { 7 }\n")?;
        assert!(projection.verify_current_inputs().is_err());
        fs::write(&sibling, "pub fn answer() -> u32 { 42 }\n")?;
        assert_eq!(digest_file(&sibling)?, sibling_digest);
        projection.verify_current_inputs()?;

        fs::write(&native, b"mutated native dependency bytes")?;
        assert!(projection.verify_current_inputs().is_err());
        fs::write(&native, b"native dependency bytes")?;
        assert_eq!(digest_file(&native)?, native_digest);
        projection.verify_current_inputs()?;
        Ok(())
    }

    #[test]
    fn retained_output_requires_an_immutable_store_owner_and_receipt() -> TestResult {
        let root = tempfile::tempdir()?;
        let (store, receipt) = cache_fixture(root.path())?;
        let compiled = root.path().join("fresh.rlib");
        fs::write(&compiled, b"compiled bytes")?;
        let output_digest = digest_file(&compiled)?;
        let candidate_index = hash('a');
        let compilation_key = hash('b');
        let compilation = NativeCompilationKey {
            candidate: NativeCompilationCandidate {
                index: candidate_index.clone(),
                material: Value::Null,
            },
            key: compilation_key.clone(),
            material: Value::Null,
        };
        let observation = NativeCompilationObservation {
            files: vec![NativeCompilationContentRow("src/lib.rs".to_string(), hash('c'))],
            environment: vec![NativeCompilationEnvironmentRow("BUILD_ID".to_string(), None)],
        };
        let published = publish(
            &store,
            "libunit.rlib",
            &compiled,
            &receipt,
            &compilation,
            &observation,
            &output_digest,
        )?
        .ok_or("cache publication was unexpectedly skipped")?;
        assert_eq!(published.compilation_key, compilation_key);
        assert_eq!(published.original_receipt_id, receipt.identity);
        assert_eq!(
            read_retained_candidates(&store, &candidate_index, "libunit.rlib")?
                .ok_or("cache partition was unexpectedly disabled")?
                .len(),
            1
        );
        assert_eq!(digest_file(&published.output)?, output_digest);
        assert!(fs::write(&published.output, b"corrupted bytes").is_err());
        Ok(())
    }

    #[test]
    fn cache_size_and_capacity_limits_do_not_fail_a_completed_compilation() -> TestResult {
        let root = tempfile::tempdir()?;
        let (store, receipt) = cache_fixture(root.path())?;
        let compiled = root.path().join("fresh.rlib");
        fs::write(&compiled, b"compiled bytes")?;
        let output_digest = digest_file(&compiled)?;
        let compilation = NativeCompilationKey {
            candidate: NativeCompilationCandidate {
                index: hash('a'),
                material: Value::Null,
            },
            key: hash('b'),
            material: Value::Null,
        };
        let oversized = NativeCompilationObservation {
            files: vec![NativeCompilationContentRow("src/lib.rs".to_string(), hash('c'))],
            environment: vec![NativeCompilationEnvironmentRow("x".repeat(RECORD_LIMIT as usize), None)],
        };
        assert!(
            publish(
                &store,
                "libunit.rlib",
                &compiled,
                &receipt,
                &compilation,
                &oversized,
                &output_digest,
            )?
            .is_none()
        );

        let capacity_blocked = OvenStore::new(
            root.path().join("capacity-blocked"),
            crate::oven::store::OvenStoreLimits::new(1, 1, 1),
        );
        let ordinary = NativeCompilationObservation {
            files: vec![NativeCompilationContentRow("src/lib.rs".to_string(), hash('c'))],
            environment: Vec::new(),
        };
        assert!(
            publish(
                &capacity_blocked,
                "libunit.rlib",
                &compiled,
                &receipt,
                &compilation,
                &ordinary,
                &output_digest,
            )?
            .is_none()
        );
        Ok(())
    }

    #[test]
    fn ambiguous_retained_observation_shapes_disable_reuse() -> TestResult {
        let root = tempfile::tempdir()?;
        let (store, receipt) = cache_fixture(root.path())?;
        let candidate_index = hash('a');
        for (key, bytes, files) in [
            (
                hash('b'),
                b"first output".as_slice(),
                vec![NativeCompilationContentRow("src/lib.rs".to_string(), hash('1'))],
            ),
            (
                hash('c'),
                b"second output".as_slice(),
                vec![
                    NativeCompilationContentRow("src/extra.rs".to_string(), hash('2')),
                    NativeCompilationContentRow("src/lib.rs".to_string(), hash('1')),
                ],
            ),
        ] {
            let compiled = root.path().join(format!("{key}.rlib"));
            fs::write(&compiled, bytes)?;
            let output_digest = digest_file(&compiled)?;
            let compilation = NativeCompilationKey {
                candidate: NativeCompilationCandidate {
                    index: candidate_index.clone(),
                    material: Value::Null,
                },
                key,
                material: Value::Null,
            };
            publish(
                &store,
                "libunit.rlib",
                &compiled,
                &receipt,
                &compilation,
                &NativeCompilationObservation {
                    files,
                    environment: Vec::new(),
                },
                &output_digest,
            )?;
        }
        assert!(read_retained_candidates(&store, &candidate_index, "libunit.rlib")?.is_none());
        Ok(())
    }

    #[test]
    fn retained_record_must_match_its_store_receipt_owner() -> TestResult {
        let root = tempfile::tempdir()?;
        let (store, receipt) = cache_fixture(root.path())?;
        let compiled = root.path().join("fresh.rlib");
        fs::write(&compiled, b"compiled bytes")?;
        let output_digest = digest_file(&compiled)?;
        let candidate_index = hash('a');
        let record = NativeCompilationRecord {
            schema_version: RECORD_SCHEMA_VERSION,
            original_receipt_id: "substituted-receipt".to_string(),
            candidate_index: candidate_index.clone(),
            compilation_key: hash('b'),
            files: vec![("src/lib.rs".to_string(), hash('c'))],
            environment: Vec::new(),
            output_digest,
        };
        let manifest = store.publish(&OvenArtifactPublishRequest {
            receipt,
            domain: native_compilation_domain(&candidate_index)?,
            kind: OvenArtifactKind::NativeCompilationOutput,
            payload: serde_json::to_vec(&record)?,
            materialized_files: vec![OvenArtifactMaterializedFile {
                source_path: compiled,
                relative_path: cache_output_relative_path("libunit.rlib")?,
            }],
        })?;
        let owner = store
            .select_payloads_for_execution(&[manifest.identity])?
            .pop()
            .ok_or("cache owner absent")?;
        assert!(
            read_retained(
                owner,
                &candidate_index,
                Some(&hash('b')),
                &cache_output_relative_path("libunit.rlib")?,
            )?
            .is_none()
        );
        Ok(())
    }

    #[test]
    fn admits_known_incan_vector_only_after_recomputing_both_identities() -> TestResult {
        let binding = binding();
        let index_material = index_material();
        let key_material = key_material(&index_material);
        let candidate_index = "sha256:2ef09aa7443147536834556b39874e0d5d9f90a838f7f63496b29d4953ea0b3c";
        let compilation_key = "sha256:c88451d897750fc74c2cd16a761ecb759675f6c595de68a7b9973a9fe793c8e9";
        assert_eq!(digest_material(&index_material)?, candidate_index);
        assert_eq!(digest_material(&key_material)?, compilation_key);
        let response = serde_json::to_vec(&serde_json::json!({
            "schema": SCHEMA,
            "status": "cacheable",
            "binding": binding,
            "candidate_index": candidate_index,
            "index_material": index_material,
            "compilation_key": compilation_key,
            "key_material": key_material,
        }))?;
        let NativeCompilationProjection::Cacheable {
            binding: returned,
            compilation,
        } = admit_response(&response, &binding)?
        else {
            return Err("known cacheable vector was not admitted".into());
        };
        assert_eq!(returned, binding);
        assert_eq!(compilation.candidate.index, candidate_index);
        assert_eq!(compilation.key, compilation_key);
        assert_eq!(compilation.material.as_array().map(Vec::len), Some(5));
        Ok(())
    }

    #[test]
    fn admits_both_explicit_uncacheable_shapes_without_granting_a_key() -> TestResult {
        let binding = binding();
        let without_index = serde_json::to_vec(&serde_json::json!({
            "schema": SCHEMA,
            "status": "uncacheable",
            "binding": binding,
            "reasons": ["missing_compiler_evidence"]
        }))?;
        let NativeCompilationProjection::Uncacheable {
            binding: returned,
            reasons,
            candidate,
        } = admit_response(&without_index, &binding)?
        else {
            return Err("uncacheable response became cacheable".into());
        };
        assert_eq!(returned, binding);
        assert_eq!(reasons, ["missing_compiler_evidence"]);
        assert!(candidate.is_none());

        let material = index_material();
        let index = digest_material(&material)?;
        let with_index = serde_json::to_vec(&serde_json::json!({
            "schema": SCHEMA,
            "status": "uncacheable",
            "binding": binding,
            "reasons": ["missing_rustc_dep_info"],
            "candidate_index": index,
            "index_material": material
        }))?;
        let NativeCompilationProjection::Uncacheable { candidate, .. } = admit_response(&with_index, &binding)? else {
            return Err("indexed uncacheable response became cacheable".into());
        };
        assert_eq!(candidate.ok_or("candidate absent")?.index, index);
        Ok(())
    }

    #[test]
    fn cold_candidate_requires_missing_dep_info_as_its_only_uncacheable_reason() -> TestResult {
        let candidate = NativeCompilationCandidate {
            index: hash('a'),
            material: Value::Null,
        };
        let eligible = NativeCompilationProjection::Uncacheable {
            binding: binding(),
            reasons: vec![MISSING_DEP_INFO_REASON.to_string()],
            candidate: Some(candidate.clone()),
        };
        assert_eq!(reusable_cold_candidate(eligible)?, Some(candidate.clone()));

        let unknown_host_effect = NativeCompilationProjection::Uncacheable {
            binding: binding(),
            reasons: vec![
                "unobserved_host_effect:dynamic native member".to_string(),
                MISSING_DEP_INFO_REASON.to_string(),
            ],
            candidate: Some(candidate),
        };
        assert!(reusable_cold_candidate(unknown_host_effect)?.is_none());
        Ok(())
    }

    #[test]
    fn oversized_engine_projection_disables_jec_without_consuming_a_partial_request() -> TestResult {
        let small = request_with_invocation_id("small".into());
        assert!(bounded_engine_request(&small)?.is_some());

        let oversized = request_with_invocation_id("x".repeat(super::super::engine_exchange::FILE_LIMIT));
        assert!(bounded_engine_request(&oversized)?.is_none());
        Ok(())
    }

    #[test]
    fn projection_transport_failure_can_fall_back_but_refusal_and_cancellation_cannot() {
        use super::super::engine_exchange::ExchangeOutcome;

        assert!(projection_exchange_is_unavailable(ExchangeOutcome::Failed));
        assert!(projection_exchange_is_unavailable(ExchangeOutcome::TimedOut));
        assert!(!projection_exchange_is_unavailable(ExchangeOutcome::Refused));
        assert!(!projection_exchange_is_unavailable(ExchangeOutcome::Cancelled));
        assert!(!projection_exchange_is_unavailable(ExchangeOutcome::Completed));
    }

    #[test]
    fn refuses_substituted_binding_material_keys_and_shapes() -> TestResult {
        let binding = binding();
        let index_material = index_material();
        let candidate_index = digest_material(&index_material)?;
        let key_material = key_material(&index_material);
        let compilation_key = digest_material(&key_material)?;
        let valid = serde_json::json!({
            "schema": SCHEMA,
            "status": "cacheable",
            "binding": binding,
            "candidate_index": candidate_index,
            "index_material": index_material,
            "compilation_key": compilation_key,
            "key_material": key_material,
        });
        let mut cases = Vec::new();
        let mut changed = valid.clone();
        changed["binding"]["unit_owner_id"] = Value::String("other-owner".into());
        cases.push(changed);
        let mut changed = valid.clone();
        changed["candidate_index"] = Value::String(hash('a'));
        cases.push(changed);
        let mut changed = valid.clone();
        changed["compilation_key"] = Value::String(hash('b'));
        cases.push(changed);
        let mut changed = valid.clone();
        changed["key_material"][1] = serde_json::json!([INDEX_DOMAIN]);
        cases.push(changed);
        let mut changed = valid.clone();
        changed["candidate_index"] = Value::String(format!("sha256:{}", "A".repeat(64)));
        cases.push(changed);
        let mut changed = valid.clone();
        changed["extra"] = Value::Bool(true);
        cases.push(changed);
        for changed in cases {
            assert!(admit_response(&serde_json::to_vec(&changed)?, &binding).is_err());
        }

        let unpaired = serde_json::json!({
            "schema": SCHEMA,
            "status": "uncacheable",
            "binding": binding,
            "reasons": ["missing_rustc_dep_info"],
            "candidate_index": candidate_index
        });
        assert!(admit_response(&serde_json::to_vec(&unpaired)?, &binding).is_err());
        let duplicate_reasons = serde_json::json!({
            "schema": SCHEMA,
            "status": "uncacheable",
            "binding": binding,
            "reasons": ["unknown", "unknown"]
        });
        assert!(admit_response(&serde_json::to_vec(&duplicate_reasons)?, &binding).is_err());
        let refused = serde_json::json!({"schema": SCHEMA, "status": "refused", "fields": ["inputs", "source"]});
        assert!(admit_response(&serde_json::to_vec(&refused)?, &binding).is_err());
        Ok(())
    }
}

//! The ordinary executable build lifecycle, shared by command and service clients.
//!
//! Selection, compilation and publication belong to the driver. Clients own presentation; their progress sink must
//! never become artifact authority. Completed-output reuse remains the existing receipt-bound implementation.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::bake::select_default_executable_project_output;
use super::output_materialization::{completed_executable_output_report, materialize_completed_executable_output};
use super::retained_inputs::ModelInputs;
use super::{BuildCommandOptions, ProjectSourceAuthorityDigester, RunProfile};
use crate::error::{CliError, CliResult};
use incan_driver::features::FeatureSelection;

/// Serial executable-build owner retaining typed ordinary/native inputs, with fresh source authority per request.
#[derive(Default)]
pub struct ExecutableBuildOwner {
    authority: ProjectSourceAuthorityDigester,
    /// Exact target decisions retained with the existing input observer; never a second dependency resolver.
    target_predicates: Option<RetainedTargetPredicates>,
    native_plans: std::collections::BTreeMap<PathBuf, super::native_loaf_plan::RetainedNativeLoafPlan>,
    /// Native test names retained after canonical executable admission; passing results are never retained.
    native_test_collections:
        std::collections::BTreeMap<PathBuf, std::sync::Arc<oven_rustc::native_test::OvenNativeTestCollection>>,
    /// Successfully published output receipt, reconciled independently of dependency selection.
    native_receipt: Option<RetainedOutputReceipt>,
}

/// One pure policy answer guarded by its full request and original compiler/engine file observations.
struct RetainedTargetPredicates {
    key: (String, String, String, Vec<String>),
    inputs: ModelInputs,
    decisions: Vec<bool>,
}

/// One canonical publication retained by the serial build owner under the existing metadata observations.
struct RetainedOutputReceipt {
    path: PathBuf,
    receipt: oven_store::OvenReceipt,
    inputs: ModelInputs,
}

impl RetainedOutputReceipt {
    /// Preserve an unchanged successful publication; changed or unsupported inputs use the canonical writer.
    /// Readback is observed before use and reconciled afterward so a replacement between publication and observation
    /// cannot become a retained hit. Failed publication discards the previous observation rather than restoring it.
    fn publish(previous: &mut Option<Self>, receipt: &oven_store::OvenReceipt, path: &Path) -> CliResult<bool> {
        let _trace = tracing::trace_span!(target: "incan::invocation", "incan_driver::publish_native_receipt", input = %path.display()).entered();
        if let Some(published) = previous.take()
            && published.path == path
            && published.receipt == *receipt
            && published.inputs.supports_retention()
            && published.inputs.current()
        {
            *previous = Some(published);
            tracing::trace!(target: "incan::invocation", decision = "retained", receipt_writes = 0_u64, receipt_serializations = 0_u64, input_bytes_read = 0_u64);
            return Ok(true);
        }

        // ---- Canonical publication and observed readback ----
        oven_store::write_receipt(receipt, path).map_err(|error| CliError::failure(error.to_string()))?;
        let mut inputs = ModelInputs::default();
        inputs.observe(path);
        if inputs.supports_retention() {
            let bytes = std::fs::read(path).map_err(|error| CliError::failure(error.to_string()))?;
            let decoded: oven_store::OvenReceipt =
                serde_json::from_slice(&bytes).map_err(|error| CliError::failure(error.to_string()))?;
            if decoded != *receipt || !inputs.current() {
                return Err(CliError::failure("native output receipt changed during publication"));
            }
            *previous = Some(Self {
                path: path.to_path_buf(),
                receipt: receipt.clone(),
                inputs,
            });
        }
        tracing::trace!(target: "incan::invocation", decision = "published", receipt_writes = 1_u64, receipt_serializations = 1_u64);
        Ok(false)
    }
}

/// Explicit receipt-bound native executable intent for compiler/tooling clients of the same build owner.
/// The receipt is checked by the existing direct executor; transport data never grants source or output authority.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeExecutableBuildRequest {
    /// Caller-selected ordinary Store; dependency owners remain leased in their original Store.
    pub store: PathBuf,
    /// Existing request and retention bounds, preserved across the transport.
    pub limits: oven_store::store::OvenStoreLimits,
    /// Original generated-source receipt, including target, profile and exact source roles.
    pub source_receipt: oven_store::OvenReceipt,
    /// Current ordinary dependency declaration, independent of sealed native recipes.
    pub declaration: PathBuf,
    /// Explicit native compiler executable.
    pub rustc: PathBuf,
    /// Current resolved native graph, including the immutable index revision.
    pub graph: PathBuf,
    /// Existing pinned index transport.
    pub index: PathBuf,
    /// Existing immutable archive transport for genuine preparation misses.
    pub blobs: PathBuf,
    /// Existing mutable preparation hints/staging directory.
    pub preparation_output: PathBuf,
    /// Actual source member bound by the receipt.
    pub source: PathBuf,
    /// Requested writable executable projection.
    pub output: PathBuf,
    /// Explicit native crate identity.
    pub crate_name: String,
    /// Explicit Rust edition.
    pub edition: String,
    /// Receipt role authenticating this source member.
    pub source_key: String,
}

impl NativeExecutableBuildRequest {
    /// Reject client-relative coordinates before any driver or Store operation.
    pub fn validate_paths(&self) -> CliResult<()> {
        if [
            &self.store,
            &self.declaration,
            &self.rustc,
            &self.graph,
            &self.index,
            &self.blobs,
            &self.preparation_output,
            &self.source,
            &self.output,
        ]
        .iter()
        .any(|path| !path.is_absolute())
        {
            return Err(CliError::failure("owner requires absolute native build paths"));
        }
        Ok(())
    }
}

/// Build or reuse one executable, publishing only after the whole build succeeds.
///
/// Paths must already be resolved against the requesting client's working directory. The operation never changes
/// process-global configuration. Its source checks and Store leases are identical for local and retained owners;
/// retaining the owner does not by itself authorize retaining a stale source digest.
pub fn build_executable(
    file_path: &str,
    output_dir: Option<&String>,
    options: BuildCommandOptions,
    progress: impl FnMut(String),
) -> CliResult<serde_json::Value> {
    ExecutableBuildOwner::default().build(file_path, output_dir, options, progress)
}

impl ExecutableBuildOwner {
    /// Evaluate target conditions once per exact declaration, target, compiler and policy generation.
    /// Unchanged requests retain the complete original cfg answer. Changed or unsupported observations recompute;
    /// failures discard the prior answer, and no conditional requests need no engine or compiler probe.
    pub fn target_predicates(
        &mut self,
        declaration: &str,
        rustc: &Path,
        target: &str,
        conditions: &[String],
    ) -> CliResult<Vec<bool>> {
        if conditions.is_empty() {
            return Ok(Vec::new());
        }
        let (engine, engine_digest) = super::native_runtime_inputs::bound_policy_engine()?;
        let compiler = rustc
            .canonicalize()
            .map_err(|error| CliError::failure(error.to_string()))?;
        let key = (
            declaration.to_owned(),
            target.to_owned(),
            format!("{}:{engine_digest}", compiler.display()),
            conditions.to_vec(),
        );
        if let Some(retained) = &self.target_predicates
            && retained.key == key
            && retained.inputs.supports_retention()
            && retained.inputs.current()
        {
            tracing::trace!(target: "incan::invocation", decision="retained_target_predicates", cfg_probes=0_u64, policy_exchanges=0_u64);
            return Ok(retained.decisions.clone());
        }
        self.target_predicates = None;
        let mut inputs = ModelInputs::default();
        inputs.observe(&compiler);
        inputs.observe(&engine);
        // Bind verification after observation too, so a replacement between initial verification and observation
        // cannot become the first retained generation.
        let _ = super::native_runtime_inputs::bound_policy_engine()?;
        let toolchain =
            oven_rustc::rustc::rustc_identity(&compiler).map_err(|error| CliError::failure(error.to_string()))?;
        let cfg = oven_rustc::rustc::rustc_cfg_snapshot(&compiler, Some(target))
            .map_err(|error| CliError::failure(error.to_string()))?;
        let decisions =
            super::native_runtime_inputs::target_predicates(&engine, declaration, target, &toolchain, cfg, conditions)?;
        if !inputs.current() {
            return Err(CliError::failure("target policy inputs changed during evaluation"));
        }
        if inputs.supports_retention() {
            self.target_predicates = Some(RetainedTargetPredicates {
                key,
                inputs,
                decisions: decisions.clone(),
            });
        }
        Ok(decisions)
    }

    /// Prepare the canonical project lock through this owner's original source and dependency sessions.
    pub fn prepare_lock(&mut self, request: &super::lock_preparation::LockPreparationRequest) -> CliResult<()> {
        super::lock_preparation::execute_request(request)
    }

    /// Run metadata-only checking through this serial owner using the single canonical driver pipeline.
    /// The request returns diagnostics and warning output, never a native artifact or execution permission.
    pub fn check(&mut self, request: &super::check::CheckRequest) -> CliResult<super::check::CheckOutput> {
        super::check::execute_request(request)
    }

    /// Open the session a test request collects and executes through, sharing this owner's vocabulary runtime.
    pub fn test_session(
        &mut self,
        entry: &Path,
        features: &FeatureSelection,
        profile: &str,
    ) -> CliResult<std::sync::Arc<crate::session::CompilationSession>> {
        self.authority.begin_request();
        let mut session = crate::session::CompilationSession::discover(entry, features, profile)?;
        session.vocab_runtime = std::sync::Arc::clone(&self.authority.retained_inputs.vocab_runtime);
        Ok(std::sync::Arc::new(session))
    }

    /// Retain the existing supervised collection for an already admitted native libtest executable.
    /// Callers must run canonical native preparation first, preserving its dependency leases during execution.
    /// Reconciliation failures discard the previous names, and each request still executes fresh selected tests.
    pub fn native_test_collection(
        &mut self,
        executable: &Path,
        environment: &std::collections::BTreeMap<String, String>,
        working_directory: &Path,
        timeout: Option<std::time::Duration>,
    ) -> Result<
        std::sync::Arc<oven_rustc::native_test::OvenNativeTestCollection>,
        oven_rustc::native_test::OvenNativeTestError,
    > {
        let previous = self.native_test_collections.remove(executable);
        let collection = oven_rustc::native_test::OvenNativeTestCollection::reconcile(
            previous,
            executable,
            environment,
            working_directory,
            timeout,
        )?;
        self.native_test_collections
            .insert(executable.to_path_buf(), std::sync::Arc::clone(&collection));
        Ok(collection)
    }

    /// Share ordinary native preparation between builds and Rust tests in the same serial owner.
    /// Each output has its own retained state; the producer reconciles dependencies, configuration and physical
    /// inputs on every request. Runtime data and test results are never retained here.
    pub fn native_loaf_plan(
        &mut self,
        store: &oven_store::store::OvenStore,
        receipt: &oven_store::OvenReceipt,
        request: &oven_rustc::native_loaf::NativeLoafConsumerRequest<'_>,
    ) -> CliResult<(
        oven_store::OvenReceipt,
        std::sync::Arc<oven_rustc::plan::OvenDirectRustcPlanSelection>,
        String,
        bool,
    )> {
        self.native_plans
            .entry(request.output.to_path_buf())
            .or_default()
            .select(store, receipt, request)
    }

    /// Prepare the union of already selected consumers once, while each consumer keeps its own imports and plan.
    /// The caller supplies only current, compatible declarations from the same owner; this operation never expands
    /// features or resolves dependencies. The original producer reconciles the union on every handoff, and distinct
    /// output plans retain their own receipt and physical-input checks under the same original execution leases.
    pub fn native_loaf_plan_in_group(
        &mut self,
        store: &oven_store::store::OvenStore,
        receipt: &oven_store::OvenReceipt,
        consumer: &oven_rustc::native_loaf::NativeLoafConsumerRequest<'_>,
        dependencies: &[oven_model::manifest::DependencySpec],
        preparation_output: &Path,
    ) -> CliResult<(
        oven_store::OvenReceipt,
        std::sync::Arc<oven_rustc::plan::OvenDirectRustcPlanSelection>,
        String,
        bool,
    )> {
        let preparation = oven_rustc::native_loaf::NativeLoafConsumerRequest {
            dependencies,
            output: preparation_output,
            ..*consumer
        };
        self.native_plans
            .entry(preparation_output.to_path_buf())
            .or_default()
            .select_group(store, receipt, &preparation, consumer)
    }

    /// Select and execute a receipt-bound native executable through existing Oven operations, retaining the same
    /// declared preparation across requests. A Store/configuration change drops that retained selection. Every
    /// execution still verifies source, selected plan, compiler and output; this is not a second native executor.
    pub fn build_native(&mut self, request: &NativeExecutableBuildRequest) -> CliResult<serde_json::Value> {
        use oven_rustc::rustc::{OvenTrustedDirectRustcTargetRequest, bake_trusted_direct_rustc_run_in_store};
        use oven_store::store::OvenStore;
        request.validate_paths()?;
        let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_driver::build_native_executable", input = %request.source.display()).entered();
        let started = Instant::now();

        // ---- Declared inputs: reuse only the original reconciled preparation in this Store ----
        let store = OvenStore::new(&request.store, request.limits);
        let declaration = request
            .declaration
            .canonicalize()
            .map_err(|error| CliError::failure(error.to_string()))?;
        let manifest = oven_model::manifest::ProjectManifest::load(&declaration)
            .map_err(|error| CliError::failure(error.to_string()))?;
        let dependencies = manifest.rust_dependency_values();
        let (receipt, selection, report, plan_reused) = self.native_loaf_plan(
            &store,
            &request.source_receipt,
            &oven_rustc::native_loaf::NativeLoafConsumerRequest {
                graph: &request.graph,
                index: &request.index,
                blobs: &request.blobs,
                output: &request.preparation_output,
                rustc: &request.rustc,
                target: &request.source_receipt.intent.target,
                profile: &request.source_receipt.intent.profile,
                dependencies: &dependencies,
                declaration_owner: manifest.project_root(),
                domain: "target",
            },
        )?;
        let native_preparation: serde_json::Value =
            serde_json::from_str(&report).map_err(|error| CliError::failure(error.to_string()))?;
        let selection_seconds = started.elapsed().as_secs_f64();

        // ---- Physical execution: the existing receipt-bound executor checks the source and output ----
        let plan = selection
            .source_artifact_plan(&request.source_key)
            .map_err(|error| CliError::failure(error.to_string()))?;
        let compile = OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: selection.artifacts(),
            artifact_root: selection.output_guard_root(),
            artifact_plan: Some(&plan),
            rustc: &request.rustc,
            source: &request.source,
            output: &request.output,
            crate_name: &request.crate_name,
            edition: &request.edition,
            source_evidence_key: &request.source_key,
            features: &receipt.intent.features,
            prefer_dynamic: false,
        };
        let execution_started = Instant::now();
        let result = bake_trusted_direct_rustc_run_in_store(&compile, &store)
            .map_err(|error| CliError::failure(error.to_string()))?;

        // ---- Publication: preserve the existing compiler receipt and measured phase report ----
        let mut receipt_path = request.output.as_os_str().to_os_string();
        receipt_path.push(".receipt.json");
        let receipt_reused =
            RetainedOutputReceipt::publish(&mut self.native_receipt, &receipt, &PathBuf::from(receipt_path))?;
        Ok(serde_json::json!({
            "output_digest":result.output_digest, "source_digest":result.source_digest,
            "receipt_identity":receipt.identity, "build_unit_identity":receipt.build_unit_identity,
            "reused":result.reused, "cargo_process_started":result.cargo_process_started,
            "native_preparation":native_preparation,
            "native_plan_reused":plan_reused,
            "receipt_reused":receipt_reused,
            "phases_seconds":{"ordinary_native_selection":selection_seconds,
                "compiler_execution_or_reuse":execution_started.elapsed().as_secs_f64()},
            "elapsed_seconds":started.elapsed().as_secs_f64(),
        }))
    }

    /// Build or reuse one executable through the direct route.
    ///
    /// A completed output whose source and preparation authority still match is materialized without frontend work;
    /// otherwise the direct route compiles the program and publishes the result.
    pub fn build(
        &mut self,
        file_path: &str,
        output_dir: Option<&String>,
        options: BuildCommandOptions,
        mut progress: impl FnMut(String),
    ) -> CliResult<serde_json::Value> {
        let _invocation =
            tracing::trace_span!(target: "incan::invocation", "incan_driver::build_executable", input = file_path)
                .entered();
        let total_start = Instant::now();
        self.authority.begin_request();

        // ---- Completed output: retain the existing admission and materialization boundary ----
        if let Some((project_root, selected, backend_receipt)) =
            select_default_executable_project_output(file_path, output_dir, &options, &mut self.authority)?
        {
            materialize_completed_executable_output(&project_root, &selected, &backend_receipt)?;
            progress(format!(
                "✓ Oven build reused sealed project Loaf: {}",
                selected.native_output.display()
            ));
            return completed_executable_output_report(&project_root, &selected, &backend_receipt, total_start);
        }

        // ---- Direct route ----
        let profile = match options.executable_profile {
            RunProfile::Debug => "debug",
            RunProfile::Release => "release",
        };
        let output_dir = output_dir.map(PathBuf::from);
        let unit = crate::compile::compile_unit(
            &crate::compile::UnitRequest {
                source: Path::new(file_path),
                kind: crate::compile::UnitKind::Executable,
                profile,
                features: &options.package_features,
                output_dir: output_dir.as_deref(),
            },
            &mut progress,
        )?;
        Ok(serde_json::json!({
            "status": "success",
            "mode": "executable",
            "profile": profile,
            "entrypoint": file_path,
            "project": { "project_root": unit.project_root },
            "artifacts": [{ "kind": "binary", "path": unit.artifact }],
        }))
    }
}


//! The ordinary executable build lifecycle, shared by command and service clients.
//!
//! Selection, compilation and publication belong to the driver. Clients own presentation; their progress sink must
//! never become artifact authority. Completed-output reuse remains the existing receipt-bound implementation.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::backend_selection::{default_backend_receipt_path, write_backend_receipt};
use super::bake::{bake_oven_project, publish_ordinary_project_output, select_default_executable_project_output};
use super::output_materialization::{completed_executable_output_report, materialize_completed_executable_output};
use super::output_paths::{normalized_project_entrypoint, project_root_for_completed_output};
use super::oven_project::prepare_oven_project_with_session;
use super::retained_inputs::{ModelInputs, RetainedExecutableDependencies, SessionPurpose};
use super::source_authority::{digest_baked_project_source_authority, ordinary_preparation_identity_digest};
use super::{BuildCommandOptions, OvenProjectPlanMode, ProjectSourceAuthorityDigester, elapsed_ms};
use crate::build_report::artifact_report;
use crate::cargo_policy::reject_normal_cargo_controls;
use crate::error::{CliError, CliResult};
use incan_provider::FeatureSelection;

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
        super::lock_preparation::execute_request(request, &mut self.authority)
    }

    /// Run metadata-only checking through this serial owner using the single canonical driver pipeline.
    /// The request returns diagnostics and warning output, never a native artifact or execution permission.
    pub fn check(&mut self, request: &super::check::CheckRequest) -> CliResult<super::check::CheckOutput> {
        super::check::execute_request(request, &mut self.authority)
    }

    /// Borrow the canonical parser-only session for a fresh test collection request.
    ///
    /// The same owner, typed lock and input reconciler used by builds guard retention. Collection mode is part of
    /// the retained identity and never admits execution artifacts. Standalone files keep canonical discovery.
    pub fn test_collection_session(
        &mut self,
        root: &Path,
        entry: &Path,
        features: &FeatureSelection,
        sdk_profile: Option<&str>,
    ) -> CliResult<std::sync::Arc<crate::session::CompilationSession>> {
        self.test_session(root, entry, features, sdk_profile, SessionPurpose::Collection)
    }

    /// Borrow canonical artifact preparation for fresh test execution, retaining no test outcomes or fixtures.
    /// The existing project, lock and provider observations guard the same artifact-purpose session used by builds.
    pub fn test_execution_session(
        &mut self,
        root: &Path,
        entry: &Path,
        features: &FeatureSelection,
        sdk_profile: Option<&str>,
    ) -> CliResult<std::sync::Arc<crate::session::CompilationSession>> {
        self.test_session(root, entry, features, sdk_profile, SessionPurpose::Artifacts)
    }

    /// Share the existing native dependency admission between build and test after session reconciliation.
    /// The canonical lock/compiler/provider generation owns this state; standalone inputs keep canonical discovery.
    pub fn native_dependency_context(
        &mut self,
        session: &std::sync::Arc<crate::session::CompilationSession>,
    ) -> CliResult<Option<std::sync::Arc<super::NativeSdkCommandContext>>> {
        let Some(manifest) = session.manifest.as_ref() else {
            return super::NativeSdkCommandContext::discover();
        };
        let root =
            std::fs::canonicalize(manifest.project_root()).map_err(|error| CliError::failure(error.to_string()))?;
        self.authority.retained_inputs.native_dependency_context(&root, session)
    }

    /// Borrow completed test generation from the existing artifact session; standalone sessions keep canonical work.
    pub fn test_generation(
        &self,
        session: &std::sync::Arc<crate::session::CompilationSession>,
    ) -> CliResult<Option<std::sync::Arc<super::test_generation::RetainedTestGeneration>>> {
        let Some(manifest) = session.manifest.as_ref() else {
            return Ok(None);
        };
        let root =
            std::fs::canonicalize(manifest.project_root()).map_err(|error| CliError::failure(error.to_string()))?;
        Ok(self.authority.retained_inputs.test_generation(&root, session))
    }

    /// Select the original discovery mode for one reconciled test request without a second session cache.
    fn test_session(
        &mut self,
        root: &Path,
        entry: &Path,
        features: &FeatureSelection,
        sdk_profile: Option<&str>,
        purpose: SessionPurpose,
    ) -> CliResult<std::sync::Arc<crate::session::CompilationSession>> {
        self.authority.begin_request();
        if !root.join(oven_model::manifest::LOAF_MANIFEST_FILENAME).is_file() {
            let mut session = match purpose {
                SessionPurpose::Collection => {
                    crate::session::CompilationSession::discover_for_collection_with_selections(
                        entry,
                        features,
                        sdk_profile,
                    )?
                }
                SessionPurpose::Artifacts => {
                    crate::session::CompilationSession::discover_for_oven(entry, features, sdk_profile)?
                }
                SessionPurpose::CheckMetadata => {
                    return Err(CliError::failure("metadata-only checks cannot prepare a test session"));
                }
            };
            session.vocab_runtime = std::sync::Arc::clone(&self.authority.retained_inputs.vocab_runtime);
            return Ok(std::sync::Arc::new(session));
        }
        self.authority
            .compilation_session(root, entry, features, sdk_profile, purpose)
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

    /// Build through the single driver lifecycle, retaining only inputs that revalidate their observations.
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
        reject_normal_cargo_controls(&options.cargo_policy, options.generated_cargo_target_dir.as_ref())?;
        let profile = options.executable_profile.as_str();
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

        // ---- Source-aware preparation: capture authority before entering the frontend ----
        let authority_trace =
            tracing::trace_span!(target: "incan::invocation", "incan_driver::capture_build_authority").entered();
        let completed_authority = if output_dir.is_none()
            && options.package_features == FeatureSelection::default()
            && options.sdk_profile.is_none()
        {
            project_root_for_completed_output(&normalized_project_entrypoint(file_path)?)?
                .map(|root| -> CliResult<_> {
                    Ok((
                        digest_baked_project_source_authority(&root)?,
                        ordinary_preparation_identity_digest()?,
                    ))
                })
                .transpose()?
        } else {
            None
        };
        drop(authority_trace);
        let prepare_start = Instant::now();
        let entry = normalized_project_entrypoint(file_path)?;
        let retained_root = project_root_for_completed_output(&entry)?;
        let session = retained_root
            .as_ref()
            .map(|root| {
                self.authority.compilation_session(
                    root,
                    &entry,
                    &options.package_features,
                    options.sdk_profile.as_deref(),
                    SessionPurpose::Artifacts,
                )
            })
            .transpose()?;
        let dependencies = retained_root
            .as_ref()
            .map(|root| self.authority.take_executable_dependencies(root))
            .transpose()?
            .flatten();
        let native_context = session
            .as_ref()
            .map(|session| self.native_dependency_context(session))
            .transpose()?
            .flatten();
        let (prepared, mut dependency_inputs) = prepare_oven_project_with_session(
            file_path,
            output_dir.map(String::as_str),
            &options.cargo_policy,
            &options.package_features,
            options.sdk_profile.as_deref(),
            options.cargo_features,
            options.cargo_no_default_features,
            options.cargo_all_features,
            profile,
            OvenProjectPlanMode::ConsumeOnly,
            None,
            session,
            dependencies,
            native_context,
            None,
        )?;
        if let Some(inputs) = dependency_inputs.as_mut()
            && !inputs.observe_plan(&prepared)?
        {
            dependency_inputs = None;
        }
        let prepare_ms = elapsed_ms(prepare_start);
        progress(format!(
            "Generated Rust project in: {}",
            prepared.generator.output_dir().display()
        ));
        progress("Building with Oven Alpha...".to_string());
        let oven_build_start = Instant::now();
        let bake = bake_oven_project(&prepared, profile, None)?;
        let oven_build_ms = elapsed_ms(oven_build_start);
        progress("✓ Oven build successful!".to_string());
        progress(format!("Binary: {}", bake.output.display()));

        // ---- Successful publication: preserve the existing receipt and portable report ----
        let report_trace =
            tracing::trace_span!(target: "incan::invocation", "incan_driver::construct_build_report").entered();
        let mut report_draft = prepared.report.clone();
        report_draft.artifacts.push(artifact_report("binary", &bake.output));
        if let Some(backend_receipt) = report_draft.backend.as_ref() {
            write_backend_receipt(backend_receipt, &default_backend_receipt_path(&prepared.project_root))?;
        }
        let mut timings_ms = prepared.prepare_timings.clone();
        timings_ms.insert("prepare".to_string(), prepare_ms);
        timings_ms.insert("oven_build".to_string(), oven_build_ms);
        timings_ms.insert("total".to_string(), elapsed_ms(total_start));
        let report = report_draft.finish(timings_ms);
        drop(report_trace);
        if let Some((source_authority, preparation_identity)) = completed_authority {
            publish_ordinary_project_output(&prepared, &bake, &source_authority, &preparation_identity, &report)?;
        }
        let _serialization =
            tracing::trace_span!(target: "incan::invocation", "incan_driver::serialize_build_report").entered();
        let report = serde_json::to_value(report)
            .map_err(|error| CliError::failure(format!("failed to serialize Oven build report: {error}")))?;
        if let Some(inputs) = dependency_inputs
            && let Some(dependencies) = RetainedExecutableDependencies::completed(inputs, prepared)
            && let Some(root) = retained_root
        {
            self.authority.remember_executable_dependencies(&root, dependencies)?;
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests;

//! Canonical metadata-only checking and stable diagnostic reports shared by command and service clients.
//!
//! Checking may inspect unbaked local dependencies, but cannot authorize their execution or native publication.
//! Clients render the returned report; dependency discovery, module collection and typechecking stay in this driver.

use std::env;
use std::path::{Path, PathBuf};

use incan_frontend::diagnostics::{self, StableDiagnostic};
use incan_provider::FeatureSelection;
use serde::{Deserialize, Serialize};

use super::retained_inputs::ModelInputs;
use crate::backend::c_abi::CAbiVerificationPlan;
use crate::diagnostics::{CliDiagnostic, CliDiagnosticFailure};
use crate::error::{CliError, CliResult};
use crate::modules::collect_modules_detailed_with_session;
use crate::session::CompilationSession;
use crate::typecheck::typecheck_modules_with_import_graph_artifacts;
use incan_frontend::typechecker::stdlib_loader::StdlibAstCache;

/// Explicit metadata-only check selection carried by the existing project-owner protocol.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckRequest {
    /// Absolute source entrypoint selected by the CLI's existing project command rules.
    pub path: PathBuf,
    /// Existing package-feature selection, independent from rendering choices.
    pub features: FeatureSelection,
    /// Explicit provider-profile override, when the caller selected one.
    pub sdk_profile: Option<String>,
    /// Declared C ABI verification target; never a native execution target.
    pub interop_target: Option<String>,
}

/// Request-local presentation of the canonical typed report, without deserializing compiler catalog references.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckOutput {
    /// Exact public JSON report, absent only when checking failed before producing a diagnostic report.
    pub json: Option<String>,
    /// Original human failure message, separate from the stable public JSON schema.
    pub human_message: Option<String>,
    /// Original command exit status; warnings alone leave this at zero.
    pub exit_code: i32,
    /// Existing formatted warnings, in production order, for this client's stderr only.
    pub warnings: Vec<String>,
}

/// A completed frontend report inside the existing dependency session, guarded by its original source owners.
/// Failed analysis and external inspection keep canonical checking until their complete inputs can be retained.
pub(super) struct CompletedCheck {
    entry: PathBuf,
    interop_target: Option<String>,
    report: DiagnosticReport,
    warnings: Vec<String>,
    inputs: ModelInputs,
    source_metadata: StdlibAstCache,
}

impl CompletedCheck {
    /// Reuse diagnostics only while source text, topology and original consulted metadata remain unchanged.
    fn matches(&self, entry: &Path, interop_target: Option<&str>) -> bool {
        self.entry == entry
            && self.interop_target.as_deref() == interop_target
            && self.inputs.supports_retention()
            && self.inputs.current()
            && self.source_metadata.supports_generation_reuse()
    }
}

/// Evaluate the single canonical check operation and transport its original diagnostics without native authority.
pub(super) fn execute_request(
    request: &CheckRequest,
    authority: &mut super::ProjectSourceAuthorityDigester,
) -> CliResult<CheckOutput> {
    if !request.path.is_absolute() {
        return Err(CliError::failure("owner requires an absolute check path"));
    }
    let (result, warnings) = crate::diagnostics::capture_warning_output(|| {
        authority.begin_request();
        check_path_report_with_session(&request.path, request.interop_target.as_deref(), |entry| {
            let inferred = crate::project::resolve_project_root(entry);
            let manifest = oven_model::manifest::ProjectManifest::discover(&inferred)
                .map_err(|error| CliError::failure(error.to_string()))?;
            if let Some(manifest) = manifest {
                let session = authority.compilation_session(
                    manifest.project_root(),
                    entry,
                    &request.features,
                    request.sdk_profile.as_deref(),
                    super::retained_inputs::SessionPurpose::CheckMetadata,
                )?;
                let report = authority.check_report(manifest.project_root(), &session);
                Ok((session, report))
            } else {
                CompilationSession::discover_for_check(entry, &request.features, request.sdk_profile.as_deref())
                    .map(|session| (std::sync::Arc::new(session), None))
            }
        })
    });
    match result {
        Ok(report) => Ok(CheckOutput {
            json: Some(serde_json::to_string_pretty(&report).map_err(|error| CliError::failure(error.to_string()))?),
            human_message: report.human_message,
            exit_code: if report.ok { 0 } else { 1 },
            warnings,
        }),
        Err(error) => Ok(CheckOutput {
            json: None,
            human_message: Some(error.message),
            exit_code: error.exit_code.0,
            warnings,
        }),
    }
}

/// Schema version for the `incan check` report envelope.
///
/// Deliberately separate from [`incan_frontend::diagnostics::DIAGNOSTIC_SCHEMA_VERSION`], which versions the
/// `StableDiagnostic` payload itself and is also stamped onto `incan explain` output and the backend execution receipt.
/// Version 2 changed only this envelope — `diagnostics` now carries non-fatal warnings alongside errors, so `ok` means
/// "no error-severity diagnostics" rather than "no diagnostics at all". Bumping the shared constant instead would have
/// falsely signaled a payload or receipt contract change to consumers of those other surfaces.
pub(crate) const CHECK_REPORT_SCHEMA_VERSION: u32 = 2;

/// One complete typecheck result, retained independently from CLI rendering so workspace orchestration can emit one
/// coherent machine-readable report for several members.
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticReport {
    schema_version: u32,
    /// Whether typechecking found no error-severity diagnostics.
    ///
    /// Warnings do not clear this flag: a successful check with warnings reports `ok: true` and a non-empty
    /// `diagnostics` array, mirroring the process exit code.
    ok: bool,
    diagnostics: Vec<StableDiagnostic>,
    #[serde(skip_serializing)]
    human_message: Option<String>,
}

impl DiagnosticReport {
    /// Whether collection and typechecking succeeded without error-severity diagnostics.
    pub fn ok(&self) -> bool {
        self.ok
    }

    /// Human-readable diagnostics retained from the compiler pipeline when the report is unsuccessful.
    pub fn human_message(&self) -> Option<&str> {
        self.human_message.as_deref()
    }
}

/// Run the canonical check pipeline for one package-feature and SDK-profile projection without rendering it.
///
/// This is the single-project compiler boundary used by both `incan check` and RFC 077 workspace command fan-out. It
/// deliberately retains stable diagnostics rather than printing them as soon as a member fails.
pub fn check_path_report_with_selections(
    path: &Path,
    feature_selection: &FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> CliResult<DiagnosticReport> {
    check_path_report_with_interop_target_selection(path, feature_selection, sdk_profile_override, None)
}

/// Run one canonical package-feature, SDK-profile, and declared C ABI target projection without rendering it.
pub fn check_path_report_with_interop_target_selection(
    path: &Path,
    feature_selection: &FeatureSelection,
    sdk_profile_override: Option<&str>,
    interop_target: Option<&str>,
) -> CliResult<DiagnosticReport> {
    check_path_report_with_session(path, interop_target, |entry| {
        CompilationSession::discover_for_check(entry, feature_selection, sdk_profile_override)
            .map(|session| (std::sync::Arc::new(session), None))
    })
}

/// Run one check pipeline using a fresh or reconciled metadata-only session from the caller's existing owner.
/// Source collection, provider selection and typechecking are identical for service and direct report callers.
fn check_path_report_with_session<'a>(
    path: &Path,
    interop_target: Option<&str>,
    session: impl FnOnce(
        &Path,
    ) -> CliResult<(
        std::sync::Arc<CompilationSession>,
        Option<&'a mut Option<CompletedCheck>>,
    )>,
) -> CliResult<DiagnosticReport> {
    let _trace = tracing::trace_span!(target: "incan::invocation", "incan_driver::check_pipeline").entered();
    let normalized_path = normalize_input_path(path)?;
    let (compilation_session, mut retained) = match session(&normalized_path) {
        Ok(session) => session,
        Err(error) => {
            let failure = CliDiagnosticFailure::single(
                normalized_path.to_string_lossy(),
                "",
                diagnostics::CompileError::new(error.to_string(), incan_frontend::ast::Span::default()),
                diagnostics::DiagnosticPhase::Import,
            );
            return Ok(diagnostic_report_from_failure(failure));
        }
    };
    // ---- Completed frontend result for this reconciled dependency session ----
    if let Some(report) = retained.as_deref().and_then(Option::as_ref)
        && report.matches(&normalized_path, interop_target)
    {
        for warning in &report.warnings {
            crate::diagnostics::emit_warning_output(warning.clone());
        }
        tracing::trace!(target: "incan::invocation", decision = "reuse_check_report");
        return Ok(report.report.clone());
    }
    if let Some(slot) = retained.as_deref_mut() {
        *slot = None;
    }

    // ---- Observe authored inputs before canonical collection and analysis ----
    let manifest = compilation_session.manifest.clone();
    let source_root = manifest
        .as_ref()
        .map(|manifest| crate::project::resolve_source_root(manifest.project_root(), Some(manifest)));
    let mut inputs = ModelInputs::default();
    if retained.is_some() {
        inputs.observe(&normalized_path);
        if let Some(root) = &source_root {
            if incan_frontend::library_manifest::visit_provider_artifact_inputs(root, &mut |path| inputs.observe(path))
                .is_err()
            {
                inputs.disable_retention();
            }
        } else {
            inputs.disable_retention();
        }
    } else {
        inputs.disable_retention();
    }
    let (checked, warnings) = crate::diagnostics::capture_warning_output(|| {
        check_collected_report(
            &normalized_path,
            interop_target,
            &compilation_session,
            &mut inputs,
            source_root.as_deref(),
        )
    });
    for warning in &warnings {
        crate::diagnostics::emit_warning_output(warning.clone());
    }
    let (report, metadata) = checked?;

    // ---- Install only a complete result whose original inputs still match ----
    if let Some(slot) = retained
        && let Some(source_metadata) = metadata
    {
        let inputs_supported = inputs.supports_retention();
        let inputs_current = inputs_supported && inputs.current();
        let metadata_current = source_metadata.supports_generation_reuse();
        tracing::trace!(target: "incan::invocation", decision = "check_report_admission", inputs_supported, inputs_current, metadata_current);
        if !inputs_current || !metadata_current {
            return Ok(report);
        }
        *slot = Some(CompletedCheck {
            entry: normalized_path,
            interop_target: interop_target.map(str::to_owned),
            report: report.clone(),
            warnings,
            inputs,
            source_metadata,
        });
    }
    Ok(report)
}

/// Execute the single canonical collection/check pipeline, preserving metadata owners only for observable inputs.
fn check_collected_report(
    normalized_path: &Path,
    interop_target: Option<&str>,
    compilation_session: &CompilationSession,
    inputs: &mut ModelInputs,
    source_root: Option<&Path>,
) -> CliResult<(DiagnosticReport, Option<StdlibAstCache>)> {
    let manifest = compilation_session.manifest.clone();
    let c_abi_plan = interop_target
        .map(|target| select_interop_c_abi_verification_plan(manifest.as_ref(), target))
        .transpose()?;
    let modules = match collect_modules_detailed_with_session(normalized_path.to_path_buf(), compilation_session) {
        Ok(modules) => modules,
        Err(failure) => return Ok((diagnostic_report_from_failure(failure), None)),
    };
    if modules.iter().any(|module| {
        module.file_path != normalized_path && !source_root.is_some_and(|root| module.file_path.starts_with(root))
    }) {
        tracing::trace!(target: "incan::invocation", decision = "check_report_unretained", reason = "module_outside_observed_source_root");
        inputs.disable_retention();
    }
    let provider_plan = compilation_session.provider_plan_for_modules(&modules)?;
    #[cfg(feature = "rust_inspect")]
    let rust_inspect_manifest_dir = compilation_session.prepare_check_rust_inspection(normalized_path, &modules)?;

    let typecheck = typecheck_modules_with_import_graph_artifacts(
        &modules,
        manifest.as_ref(),
        &provider_plan,
        c_abi_plan.as_ref(),
        #[cfg(feature = "rust_inspect")]
        rust_inspect_manifest_dir
            .as_ref()
            .map(|workspace| workspace.manifest_dir()),
    );

    match typecheck {
        Ok(artifacts) => {
            #[cfg(feature = "rust_inspect")]
            if rust_inspect_manifest_dir.is_some() {
                tracing::trace!(target: "incan::invocation", decision = "check_report_unretained", reason = "rust_inspection_workspace");
                inputs.disable_retention();
            }
            if c_abi_plan.is_some() || artifacts.type_infos.iter().any(|info| !info.c_abi.bindings.is_empty()) {
                inputs.disable_retention();
            }
            Ok((
                DiagnosticReport {
                    schema_version: CHECK_REPORT_SCHEMA_VERSION,
                    ok: true,
                    diagnostics: stable_diagnostics_from(&artifacts.warnings),
                    human_message: None,
                },
                Some(artifacts.stdlib_cache),
            ))
        }
        Err(failure) => Ok((diagnostic_report_from_failure(failure), None)),
    }
}

/// Select exactly one Oven interop C ABI profile for an invocation that names `--interop-target`.
fn select_interop_c_abi_verification_plan(
    manifest: Option<&oven_model::manifest::ProjectManifest>,
    requested_target: &str,
) -> CliResult<CAbiVerificationPlan> {
    let Some(manifest) = manifest else {
        return Err(CliError::failure(format!(
            "`--interop-target {requested_target}` requires a project manifest with an [interop.c] declaration"
        )));
    };
    let Some(interop) = manifest.interop_c() else {
        return Err(CliError::failure(format!(
            "`--interop-target {requested_target}` requires an [interop.c] declaration in loaf.toml"
        )));
    };
    let Some(interop_target) = interop.targets.iter().find(|target| target.target == requested_target) else {
        return Err(CliError::failure(format!(
            "`--interop-target {requested_target}` is not declared by [[interop.c.targets]] in loaf.toml"
        )));
    };
    CAbiVerificationPlan::from_interop_target(interop_target).map_err(CliError::failure)
}

/// Convert source-aware compiler failures into the stable report consumed by both single-project and workspace paths.
/// Project collected CLI diagnostics into their stable machine-readable form, preserving input order.
///
/// Shared by the success and failure paths so warnings and errors are projected identically — the only thing that
/// distinguishes them in the payload is the `severity` each one already carries.
pub(crate) fn stable_diagnostics_from(collected: &[CliDiagnostic]) -> Vec<StableDiagnostic> {
    collected
        .iter()
        .map(|diagnostic| {
            diagnostics::stable_diagnostic(
                &diagnostic.file_path,
                &diagnostic.source,
                &diagnostic.error,
                diagnostic.phase,
            )
        })
        .collect()
}

/// Build the machine-readable check report for a failed collection or typecheck pass.
///
/// Reports the failure's errors together with any warnings gathered before it, so a file that has both does not
/// hide the warning behind the error. The human message stays errors-only: warnings were already printed to
/// stderr as they were produced.
fn diagnostic_report_from_failure(failure: CliDiagnosticFailure) -> DiagnosticReport {
    let human_message = failure.render_human();
    // Errors first, then any warnings gathered before the failure, so a file with both reports both.
    let mut diagnostics = stable_diagnostics_from(&failure.diagnostics);
    diagnostics.extend(stable_diagnostics_from(&failure.warnings));
    DiagnosticReport {
        schema_version: CHECK_REPORT_SCHEMA_VERSION,
        ok: false,
        diagnostics,
        human_message: Some(human_message),
    }
}

/// Resolve the user-supplied check target relative to the current directory for project discovery.
fn normalize_input_path(path: &Path) -> CliResult<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(env::current_dir()
            .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))?
            .join(path))
    }
}

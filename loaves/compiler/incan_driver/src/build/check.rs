//! Canonical metadata-only checking and stable diagnostic reports shared by command and service clients.
//!
//! Checking reads the Loaves a program imports from their artifacts and never executes or publishes anything.
//! Clients render the returned report; module collection and typechecking stay in this driver.

use std::env;
use std::path::{Path, PathBuf};

use incan_frontend::diagnostics::{self, StableDiagnostic};
use serde::{Deserialize, Serialize};

use crate::backend::c_abi::CAbiVerificationPlan;
use crate::diagnostics::{CliDiagnostic, CliDiagnosticFailure};
use crate::error::{CliError, CliResult};
use crate::features::FeatureSelection;
use crate::modules::collect_modules_detailed_with_session;
use crate::session::{CompilationSession, DEFAULT_PROFILE};
use crate::typecheck::typecheck_modules;

/// Explicit metadata-only check selection carried by the existing project-owner protocol.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckRequest {
    /// Absolute source entrypoint selected by the CLI's existing project command rules.
    pub path: PathBuf,
    /// Existing package-feature selection, independent from rendering choices.
    pub features: FeatureSelection,
    /// The build profile whose Loaf artifacts the check reads; the default profile when absent.
    pub profile: Option<String>,
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

/// Evaluate the single canonical check operation and transport its original diagnostics without native authority.
pub(super) fn execute_request(request: &CheckRequest) -> CliResult<CheckOutput> {
    if !request.path.is_absolute() {
        return Err(CliError::failure("owner requires an absolute check path"));
    }
    let (result, warnings) = crate::diagnostics::capture_warning_output(|| {
        check_path_report(
            &request.path,
            &request.features,
            request.profile.as_deref().unwrap_or(DEFAULT_PROFILE),
            request.interop_target.as_deref(),
        )
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

/// Run the canonical check pipeline for one package-feature projection, profile and optional declared C ABI target,
/// without rendering it.
///
/// This is the single-project compiler boundary used by both `incan check` and RFC 077 workspace command fan-out. It
/// retains stable diagnostics rather than printing them as soon as a member fails.
pub fn check_path_report(
    path: &Path,
    feature_selection: &FeatureSelection,
    profile: &str,
    interop_target: Option<&str>,
) -> CliResult<DiagnosticReport> {
    let _trace = tracing::trace_span!(target: "incan::invocation", "incan_driver::check_pipeline").entered();
    let normalized_path = normalize_input_path(path)?;
    let session = match CompilationSession::discover(&normalized_path, feature_selection, profile) {
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
    let c_abi_plan = interop_target
        .map(|target| select_interop_c_abi_verification_plan(session.manifest.as_ref(), target))
        .transpose()?;
    let modules = match collect_modules_detailed_with_session(normalized_path.clone(), &session) {
        Ok(modules) => modules,
        Err(failure) => return Ok(diagnostic_report_from_failure(failure)),
    };
    #[cfg(feature = "rust_inspect")]
    let rust_inspect = session.prepare_check_rust_inspection(&normalized_path, &modules)?;
    #[cfg(feature = "rust_inspect")]
    if let Some(workspace) = rust_inspect.as_ref() {
        workspace.verify_ordinary_native()?;
    }
    let typecheck = typecheck_modules(
        &modules,
        session.manifest.as_ref(),
        session.loaves(),
        session.own_standard_package(),
        c_abi_plan.as_ref(),
        &mut |_, _| {},
        #[cfg(feature = "rust_inspect")]
        rust_inspect.as_ref().map(|workspace| workspace.manifest_dir()),
    );
    #[cfg(feature = "rust_inspect")]
    if let Some(workspace) = rust_inspect.as_ref() {
        workspace.verify_ordinary_native()?;
    }
    match typecheck {
        Ok(checked) => Ok(DiagnosticReport {
            schema_version: CHECK_REPORT_SCHEMA_VERSION,
            ok: true,
            diagnostics: stable_diagnostics_from(&checked.warnings),
            human_message: None,
        }),
        Err(failure) => Ok(diagnostic_report_from_failure(failure)),
    }
}

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

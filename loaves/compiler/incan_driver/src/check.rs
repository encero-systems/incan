//! Checking one described unit (DD-0008), and the stable diagnostic report it produces.
//!
//! Checking reads imported Loaf artifacts and runs admitted vocabulary expansion before ordinary typechecking.
//! Callers render the returned report; collection, retained expansion errors and checking stay in this driver.

use incan_frontend::diagnostics::{self, StableDiagnostic};
use serde::{Deserialize, Serialize};

use crate::diagnostics::{CliDiagnostic, CliDiagnosticFailure};
use crate::error::{CliError, CliResult};
use crate::modules::collect_modules_detailed_with_session;
use crate::session::{CompilationSession, RustFactsSetup};
use crate::unit::UnitDescription;

/// One check's presentation: its typed report as JSON, its human message and its warnings.
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
    /// Whether unchanged inputs reproduce exactly this outcome: a passing check or one refused by source
    /// diagnostics, never an import, resource or tooling failure.
    pub reproducible: bool,
}

/// Check one unit, capturing the warnings it prints alongside its report.
pub fn check_unit(unit: &UnitDescription, rust: &RustFactsSetup) -> CliResult<CheckOutput> {
    let (result, warnings) = crate::diagnostics::capture_warning_output(|| check_report(unit, rust));
    match result {
        Ok(report) => Ok(CheckOutput {
            json: Some(serde_json::to_string_pretty(&report).map_err(|error| CliError::failure(error.to_string()))?),
            human_message: report.human_message,
            exit_code: if report.ok { 0 } else { 1 },
            warnings,
            reproducible: report.reproducible,
        }),
        Err(error) => Ok(CheckOutput {
            json: None,
            human_message: Some(error.message),
            exit_code: error.exit_code.0,
            warnings,
            reproducible: false,
        }),
    }
}

/// Schema version for the `incan check` report envelope.
///
/// Deliberately separate from [`incan_frontend::diagnostics::DIAGNOSTIC_SCHEMA_VERSION`], which versions the
/// `StableDiagnostic` payload itself. Version 2 changed only this envelope — `diagnostics` now carries non-fatal
/// warnings alongside errors, so `ok` means "no error-severity diagnostics" rather than "no diagnostics at all".
/// Bumping the payload constant instead would have falsely signaled a payload contract change.
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
    /// Whether unchanged inputs reproduce this report; never part of the public JSON report.
    #[serde(skip_serializing)]
    reproducible: bool,
}

/// Check one unit without rendering the result: its stable diagnostics, warnings included, and whether it passed.
pub fn check_report(unit: &UnitDescription, rust: &RustFactsSetup) -> CliResult<DiagnosticReport> {
    let _trace = tracing::trace_span!(target: "incan::invocation", "incan_driver::check_pipeline").entered();
    // Opening the session and extracting Rust facts read artifacts and run rustc, so their failures stay retryable.
    let import_failure = |error: CliError| DiagnosticReport {
        reproducible: false,
        ..diagnostic_report_from_failure(CliDiagnosticFailure::single(
            unit.source.to_string_lossy(),
            "",
            diagnostics::CompileError::new(error.to_string(), incan_frontend::ast::Span::default()),
            diagnostics::DiagnosticPhase::Import,
        ))
    };
    let mut session = match CompilationSession::new(unit).and_then(|session| session.with_rust_facts(rust)) {
        Ok(session) => session,
        Err(error) => return Ok(import_failure(error)),
    };
    let modules = match collect_modules_detailed_with_session(unit.source.clone(), &session) {
        Ok(modules) => modules,
        Err(failure) => return Ok(diagnostic_report_from_failure(failure)),
    };
    if let Err(error) = session.resolve_rust_facts(&modules) {
        return Ok(import_failure(error));
    }
    let typecheck = session.typecheck_unit(&modules, None, &mut |_, _| {});
    match typecheck {
        Ok(checked) => Ok(DiagnosticReport {
            schema_version: CHECK_REPORT_SCHEMA_VERSION,
            ok: true,
            diagnostics: stable_diagnostics_from(&checked.warnings),
            human_message: None,
            reproducible: true,
        }),
        Err(failure) => Ok(diagnostic_report_from_failure(failure)),
    }
}

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
        reproducible: failure.reproducible(),
    }
}

//! Checking one described unit (DD-0008), and the stable diagnostic report it produces.
//!
//! Checking reads imported Loaf artifacts and runs admitted vocabulary expansion before ordinary typechecking.
//! Callers render the returned report; collection, retained expansion errors and checking stay in this driver.
//!
//! A check reuses checked bodies per owner exactly as a build does: the engine's graph-selected records are installed
//! in the session, and the check reports which top-level function bodies it restored and which it checked afresh.

use std::path::Path;

use incan_frontend::checker_records::GraphBodyRecords;
use incan_frontend::diagnostics::{self, StableDiagnostic};
use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::typechecker::TypeChecker;
use serde::{Deserialize, Serialize};

use crate::diagnostics::{CliDiagnostic, CliDiagnosticFailure};
use crate::error::{CliError, CliResult};
use crate::modules::collect_modules_detailed_with_session;
use crate::session::{CompilationSession, RustFactsSetup};
use crate::unit::UnitDescription;

/// Schema of the check's body report, the sidecar an engine reads to count a check's body work.
pub const CHECKED_BODIES_SCHEMA: &str = "incan.checked-bodies/1";

/// The top-level function bodies of one check's project modules: restored from a selected record, or checked afresh.
#[derive(Debug, Clone, Default)]
pub struct CheckedBodies {
    /// Each checked project module, in check order. A module refused before checking contributes nothing.
    pub modules: Vec<ModuleBodies>,
}

/// One project module's top-level function bodies, by name.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ModuleBodies {
    /// The module's source path relative to the project root.
    pub module: String,
    /// Functions whose statements this check checked.
    pub checked: Vec<String>,
    /// Functions whose checked bodies this check restored from a selected record.
    pub reused: Vec<String>,
}

impl CheckedBodies {
    /// Record one checked module's top-level functions; modules outside the project are dependencies, not owners.
    fn observe(&mut self, project_roots: &[&Path], module: &ParsedModule, checker: &TypeChecker) {
        let Some(relative) = project_roots
            .iter()
            .find_map(|root| module.file_path.strip_prefix(root).ok())
        else {
            return;
        };
        let mut bodies = ModuleBodies {
            module: relative.to_string_lossy().into_owned(),
            ..ModuleBodies::default()
        };
        for declaration in &module.ast.declarations {
            if let incan_frontend::ast::Declaration::Function(function) = &declaration.node {
                if checker.restored_body_record(declaration.span) {
                    bodies.reused.push(function.name.clone());
                } else {
                    bodies.checked.push(function.name.clone());
                }
            }
        }
        self.modules.push(bodies);
    }

    /// The `incan.checked-bodies/1` report: totals, then each module's functions.
    pub fn to_json(&self) -> CliResult<String> {
        let checked: usize = self.modules.iter().map(|module| module.checked.len()).sum();
        let reused: usize = self.modules.iter().map(|module| module.reused.len()).sum();
        serde_json::to_string(&serde_json::json!({
            "schema": CHECKED_BODIES_SCHEMA,
            "checked": checked,
            "reused": reused,
            "modules": self.modules,
        }))
        .map_err(|error| CliError::failure(error.to_string()))
    }
}

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

/// Check one unit with the engine's selected body records, capturing the warnings it prints alongside its report.
/// Returns the presentation and the bodies the check restored or checked.
pub fn check_unit(
    unit: &UnitDescription,
    rust: &RustFactsSetup,
    records: GraphBodyRecords,
) -> CliResult<(CheckOutput, CheckedBodies)> {
    let (result, warnings) = crate::diagnostics::capture_warning_output(|| check_report(unit, rust, records));
    match result {
        Ok(mut report) => {
            let bodies = std::mem::take(&mut report.bodies);
            let output = CheckOutput {
                json: Some(
                    serde_json::to_string_pretty(&report).map_err(|error| CliError::failure(error.to_string()))?,
                ),
                human_message: report.human_message,
                exit_code: if report.ok { 0 } else { 1 },
                warnings,
                reproducible: report.reproducible,
            };
            Ok((output, bodies))
        }
        Err(error) => Ok((
            CheckOutput {
                json: None,
                human_message: Some(error.message),
                exit_code: error.exit_code.0,
                warnings,
                reproducible: false,
            },
            CheckedBodies::default(),
        )),
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
    /// The project's top-level function bodies this check restored or checked; never part of the public JSON report.
    #[serde(skip_serializing)]
    bodies: CheckedBodies,
}

/// Check one unit without rendering the result: its stable diagnostics, warnings included, and whether it passed.
/// `records` are the engine's graph-selected checked bodies; an owner they hold is restored instead of checked.
pub fn check_report(
    unit: &UnitDescription,
    rust: &RustFactsSetup,
    records: GraphBodyRecords,
) -> CliResult<DiagnosticReport> {
    check_report_observed(unit, rust, records, &mut |_, _| {})
}

/// Inspect completed semantic products in the same project and provider environment that performed the check.
fn check_report_observed(
    unit: &UnitDescription,
    rust: &RustFactsSetup,
    records: GraphBodyRecords,
    observe: &mut dyn FnMut(&ParsedModule, &TypeChecker),
) -> CliResult<DiagnosticReport> {
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
    session.set_graph_body_records(records);
    let modules = match collect_modules_detailed_with_session(unit.source.clone(), &session) {
        Ok(modules) => modules,
        Err(failure) => return Ok(diagnostic_report_from_failure(failure)),
    };
    if let Err(error) = session.resolve_rust_facts(&modules) {
        return Ok(import_failure(error));
    }
    // Module paths are physical; the root is compared as given and resolved, as checked-body publication does.
    let resolved_root = unit.project_root.canonicalize().ok();
    let mut roots = vec![unit.project_root.as_path()];
    roots.extend(resolved_root.as_deref());
    let mut bodies = CheckedBodies::default();
    let typecheck = session.typecheck_unit(&modules, None, &mut |module, checker| {
        bodies.observe(&roots, module, checker);
        observe(module, checker);
    });
    let mut report = match typecheck {
        Ok(checked) => DiagnosticReport {
            schema_version: CHECK_REPORT_SCHEMA_VERSION,
            ok: true,
            diagnostics: stable_diagnostics_from(&checked.warnings),
            human_message: None,
            reproducible: true,
            bodies: CheckedBodies::default(),
        },
        Err(failure) => diagnostic_report_from_failure(failure),
    };
    report.bodies = bodies;
    Ok(report)
}

/// Evaluate one ranked reader with the unit's real imported interfaces, Rust facts and source-level checker.
/// The caller receives fresh local and final products only if every other top-level owner restored its prior work.
pub fn ranked_reader(
    unit: &UnitDescription,
    rust: &RustFactsSetup,
    records: GraphBodyRecords,
    module: &str,
    identity: &str,
) -> CliResult<Option<incan_frontend::checker_records::CheckedOwnerRecordFields>> {
    use incan_frontend::checker_records::{CheckedOwnerRecordOutput, checked_record_outputs};
    let name = identity
        .split_once('|')
        .and_then(|(qualified, _)| qualified.strip_prefix("function:"))
        .and_then(|qualified| qualified.rsplit_once("::"))
        .map(|(_, name)| name)
        .ok_or_else(|| CliError::failure("ranked demand has no project function identity"))?;
    let source = unit.project_root.join(module);
    let mut selected = None;
    let exact = records.for_ranked_owner(identity);
    let report = check_report_observed(unit, rust, exact.clone(), &mut |current, checker| {
        if current.file_path == source {
            selected = Some(checked_record_outputs(
                &current.ast,
                checker,
                &current.path_segments,
                Some(name),
            ));
        }
    })?;
    match exact.ranked_admission() {
        Some((0..=1, true, false)) => return Ok(None),
        Some((1, false, false)) => {}
        state => {
            return Err(CliError::failure(format!(
                "ranked reader has invalid statement-entry evidence: {state:?}"
            )));
        }
    }
    if !report.ok {
        return Err(CliError::failure(report.human_message.unwrap_or_else(|| {
            format!("ranked checker refused: {:?}", report.diagnostics)
        })));
    }
    let checked: Vec<_> = report
        .bodies
        .modules
        .iter()
        .flat_map(|entry| entry.checked.iter().map(move |name| (&entry.module, name)))
        .collect();
    if checked.len() != 1 || checked[0].0 != module || checked[0].1 != name {
        return Err(CliError::failure(format!(
            "ranked checker did not evaluate exactly its selected owner: {checked:?}"
        )));
    }
    let outputs = selected
        .ok_or_else(|| CliError::failure("ranked checker did not visit the certified module"))?
        .map_err(CliError::failure)?;
    match outputs.into_iter().next() {
        Some(CheckedOwnerRecordOutput::Value(fields)) if fields.identity == identity => Ok(Some(fields)),
        Some(CheckedOwnerRecordOutput::Fallback(refusal)) => Err(CliError::failure(format!(
            "ranked owner is a full-source fallback: {}",
            refusal.reason
        ))),
        _ => Err(CliError::failure(
            "ranked checker did not produce the requested stable identity",
        )),
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
        bodies: CheckedBodies::default(),
    }
}

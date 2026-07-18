//! Stable diagnostic CLI surfaces.
//!
//! `incan check` and `incan explain` expose the same compiler diagnostics as the legacy debug flags, but with stable
//! JSON output and catalog-backed explanations for tooling consumers.

use std::env;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use serde::Serialize;

use crate::cli::{CliError, CliResult, ExitCode};
use crate::frontend::diagnostics::{self, DIAGNOSTIC_SCHEMA_VERSION, StableDiagnostic};
#[cfg(feature = "rust_inspect")]
use crate::lockfile::CargoFeatureSelection;
use crate::provider::FeatureSelection;

#[cfg(feature = "rust_inspect")]
use super::common::CargoPolicy;
use super::common::{
    CliDiagnosticFailure, CompilationSession, collect_modules_detailed_with_session, resolve_project_root,
    typecheck_modules_with_import_graph_detailed,
};
#[cfg(feature = "rust_inspect")]
use super::lock::{RustInspectTypecheckRequest, prepare_rust_inspect_typecheck_workspace};

/// Output format for stable diagnostics commands.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticOutputFormat {
    Text,
    Json,
}

/// One complete typecheck result, retained independently from CLI rendering so workspace orchestration can emit one
/// coherent machine-readable report for several members.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct DiagnosticReport {
    schema_version: u32,
    ok: bool,
    diagnostics: Vec<StableDiagnostic>,
    #[serde(skip_serializing)]
    human_message: Option<String>,
}

impl DiagnosticReport {
    /// Whether collection and typechecking succeeded without diagnostics.
    pub(crate) fn ok(&self) -> bool {
        self.ok
    }

    /// Human-readable diagnostics retained from the compiler pipeline when the report is unsuccessful.
    pub(crate) fn human_message(&self) -> Option<&str> {
        self.human_message.as_deref()
    }
}

#[derive(Debug, Serialize)]
struct ExplainReport {
    schema_version: u32,
    found: bool,
    entry: diagnostics::DiagnosticCatalogEntry,
}

/// Run the canonical check pipeline for a file or project entrypoint.
pub fn check_path(path: &Path, format: DiagnosticOutputFormat) -> CliResult<ExitCode> {
    check_path_with_features(path, format, &FeatureSelection::default())
}

/// Run the canonical check pipeline for an explicit Incan package-feature projection.
pub fn check_path_with_features(
    path: &Path,
    format: DiagnosticOutputFormat,
    feature_selection: &FeatureSelection,
) -> CliResult<ExitCode> {
    check_path_with_selections(path, format, feature_selection, None)
}

/// Run the canonical check pipeline for explicit package-feature and transient SDK-profile selections.
pub fn check_path_with_selections(
    path: &Path,
    format: DiagnosticOutputFormat,
    feature_selection: &FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> CliResult<ExitCode> {
    let report = check_path_report_with_selections(path, feature_selection, sdk_profile_override)?;
    render_check_report(&report, format)
}

/// Run the canonical check pipeline for one package-feature and SDK-profile projection without rendering it.
///
/// This is the single-project compiler boundary used by both `incan check` and RFC 077 workspace command fan-out. It
/// deliberately retains stable diagnostics rather than printing them as soon as a member fails.
pub(crate) fn check_path_report_with_selections(
    path: &Path,
    feature_selection: &FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> CliResult<DiagnosticReport> {
    let normalized_path = normalize_input_path(path)?;
    let compilation_session =
        match CompilationSession::discover_with_selections(&normalized_path, feature_selection, sdk_profile_override) {
            Ok(session) => session,
            Err(error) => {
                let failure = CliDiagnosticFailure::single(
                    normalized_path.to_string_lossy(),
                    "",
                    diagnostics::CompileError::new(error.to_string(), crate::frontend::ast::Span::default()),
                    diagnostics::DiagnosticPhase::Import,
                );
                return Ok(diagnostic_report_from_failure(failure));
            }
        };
    let modules = match collect_modules_detailed_with_session(normalized_path.clone(), &compilation_session) {
        Ok(modules) => modules,
        Err(failure) => return Ok(diagnostic_report_from_failure(failure)),
    };
    let project_root = resolve_project_root(&normalized_path);
    let manifest = compilation_session.manifest.clone();
    let library_manifest_index = compilation_session.library_manifest_index.clone();
    let provider_plan = compilation_session.provider_plan_for_modules(&modules, true)?;
    #[cfg(feature = "rust_inspect")]
    let project_name = manifest
        .as_ref()
        .and_then(|manifest| manifest.project.as_ref().and_then(|project| project.name.clone()))
        .or_else(|| {
            normalized_path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| "incan_check".to_string());
    #[cfg(feature = "rust_inspect")]
    let cargo_features = CargoFeatureSelection::default().normalized();
    #[cfg(feature = "rust_inspect")]
    let cargo_policy = CargoPolicy::default();
    #[cfg(feature = "rust_inspect")]
    let rust_inspect_manifest_dir = prepare_rust_inspect_typecheck_workspace(RustInspectTypecheckRequest {
        project_root: &project_root,
        project_name: project_name.as_str(),
        manifest: manifest.as_ref(),
        modules: &modules,
        library_manifest_index: &library_manifest_index,
        cargo_features: &cargo_features,
        cargo_policy: &cargo_policy,
        rust_edition: manifest
            .as_ref()
            .and_then(|manifest| manifest.build.as_ref().and_then(|build| build.rust_edition.clone())),
    })?;

    match typecheck_modules_with_import_graph_detailed(
        &modules,
        manifest.as_ref(),
        &provider_plan,
        #[cfg(feature = "rust_inspect")]
        rust_inspect_manifest_dir.as_deref(),
    ) {
        Ok(()) => Ok(DiagnosticReport {
            schema_version: DIAGNOSTIC_SCHEMA_VERSION,
            ok: true,
            diagnostics: Vec::new(),
            human_message: None,
        }),
        Err(failure) => Ok(diagnostic_report_from_failure(failure)),
    }
}

/// Print a catalog-backed diagnostic explanation.
pub fn explain_diagnostic(code: &str, format: DiagnosticOutputFormat) -> CliResult<ExitCode> {
    if let Some(entry) = diagnostics::explain(code) {
        match format {
            DiagnosticOutputFormat::Text => {
                println!("{}", format_explain_text(entry));
                Ok(ExitCode::SUCCESS)
            }
            DiagnosticOutputFormat::Json => {
                let report = ExplainReport {
                    schema_version: DIAGNOSTIC_SCHEMA_VERSION,
                    found: true,
                    entry: *entry,
                };
                print_json(&report)?;
                Ok(ExitCode::SUCCESS)
            }
        }
    } else {
        let unknown = diagnostics::explain("INCAN-U0001")
            .ok_or_else(|| CliError::failure("internal error: missing INCAN-U0001 diagnostic catalog entry"))?;
        match format {
            DiagnosticOutputFormat::Text => Err(CliError::failure(format!(
                "Unknown diagnostic code `{code}`.\n\n{}",
                format_explain_text(unknown)
            ))),
            DiagnosticOutputFormat::Json => {
                let report = ExplainReport {
                    schema_version: DIAGNOSTIC_SCHEMA_VERSION,
                    found: false,
                    entry: *unknown,
                };
                print_json(&report)?;
                Err(CliError::new("", ExitCode::FAILURE))
            }
        }
    }
}

/// Render one already-evaluated check result in either human text or the stable JSON report shape.
fn render_check_report(report: &DiagnosticReport, format: DiagnosticOutputFormat) -> CliResult<ExitCode> {
    match format {
        DiagnosticOutputFormat::Text => {
            if report.ok {
                println!("✓ Type check passed!");
                Ok(ExitCode::SUCCESS)
            } else {
                Err(CliError::failure(render_check_report_human(report)))
            }
        }
        DiagnosticOutputFormat::Json => {
            print_json(report)?;
            if report.ok {
                Ok(ExitCode::SUCCESS)
            } else {
                Err(CliError::new("", ExitCode::FAILURE))
            }
        }
    }
}

/// Convert source-aware compiler failures into the stable report consumed by both single-project and workspace paths.
fn diagnostic_report_from_failure(failure: CliDiagnosticFailure) -> DiagnosticReport {
    let human_message = failure.render_human();
    let diagnostics = failure
        .diagnostics
        .iter()
        .map(|diagnostic| {
            diagnostics::stable_diagnostic(
                &diagnostic.file_path,
                &diagnostic.source,
                &diagnostic.error,
                diagnostic.phase,
            )
        })
        .collect();
    DiagnosticReport {
        schema_version: DIAGNOSTIC_SCHEMA_VERSION,
        ok: false,
        diagnostics,
        human_message: Some(human_message),
    }
}

/// Render diagnostics for the human CLI without rebuilding source context in the workspace orchestration layer.
fn render_check_report_human(report: &DiagnosticReport) -> String {
    report
        .human_message
        .clone()
        .unwrap_or_else(|| "type check failed without diagnostics".to_string())
}

/// Pretty-print a serializable diagnostics payload to stdout.
fn print_json<T: Serialize>(value: &T) -> CliResult<()> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|error| CliError::failure(format!("failed to serialize diagnostic JSON: {error}")))?;
    println!("{json}");
    Ok(())
}

/// Format one catalog entry for the default `incan explain` human output.
fn format_explain_text(entry: &diagnostics::DiagnosticCatalogEntry) -> String {
    let mut text = String::new();
    text.push_str(entry.code);
    text.push_str(": ");
    text.push_str(entry.title);
    text.push('\n');
    text.push_str(entry.summary);
    text.push_str("\n\n");
    text.push_str(entry.explanation);
    if !entry.common_causes.is_empty() {
        text.push_str("\n\nCommon causes:");
        for cause in entry.common_causes {
            text.push_str("\n- ");
            text.push_str(cause);
        }
    }
    if !entry.fixes.is_empty() {
        text.push_str("\n\nFixes:");
        for fix in entry.fixes {
            text.push_str("\n- ");
            text.push_str(fix);
        }
    }
    if let Some(url) = entry.docs_url {
        text.push_str("\n\nDocs: ");
        text.push_str(url);
    }
    text
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

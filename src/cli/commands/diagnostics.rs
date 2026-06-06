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
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::manifest::ProjectManifest;

use super::common::{
    CliDiagnosticFailure, collect_modules_detailed, resolve_project_root, typecheck_modules_with_import_graph_detailed,
};

/// Output format for stable diagnostics commands.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticOutputFormat {
    Text,
    Json,
}

#[derive(Debug, Serialize)]
struct DiagnosticReport {
    schema_version: u32,
    ok: bool,
    diagnostics: Vec<StableDiagnostic>,
}

#[derive(Debug, Serialize)]
struct ExplainReport {
    schema_version: u32,
    found: bool,
    entry: diagnostics::DiagnosticCatalogEntry,
}

/// Run the canonical check pipeline for a file or project entrypoint.
pub fn check_path(path: &Path, format: DiagnosticOutputFormat) -> CliResult<ExitCode> {
    let modules = match collect_modules_detailed(&path.to_string_lossy()) {
        Ok(modules) => modules,
        Err(failure) => return render_check_failure(failure, format),
    };
    let normalized_path = normalize_input_path(path)?;
    let project_root = resolve_project_root(&normalized_path);
    let manifest = match ProjectManifest::discover(&project_root) {
        Ok(manifest) => manifest,
        Err(error) => {
            let failure = CliDiagnosticFailure::single(
                normalized_path.to_string_lossy(),
                "",
                diagnostics::CompileError::new(error.to_string(), crate::frontend::ast::Span::default()),
                diagnostics::DiagnosticPhase::Tooling,
            );
            return render_check_failure(failure, format);
        }
    };
    let library_manifest_index = manifest
        .as_ref()
        .map(LibraryManifestIndex::from_project_manifest)
        .unwrap_or_default();

    match typecheck_modules_with_import_graph_detailed(
        &modules,
        manifest.as_ref(),
        &library_manifest_index,
        #[cfg(feature = "rust_inspect")]
        None,
    ) {
        Ok(()) => render_check_success(format),
        Err(failure) => render_check_failure(failure, format),
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

/// Render a successful check result in either human text or the stable JSON report shape.
fn render_check_success(format: DiagnosticOutputFormat) -> CliResult<ExitCode> {
    match format {
        DiagnosticOutputFormat::Text => {
            println!("✓ Type check passed!");
            Ok(ExitCode::SUCCESS)
        }
        DiagnosticOutputFormat::Json => {
            let report = DiagnosticReport {
                schema_version: DIAGNOSTIC_SCHEMA_VERSION,
                ok: true,
                diagnostics: Vec::new(),
            };
            print_json(&report)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Render failed collection or typechecking diagnostics without losing structured JSON context.
fn render_check_failure(failure: CliDiagnosticFailure, format: DiagnosticOutputFormat) -> CliResult<ExitCode> {
    match format {
        DiagnosticOutputFormat::Text => Err(CliError::failure(failure.render_human())),
        DiagnosticOutputFormat::Json => {
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
            let report = DiagnosticReport {
                schema_version: DIAGNOSTIC_SCHEMA_VERSION,
                ok: false,
                diagnostics,
            };
            print_json(&report)?;
            Err(CliError::new("", ExitCode::FAILURE))
        }
    }
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

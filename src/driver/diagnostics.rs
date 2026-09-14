//! Diagnostics as a command outcome: the failure shape every driver stage returns when the program does not
//! check, and the warning renderers the stages share.
//!
//! `CliDiagnosticFailure` keeps the name it had while it lived under the CLI so its many constructors do not churn
//! in this move; it is the driver's type. The CLI renders it, the LSP maps it to publish-diagnostics, codegraph
//! records it — none of them shape it.

use crate::driver::error::CliError;
use crate::frontend::ast::Span;
use crate::frontend::diagnostics;
use crate::frontend::parsed_module::ParsedModule;
/// One compiler diagnostic with enough source context for either human or machine-readable rendering.
#[derive(Debug, Clone)]
pub(crate) struct CliDiagnostic {
    pub file_path: String,
    pub source: String,
    pub error: diagnostics::CompileError,
    pub phase: diagnostics::DiagnosticPhase,
}

/// Structured failure produced by shared CLI collection/typechecking helpers.
#[derive(Debug, Clone)]
pub(crate) struct CliDiagnosticFailure {
    pub diagnostics: Vec<CliDiagnostic>,
    /// Non-fatal diagnostics gathered before the failure, reported but never re-rendered.
    ///
    /// Kept apart from `diagnostics` because these have already been printed to stderr as they were produced.
    /// Folding them in would print them twice for a human, while dropping them would make the machine-readable
    /// report inconsistent: a file with both a warning and an error would report only the error, even though the
    /// same file reports the warning fine when it compiles.
    pub warnings: Vec<CliDiagnostic>,
}

impl CliDiagnosticFailure {
    /// Build one structured diagnostic failure while preserving the source text needed for JSON span projection.
    pub(crate) fn single(
        file_path: impl Into<String>,
        source: impl Into<String>,
        error: diagnostics::CompileError,
        phase: diagnostics::DiagnosticPhase,
    ) -> Self {
        Self {
            diagnostics: vec![CliDiagnostic {
                file_path: file_path.into(),
                source: source.into(),
                error,
                phase,
            }],
            warnings: Vec::new(),
        }
    }

    /// Build one structured failure from parser or typechecker errors that all belong to the same source file.
    pub(crate) fn from_errors(
        file_path: impl Into<String>,
        source: impl Into<String>,
        errors: Vec<diagnostics::CompileError>,
        phase: diagnostics::DiagnosticPhase,
    ) -> Self {
        let file_path = file_path.into();
        let source = source.into();
        Self {
            diagnostics: errors
                .into_iter()
                .map(|error| CliDiagnostic {
                    file_path: file_path.clone(),
                    source: source.clone(),
                    error,
                    phase,
                })
                .collect(),
            warnings: Vec::new(),
        }
    }

    /// Render the failing diagnostics through the existing source-highlighted human diagnostic formatter.
    ///
    /// Warnings are deliberately excluded: they were already shown when they were produced.
    pub(crate) fn render_human(&self) -> String {
        let mut rendered = String::new();
        for diagnostic in &self.diagnostics {
            rendered.push_str(&diagnostics::format_error(
                &diagnostic.file_path,
                &diagnostic.source,
                &diagnostic.error,
            ));
            rendered.push('\n');
        }
        rendered.trim_end().to_string()
    }
}

impl From<CliError> for CliDiagnosticFailure {
    fn from(error: CliError) -> Self {
        Self::single(
            "<command>",
            "",
            diagnostics::CompileError::new(error.message, Span::default()),
            diagnostics::DiagnosticPhase::Tooling,
        )
    }
}

/// Classify diagnostics that are still emitted by the typechecker but originate from an import declaration span.
pub(crate) fn typecheck_diagnostic_phase(module: &ParsedModule, span: Span) -> diagnostics::DiagnosticPhase {
    diagnostics::phase_for_typecheck_span(&module.ast, span)
}

/// Render one module's non-fatal diagnostics to stderr in the shared CLI format.
///
/// Warnings never fail a command, so they are shown the moment they are produced rather than held until a
/// machine-readable report is assembled: `run`, `build`, and `fmt` invocations that never emit JSON must still
/// surface them.
pub(crate) fn render_module_warnings(file_path: &str, source: &str, warnings: &[diagnostics::CompileError]) {
    for warning in warnings {
        eprint!("{}", diagnostics::format_error(file_path, source, warning));
    }
}

/// Project one module's non-fatal parser diagnostics into the structured shape machine-readable reports consume.
///
/// Parser warnings are read back off [`ParsedModule::ast`] rather than captured at parse time because the parse
/// pass is shared by commands that never typecheck; keeping collection here lets the report gather both warning
/// classes in one deterministic sweep without changing when the user first sees them.
pub(crate) fn parser_warning_diagnostics(module: &ParsedModule) -> Vec<CliDiagnostic> {
    module
        .ast
        .warnings
        .iter()
        .map(|warning| CliDiagnostic {
            file_path: module.file_path.to_string_lossy().to_string(),
            source: module.source.clone(),
            phase: diagnostics::DiagnosticPhase::Parse,
            error: warning.clone(),
        })
        .collect()
}

/// Project one module's non-fatal typechecker diagnostics into the same structured shape.
///
/// Warnings and errors share [`CliDiagnostic`] deliberately: severity already travels on
/// `CompileError::kind` and survives into `StableDiagnostic::severity`, so which collection a diagnostic arrived
/// in carries no information the payload does not already hold.
pub(crate) fn typecheck_warning_diagnostics(
    module: &ParsedModule,
    warnings: &[diagnostics::CompileError],
) -> Vec<CliDiagnostic> {
    warnings
        .iter()
        .map(|warning| CliDiagnostic {
            file_path: module.file_path.to_string_lossy().to_string(),
            source: module.source.clone(),
            phase: typecheck_diagnostic_phase(module, warning.span),
            error: warning.clone(),
        })
        .collect()
}

// ============================================================================
// Tests
// ============================================================================

//! Stable diagnostic CLI surface: `incan check` renders the compiler's diagnostics as human text or as the stable
//! JSON report for tooling consumers.

use incan_driver::error::{CliError, CliResult, ExitCode};

/// Output format for stable diagnostics commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiagnosticOutputFormat {
    /// Human-readable diagnostics.
    Text,
    /// The stable JSON check report.
    Json,
}

/// Render one already-evaluated check result (see `incan_driver::check::check_unit`) in either human text or the
/// stable JSON report shape.
pub(crate) fn render_check_report(
    report: incan_driver::check::CheckOutput,
    format: DiagnosticOutputFormat,
) -> CliResult<ExitCode> {
    for warning in report.warnings {
        eprint!("{warning}");
    }
    if format == DiagnosticOutputFormat::Json
        && let Some(json) = report.json
    {
        println!("{json}");
        return if report.exit_code == 0 {
            Ok(ExitCode::SUCCESS)
        } else {
            Err(CliError::new("", ExitCode(report.exit_code)))
        };
    }
    if report.exit_code == 0 {
        println!("✓ Type check passed!");
        Ok(ExitCode::SUCCESS)
    } else {
        Err(CliError::new(
            report
                .human_message
                .unwrap_or_else(|| "type check failed without diagnostics".to_string()),
            ExitCode(report.exit_code),
        ))
    }
}

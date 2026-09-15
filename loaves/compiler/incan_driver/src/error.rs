//! The error a driver stage returns: a rendered message and the process exit code it implies.
//!
//! Exit codes are semantic in Incan — a refused Cargo invocation, a diagnostics failure, a missing toolchain each
//! have theirs — and they are decided where the failure is understood, which is the driver. The CLI only carries
//! the code to the process boundary.

use oven_rustc::plan::OvenPlanError;
use oven_rustc::rustc::OvenRustcError;

use std::fmt;
/// Exit code for CLI operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitCode(pub i32);

impl ExitCode {
    pub const SUCCESS: ExitCode = ExitCode(0);
    pub const FAILURE: ExitCode = ExitCode(1);
}

/// Error type for CLI operations.
///
/// Contains a user-facing message and an exit code. The CLI entry point catches these errors, prints the message, and
/// exits with the code.
#[derive(Debug)]
pub struct CliError {
    /// User-facing error message (already formatted for display)
    pub message: String,
    /// Exit code to return to the shell
    pub exit_code: ExitCode,
}

impl CliError {
    /// Create a new CLI error with a message and exit code.
    pub fn new(message: impl Into<String>, exit_code: ExitCode) -> Self {
        Self {
            message: message.into(),
            exit_code,
        }
    }

    /// Create a failure error (exit code 1).
    pub fn failure(message: impl Into<String>) -> Self {
        Self::new(message, ExitCode::FAILURE)
    }

    /// Create an error with a custom exit code.
    pub fn with_code(message: impl Into<String>, code: i32) -> Self {
        Self::new(message, ExitCode(code))
    }
}

impl fmt::Display for CliError {
    /// Render the user-facing CLI error message.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CliError {}

/// Result type for CLI operations.
pub type CliResult<T> = Result<T, CliError>;

impl From<incan_provider::error::ProviderError> for CliError {
    /// A provider failure is a command failure with the provider's message; the provider never chooses an exit code.
    fn from(error: incan_provider::error::ProviderError) -> Self {
        Self::failure(error.message)
    }
}

/// Render a plan selection or composition refusal for the CLI, keeping direct-rustc transcripts intact.
pub fn oven_plan_error(error: OvenPlanError) -> CliError {
    match error {
        OvenPlanError::Rustc(error) => oven_rustc_error(error),
        OvenPlanError::Selection(message) => CliError::failure(message),
    }
}

/// Preserve direct-rustc diagnostics rather than reducing a normal Oven compilation failure to a generic status.
pub fn oven_rustc_error(error: OvenRustcError) -> CliError {
    match error {
        OvenRustcError::CompilationFailed { report } => {
            let rendered = report
                .diagnostics
                .into_iter()
                .map(|diagnostic| {
                    diagnostic
                        .rendered
                        .unwrap_or_else(|| format!("{}: {}", diagnostic.level, diagnostic.message))
                })
                .collect::<Vec<_>>()
                .join("\n");
            let mut output = format!("{rendered}\n{}", report.unstructured_output).trim().to_string();
            if let Some(invocation) = report.invocation {
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str("direct rustc invocation: ");
                output.push_str(&invocation);
            }
            CliError::failure(if output.is_empty() {
                "Oven direct-rustc compilation failed without a diagnostic transcript".to_string()
            } else {
                format!("Oven direct-rustc compilation failed:\n{output}")
            })
        }
        error => CliError::failure(error.to_string()),
    }
}

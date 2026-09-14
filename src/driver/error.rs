//! The error a driver stage returns: a rendered message and the process exit code it implies.
//!
//! Exit codes are semantic in Incan — a refused Cargo invocation, a diagnostics failure, a missing toolchain each
//! have theirs — and they are decided where the failure is understood, which is the driver. The CLI only carries
//! the code to the process boundary.

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
/// Contains a user-facing message and an exit code. The CLI entry point
/// catches these errors, prints the message, and exits with the code.
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

impl From<crate::provider::error::ProviderError> for CliError {
    /// A provider failure is a command failure with the provider's message; the provider never chooses an exit code.
    fn from(error: crate::provider::error::ProviderError) -> Self {
        Self::failure(error.message)
    }
}

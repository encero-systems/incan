//! Command-line entrypoint for Linux bootstrap tool evidence.

use std::process::ExitCode;

/// Parse and execute one tool-output evidence operation.
fn main() -> ExitCode {
    match incan_ci_tools::tool_outputs::run_from_environment() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!(
                "{}",
                serde_json::json!({"status": "unavailable", "detail": error.to_string()})
            );
            ExitCode::FAILURE
        }
    }
}

//! Command-line entrypoint for Linux bootstrap tool evidence.

use std::process::ExitCode;

use incan_ci_tools::tool_outputs::Completion;

/// Parse and execute one tool-output evidence operation.
///
/// An unavailable operation exits successfully: its fail-closed outputs send CI down the cold compiler build.
fn main() -> ExitCode {
    match incan_ci_tools::tool_outputs::run_from_environment() {
        Ok(Completion::Done) => ExitCode::SUCCESS,
        Ok(Completion::Unavailable(error)) => {
            eprintln!(
                "{}",
                serde_json::json!({"status": "unavailable", "detail": error.to_string()})
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!(
                "{}",
                serde_json::json!({"status": "unpublished", "detail": error.to_string()})
            );
            ExitCode::FAILURE
        }
    }
}

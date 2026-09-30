//! Command-line entrypoint for the Cargo manifest retirement inventory.

use std::process::ExitCode;

/// Parse and execute one manifest inventory operation.
fn main() -> ExitCode {
    match incan_ci_tools::manifest_inventory::run_from_environment() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

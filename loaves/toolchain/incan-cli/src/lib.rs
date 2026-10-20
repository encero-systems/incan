//! The compiler's commands: checking, explaining, formatting and inspecting one unit the Oven describes (DD-0008).
//!
//! The `incan` command line is the Oven's. It reads the project manifest, describes each unit and calls these
//! functions; building, running and testing compile units through `incan-rustc-driver`.

#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]

pub mod commands;

pub use incan_driver::error::{CliError, CliResult, ExitCode};
/// Shared opt-in invocation diagnostics.
pub use incan_driver::invocation_trace;

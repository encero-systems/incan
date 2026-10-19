//! Executable builds on the direct route: the program and its Loaf dependencies from source, their Rust facets from
//! the Oven native graph, compiled by the pinned driver (`incan-rustc-driver --source`).

use super::BuildCommandOptions;
use crate::error::{CliError, CliResult};

/// Compile one executable through the direct route and publish its completed output.
///
/// Completed-output reuse has already missed when this is called.
pub(super) fn build(
    file_path: &str,
    _output_dir: Option<&String>,
    _options: &BuildCommandOptions,
    _progress: &mut impl FnMut(String),
) -> CliResult<serde_json::Value> {
    Err(CliError::failure(format!(
        "{file_path}: direct-route executable builds are not wired into `incan build` yet (dev.8 fix stage F2)"
    )))
}

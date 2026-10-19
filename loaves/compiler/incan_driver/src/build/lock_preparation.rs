//! Canonical explicit lock preparation in the executable owner.
//!
//! Resolution and publication stay in `crate::lock`; this host adapter only validates the request and publishes.

use crate::error::{CliError, CliResult};
use crate::features::FeatureSelection;
use oven_model::lock::CargoFeatureSelection;
use oven_model::manifest::ProjectManifest;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Existing lock selections transported to the same owner as build, check and tests.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockPreparationRequest {
    /// Absolute discovery directory; the owner inherits the command's original working directory.
    pub directory: PathBuf,
    /// Optional explicit source entrypoint, normalized by the client before transport.
    pub entry: Option<PathBuf>,
    /// Existing package feature selection.
    pub features: FeatureSelection,
    /// Existing Rust feature selection; workspace semantics remain in the canonical collector.
    pub cargo_features: CargoFeatureSelection,
}

/// Collect the project's requirements and publish its canonical lock.
pub(super) fn execute_request(request: &LockPreparationRequest) -> CliResult<()> {
    let _trace = tracing::trace_span!(target: "incan::invocation", "incan_driver::prepare_project_lock").entered();
    if !request.directory.is_absolute() || request.entry.as_ref().is_some_and(|path| !path.is_absolute()) {
        return Err(CliError::failure("owner requires absolute lock request paths"));
    }
    let manifest = ProjectManifest::discover(&request.directory)
        .map_err(|error| CliError::failure(error.to_string()))?
        .ok_or_else(|| CliError::failure("No loaf.toml found (run `incan init`)"))?;
    crate::project::enforce_project_toolchain_constraint(&manifest)?;
    crate::lock::resolution::collect_and_publish_project_lock(
        &manifest,
        request.entry.as_deref(),
        &request.cargo_features,
        &request.features,
    )
    .map(|_| ())
}

//! The `incan lock` command: normalize command selections and submit lock preparation to the existing project owner.
//!
//! Resolution, workspace collection, registry-source authorities and the rust-inspect workspace all live in
//! `incan_driver::lock`; this file only turns arguments into one request.

use std::path::PathBuf;

use crate::{CliError, CliResult, ExitCode};
use incan_driver::features::FeatureSelection;
use oven_model::lock::CargoFeatureSelection;

/// Generate or update oven.lock for a project.
pub fn lock_project(
    entry_file: Option<&PathBuf>,
    package_features: &FeatureSelection,
) -> CliResult<ExitCode> {
    let start_dir = entry_file
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    crate::project_owner::prepare_lock(incan_driver::build::lock_preparation::LockPreparationRequest {
        directory: std::path::absolute(&start_dir).map_err(|error| CliError::failure(error.to_string()))?,
        entry: entry_file
            .map(std::path::absolute)
            .transpose()
            .map_err(|error| CliError::failure(error.to_string()))?,
        features: package_features.clone(),
        cargo_features: CargoFeatureSelection::default(),
    })?;

    Ok(ExitCode::SUCCESS)
}

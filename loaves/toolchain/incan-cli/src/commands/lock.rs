//! The `incan lock` command: discover the project, apply the Cargo feature selection, and publish its lock through
//! the driver.
//!
//! Resolution, workspace collection, registry-source authorities and the rust-inspect workspace all live in
//! `incan_driver::lock`; this file only turns arguments into one request.

use std::path::PathBuf;

use crate::{CliError, CliResult, ExitCode};
use incan_driver::cargo_policy::enforce_project_toolchain_constraint;
use incan_driver::lock::resolution::collect_and_publish_project_lock;
use incan_provider::FeatureSelection;
use oven_model::lock::CargoFeatureSelection;
use oven_model::manifest::ProjectManifest;

/// Generate or update oven.lock for a project.
pub fn lock_project(
    entry_file: Option<&PathBuf>,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
) -> CliResult<ExitCode> {
    let start_dir = entry_file
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let manifest = ProjectManifest::discover(&start_dir)
        .map_err(|e| CliError::failure(e.to_string()))?
        .ok_or_else(|| CliError::failure("No loaf.toml found (run `incan init`)"))?;
    enforce_project_toolchain_constraint(&manifest)?;

    let cargo_features = CargoFeatureSelection {
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    }
    .normalized();
    let _ = collect_and_publish_project_lock(
        &manifest,
        entry_file.map(PathBuf::as_path),
        &cargo_features,
        package_features,
        sdk_profile_override,
    )?;

    Ok(ExitCode::SUCCESS)
}

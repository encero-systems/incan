//! Machine-readable public package-feature inspection from RFC 114.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use serde::Serialize;

use crate::{CliError, CliResult, ExitCode};
use incan_driver::features::{FeatureActivationReason, FeatureSelection, PackageFeatureGraph};
use incan_driver::project::resolve_project_root;
use oven_model::manifest::ProjectManifest;

/// Human or JSON output for feature inspection.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureInspectionFormat {
    Text,
    Json,
}

#[derive(Debug, Serialize)]
struct FeaturesReport {
    schema_version: u32,
    packages: Vec<PackageFeaturesReport>,
}

#[derive(Debug, Serialize)]
struct PackageFeaturesReport {
    package: String,
    project_root: PathBuf,
    active_features: BTreeSet<String>,
    active_optional_dependencies: BTreeSet<String>,
    dependency_features: BTreeMap<String, BTreeSet<String>>,
    reasons: BTreeMap<String, BTreeSet<FeatureActivationReason>>,
}

/// Inspect the active feature projection of the package containing `path`.
pub fn inspect_features(
    path: &Path,
    format: FeatureInspectionFormat,
    feature_selection: &FeatureSelection,
) -> CliResult<ExitCode> {
    let input = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))?
            .join(path)
    };
    let manifest = ProjectManifest::discover(&resolve_project_root(&input))
        .map_err(|error| CliError::failure(error.to_string()))?;
    let mut packages = Vec::new();
    if let Some(manifest) = manifest {
        let graph = PackageFeatureGraph::from_manifest(&manifest).map_err(|error| CliError::failure(error.to_string()))?;
        let resolved = graph
            .resolve(feature_selection)
            .map_err(|error| CliError::failure(error.to_string()))?;
        packages.push(PackageFeaturesReport {
            package: manifest
                .project
                .as_ref()
                .and_then(|project| project.name.clone())
                .unwrap_or_default(),
            project_root: manifest.project_root().to_path_buf(),
            active_features: resolved.active_features,
            active_optional_dependencies: resolved.active_optional_dependencies,
            dependency_features: resolved.dependency_features,
            reasons: resolved.reasons,
        });
    }
    emit_feature_report(&FeaturesReport { schema_version: 2, packages }, format)?;
    Ok(ExitCode::SUCCESS)
}

/// Render one feature report as JSON or a short text summary.
fn emit_feature_report(report: &FeaturesReport, format: FeatureInspectionFormat) -> CliResult<()> {
    match format {
        FeatureInspectionFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(report)
                .map_err(|error| CliError::failure(format!("failed to serialize feature inspection: {error}")))?
        ),
        FeatureInspectionFormat::Text => {
            if report.packages.is_empty() {
                println!("No package feature graph is active.");
            }
            for package in &report.packages {
                println!(
                    "Package {}: {}",
                    package.package,
                    package.active_features.iter().cloned().collect::<Vec<_>>().join(", ")
                );
            }
        }
    }
    Ok(())
}

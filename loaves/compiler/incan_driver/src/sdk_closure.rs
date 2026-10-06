//! SDK source-authority publication from a retained, directly compiled Loaf closure.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use oven_rustc::sdk_closure::SdkCompiledClosure;
use rust_inspect::{
    OVEN_DIRECT_LOAF_PROJECT_FILE, OvenInspectionRegistrySource, write_sealed_oven_inspection_source_authority,
};

use crate::error::CliError;

/// Publish the successfully compiled SDK subgraph and preserve named unavailable units alongside it.
///
/// The caller retains the closure while linking SDK components and inspecting their Rust facets. This authority
/// covers only the compiled units, and does not establish that every SDK component can be published. Component
/// publication must omit unavailable roots and preserve their named import diagnostics.
pub fn seal_sdk_closure_inspection_sources(
    closure: &SdkCompiledClosure,
    authority_root: &Path,
) -> Result<PathBuf, CliError> {
    let mut sources = BTreeMap::new();
    for unit in closure.units() {
        let source_root = unit.source_root();
        let source_digest =
            oven_store::digest_source_tree(&source_root).map_err(|error| CliError::failure(error.to_string()))?;
        let binding = unit.binding();
        let package = binding.loaf.strip_prefix("crates-io/").ok_or_else(|| {
            CliError::failure(format!(
                "SDK source Loaf has no adopted registry coordinate: {}",
                binding.loaf
            ))
        })?;
        sources
            .entry((
                package.to_string(),
                binding.version.clone(),
                binding.features.clone(),
                source_root.clone(),
            ))
            .or_insert_with(|| OvenInspectionRegistrySource {
                package: package.to_string(),
                version: binding.version.clone(),
                registry: "registry+incan.pub/crates-io".to_string(),
                checksum: binding.archive_digest.clone(),
                features: binding.features.clone(),
                source_root,
                source_digest,
            });
    }
    let authority = write_sealed_oven_inspection_source_authority(authority_root, sources.into_values().collect())
        .map_err(|error| CliError::failure(error.to_string()))?;
    let graph = serde_json::to_vec_pretty(&closure.inspection_project())
        .map_err(|error| CliError::failure(error.to_string()))?;
    std::fs::write(authority_root.join(OVEN_DIRECT_LOAF_PROJECT_FILE), graph)
        .map_err(|error| CliError::failure(error.to_string()))?;
    let report = serde_json::to_vec_pretty(closure.report()).map_err(|error| CliError::failure(error.to_string()))?;
    std::fs::write(authority_root.join(".incan_sdk_closure_report.json"), report)
        .map_err(|error| CliError::failure(error.to_string()))?;
    Ok(authority)
}

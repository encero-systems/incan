//! SDK source-authority publication from a retained, directly compiled Loaf closure.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use oven_rustc::sdk_closure::SdkCompiledClosure;
use rust_inspect::{OvenInspectionRegistrySource, write_sealed_oven_inspection_source_authority};

use crate::error::CliError;

/// Publish the existing direct-inspection source authority only after every SDK seed unit was compiled.
///
/// The caller retains the closure while linking SDK components and inspecting their Rust facets. Missing facts or
/// failed dependencies prevent any authority file from being written; the legacy declaration reader's conversion
/// to Loaf declarations is a separate requirement before that authority can be consumed without Cargo metadata.
pub fn seal_sdk_closure_inspection_sources(
    closure: &SdkCompiledClosure,
    authority_root: &Path,
) -> Result<PathBuf, CliError> {
    closure
        .require_complete()
        .map_err(|error| CliError::failure(error.to_string()))?;
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
            .entry((package.to_string(), binding.version.clone(), binding.features.clone()))
            .or_insert_with(|| OvenInspectionRegistrySource {
                package: package.to_string(),
                version: binding.version.clone(),
                registry: "incan.pub/crates-io".to_string(),
                checksum: binding.archive_digest.clone(),
                features: binding.features.clone(),
                source_root,
                source_digest,
            });
    }
    write_sealed_oven_inspection_source_authority(authority_root, sources.into_values().collect())
        .map_err(|error| CliError::failure(error.to_string()))
}

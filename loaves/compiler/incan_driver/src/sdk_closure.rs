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
        let (package, registry) = match binding.loaf.strip_prefix("crates-io/") {
            Some(package) => (package, "registry+incan.pub/crates-io"),
            None => (binding.loaf.as_str(), "sdk+native"),
        };
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
                registry: registry.to_string(),
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

#[cfg(test)]
mod tests {
    use super::seal_sdk_closure_inspection_sources;
    use oven_rustc::sdk_closure::{compile_local_sdk_facet, prepare_sdk_seed};
    use rust_inspect::{OVEN_DIRECT_INSPECTION_MARKER, OVEN_LOAF_ONLY_INSPECTION_MARKER, RustWorkspace};

    /// A real local native facet can be sealed and loaded without consulting poisoned Cargo metadata.
    #[test]
    fn local_sdk_source_seals_and_loads_without_cargo() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let seed = root.path().join("seed.json");
        std::fs::write(&seed, r#"{"schema":"incan.oven.loaf-resolution/1","units":[]}"#)?;
        let output = root.path().join("native");
        let rustc = oven_rustc::rustc::resolve_active_rustc()?;
        let mut closure = prepare_sdk_seed(&seed, root.path(), &output, &rustc, root.path())?;
        let project = root.path().join("local");
        std::fs::create_dir_all(project.join("src"))?;
        std::fs::write(
            project.join("loaf.toml"),
            "[project]\nname='local'\nversion='1.0.0'\n[rust]\nname='local'\ntype='lib'\nedition='2024'\n",
        )?;
        std::fs::write(project.join("src/lib.rs"), "pub fn value() -> u8 { 1 }")?;
        compile_local_sdk_facet(&mut closure, &project, &[], "target", &output, &rustc)?;
        let authority = root.path().join("inspection");
        std::fs::create_dir(&authority)?;
        seal_sdk_closure_inspection_sources(&closure, &authority)?;
        std::fs::write(authority.join(OVEN_DIRECT_INSPECTION_MARKER), "1\n")?;
        std::fs::write(authority.join(OVEN_LOAF_ONLY_INSPECTION_MARKER), "1\n")?;
        std::fs::write(authority.join("Cargo.toml"), "invalid TOML")?;
        std::fs::write(authority.join("Cargo.lock"), "invalid TOML")?;
        let workspace = RustWorkspace::load_with_options(&authority, &|_| {}, false)?;
        assert!(workspace.crate_by_name("local").is_some());
        let cache = rust_inspect::RustMetadataCache::new();
        let metadata = cache.get_or_extract(&authority, "local::value", &|_| {})?;
        assert_eq!(metadata.canonical_path, "local::value");
        assert!(
            rust_inspect::RustMetadataCache::new()
                .get_cached(&authority, "local::value")?
                .is_some()
        );
        std::fs::write(project.join("src/lib.rs"), "pub fn value() -> u16 { 1 }")?;
        let mut replacement = prepare_sdk_seed(&seed, root.path(), &output, &rustc, root.path())?;
        compile_local_sdk_facet(&mut replacement, &project, &[], "target", &output, &rustc)?;
        seal_sdk_closure_inspection_sources(&replacement, &authority)?;
        assert!(
            rust_inspect::RustMetadataCache::new()
                .get_cached(&authority, "local::value")?
                .is_none()
        );
        Ok(())
    }
}

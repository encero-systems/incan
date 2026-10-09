//! SDK source-authority publication from a retained, directly compiled Loaf closure.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use oven_rustc::sdk_closure::SdkCompiledClosure;
use rust_inspect::{
    OVEN_DIRECT_LOAF_PROJECT_FILE, OvenInspectionRegistrySource, write_sealed_oven_inspection_source_authority,
};

use crate::error::CliError;

/// Install the source-current SDK's frozen native graph when it covers every requested registry dependency.
///
/// This read-only consumer boundary accepts only a published SDK inventory and its complete native receipt catalog.
/// Non-SDK paths and missing or incompatible bindings stay with the project's own authority; no Cargo reader or
/// dependency resolver is used to extend the SDK graph.
pub fn install_published_sdk_inspection_authority(
    destination: &Path,
    dependencies: &[oven_model::manifest::DependencySpec],
) -> Result<Option<Vec<oven_store::store::OvenStoreExecutionPayload>>, CliError> {
    let Some(inventory) = incan_provider::inventory::discover_or_reuse_published_sdk_inventory()
        .map_err(|error| CliError::failure(error.to_string()))?
    else {
        return Ok(None);
    };
    install_sdk_inspection_authority_from(&inventory.root, destination, dependencies)
}

/// Validate requested bindings before copying one immutable SDK generation's paired graph and source authority.
fn install_sdk_inspection_authority_from(
    root: &Path,
    destination: &Path,
    dependencies: &[oven_model::manifest::DependencySpec],
) -> Result<Option<Vec<oven_store::store::OvenStoreExecutionPayload>>, CliError> {
    let receipts = root.join(".sealed-native-receipts.json");
    if !receipts.is_file() {
        return Ok(None);
    }
    let receipts: BTreeMap<String, String> =
        serde_json::from_slice(&std::fs::read(receipts).map_err(|error| CliError::failure(error.to_string()))?)
            .map_err(|error| CliError::failure(error.to_string()))?;
    if receipts.is_empty()
        || receipts.values().any(|receipt| {
            !receipt.starts_with("sha256:")
                || receipt.len() != 71
                || !receipt[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    {
        return Err(CliError::failure(
            "SDK inspection selection has an invalid native receipt catalog",
        ));
    }
    let inventory = incan_provider::SdkInventory::read_from_path(&root.join(incan_provider::SDK_INVENTORY_FILE))
        .map_err(|error| CliError::failure(error.to_string()))?;
    let selection = incan_provider::sdk_native::select_sdk_native_artifacts(root)?;
    for dependency in dependencies {
        if !incan_provider::sdk_native::sdk_native_dependency_is_covered(&inventory, &selection, dependency)? {
            tracing::debug!(
                ?dependency,
                "requested dependency is outside the sealed SDK inspection selection"
            );
            return Ok(None);
        }
    }
    let selected = selection.owners;
    let mut retained_roots = selected
        .iter()
        .map(|owner| owner.artifact_root.join("source").canonicalize())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| CliError::failure(error.to_string()))?;
    retained_roots.sort();
    retained_roots.dedup();
    let declared_roots = rust_inspect::oven_inspection_registry_source_roots(root)
        .map_err(|error| CliError::failure(error.to_string()))?;
    if retained_roots != declared_roots {
        return Err(CliError::failure(
            "SDK inspection source authority does not match its retained native units",
        ));
    }
    for file in [
        rust_inspect::OVEN_DIRECT_INSPECTION_AUTHORITY_FILE,
        OVEN_DIRECT_LOAF_PROJECT_FILE,
        rust_inspect::OVEN_DIRECT_PROC_MACRO_AUTHORITY_FILE,
    ] {
        std::fs::copy(root.join(file), destination.join(file)).map_err(|error| {
            CliError::failure(format!("failed to install sealed SDK inspection authority: {error}"))
        })?;
    }
    Ok(Some(selected))
}

/// Publish the successfully compiled SDK subgraph and preserve named unavailable units alongside it.
///
/// The caller retains the closure while linking SDK components and inspecting their Rust facets. This authority
/// covers only the compiled units, and does not establish that every SDK component can be published. Automatic
/// provider publication requires a complete closure and uses the provider's native publication transaction instead.
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
        incan_provider::sdk_native::write_sdk_native_authority(&closure, &authority)?;
        incan_provider::sdk_native::write_sdk_native_artifact_catalog(&closure, &output.join("store"), &authority)?;
        let consumer = root.path().join("consumer");
        std::fs::create_dir(&consumer)?;
        let consumer_native = super::install_sdk_inspection_authority_from(&authority, &consumer, &[])?
            .ok_or("native SDK selection must cover this source-only consumer")?;
        assert_eq!(consumer_native.len(), 1);
        std::fs::write(consumer.join(OVEN_DIRECT_INSPECTION_MARKER), "1\n")?;
        std::fs::write(consumer.join(OVEN_LOAF_ONLY_INSPECTION_MARKER), "1\n")?;
        std::fs::write(consumer.join("Cargo.toml"), "invalid TOML")?;
        assert!(
            RustWorkspace::load_with_options(&consumer, &|_| {}, false)?
                .crate_by_name("local")
                .is_some()
        );
        let missing = oven_model::manifest::DependencySpec {
            crate_name: "absent".to_string(),
            version: Some("1".to_string()),
            features: Vec::new(),
            default_features: true,
            source: oven_model::manifest::DependencySource::Registry,
            optional: false,
            package: None,
        };
        assert!(super::install_sdk_inspection_authority_from(&authority, &consumer, &[missing])?.is_none());
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

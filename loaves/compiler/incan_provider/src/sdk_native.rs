//! Native SDK preparation and frozen inspection authority, selected from the SDK seed rather than Cargo metadata.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use oven_rustc::sdk_closure::{SdkCompiledClosure, compile_local_sdk_facet, prepare_sdk_seed};
use oven_store::store::{OvenStore, OvenStoreExecutionPayload, OvenStoreLimits};
use rust_inspect::{
    OVEN_DIRECT_LOAF_PROJECT_FILE, OvenInspectionRegistrySource, write_sealed_oven_inspection_source_authority,
};
use serde::{Deserialize, Serialize};

use crate::SdkSourceCatalog;
use crate::error::{ProviderError, ProviderResult};

/// Native output catalog accompanying one complete SDK publication.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SdkNativeArtifactCatalog {
    /// Version of the publication's native-coordinate schema.
    schema_version: u32,
    /// Store containing the immutable entries recorded below.
    store: PathBuf,
    /// Complete output selection accompanying the native receipt map.
    units: Vec<oven_rustc::sdk_closure::SdkNativeArtifact>,
}

/// Persist the exact admitted native outputs so consumers can reacquire their original store leases.
pub fn write_sdk_native_artifact_catalog(
    closure: &SdkCompiledClosure,
    store: &Path,
    root: &Path,
) -> ProviderResult<()> {
    let units = closure
        .units()
        .iter()
        .map(|unit| unit.native_artifact())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    let catalog = SdkNativeArtifactCatalog {
        schema_version: 1,
        store: store.to_path_buf(),
        units,
    };
    std::fs::write(
        root.join(".sealed-native-units.json"),
        serde_json::to_vec_pretty(&catalog).map_err(|error| ProviderError::failure(error.to_string()))?,
    )
    .map_err(|error| ProviderError::failure(error.to_string()))
}

/// Reacquire all SDK native source and output leases, validating their payload bindings and output descriptors.
///
/// Selection uses immutable store coordinates recorded during publication. It never discovers an ambient source
/// cache, resolves requirements, or trusts a native path independently of its admitted output digest.
pub fn retain_sdk_native_artifacts(root: &Path) -> ProviderResult<Vec<OvenStoreExecutionPayload>> {
    select_sdk_native_artifacts(root).map(|selection| selection.owners)
}

/// Native coordinates and their verified owners selected together from one SDK publication.
pub struct SdkNativeSelection {
    /// Exact output descriptors in the same order as their retained execution owners.
    pub units: Vec<oven_rustc::sdk_closure::SdkNativeArtifact>,
    /// Leases and verified source/output payloads that authorize the coordinates above.
    pub owners: Vec<OvenStoreExecutionPayload>,
}

/// Select native descriptors with their admitted owners instead of allowing consumers to trust output paths alone.
pub fn select_sdk_native_artifacts(root: &Path) -> ProviderResult<SdkNativeSelection> {
    let catalog: SdkNativeArtifactCatalog = serde_json::from_slice(
        &std::fs::read(root.join(".sealed-native-units.json"))
            .map_err(|error| ProviderError::failure(error.to_string()))?,
    )
    .map_err(|error| ProviderError::failure(error.to_string()))?;
    if catalog.schema_version != 1 || catalog.units.is_empty() {
        return Err(ProviderError::failure(
            "SDK native output catalog is empty or unsupported",
        ));
    }
    let receipts: BTreeMap<String, String> = serde_json::from_slice(
        &std::fs::read(root.join(".sealed-native-receipts.json"))
            .map_err(|error| ProviderError::failure(error.to_string()))?,
    )
    .map_err(|error| ProviderError::failure(error.to_string()))?;
    if receipts.len() != catalog.units.len() {
        return Err(ProviderError::failure(
            "SDK native coordinate and receipt catalogs disagree",
        ));
    }
    let store = OvenStore::new(
        catalog.store,
        OvenStoreLimits::new(4 * 1024 * 1024 * 1024, 4 * 1024 * 1024 * 1024, 4 * 1024 * 1024 * 1024),
    );
    let identities = catalog
        .units
        .iter()
        .map(|unit| unit.store_identity.clone())
        .collect::<Vec<_>>();
    let selected = store
        .select_payloads_for_execution(&identities)
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    for (unit, owner) in catalog.units.iter().zip(&selected) {
        let key = serde_json::to_string(&unit.binding.identity_binding())
            .map_err(|error| ProviderError::failure(error.to_string()))?;
        let actual: oven_rustc::sdk_closure::SdkLockedUnit =
            serde_json::from_slice(&owner.payload).map_err(|error| ProviderError::failure(error.to_string()))?;
        let actual = serde_json::to_value(actual.identity_binding())
            .map_err(|error| ProviderError::failure(error.to_string()))?;
        let expected = serde_json::to_value(unit.binding.identity_binding())
            .map_err(|error| ProviderError::failure(error.to_string()))?;
        if receipts.get(&key) != Some(&unit.receipt_identity)
            || actual != expected
            || owner.manifest.domain != format!("sdk-source-unit-{}", unit.binding.domain)
            || owner.manifest.receipt_identity != unit.receipt_identity
            || unit.relative_path.contains(['/', '\\'])
            || !matches!(
                Path::new(&unit.relative_path)
                    .extension()
                    .and_then(|extension| extension.to_str()),
                Some("rlib" | "dylib" | "so" | "dll")
            )
            || !owner
                .admitted_materialized_files()
                .iter()
                .any(|file| file.relative_path == unit.relative_path && file.digest == unit.digest)
        {
            return Err(ProviderError::failure(
                "SDK native output catalog disagrees with its admitted store entry",
            ));
        }
        owner
            .verify_admitted_payload()
            .map_err(|error| ProviderError::failure(error.to_string()))?;
    }
    Ok(SdkNativeSelection {
        units: catalog.units,
        owners: selected,
    })
}

/// Explicit immutable archive and index inputs for automatic source SDK preparation.
pub struct SdkNativeInputs {
    /// Digest-addressed admitted archive directory.
    pub blobs: PathBuf,
    /// Repository containing the closure executor's pinned index commit.
    pub index: PathBuf,
    /// Retained receipt store; independent of a provider publication's staging directory.
    pub output: PathBuf,
    /// Managed compiler selected for all local and adopted units.
    pub rustc: PathBuf,
}

impl SdkNativeInputs {
    /// Resolve explicitly supplied archives and index without discovering ambient Cargo caches.
    pub fn discover(store: &Path) -> ProviderResult<Self> {
        let required = |name: &str| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .ok_or_else(|| ProviderError::failure(format!("native SDK preparation requires {name}")))
        };
        Ok(Self {
            blobs: required("INCAN_SDK_NATIVE_BLOBS")?,
            index: required("INCAN_SDK_NATIVE_INDEX")?,
            output: store.join(".native"),
            rustc: oven_rustc::rustc::resolve_active_rustc()
                .map_err(|error| ProviderError::failure(error.to_string()))?,
        })
    }
}

/// Compile the sealed closure, its local companions, and every component declaring a Rust facet.
///
/// Successful independent units remain receipt-bound and reusable when native compilation fails. The checked
/// publisher admits only components whose own facets and dependency providers are available; named failures remain
/// in the closure report and cannot authorize missing outputs.
pub fn prepare_sdk_native_closure(
    stdlib: &Path,
    catalog: &SdkSourceCatalog,
    inputs: &SdkNativeInputs,
) -> ProviderResult<SdkCompiledClosure> {
    let mut closure = prepare_sdk_seed(
        &stdlib.join("sdk-lock.json"),
        &inputs.blobs,
        &inputs.output,
        &inputs.rustc,
        &inputs.index,
    )
    .map_err(|error| ProviderError::failure(error.to_string()))?;
    std::fs::write(
        inputs.output.join("closure-report.json"),
        serde_json::to_vec_pretty(closure.report()).map_err(|error| ProviderError::failure(error.to_string()))?,
    )
    .map_err(|error| ProviderError::failure(error.to_string()))?;
    let mut local_failures = Vec::new();
    let source_root = stdlib
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| ProviderError::failure("SDK source root has no owning toolchain"))?;
    for (relative, domain, features) in [
        ("loaves/kernel/incan_lang", "target", Vec::new()),
        ("loaves/stdlib/derive/incan_derive", "host", Vec::new()),
        ("loaves/stdlib/derive/incan_web_macros", "host", Vec::new()),
        ("loaves/kernel/incan_vocab", "target", vec!["serde".to_string()]),
    ] {
        if let Err(error) = compile_local_sdk_facet(
            &mut closure,
            &source_root.join(relative),
            &features,
            domain,
            &inputs.output,
            &inputs.rustc,
        ) {
            local_failures.push(format!("SDK companion {relative}: {error}"));
        }
    }
    for component in catalog.publication_order() {
        let declaration = std::fs::read_to_string(component.project_root.join("loaf.toml"))
            .map_err(|error| ProviderError::failure(error.to_string()))?;
        let declaration: toml::Value =
            toml::from_str(&declaration).map_err(|error| ProviderError::failure(error.to_string()))?;
        if declaration.get("rust").is_some()
            && let Err(error) = compile_local_sdk_facet(
                &mut closure,
                &component.project_root,
                &[],
                "target",
                &inputs.output,
                &inputs.rustc,
            )
        {
            local_failures.push(format!("SDK component {}: {error}", component.id));
        }
    }
    let mut report =
        serde_json::to_value(closure.report()).map_err(|error| ProviderError::failure(error.to_string()))?;
    report["local_failures"] =
        serde_json::to_value(&local_failures).map_err(|error| ProviderError::failure(error.to_string()))?;
    std::fs::write(
        inputs.output.join("closure-report.json"),
        serde_json::to_vec_pretty(&report).map_err(|error| ProviderError::failure(error.to_string()))?,
    )
    .map_err(|error| ProviderError::failure(error.to_string()))?;
    Ok(closure)
}

/// Identify complete native bindings, retaining domain and features so host and target units cannot collide.
pub fn sdk_native_receipts(closure: &SdkCompiledClosure) -> ProviderResult<BTreeMap<String, String>> {
    let mut receipts = BTreeMap::new();
    for unit in closure.units() {
        let key = serde_json::to_string(&unit.binding().identity_binding())
            .map_err(|error| ProviderError::failure(error.to_string()))?;
        if receipts.insert(key, unit.compiled_identity().to_string()).is_some() {
            return Err(ProviderError::failure("duplicate native SDK binding"));
        }
    }
    Ok(receipts)
}

/// Write receipt-bound source authority and the exact compiled alias graph without resolving declarations again.
///
/// The caller retains the closure's leases until its complete publication transaction is durable. This helper does
/// not authorize a partial provider inventory or an unavailable native unit.
pub fn write_sdk_native_authority(closure: &SdkCompiledClosure, root: &Path) -> ProviderResult<PathBuf> {
    let mut sources = BTreeMap::new();
    for unit in closure.units() {
        let source_root = unit.source_root();
        let binding = unit.binding();
        let (package, registry) = match binding.loaf.strip_prefix("crates-io/") {
            Some(package) => (package, "registry+incan.pub/crates-io"),
            None => (binding.loaf.as_str(), "sdk+native"),
        };
        let source_digest =
            oven_store::digest_source_tree(&source_root).map_err(|error| ProviderError::failure(error.to_string()))?;
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
    let authority = write_sealed_oven_inspection_source_authority(root, sources.into_values().collect())
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    std::fs::write(
        root.join(OVEN_DIRECT_LOAF_PROJECT_FILE),
        serde_json::to_vec_pretty(&closure.inspection_project())
            .map_err(|error| ProviderError::failure(error.to_string()))?,
    )
    .map_err(|error| ProviderError::failure(error.to_string()))?;
    std::fs::write(
        root.join(".sealed-native-receipts.json"),
        serde_json::to_vec_pretty(&sdk_native_receipts(closure)?)
            .map_err(|error| ProviderError::failure(error.to_string()))?,
    )
    .map_err(|error| ProviderError::failure(error.to_string()))?;
    Ok(authority)
}

#[cfg(test)]
mod tests {
    /// Component imports parse using their Loaf dependency declarations instead of removed inline version hints.
    #[test]
    fn system_sources_use_loaf_dependency_declarations() -> Result<(), Box<dyn std::error::Error>> {
        let root = oven_model::toolchain_layout::development_root().join("loaves/stdlib/system/src");
        for relative in [
            "fs/file.incn",
            "fs/locking.incn",
            "fs/path.incn",
            "io.incn",
            "tempfile.incn",
        ] {
            let source = std::fs::read_to_string(root.join(relative))?;
            crate::test_support::parsed_module_for_test(&source).map_err(|error| format!("{relative}: {error}"))?;
        }
        Ok(())
    }
}

/// Check a consumer requirement against one admitted SDK selection without resolving another dependency graph.
///
/// Registry requirements bind package, version, domain, and features. Path requirements must name either an exact
/// published SDK provider root or a compiler-owned facet source root from the current SDK catalog; arbitrary paths
/// with matching package names cannot borrow the SDK's native authority.
pub fn sdk_native_dependency_is_covered(
    inventory: &crate::SdkInventory,
    selection: &SdkNativeSelection,
    dependency: &oven_model::manifest::DependencySpec,
) -> ProviderResult<bool> {
    use oven_model::manifest::DependencySource;
    let package = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
    let requirement = dependency
        .version
        .as_deref()
        .map(semver::VersionReq::parse)
        .transpose()
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    let accepts_version = |version: &str| {
        semver::Version::parse(version).is_ok_and(|version| {
            requirement
                .as_ref()
                .is_none_or(|requirement| requirement.matches(&version))
        })
    };
    if let DependencySource::Path { path } = &dependency.source {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        for provider in inventory
            .components
            .values()
            .filter(|component| component.available)
            .flat_map(|component| &component.providers)
        {
            if let Some(root) = provider.crate_root.as_ref()
                && canonical == root.canonicalize().unwrap_or_else(|_| root.clone())
                && package == provider.name
                && accepts_version(&provider.version)
            {
                return Ok(true);
            }
        }
        if !sdk_native_facet_path_matches(package, &canonical)? {
            return Ok(false);
        }
    } else if !matches!(dependency.source, DependencySource::Registry) {
        return Ok(false);
    }
    let candidates = selection
        .units
        .iter()
        .filter(|unit| {
            let name = Path::new(&unit.relative_path)
                .file_stem()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix("lib"));
            let package_matches = match dependency.source {
                DependencySource::Registry => unit.binding.loaf == format!("crates-io/{package}"),
                DependencySource::Path { .. } => {
                    name == Some(package.replace('-', "_").as_str()) && !unit.binding.loaf.starts_with("crates-io/")
                }
                DependencySource::Git { .. } => false,
            };
            package_matches
                && accepts_version(&unit.binding.version)
                && (unit.binding.domain == "target"
                    || Path::new(&unit.relative_path)
                        .extension()
                        .is_some_and(|extension| extension == "dylib"))
                && dependency
                    .features
                    .iter()
                    .all(|feature| unit.binding.features.contains(feature))
        })
        .count();
    Ok(candidates == 1)
}

/// Match only the compiler's named native companions and component facets against their owning source catalog.
fn sdk_native_facet_path_matches(crate_name: &str, path: &Path) -> ProviderResult<bool> {
    let Some(stdlib) = oven_model::toolchain_layout::find_stdlib_root() else {
        return Ok(false);
    };
    let Some(source) = stdlib.parent().and_then(Path::parent) else {
        return Ok(false);
    };
    let mut roots = match crate_name {
        "incan_lang" | "incan_vocab" => vec![source.join("loaves/kernel").join(crate_name)],
        "incan_derive" | "incan_web_macros" => vec![stdlib.join("derive").join(crate_name)],
        _ => Vec::new(),
    };
    let catalog = SdkSourceCatalog::read_from_path(&stdlib.join(crate::SDK_SOURCE_CATALOG_FILE))
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    for component in catalog.components.values() {
        let text = std::fs::read_to_string(component.project_root.join("loaf.toml"))
            .map_err(|error| ProviderError::failure(error.to_string()))?;
        let declaration: toml::Value =
            toml::from_str(&text).map_err(|error| ProviderError::failure(error.to_string()))?;
        if declaration
            .get("rust")
            .and_then(|rust| rust.get("name"))
            .and_then(toml::Value::as_str)
            == Some(crate_name)
        {
            roots.push(component.project_root.clone());
            roots.push(component.project_root.join("rust"));
        }
    }
    Ok(roots
        .into_iter()
        .any(|root| root.canonicalize().unwrap_or(root) == path))
}

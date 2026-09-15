//! What Oven learns about Incan, in one place: the compiler's identity and the provider facts the ring asks for
//! through [`OvenProviderHooks`]. Oven's crates name no compiler crate; the driver hands them these values.
//!
//! This is the seed of `incan_oven_facet`; it moves to that crate once the provider loaders it reads are
//! `incan_provider`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oven_model::compiler_identity::CompilerIdentity;

use crate::library_manifest::published_layout::LIBRARY_MANIFEST_EXTENSION;
use crate::library_manifest::{LibraryManifest, digest_provider_artifact};
use crate::oven::legacy_cargo::{OvenLegacyCargoError, make_publisher_staging_file_writable};
use crate::oven::{OvenProviderHookError, OvenProviderHooks};
use crate::provider::inventory::discover_active_sdk_inventory;
use crate::provider::{SDK_INVENTORY_FILE, SdkInventory};
use crate::version::{INCAN_VERSION, SDK_PROVIDER_CODEGEN_REVISION};

/// The running compiler's identity for Oven: its version and the generated-provider revision it emits.
pub fn compiler_identity() -> CompilerIdentity {
    CompilerIdentity::new(INCAN_VERSION, SDK_PROVIDER_CODEGEN_REVISION)
}

/// The provider hooks every Oven request from this compiler carries.
pub fn provider_hooks() -> Arc<dyn OvenProviderHooks> {
    Arc::new(IncanProviderHooks)
}

/// Incan's answers to Oven's provider questions: the SDK inventory the toolchain ships, the staged providers'
/// digests, and what a packaged provider looks like on disk.
#[derive(Debug, Default, Clone, Copy)]
pub struct IncanProviderHooks;

impl OvenProviderHooks for IncanProviderHooks {
    fn sdk_provider_root(&self, explicit_inventory: Option<&Path>) -> Result<PathBuf, OvenProviderHookError> {
        // The suite publisher copies an already prepared, read-only SDK inventory into the immutable entry. Rebuilding
        // source components here used the ordinary `incan build --lib` helper, which can recurse into generated-Cargo
        // work and turn the hidden Loaf baker into an unbounded second build system. A missing inventory is an
        // explicit Oven preparation miss, never authority to launch that helper or create a hidden Cargo cache.
        match explicit_inventory {
            Some(inventory) => SdkInventory::read_from_path(inventory)
                .map(|inventory| inventory.root)
                .map_err(|error| OvenProviderHookError::InventoryUnreadable {
                    path: inventory.to_path_buf(),
                    source: Box::new(error),
                }),
            None => discover_active_sdk_inventory()
                .map_err(|error| OvenProviderHookError::InventoryDiscovery {
                    source: Box::new(error),
                })?
                .map(|inventory| inventory.root.clone())
                .ok_or_else(|| OvenProviderHookError::InventoryUnavailable {
                    guidance: "compiler-suite publication requires a prebuilt compatible SDK provider inventory; set \
                               INCAN_SDK_INVENTORY or use an installed Oven toolchain"
                        .to_string(),
                }),
        }
    }

    fn sdk_inventory_file(&self) -> &'static str {
        SDK_INVENTORY_FILE
    }

    fn refresh_staged_sdk_provider_digests(&self, provider_root: &Path) -> Result<(), OvenProviderHookError> {
        refresh_staged_sdk_provider_digests(provider_root).map_err(|error| OvenProviderHookError::DigestRefresh {
            provider_root: provider_root.to_path_buf(),
            source: Box::new(error),
        })
    }

    fn packaged_provider_digest(&self, dependency_root: &Path) -> Option<Result<String, OvenProviderHookError>> {
        is_packaged_provider_root(dependency_root).then(|| {
            digest_provider_artifact(dependency_root).map_err(|error| OvenProviderHookError::PackagedDigest {
                dependency_root: dependency_root.to_path_buf(),
                source: Box::new(error),
            })
        })
    }
}

/// Whether a path dependency root is a packaged Incan provider: a generated library crate carrying its `.incnlib`
/// manifest beside `Cargo.toml`. An authored Rust crate has no such manifest.
fn is_packaged_provider_root(root: &Path) -> bool {
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry
            .path()
            .extension()
            .is_some_and(|extension| extension == LIBRARY_MANIFEST_EXTENSION)
    })
}

/// Re-seal provider and dependency artifact digests after their Cargo path metadata is relocated.
///
/// A provider artifact digest deliberately covers `Cargo.toml`; changing its compiler-owned path dependencies must
/// therefore change both the inventory descriptor and every checked provider edge that references it.  Resolve that
/// DAG from the copied manifests, write children before parents, and only then rewrite the copied inventory.  This
/// keeps the normal provider-plan integrity check meaningful after the suite entry has become self-contained.
fn refresh_staged_sdk_provider_digests(provider_root: &Path) -> Result<(), OvenLegacyCargoError> {
    let inventory_path = provider_root.join(SDK_INVENTORY_FILE);
    let mut inventory = SdkInventory::read_from_path(&inventory_path)
        .map_err(|error| OvenLegacyCargoError::Plan(format!("failed to read staged SDK inventory: {error}")))?;
    let canonical_provider_root = fs::canonicalize(provider_root).map_err(|source| OvenLegacyCargoError::Io {
        path: provider_root.to_path_buf(),
        source,
    })?;
    let mut digests = BTreeMap::new();
    let mut visiting = BTreeSet::new();
    for component in inventory.components.values() {
        for descriptor in &component.providers {
            let Some(crate_root) = descriptor.crate_root.as_ref() else {
                continue;
            };
            let digest = refresh_staged_provider_artifact_digest(
                crate_root,
                &canonical_provider_root,
                &mut digests,
                &mut visiting,
            )?;
            digests.insert(crate_root.clone(), digest);
        }
    }
    for component in inventory.components.values_mut() {
        for descriptor in &mut component.providers {
            let Some(crate_root) = descriptor.crate_root.as_ref() else {
                continue;
            };
            let digest = digests.get(crate_root).ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!(
                    "staged SDK provider {} has no refreshed artifact digest",
                    descriptor.name
                ))
            })?;
            descriptor.digest = digest.clone();
        }
    }
    make_publisher_staging_file_writable(&inventory_path)?;
    inventory
        .write_to_path(&inventory_path)
        .map_err(|error| OvenLegacyCargoError::Plan(format!("failed to write staged SDK inventory: {error}")))?;
    Ok(())
}

/// Update a copied provider manifest's dependency digests in dependency-first order and return its new digest.
pub(crate) fn refresh_staged_provider_artifact_digest(
    crate_root: &Path,
    provider_root: &Path,
    digests: &mut BTreeMap<PathBuf, String>,
    visiting: &mut BTreeSet<PathBuf>,
) -> Result<String, OvenLegacyCargoError> {
    let crate_root = fs::canonicalize(crate_root).map_err(|source| OvenLegacyCargoError::Io {
        path: crate_root.to_path_buf(),
        source,
    })?;
    if !crate_root.starts_with(provider_root) {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "staged SDK provider dependency",
            message: format!(
                "{} escapes staged provider root {}",
                crate_root.display(),
                provider_root.display()
            ),
        });
    }
    if let Some(digest) = digests.get(&crate_root) {
        return Ok(digest.clone());
    }
    if !visiting.insert(crate_root.clone()) {
        return Err(OvenLegacyCargoError::Plan(format!(
            "staged SDK provider dependency graph cycles at {}",
            crate_root.display()
        )));
    }
    let manifest_path = staged_provider_manifest_path(&crate_root)?;
    let mut manifest = LibraryManifest::read_from_path(&manifest_path)
        .map_err(|error| OvenLegacyCargoError::Plan(format!("failed to read {}: {error}", manifest_path.display())))?;
    let mut changed = false;
    for dependency in &mut manifest.contract_metadata.provider.provider_dependencies {
        let dependency_root = crate_root.join(&dependency.relative_artifact_path);
        let digest = refresh_staged_provider_artifact_digest(&dependency_root, provider_root, digests, visiting)?;
        if dependency.artifact_digest != digest {
            dependency.artifact_digest = digest;
            changed = true;
        }
    }
    if changed {
        make_publisher_staging_file_writable(&manifest_path)?;
        manifest.write_to_path(&manifest_path).map_err(|error| {
            OvenLegacyCargoError::Plan(format!("failed to write {}: {error}", manifest_path.display()))
        })?;
    }
    let digest = digest_provider_artifact(&crate_root)
        .map_err(|error| OvenLegacyCargoError::Plan(format!("failed to digest {}: {error}", crate_root.display())))?;
    visiting.remove(&crate_root);
    digests.insert(crate_root, digest.clone());
    Ok(digest)
}

/// Find the one provider library manifest that owns a copied component root.
fn staged_provider_manifest_path(crate_root: &Path) -> Result<PathBuf, OvenLegacyCargoError> {
    let mut manifests = fs::read_dir(crate_root)
        .map_err(|source| OvenLegacyCargoError::Io {
            path: crate_root.to_path_buf(),
            source,
        })?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some(LIBRARY_MANIFEST_EXTENSION))
        .collect::<Vec<_>>();
    manifests.sort();
    let [manifest] = manifests.as_slice() else {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "staged SDK provider artifact",
            message: format!(
                "{} must contain exactly one .incnlib manifest; found {}",
                crate_root.display(),
                manifests.len()
            ),
        });
    };
    Ok(manifest.clone())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;

    use incan_core::lang::stdlib::{self, StdlibExtraCrateSource};

    use crate::manifest::{DependencySource, DependencySpec};
    use crate::oven::digest_dependency_specs;
    use crate::oven::loaf::{
        OvenLoafEnvelope, OvenLoafMemberRole, loaf_envelope_inspection_packages, loaf_envelope_specifications,
    };
    use crate::oven_facet::provider_hooks;

    #[test]
    fn a_packaged_provider_is_identified_by_its_sealed_artifact_not_its_private_cargo_edges_issue1469()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = tempfile::tempdir()?;
        let provider = fixture.path().join("catalog/target/lib");
        fs::create_dir_all(provider.join("src"))?;
        // The generated manifest still names a private Rust crate that no longer exists.
        fs::write(
            provider.join("Cargo.toml"),
            "[package]\nname = \"immutable_catalog\"\nversion = \"0.1.0\"\n\n[dependencies.package_store_witness]\npath = \"../../../package-store-witness\"\n",
        )?;
        fs::write(provider.join("src/lib.rs"), "pub fn answer() -> i64 { 42 }\n")?;
        let dependency = DependencySpec {
            crate_name: "stock".to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path { path: provider.clone() },
            optional: false,
            package: None,
        };
        let as_authored_crate = digest_dependency_specs(std::slice::from_ref(&dependency), provider_hooks().as_ref());
        assert!(
            as_authored_crate.is_err(),
            "an authored crate's missing path dependency is still a fault: {as_authored_crate:?}"
        );

        fs::write(provider.join("immutable_catalog.incnlib"), "{}")?;
        // With its `.incnlib` beside the manifest the same tree is a packaged provider, and the missing private
        // crate is no longer anyone's business.
        let sealed = digest_dependency_specs(std::slice::from_ref(&dependency), provider_hooks().as_ref())?;
        fs::write(provider.join("src/lib.rs"), "pub fn answer() -> i64 { 43 }\n")?;
        assert_ne!(
            sealed,
            digest_dependency_specs(std::slice::from_ref(&dependency), provider_hooks().as_ref())?,
            "the sealed artifact's own bytes still decide its identity"
        );
        Ok(())
    }

    /// Return the canonical standard-library modules owned by checked SDK component sources.
    ///
    /// Component entrypoints are the source-of-truth provider surface. Normalizing their `*.prelude` implementation
    /// modules to their public facade mirrors provider publication, while `std.interop` is the intentionally
    /// source-less vocabulary-backed provider component.
    fn checked_stdlib_component_modules() -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
        let component_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/incan_stdlib/stdlib/components");
        let mut modules = BTreeSet::from(["std.interop".to_string()]);
        for entry in fs::read_dir(&component_root)? {
            let entry = entry?;
            let source = entry.path().join("src/lib.incn");
            if !source.is_file() {
                continue;
            }
            for line in fs::read_to_string(&source)?.lines() {
                let Some(import) = line.trim().strip_prefix("import ") else {
                    continue;
                };
                let module = import
                    .split_whitespace()
                    .next()
                    .ok_or("stdlib component import has no module path")?;
                let module = match module.strip_suffix(".prelude") {
                    Some(facade) => facade,
                    None => module,
                };
                modules.insert(format!("std.{module}"));
            }
        }
        Ok(modules)
    }

    /// Return the standard-library imports a checked complete-stdlib fixture declares.
    fn checked_stdlib_fixture_imports(source: &str) -> BTreeSet<String> {
        source
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let module = line
                    .strip_prefix("import ")
                    .or_else(|| line.strip_prefix("from "))?
                    .split_whitespace()
                    .next()?;
                module.starts_with("std.").then(|| module.to_string())
            })
            .collect()
    }

    #[test]
    fn complete_stdlib_loaf_fixtures_cover_every_checked_component_module() -> Result<(), Box<dyn std::error::Error>> {
        let expected_modules = checked_stdlib_component_modules()?;
        for envelope in [OvenLoafEnvelope::Release, OvenLoafEnvelope::CompilerSuite] {
            for specification in loaf_envelope_specifications(envelope) {
                let fixture_modules = checked_stdlib_fixture_imports(specification.source);
                let missing = expected_modules
                    .difference(&fixture_modules)
                    .cloned()
                    .collect::<Vec<_>>();
                if !missing.is_empty() {
                    return Err(format!(
                        "{envelope:?}/{}/{} complete stdlib fixture omits checked provider modules: {}",
                        specification.label,
                        specification.profile,
                        missing.join(", ")
                    )
                    .into());
                }
            }
        }
        Ok(())
    }

    #[test]
    fn checked_envelope_names_the_complete_declared_repository_test_inspection_surface()
    -> Result<(), Box<dyn std::error::Error>> {
        let specifications = loaf_envelope_specifications(OvenLoafEnvelope::CompilerSuite);
        let packages = loaf_envelope_inspection_packages(OvenLoafEnvelope::CompilerSuite)?;
        assert_eq!(
            loaf_envelope_inspection_packages(OvenLoafEnvelope::Release)?,
            packages,
            "the release and compiler-suite `stdlib` Loafs must declare one identical complete standard-library dependency surface"
        );
        let mut expected_packages = stdlib::extra_crate_deps()
            .filter(|dependency| matches!(dependency.source, StdlibExtraCrateSource::Version(_)))
            .map(|dependency| {
                stdlib::extra_crate_package_alias(dependency.crate_name)
                    .unwrap_or(dependency.crate_name)
                    .to_string()
            })
            .collect::<BTreeSet<_>>();
        expected_packages.extend([
            "bitflags".to_string(),
            "semver".to_string(),
            "serde".to_string(),
            "serde_json".to_string(),
            "uuid".to_string(),
        ]);
        assert_eq!(
            packages
                .iter()
                .map(|package| package.package.clone())
                .collect::<BTreeSet<_>>(),
            expected_packages
        );
        let provider = specifications
            .iter()
            .find(|specification| specification.label == "stdlib" && specification.profile == "debug")
            .ok_or("missing compiler-suite standard-provider Loaf")?;
        assert_eq!(
            provider
                .inspection_packages()?
                .iter()
                .map(|package| package.package.clone())
                .collect::<BTreeSet<_>>(),
            expected_packages
        );
        for imported_crate in ["rand", "uuid"] {
            assert!(
                provider.source.contains(&format!("from rust::{imported_crate}")),
                "the compiler-suite provider Loaf must retain an actual checked source import for `{imported_crate}` so reachable dependency resolution produces its direct-rustc leaf"
            );
        }
        assert!(
            provider.source.contains("std.serde"),
            "the compiler-suite provider Loaf must retain the checked stdlib serde surface that produces its derive-enabled direct-rustc leaf"
        );
        for reachable_use in ["Uuid.new_v4()", "thread_rng()", ".gen_range("] {
            assert!(
                provider.source.contains(reachable_use),
                "the compiler-suite provider Loaf must exercise `{reachable_use}` so its declared raw Rust dependency is usable rather than merely imported"
            );
        }
        assert!(provider.role.provides_source_authority());
        assert!(specifications.iter().all(|specification| {
            specification.role == OvenLoafMemberRole::CompiledClosureAndSourceAuthority
                && specification.retain_complete_registry_leaves
        }));
        Ok(())
    }
}

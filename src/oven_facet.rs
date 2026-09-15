//! What Oven learns about Incan, in one place: the compiler's identity and the provider facts the ring asks for
//! through [`OvenProviderHooks`]. Oven's crates name no compiler crate; the driver hands them these values.
//!
//! This is the seed of `incan_oven_facet`; it moves to that crate once the Oven ring is its own crates.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oven_model::compiler_identity::CompilerIdentity;

use crate::library_manifest::published_layout::LIBRARY_MANIFEST_EXTENSION;
use crate::library_manifest::{LibraryManifest, digest_provider_artifact};
use crate::oven::OvenProviderHooks;
use crate::oven::legacy_cargo::{OvenLegacyCargoError, make_publisher_staging_file_writable};
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
    fn sdk_provider_root(&self, explicit_inventory: Option<&Path>) -> Result<PathBuf, String> {
        // The suite publisher copies an already prepared, read-only SDK inventory into the immutable entry. Rebuilding
        // source components here used the ordinary `incan build --lib` helper, which can recurse into generated-Cargo
        // work and turn the hidden Loaf baker into an unbounded second build system. A missing inventory is an
        // explicit Oven preparation miss, never authority to launch that helper or create a hidden Cargo cache.
        match explicit_inventory {
            Some(inventory) => SdkInventory::read_from_path(inventory)
                .map(|inventory| inventory.root)
                .map_err(|error| {
                    format!(
                        "failed to load explicit compiler-suite SDK provider inventory {}: {error}",
                        inventory.display()
                    )
                }),
            None => discover_active_sdk_inventory()
                .map_err(|error| format!("failed to discover active SDK provider inventory: {error}"))?
                .map(|inventory| inventory.root.clone())
                .ok_or_else(|| {
                    "compiler-suite publication requires a prebuilt compatible SDK provider inventory; set INCAN_SDK_INVENTORY or use an installed Oven toolchain"
                        .to_string()
                }),
        }
    }

    fn sdk_inventory_file(&self) -> &'static str {
        SDK_INVENTORY_FILE
    }

    fn refresh_staged_sdk_provider_digests(&self, provider_root: &Path) -> Result<(), String> {
        refresh_staged_sdk_provider_digests(provider_root).map_err(|error| error.to_string())
    }

    fn packaged_provider_digest(&self, dependency_root: &Path) -> Option<Result<String, String>> {
        is_packaged_provider_root(dependency_root)
            .then(|| digest_provider_artifact(dependency_root).map_err(|error| error.to_string()))
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

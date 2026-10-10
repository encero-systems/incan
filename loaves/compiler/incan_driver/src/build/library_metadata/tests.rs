//! Real ordinary metadata Store controls; no mirrored receipt or admission implementation.

mod published;

use super::{
    LIBRARY_METADATA_DOMAIN, LibraryMetadataDependency, LibraryMetadataRecipe, SelectedLibraryMetadata,
    publish_library_metadata, select_library_metadata, select_library_metadata_reference,
};
use crate::build::OvenPackagedLibraryMetadataFile;
use crate::error::CliResult;
use incan_frontend::library_manifest::LibraryManifest;
use incan_frontend::library_manifest::LibraryRustAbi;
use incan_lang::interop::metadata::RustItemKind;
use incan_lang::interop::metadata::{RustItemMetadata, RustTypeInfo, RustTypeMetadataCompleteness, RustVisibility};
use oven_store::digest_bytes;
use oven_store::store::OvenStoreLimits;
use oven_store::store::{OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore};
use std::collections::{BTreeMap, BTreeSet};
use std::{fs, sync::Arc};
use tempfile::TempDir;

/// Give every test a complete ordinary source declaration and finalized generated artifact.
struct Package {
    source: TempDir,
    output: TempDir,
    store_root: TempDir,
    recipe: LibraryMetadataRecipe,
}

impl Package {
    /// Construct the same package contract for arbitrary ordinary or installed-standard package names.
    fn new(name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let source = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let store_root = tempfile::tempdir()?;
        fs::write(
            source.path().join("loaf.toml"),
            format!("[project]\nname = {name:?}\nversion = \"1.0.0\"\n"),
        )?;
        fs::create_dir(output.path().join("src"))?;
        fs::write(output.path().join("src/lib.rs"), "pub fn answer() -> i64 { 42 }\n")?;
        LibraryManifest::new(name, "1.0.0").write_to_path(&output.path().join(format!("{name}.incnlib")))?;
        let recipe = LibraryMetadataRecipe {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            source_digest: digest_bytes(b"source-current"),
            producer_digest: digest_bytes(b"actual-producer"),
            semantic_authority_digest: digest_bytes(b"complete-semantic-closure"),
            dependencies: BTreeMap::new(),
            policy_digest: digest_bytes(b"ordinary-publication"),
            target: "x86_64-unknown-linux-gnu".to_string(),
            toolchain: "rustc exact test identity".to_string(),
            features: Vec::new(),
        };
        Ok(Self {
            source,
            output,
            store_root,
            recipe,
        })
    }

    /// Open the actual bounded Store, independent of ambient compiler-home environment.
    fn store(&self) -> OvenStore {
        OvenStore::new(
            self.store_root.path(),
            OvenStoreLimits::new(16 * 1024 * 1024, 16 * 1024 * 1024, 16 * 1024 * 1024),
        )
    }

    /// Publish this fixture through the actual ordinary checked owner API.
    fn publish(&self, required: BTreeSet<String>) -> CliResult<Arc<SelectedLibraryMetadata>> {
        publish_library_metadata(
            &self.store(),
            &self.recipe,
            &self.recipe.receipt(self.source.path())?,
            self.output.path(),
            &self.output.path().join(format!("{}.incnlib", self.recipe.name)),
            required,
        )
    }
}

/// Ordinary and installed-standard package names exercise exactly the same producer, selector and replay API.
#[test]
fn ordinary_metadata_owner_first_repeat_and_fresh_output_replay() -> Result<(), Box<dyn std::error::Error>> {
    for name in ["third_party_geometry", "incan_std_core"] {
        let package = Package::new(name)?;
        let receipt = package.recipe.receipt(package.source.path())?;
        assert!(select_library_metadata(&package.store(), &package.recipe, &receipt)?.is_none());
        let first = package.publish(BTreeSet::new())?;
        let repeat =
            select_library_metadata(&package.store(), &package.recipe, &receipt)?.ok_or("metadata owner miss")?;
        assert_eq!(first.reference().owner_identity, repeat.reference().owner_identity);
        assert_eq!(repeat.manifest().name, name);
        assert!(repeat.owner.original_native_receipt().is_none());
        let fresh = tempfile::tempdir()?;
        repeat.replay(fresh.path())?;
        assert_eq!(
            fs::read(fresh.path().join("src/lib.rs"))?,
            fs::read(package.output.path().join("src/lib.rs"))?
        );
        assert_eq!(
            LibraryManifest::read_from_path(&fresh.path().join(format!("{name}.incnlib")))?.to_json_string()?,
            repeat.manifest().to_json_string()?
        );
    }
    Ok(())
}

/// Every semantic input changes selection; equal output bytes and dependency package names cannot select old owners.
#[test]
fn ordinary_metadata_recipe_invalidation_is_complete() -> Result<(), Box<dyn std::error::Error>> {
    let package = Package::new("ordinary")?;
    let _selected = package.publish(BTreeSet::new())?;
    let original = &package.recipe;
    let mut changes = Vec::new();
    for coordinate in 0..8 {
        let mut changed = original.clone();
        match coordinate {
            0 => changed.source_digest = digest_bytes(b"changed source"),
            1 => changed.producer_digest = digest_bytes(b"changed actual producer"),
            2 => changed.semantic_authority_digest = digest_bytes(b"changed macro or Rust graph"),
            3 => changed.policy_digest = digest_bytes(b"changed namespace or emission policy"),
            4 => changed.target = "aarch64-apple-darwin".to_string(),
            5 => changed.toolchain.push_str(" changed"),
            6 => changed.features.push("selected-feature".to_string()),
            _ => {
                changed.dependencies.insert(
                    "renamed_import".to_string(),
                    LibraryMetadataDependency {
                        name: "dependency".to_string(),
                        version: "2.0.0".to_string(),
                        receipt_identity: digest_bytes(b"receipt"),
                        owner_identity: digest_bytes(b"different owner same output"),
                        checked_digest: digest_bytes(b"same output"),
                    },
                );
            }
        }
        changes.push(changed);
    }
    for changed in changes {
        assert!(
            select_library_metadata(&package.store(), &changed, &changed.receipt(package.source.path())?)?.is_none()
        );
    }
    Ok(())
}

/// A publication may not promise absent or syntax-only Rust ABI facts as a complete checked dependency.
#[test]
fn ordinary_metadata_requires_complete_promised_rust_abi() -> Result<(), Box<dyn std::error::Error>> {
    let package = Package::new("ordinary")?;
    let required = BTreeSet::from(["companion::Container".to_string()]);
    assert!(package.publish(required.clone()).is_err());
    let path = package.output.path().join("ordinary.incnlib");
    let mut manifest = LibraryManifest::read_from_path(&path)?;
    manifest.rust_abi = LibraryRustAbi::from_items(vec![RustItemMetadata {
        canonical_path: "companion::Container".to_string(),
        definition_path: None,
        visibility: RustVisibility::Public,
        kind: RustItemKind::Type(RustTypeInfo {
            metadata_completeness: RustTypeMetadataCompleteness::FieldsAndVariantsOnly,
            ..RustTypeInfo::default()
        }),
    }]);
    manifest.write_to_path(&path)?;
    assert!(package.publish(required.clone()).is_err());
    let abi = manifest.rust_abi.as_mut().ok_or("missing ABI")?;
    let RustItemKind::Type(ty) = &mut abi.items.get_mut(0).ok_or("missing ABI item")?.kind else {
        return Err("wrong ABI kind".into());
    };
    ty.metadata_completeness = RustTypeMetadataCompleteness::Complete;
    manifest.write_to_path(&path)?;
    let _selected = package.publish(required)?;
    Ok(())
}

/// Changed immutable bytes and extra generated files refuse rather than silently becoming a new checked contract.
#[test]
fn ordinary_metadata_refuses_changed_owner_and_extra_replay_files() -> Result<(), Box<dyn std::error::Error>> {
    let package = Package::new("ordinary")?;
    let selected = package.publish(BTreeSet::new())?;
    let destination = tempfile::tempdir()?;
    selected.replay(destination.path())?;
    fs::write(destination.path().join("src/injected.rs"), "pub fn injected() {}\n")?;
    assert!(selected.replay(destination.path()).is_err());
    let materialized = selected.owner.artifact_root.join("src/lib.rs");
    let original_metadata = fs::metadata(&materialized)?;
    let original_permissions = original_metadata.permissions();
    let original_modified = original_metadata.modified()?;
    assert!(original_permissions.readonly());
    // Corrupt this fixture's sealed bytes, then restore its permissions before testing admission.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            &materialized,
            fs::Permissions::from_mode(original_permissions.mode() | 0o200),
        )?;
    }
    #[cfg(not(unix))]
    {
        let mut writable = original_permissions.clone();
        writable.set_readonly(false);
        fs::set_permissions(&materialized, writable)?;
    }
    let corruption = (|| -> std::io::Result<()> {
        fs::write(&materialized, "pub fn answer() -> i64 { 99 }\n")?;
        fs::File::options()
            .write(true)
            .open(&materialized)?
            .set_modified(original_modified)
    })();
    fs::set_permissions(&materialized, original_permissions)?;
    corruption?;
    assert!(fs::metadata(&materialized)?.permissions().readonly());
    assert_eq!(fs::metadata(&materialized)?.modified()?, original_modified);
    assert_eq!(fs::metadata(&materialized)?.len(), original_metadata.len());
    assert_eq!(fs::read(&materialized)?, b"pub fn answer() -> i64 { 99 }\n");
    assert!(selected.replay(tempfile::tempdir()?.path()).is_err());
    Ok(())
}

/// A canonical outer Store manifest does not make a forged metadata payload or substituted recipe admissible.
#[test]
fn ordinary_metadata_refuses_forged_payload_and_receipt() -> Result<(), Box<dyn std::error::Error>> {
    for corruption in 0..4 {
        let package = Package::new("ordinary")?;
        let selected = package.publish(BTreeSet::new())?;
        let mut payload = selected.payload.clone();
        match corruption {
            0 => payload.schema_version += 1,
            1 => payload.recipe.source_digest = digest_bytes(b"wrong recipe"),
            2 => payload.receipt.identity = digest_bytes(b"wrong receipt"),
            _ => {
                payload.generated_files.push(OvenPackagedLibraryMetadataFile {
                    relative_path: "src/absent.rs".to_string(),
                    digest: digest_bytes(b"absent"),
                });
            }
        }
        let other = tempfile::tempdir()?;
        let store = OvenStore::new(other.path(), *package.store().limits());
        store.publish(&OvenArtifactPublishRequest {
            receipt: selected.payload.receipt.clone(),
            domain: LIBRARY_METADATA_DOMAIN.to_string(),
            kind: OvenArtifactKind::Engine,
            payload: serde_json::to_vec(&payload)?,
            materialized_files: selected
                .owner
                .manifest
                .materialized_files
                .iter()
                .map(|file| OvenArtifactMaterializedFile {
                    source_path: selected.owner.artifact_root.join(&file.relative_path),
                    relative_path: file.relative_path.clone(),
                })
                .collect(),
            materialized_directories: Vec::new(),
        })?;
        assert!(
            select_library_metadata(&store, &package.recipe, &package.recipe.receipt(package.source.path())?).is_err()
        );
    }
    Ok(())
}

/// Cloned selections retain the original lease rather than acquiring a substitute owner during replay.
#[test]
fn ordinary_metadata_clone_keeps_owner_leased() -> Result<(), Box<dyn std::error::Error>> {
    let package = Package::new("ordinary")?;
    let selected = package.publish(BTreeSet::new())?;
    let retained = Arc::clone(&selected);
    drop(selected);
    let bounded = OvenStore::new(package.store_root.path(), OvenStoreLimits::new(1, 1, 1));
    let _ = bounded.prune()?;
    retained.replay(tempfile::tempdir()?.path())?;
    Ok(())
}

/// Symlinked generated sources and destination parents never enter the ordinary checked closure.
#[cfg(unix)]
#[test]
fn ordinary_metadata_refuses_symlink_source_and_replay_destination() -> Result<(), Box<dyn std::error::Error>> {
    let package = Package::new("ordinary")?;
    let selected = package.publish(BTreeSet::new())?;
    let destination = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    std::os::unix::fs::symlink(outside.path(), destination.path().join("src"))?;
    assert!(selected.replay(destination.path()).is_err());
    std::os::unix::fs::symlink(outside.path(), package.output.path().join("src/redirect"))?;
    assert!(package.publish(BTreeSet::new()).is_err());
    Ok(())
}

/// Multiple published aliases may share one proven definition only when their checked facts agree.
#[test]
fn ordinary_metadata_refuses_competing_canonical_rust_facts() -> Result<(), Box<dyn std::error::Error>> {
    let package = Package::new("ordinary")?;
    let path = package.output.path().join("ordinary.incnlib");
    let mut manifest = LibraryManifest::read_from_path(&path)?;
    let first = RustItemMetadata {
        canonical_path: "companion::First".to_string(),
        definition_path: Some("companion::Canonical".to_string()),
        visibility: RustVisibility::Public,
        kind: RustItemKind::Constant {
            type_display: "i64".to_string(),
        },
    };
    let mut second = first.clone();
    second.canonical_path = "companion::Second".to_string();
    manifest.rust_abi = LibraryRustAbi::from_items(vec![first.clone(), second.clone()]);
    manifest.write_to_path(&path)?;
    let _consistent = package.publish(BTreeSet::new())?;
    second.kind = RustItemKind::Constant {
        type_display: "String".to_string(),
    };
    manifest.rust_abi = LibraryRustAbi::from_items(vec![first, second]);
    manifest.write_to_path(&path)?;
    assert!(package.publish(BTreeSet::new()).is_err());
    Ok(())
}

/// Executable sidecars are bound both by the complete Loaf closure and their manifest-selected content digest.
#[test]
fn ordinary_metadata_refuses_mismatched_semantic_sidecar() -> Result<(), Box<dyn std::error::Error>> {
    let package = Package::new("ordinary")?;
    let path = package.output.path().join("ordinary.incnlib");
    let mut manifest = LibraryManifest::read_from_path(&path)?;
    let bytes = incan_semantics_core::executable_representation::build_surface(
        &[],
        "ordinary",
        "1.0.0",
        &BTreeSet::new(),
        &BTreeSet::new(),
    )?;
    let content_digest = digest_bytes(&bytes)
        .strip_prefix("sha256:")
        .ok_or("invalid digest")?
        .to_string();
    manifest.contract_metadata.executable_representation =
        Some(incan_frontend::library_manifest::ExecutableRepresentationExport {
            representation_version: incan_semantics_core::executable_representation::EXECUTABLE_REPRESENTATION_VERSION,
            content_digest,
        });
    let surface = incan_frontend::library_manifest::published_layout::executable_surface_path(&path, &manifest)
        .ok_or("invalid surface path")?;
    fs::create_dir_all(surface.parent().ok_or("surface has no parent")?)?;
    fs::write(&surface, &bytes)?;
    manifest.write_to_path(&path)?;
    let selected = package.publish(BTreeSet::new())?;
    selected.replay(tempfile::tempdir()?.path())?;
    fs::write(&surface, b"changed checked body")?;
    assert!(package.publish(BTreeSet::new()).is_err());
    Ok(())
}

/// Portable handoff imports the original Engine owner, not merely a reference to an ambient Store coordinate.
#[test]
fn ordinary_metadata_package_export_preserves_original_owner_and_lease() -> Result<(), Box<dyn std::error::Error>> {
    let package = Package::new("ordinary")?;
    let selected = package.publish(BTreeSet::new())?;
    let root = tempfile::tempdir()?;
    let destination = OvenStore::new(root.path(), *package.store().limits());
    let reference = selected.export_into(&destination)?;
    assert_eq!(reference.owner_identity, selected.reference().owner_identity);
    let admitted = select_library_metadata_reference(&destination, &reference)?;
    drop(selected);
    let bounded = OvenStore::new(package.store_root.path(), OvenStoreLimits::new(1, 1, 1));
    let _ = bounded.prune()?;
    admitted.replay(tempfile::tempdir()?.path())?;
    assert_eq!(admitted.reference().owner_identity, reference.owner_identity);
    let mut wrong = reference.clone();
    wrong.owner_identity = digest_bytes(b"same bytes different owner");
    assert!(select_library_metadata_reference(&destination, &wrong).is_err());
    Ok(())
}

/// Dependency retention checks complete package/receipt/contract facts and holds the actual original owner.
#[test]
fn ordinary_metadata_dependency_retention_requires_exact_checked_owner() -> Result<(), Box<dyn std::error::Error>> {
    let dependency = Package::new("dependency")?;
    let selected = dependency.publish(BTreeSet::new())?;
    let reference = selected.reference();
    let mut package = Package::new("ordinary")?;
    package.recipe.dependencies.insert(
        "renamed_dependency".into(),
        LibraryMetadataDependency {
            name: "dependency".into(),
            version: "1.0.0".into(),
            receipt_identity: reference.receipt.identity.clone(),
            owner_identity: reference.owner_identity.clone(),
            checked_digest: digest_bytes(&serde_json::to_vec(selected.checked_files())?),
        },
    );
    let root = package.publish(BTreeSet::new())?;
    assert!(root.retaining_dependencies(&[]).is_err());
    let retained = root.retaining_dependencies(&[Arc::clone(&selected)])?;
    drop(selected);
    let bounded = OvenStore::new(dependency.store_root.path(), OvenStoreLimits::new(1, 1, 1));
    let _ = bounded.prune()?;
    retained._dependency_owners[0].verify()?;
    let other_package = Package::new("other")?;
    let other = other_package.publish(BTreeSet::new())?;
    assert!(root.retaining_dependencies(&[other]).is_err());
    let mut wrong = package.recipe.clone();
    wrong
        .dependencies
        .get_mut("renamed_dependency")
        .ok_or("missing dependency")?
        .name = "other".into();
    package.recipe = wrong;
    let mismatch = package.publish(BTreeSet::new())?;
    assert!(mismatch.retaining_dependencies(&retained._dependency_owners).is_err());
    Ok(())
}

/// Separately valid dependency manifests cannot silently select different facts for one shared canonical definition.
#[test]
fn ordinary_metadata_refuses_competing_canonical_rust_facts_across_dependencies()
-> Result<(), Box<dyn std::error::Error>> {
    let mut first = LibraryManifest::new("first", "1.0.0");
    let mut second = LibraryManifest::new("second", "1.0.0");
    let item = RustItemMetadata {
        canonical_path: "companion::First".into(),
        definition_path: Some("companion::Canonical".into()),
        visibility: RustVisibility::Public,
        kind: RustItemKind::Constant {
            type_display: "i64".into(),
        },
    };
    first.rust_abi = LibraryRustAbi::from_items(vec![item.clone()]);
    let mut alias = item;
    alias.canonical_path = "companion::Second".into();
    second.rust_abi = LibraryRustAbi::from_items(vec![alias.clone()]);
    super::validate_rust_fact_agreement([&first, &second])?;
    alias.kind = RustItemKind::Constant {
        type_display: "String".into(),
    };
    second.rust_abi = LibraryRustAbi::from_items(vec![alias]);
    assert!(super::validate_rust_fact_agreement([&first, &second]).is_err());
    Ok(())
}

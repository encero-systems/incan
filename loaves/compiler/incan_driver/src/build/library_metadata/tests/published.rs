//! Source-free ordinary metadata/generation admission through the actual read-only Store boundary.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use oven_store::digest_bytes;
use oven_store::store::{OvenStore, OvenStoreLimits, PublishedOvenStore};

use super::Package;
use crate::build::library_generation::{
    SelectedLibraryGeneration, publish_library_generation, select_published_library_generation_reference,
};
use crate::build::library_metadata::{
    LibraryMetadataDependency, MetadataDependencyTraversal, MetadataOwnerTraversal, SelectedLibraryMetadata,
    select_library_metadata_reference, select_published_library_metadata_reference,
};
use crate::build::library_project::metadata_replay::observe_library_source_digest;
use crate::build::source_authority::digest_baked_project_source_authority;

/// Retain genuine producer selections while a test removes source or transports their original Store bytes.
struct PublishedPackage {
    package: Package,
    metadata: Arc<SelectedLibraryMetadata>,
    generation: Arc<SelectedLibraryGeneration>,
    source_authority: String,
}

impl PublishedPackage {
    /// Publish a real source-current association, keeping the two source digest domains independent.
    fn new(name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let mut package = Package::new(name)?;
        fs::create_dir(package.source.path().join("src"))?;
        fs::write(
            package.source.path().join("src/lib.incn"),
            "pub def answer() -> int:\n    return 42\n",
        )?;
        package.recipe.source_digest = observe_library_source_digest(package.source.path(), &[])?;
        let source_authority = digest_baked_project_source_authority(package.source.path())?;
        assert_ne!(package.recipe.source_digest, source_authority);
        let metadata = package.publish(BTreeSet::new())?;
        let generation =
            publish_library_generation(&package.store(), package.source.path(), &source_authority, &metadata)?;
        Ok(Self {
            package,
            metadata,
            generation,
            source_authority,
        })
    }
}

/// Complete entry names, bytes, modification times and permission state; access times are intentionally irrelevant.
#[derive(Debug, PartialEq, Eq)]
struct EntryObservation {
    bytes: Option<Vec<u8>>,
    modified: SystemTime,
    readonly: bool,
}

/// Observe the whole tree without following links, including bookkeeping and abandoned staging.
fn inventory(root: &Path) -> Result<BTreeMap<PathBuf, EntryObservation>, Box<dyn std::error::Error>> {
    let mut observed = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = fs::symlink_metadata(&path)?;
        assert!(
            !metadata.file_type().is_symlink(),
            "unexpected fixture symlink: {path:?}"
        );
        let bytes = if metadata.is_dir() {
            for entry in fs::read_dir(&path)? {
                pending.push(entry?.path());
            }
            None
        } else {
            assert!(metadata.is_file(), "unexpected fixture entry: {path:?}");
            Some(fs::read(&path)?)
        };
        observed.insert(
            path.strip_prefix(root)?.to_path_buf(),
            EntryObservation {
                bytes,
                modified: metadata.modified()?,
                readonly: metadata.permissions().readonly(),
            },
        );
    }
    Ok(observed)
}

/// Restore every original permission even when a read-only admission assertion or fixture operation fails.
struct ReadOnlyTree {
    originals: Vec<(PathBuf, fs::Permissions)>,
}

impl ReadOnlyTree {
    /// Make all published files and, on Unix, directories genuinely unwritable without creating entries.
    fn new(root: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let mut guard = Self { originals: Vec::new() };
        for relative in inventory(root)?.keys() {
            let path = root.join(relative);
            let original = fs::metadata(&path)?.permissions();
            guard.originals.push((path.clone(), original.clone()));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(original.mode() & !0o222))?;
            }
            #[cfg(not(unix))]
            if path.is_file() {
                let mut readonly = original;
                readonly.set_readonly(true);
                fs::set_permissions(&path, readonly)?;
            }
        }
        Ok(guard)
    }

    /// Restore before TempDir cleanup and propagate permission errors instead of hiding them in Drop.
    fn restore(&mut self) -> std::io::Result<()> {
        for (path, permissions) in &self.originals {
            fs::set_permissions(path, permissions.clone())?;
        }
        self.originals.clear();
        Ok(())
    }
}

impl Drop for ReadOnlyTree {
    /// Preserve fixture cleanup if an earlier operation returns before explicit restoration.
    fn drop(&mut self) {
        for (path, permissions) in &self.originals {
            let _ = fs::set_permissions(path, permissions.clone());
        }
    }
}

/// Reach an exact genuine owner's coordinate through the actual read-only selector, without writable test hooks.
fn owner_entry(store: &PublishedOvenStore, identity: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let owners = store.select_payloads_matching_for_execution(|manifest| manifest.identity == identity)?;
    assert_eq!(owners.len(), 1);
    Ok(owners[0]
        .artifact_root
        .parent()
        .ok_or("owner has no entry root")?
        .to_path_buf())
}

/// Corrupt equal-length sealed bytes while restoring both the original modified time and permissions.
fn replace_preserving_metadata(path: &Path, replacement: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let metadata = fs::metadata(path)?;
    let original = metadata.permissions();
    assert_eq!(metadata.len(), u64::try_from(replacement.len())?);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(original.mode() | 0o200))?;
    }
    #[cfg(not(unix))]
    {
        let mut writable = original.clone();
        writable.set_readonly(false);
        fs::set_permissions(path, writable)?;
    }
    let changed = (|| -> std::io::Result<()> {
        fs::write(path, replacement)?;
        fs::File::options()
            .write(true)
            .open(path)?
            .set_modified(metadata.modified()?)
    })();
    fs::set_permissions(path, original)?;
    changed?;
    assert_eq!(fs::metadata(path)?.modified()?, metadata.modified()?);
    assert_eq!(fs::metadata(path)?.len(), metadata.len());
    Ok(())
}

/// Ordinary and standard package names share source-free, relocated, truly read-only first/repeat admission.
#[test]
fn published_ordinary_metadata_generation_source_free_readonly_relocated_repeat()
-> Result<(), Box<dyn std::error::Error>> {
    for name in ["third_party_geometry", "incan_std_core"] {
        let fixture = PublishedPackage::new(name)?;
        let metadata_reference = fixture.metadata.reference();
        let generation_reference = fixture.generation.reference();
        let files = fixture.metadata.checked_files().to_vec();
        drop(fixture.generation);
        drop(fixture.metadata);
        fs::remove_dir_all(fixture.package.source.path())?;
        fs::remove_dir_all(fixture.package.output.path())?;
        let relocated = tempfile::tempdir()?;
        let root = relocated.path().join("original-loaves");
        fs::rename(fixture.package.store_root.path(), &root)?;
        let abandoned = root.join("staging/retained-staging");
        fs::create_dir_all(&abandoned)?;
        fs::write(
            abandoned.join("evidence"),
            b"read-only admission must not reclaim staging",
        )?;
        let mut permissions = ReadOnlyTree::new(&root)?;
        let before = inventory(&root)?;
        let published = PublishedOvenStore::new(&root);
        for _ in 0..2 {
            let metadata = select_published_library_metadata_reference(&published, &metadata_reference, &[])?;
            assert_eq!(metadata.manifest().name, name);
            let entry = metadata.owner.artifact_root.parent().ok_or("metadata root missing")?;
            let contender = fs::File::open(entry.join(".active.lock"))?;
            let generation = select_published_library_generation_reference(
                &published,
                &generation_reference,
                Arc::clone(&metadata),
                &fixture.source_authority,
                &files,
            )?;
            let generation_entry = owner_entry(&published, &generation_reference.owner_identity)?;
            let generation_contender = fs::File::open(generation_entry.join(".active.lock"))?;
            drop(metadata);
            generation.verify()?;
            assert!(matches!(contender.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
            assert!(matches!(
                generation_contender.try_lock(),
                Err(std::fs::TryLockError::WouldBlock)
            ));
            assert_eq!(inventory(&root)?, before);
            drop(generation);
            contender.try_lock()?;
            contender.unlock()?;
            generation_contender.try_lock()?;
            generation_contender.unlock()?;
        }
        assert_eq!(inventory(&root)?, before);
        permissions.restore()?;
    }
    Ok(())
}

/// Genuine other owners, absent association knowledge and a missing Store never become replacement authority.
#[test]
fn published_ordinary_references_refuse_substitution_missing_and_wrong_association()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = PublishedPackage::new("ordinary")?;
    let other = PublishedPackage::new("ordinary")?;
    let published = PublishedOvenStore::new(fixture.package.store_root.path());
    let reference = fixture.metadata.reference();
    let generation = fixture.generation.reference();
    let files = fixture.metadata.checked_files();
    assert_eq!(fixture.metadata.checked_files(), other.metadata.checked_files());
    assert_ne!(reference.owner_identity, other.metadata.reference().owner_identity);
    let other_reference = other.metadata.export_into(&fixture.package.store())?;
    let other_metadata = select_published_library_metadata_reference(&published, &other_reference, &[])?;
    assert!(
        select_published_library_generation_reference(
            &published,
            &generation,
            other_metadata,
            &fixture.source_authority,
            files,
        )
        .is_err()
    );
    for changed in [
        {
            let mut changed = reference.clone();
            changed.receipt = other_reference.receipt.clone();
            changed
        },
        {
            let mut changed = reference.clone();
            changed.owner_identity = digest_bytes(b"missing original");
            changed
        },
        {
            let mut changed = reference.clone();
            changed.schema_version += 1;
            changed
        },
    ] {
        assert!(select_published_library_metadata_reference(&published, &changed, &[]).is_err());
    }
    let other_generation = other.generation.export_into(&fixture.package.store())?;
    for changed in [
        {
            let mut changed = generation.clone();
            changed.receipt = other_generation.receipt.clone();
            changed
        },
        {
            let mut changed = generation.clone();
            changed.owner_identity = digest_bytes(b"missing generation");
            changed
        },
        {
            let mut changed = generation.clone();
            changed.schema_version += 1;
            changed
        },
        other_generation,
    ] {
        assert!(
            select_published_library_generation_reference(
                &published,
                &changed,
                Arc::clone(&fixture.metadata),
                &fixture.source_authority,
                files,
            )
            .is_err()
        );
    }
    assert!(
        select_published_library_generation_reference(
            &published,
            &generation,
            Arc::clone(&fixture.metadata),
            &digest_bytes(b"another source generation"),
            files,
        )
        .is_err()
    );
    assert!(
        select_published_library_generation_reference(
            &published,
            &generation,
            Arc::clone(&fixture.metadata),
            &fixture.source_authority,
            &[],
        )
        .is_err()
    );
    let empty = tempfile::tempdir()?;
    let absent = empty.path().join("missing-store");
    let missing = PublishedOvenStore::new(&absent);
    assert!(select_published_library_metadata_reference(&missing, &reference, &[]).is_err());
    assert!(
        select_published_library_generation_reference(
            &missing,
            &generation,
            Arc::clone(&fixture.metadata),
            &fixture.source_authority,
            files,
        )
        .is_err()
    );
    assert!(!absent.exists());
    let before = inventory(empty.path())?;
    let uninitialized = PublishedOvenStore::new(empty.path());
    assert!(select_published_library_metadata_reference(&uninitialized, &reference, &[]).is_err());
    assert_eq!(inventory(empty.path())?, before);
    Ok(())
}

/// Preserved timestamp/length cannot hide changed actual primary payload or checked/generated bytes.
#[test]
fn published_ordinary_admission_refuses_preserved_mtime_corruption() -> Result<(), Box<dyn std::error::Error>> {
    for owner_kind in ["metadata", "generation", "generated", "checked", "extra"] {
        let fixture = PublishedPackage::new("ordinary")?;
        let published = PublishedOvenStore::new(fixture.package.store_root.path());
        let metadata = fixture.metadata.reference();
        let generation = fixture.generation.reference();
        if owner_kind == "extra" {
            fs::write(
                fixture.metadata.owner.artifact_root.join("injected.txt"),
                b"not in checked closure",
            )?;
        } else {
            let path = match owner_kind {
                "metadata" => owner_entry(&published, &metadata.owner_identity)?.join("payload"),
                "generation" => owner_entry(&published, &generation.owner_identity)?.join("payload"),
                "generated" => fixture.metadata.owner.artifact_root.join("src/lib.rs"),
                _ => fixture.metadata.owner.artifact_root.join("ordinary.incnlib"),
            };
            let mut bytes = fs::read(&path)?;
            let first = bytes.first_mut().ok_or("empty corruption fixture")?;
            *first ^= 1;
            replace_preserving_metadata(&path, &bytes)?;
        }
        let before = inventory(fixture.package.store_root.path())?;
        if owner_kind != "generation" {
            assert!(select_published_library_metadata_reference(&published, &metadata, &[]).is_err());
        }
        assert!(
            select_published_library_generation_reference(
                &published,
                &generation,
                Arc::clone(&fixture.metadata),
                &fixture.source_authority,
                fixture.metadata.checked_files(),
            )
            .is_err()
        );
        // Existing Store revalidation observes both primary payload bytes and the materialized closure.
        if owner_kind != "generation" {
            assert!(fixture.metadata.verify().is_err());
        }
        assert!(fixture.generation.verify().is_err());
        assert_eq!(inventory(fixture.package.store_root.path())?, before);
    }
    Ok(())
}

/// Every direct/transitive checked owner must be held; shared aliases reuse one original lease and damage propagates.
#[test]
fn published_ordinary_metadata_requires_original_dependency_closure() -> Result<(), Box<dyn std::error::Error>> {
    let child = Package::new("child")?;
    let child_owner = child.publish(BTreeSet::new())?;
    let child_reference = child_owner.reference();
    let mut middle = Package::new("middle")?;
    let edge = LibraryMetadataDependency {
        name: "child".into(),
        version: "1.0.0".into(),
        receipt_identity: child_reference.receipt.identity.clone(),
        owner_identity: child_reference.owner_identity.clone(),
        checked_digest: digest_bytes(&serde_json::to_vec(child_owner.checked_files())?),
    };
    middle.recipe.dependencies.insert("renamed_child".into(), edge.clone());
    middle.recipe.dependencies.insert("second_child_alias".into(), edge);
    let middle_owner = middle.publish(BTreeSet::new())?;
    let middle_reference = middle_owner.reference();
    let middle_published = PublishedOvenStore::new(middle.store_root.path());
    assert!(select_published_library_metadata_reference(&middle_published, &middle_reference, &[]).is_err());
    let child_published = PublishedOvenStore::new(child.store_root.path());
    let child_selected = select_published_library_metadata_reference(&child_published, &child_reference, &[])?;
    let selected = select_published_library_metadata_reference(
        &middle_published,
        &middle_reference,
        &[Arc::clone(&child_selected)],
    )?;
    let fixture = PublishedPackage::new("root")?;
    let mut root_package = fixture.package;
    root_package.recipe.dependencies.insert(
        "middle".into(),
        LibraryMetadataDependency {
            name: "middle".into(),
            version: "1.0.0".into(),
            receipt_identity: middle_reference.receipt.identity.clone(),
            owner_identity: middle_reference.owner_identity.clone(),
            checked_digest: digest_bytes(&serde_json::to_vec(middle_owner.checked_files())?),
        },
    );
    let root = root_package.publish(BTreeSet::new())?;
    let root_generation = publish_library_generation(
        &root_package.store(),
        root_package.source.path(),
        &fixture.source_authority,
        &root,
    )?;
    let root_published = PublishedOvenStore::new(root_package.store_root.path());
    assert!(select_published_library_metadata_reference(&root_published, &root.reference(), &[middle_owner]).is_err());
    assert!(
        select_published_library_generation_reference(
            &root_published,
            &root_generation.reference(),
            Arc::clone(&root),
            &fixture.source_authority,
            root.checked_files(),
        )
        .is_err()
    );
    let retained = select_published_library_metadata_reference(&root_published, &root.reference(), &[selected])?;
    let generation = select_published_library_generation_reference(
        &root_published,
        &root_generation.reference(),
        Arc::clone(&retained),
        &fixture.source_authority,
        retained.checked_files(),
    )?;
    let contender =
        fs::File::open(owner_entry(&child_published, &child_reference.owner_identity)?.join(".active.lock"))?;
    drop(child_owner);
    drop(child_selected);
    drop(retained);
    assert!(matches!(contender.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
    // Reach the genuine held child Store coordinate, then damage its generated bytes without changing mtime.
    let path = owner_entry(&child_published, &child_reference.owner_identity)?.join("artifacts/src/lib.rs");
    let mut bytes = fs::read(&path)?;
    let first = bytes.first_mut().ok_or("empty child fixture")?;
    *first ^= 1;
    replace_preserving_metadata(&path, &bytes)?;
    assert!(generation.verify().is_err());
    drop(generation);
    contender.try_lock()?;
    contender.unlock()?;
    Ok(())
}

/// Removing the genuine original metadata or generation owner cannot trigger read-only repair or reminting.
#[test]
fn published_ordinary_missing_original_owner_refuses_without_repair() -> Result<(), Box<dyn std::error::Error>> {
    for missing_kind in ["metadata", "generation"] {
        let fixture = PublishedPackage::new("ordinary")?;
        let published = PublishedOvenStore::new(fixture.package.store_root.path());
        let metadata_reference = fixture.metadata.reference();
        let generation_reference = fixture.generation.reference();
        let files = fixture.metadata.checked_files().to_vec();
        let identity = if missing_kind == "metadata" {
            &metadata_reference.owner_identity
        } else {
            &generation_reference.owner_identity
        };
        let entry = owner_entry(&published, identity)?;
        drop(fixture.generation);
        drop(fixture.metadata);
        fs::remove_dir_all(entry)?;
        fs::remove_dir_all(fixture.package.source.path())?;
        let before = inventory(fixture.package.store_root.path())?;
        if missing_kind == "metadata" {
            assert!(select_published_library_metadata_reference(&published, &metadata_reference, &[]).is_err());
        } else {
            let metadata = select_published_library_metadata_reference(&published, &metadata_reference, &[])?;
            assert!(
                select_published_library_generation_reference(
                    &published,
                    &generation_reference,
                    metadata,
                    &fixture.source_authority,
                    &files,
                )
                .is_err()
            );
        }
        assert_eq!(inventory(fixture.package.store_root.path())?, before);
    }
    Ok(())
}

/// Publish a real metadata node retaining the supplied original dependencies under exact distinct aliases.
fn dependency_package(
    name: &str,
    dependencies: &[Arc<SelectedLibraryMetadata>],
) -> Result<(Package, Arc<SelectedLibraryMetadata>), Box<dyn std::error::Error>> {
    let mut package = Package::new(name)?;
    for (index, dependency) in dependencies.iter().enumerate() {
        let reference = dependency.reference();
        package.recipe.dependencies.insert(
            format!("alias_{index}"),
            LibraryMetadataDependency {
                name: dependency.manifest().name.clone(),
                version: dependency.manifest().version.clone(),
                receipt_identity: reference.receipt.identity,
                owner_identity: reference.owner_identity,
                checked_digest: digest_bytes(&serde_json::to_vec(dependency.checked_files())?),
            },
        );
    }
    let selected = package.publish(BTreeSet::new())?.retaining_dependencies(dependencies)?;
    Ok((package, selected))
}

/// Repeated diamonds observe physical owners once and inspect distinct capabilities without cross-call caching.
#[test]
fn published_ordinary_diamond_verification_observes_each_owner_once() -> Result<(), Box<dyn std::error::Error>> {
    let (leaf_package, leaf) = dependency_package("diamond_leaf", &[])?;
    let mut packages = vec![leaf_package];
    let mut level = vec![Arc::clone(&leaf)];
    for depth in 0..3 {
        let mut next = Vec::new();
        for side in ["left", "right"] {
            let (package, owner) = dependency_package(&format!("diamond_{side}_{depth}"), &level)?;
            packages.push(package);
            next.push(owner);
        }
        level = next;
    }
    level.push(Arc::clone(&level[0]));
    let (top_package, top) = dependency_package("diamond_top", &level)?;
    packages.push(top_package);
    let distinct_leaf_capability = leaf.retaining_dependencies(&[])?;
    assert!(!Arc::ptr_eq(&leaf, &distinct_leaf_capability));
    let (root_package, root) = dependency_package("diamond_root", &[top, distinct_leaf_capability])?;
    packages.push(root_package);
    assert_eq!(packages.len(), 9);
    let mut bytes = MetadataOwnerTraversal::default();
    root.verify_with_traversal(&mut bytes)?;
    assert_eq!(bytes.owner_checks, 9);
    assert_eq!(bytes.physical_owners.len(), 9);
    assert_eq!(bytes.capabilities.len(), 10);
    let mut contracts = MetadataDependencyTraversal::default();
    root.verify_dependency_closure_with_traversal(&mut contracts)?;
    assert_eq!(contracts.contract_checks, 10);
    assert_eq!(contracts.capabilities.len(), 10);
    root.verify()?;
    root.verify_dependency_closure()?;
    let path = leaf.owner.artifact_root.join("src/lib.rs");
    let mut corrupt = fs::read(&path)?;
    *corrupt.first_mut().ok_or("missing diamond leaf bytes")? ^= 1;
    replace_preserving_metadata(&path, &corrupt)?;
    assert!(
        root.verify().is_err(),
        "per-call observations must not survive into another verification"
    );
    Ok(())
}

/// Equal immutable IDs at separate physical roots require separate actual byte observations.
#[test]
fn published_ordinary_equal_owner_ids_at_distinct_roots_are_both_verified() -> Result<(), Box<dyn std::error::Error>> {
    let (_leaf_package, leaf) = dependency_package("physical_leaf", &[])?;
    let destination = tempfile::tempdir()?;
    let store = OvenStore::new(
        destination.path(),
        OvenStoreLimits::new(16 * 1024 * 1024, 16 * 1024 * 1024, 16 * 1024 * 1024),
    );
    let reference = leaf.export_into(&store)?;
    let published = PublishedOvenStore::new(destination.path());
    let copied = select_published_library_metadata_reference(&published, &reference, &[])?;
    assert_eq!(copied.reference().owner_identity, leaf.reference().owner_identity);
    assert_ne!(copied.owner.artifact_root, leaf.owner.artifact_root);
    let (_root_package, root) = dependency_package("physical_root", &[Arc::clone(&leaf), Arc::clone(&copied)])?;
    let mut work = MetadataOwnerTraversal::default();
    root.verify_with_traversal(&mut work)?;
    assert_eq!(work.owner_checks, 3);
    assert_eq!(work.physical_owners.len(), 3);
    root.verify_dependency_closure()?;
    let path = copied.owner.artifact_root.join("src/lib.rs");
    let mut corrupt = fs::read(&path)?;
    *corrupt.first_mut().ok_or("missing copied leaf bytes")? ^= 1;
    replace_preserving_metadata(&path, &corrupt)?;
    leaf.verify()?;
    assert!(
        root.verify().is_err(),
        "a genuine same-ID owner at another root must not hide damage"
    );
    Ok(())
}

/// A complete capability cannot hide another same-owner capability whose original child authority is unattached.
#[test]
fn published_ordinary_shared_owner_does_not_hide_incomplete_capability() -> Result<(), Box<dyn std::error::Error>> {
    let (_child_package, child) = dependency_package("retained_child", &[])?;
    let (middle_package, complete) = dependency_package("retained_middle", &[child])?;
    let incomplete = select_library_metadata_reference(&middle_package.store(), &complete.reference())?;
    assert_eq!(
        complete.reference().owner_identity,
        incomplete.reference().owner_identity
    );
    assert_eq!(complete.owner.artifact_root, incomplete.owner.artifact_root);
    assert!(!Arc::ptr_eq(&complete, &incomplete));
    // Writable preparation may hold an intermediate original selection before attaching its children.
    incomplete.verify()?;
    assert!(incomplete.verify_dependency_closure().is_err());
    for dependencies in [
        [Arc::clone(&complete), Arc::clone(&incomplete)],
        [Arc::clone(&incomplete), Arc::clone(&complete)],
    ] {
        let (package, root) = dependency_package("retained_root", &dependencies)?;
        let mut bytes = MetadataOwnerTraversal::default();
        root.verify_with_traversal(&mut bytes)?;
        assert_eq!(bytes.owner_checks, 3);
        assert_eq!(bytes.capabilities.len(), 4);
        let mut contracts = MetadataDependencyTraversal::default();
        assert!(root.verify_dependency_closure_with_traversal(&mut contracts).is_err());
        assert!(
            contracts
                .capabilities
                .contains(&std::ptr::from_ref(incomplete.as_ref()))
        );
        let published = PublishedOvenStore::new(package.store_root.path());
        assert!(select_published_library_metadata_reference(&published, &root.reference(), &dependencies).is_err());
    }
    Ok(())
}

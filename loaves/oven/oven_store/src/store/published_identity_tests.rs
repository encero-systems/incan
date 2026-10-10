//! Exact read-only original-owner selection without unrelated Store traversal (#1337/#1698).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::{OvenArtifactMaterializedFile, OvenStore, OvenStoreError, OvenStoreLimits, PublishedOvenStore};
use crate::test_support::{request, write_project};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Complete bytes and modification times, including mutable bookkeeping and abandoned staging.
fn inventory(root: &Path) -> Result<BTreeMap<PathBuf, (Option<Vec<u8>>, SystemTime)>, Box<dyn std::error::Error>> {
    let mut pending = vec![root.to_path_buf()];
    let mut result = BTreeMap::new();
    while let Some(path) = pending.pop() {
        let metadata = fs::symlink_metadata(&path)?;
        assert!(!metadata.file_type().is_symlink());
        let bytes = if metadata.is_dir() {
            for entry in fs::read_dir(&path)? {
                pending.push(entry?.path());
            }
            None
        } else {
            Some(fs::read(&path)?)
        };
        result.insert(path.strip_prefix(root)?.to_path_buf(), (bytes, metadata.modified()?));
    }
    Ok(result)
}

/// Exact known owners remain reusable when unrelated Store entries cannot even be decoded.
#[test]
fn dev7_published_exact_selection_ignores_unrelated_entries_and_preserves_inventory() -> TestResult {
    let root = tempfile::tempdir()?;
    let project = tempfile::tempdir()?;
    write_project(project.path())?;
    let store = OvenStore::new(root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
    let first = store.publish(&request(project.path(), "first", b"original first")?)?;
    let second = store.publish(&request(project.path(), "second", b"original second")?)?;
    let other = store.publish(&request(project.path(), "unrelated", b"unrelated bytes")?)?;
    let bad = super::manifest_path_for_entry(&store.entry_root(&other.identity));
    fs::remove_file(&bad)?;
    fs::write(&bad, b"not a manifest")?;
    let staging = root.path().join(super::STAGING_DIRECTORY).join("must-not-reclaim");
    fs::create_dir_all(&staging)?;
    fs::write(staging.join("evidence"), b"retained")?;
    let before = inventory(root.path())?;
    let published = PublishedOvenStore::new(root.path());
    for _ in 0..2 {
        let selected = published.select_payloads_for_execution(&[second.identity.clone(), first.identity.clone()])?;
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].manifest.identity, second.identity);
        assert_eq!(selected[0].payload, b"original second");
        assert_eq!(selected[1].manifest.identity, first.identity);
        assert_eq!(selected[1].payload, b"original first");
        for owner in &selected {
            owner.verify_admitted_payload()?;
        }
    }
    assert_eq!(inventory(root.path())?, before);
    assert!(published.select_payloads_for_execution(&[other.identity]).is_err());
    assert!(published.select_payloads_matching_for_execution(|_| true).is_err());
    assert_eq!(inventory(root.path())?, before);
    Ok(())
}

/// Exact batches validate canonical identities before touching even an absent Store.
#[test]
fn dev7_published_exact_selection_validates_batch_before_filesystem_access() -> TestResult {
    let root = tempfile::tempdir()?;
    let missing = root.path().join("absent");
    let published = PublishedOvenStore::new(&missing);
    let valid = format!("sha256:{}", "a".repeat(64));
    for identities in [
        Vec::new(),
        vec![valid.clone(), valid],
        vec!["../escape".to_string()],
        vec![format!("sha256:{}", "A".repeat(64))],
    ] {
        assert!(matches!(
            published.select_payloads_for_execution(&identities),
            Err(OvenStoreError::InvalidInput { .. })
        ));
    }
    assert!(!missing.exists());
    Ok(())
}

/// Missing original state is refused without repairing locks or creating a substitute publication.
#[test]
fn dev7_published_exact_selection_refuses_missing_owner_and_locks_without_repair() -> TestResult {
    for missing in ["owner", "manager", "lease"] {
        let root = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let manifest = store.publish(&request(project.path(), "selected", b"original")?)?;
        match missing {
            "owner" => fs::remove_dir_all(store.entry_root(&manifest.identity))?,
            "manager" => fs::remove_file(root.path().join(super::MANAGER_LOCK_FILE))?,
            _ => fs::remove_file(store.entry_root(&manifest.identity).join(super::ACTIVE_LOCK_FILE))?,
        }
        let before = inventory(root.path())?;
        assert!(
            PublishedOvenStore::new(root.path())
                .select_payloads_for_execution(&[manifest.identity])
                .is_err()
        );
        assert_eq!(inventory(root.path())?, before);
    }
    Ok(())
}

/// Actual bytes remain authority even when external corruption preserves modification time and length.
#[test]
fn dev7_published_exact_selection_refuses_preserved_mtime_payload_and_materialization_damage() -> TestResult {
    for payload_damage in [true, false] {
        let root = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let native = project.path().join("native.rlib");
        fs::write(&native, b"original bytes")?;
        let store = OvenStore::new(root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let mut publication = request(project.path(), "selected", b"original bytes")?;
        publication.materialized_files.push(OvenArtifactMaterializedFile {
            source_path: native,
            relative_path: "native.rlib".to_string(),
        });
        let manifest = store.publish(&publication)?;
        let published = PublishedOvenStore::new(root.path());
        let selected = published.select_payloads_for_execution(std::slice::from_ref(&manifest.identity))?;
        selected[0].verify_materialized_files()?;
        let entry = store.entry_root(&manifest.identity);
        let damaged = if payload_damage {
            entry.join(super::PAYLOAD_FILE)
        } else {
            entry.join(super::MATERIALIZED_DIRECTORY).join("native.rlib")
        };
        let modified = fs::metadata(&damaged)?.modified()?;
        fs::remove_file(&damaged)?;
        fs::write(&damaged, b"tampered bytes")?;
        fs::OpenOptions::new()
            .write(true)
            .open(&damaged)?
            .set_times(fs::FileTimes::new().set_modified(modified))?;
        assert!(selected[0].verify_admitted_payload().is_err());
        let before = inventory(root.path())?;
        let result = published
            .select_payloads_for_execution(&[manifest.identity])
            .and_then(|owners| owners[0].verify_materialized_files());
        assert!(result.is_err());
        assert_eq!(inventory(root.path())?, before);
    }
    Ok(())
}

/// A selected batch holds the existing active leases until the complete consumer releases it.
#[test]
fn dev7_published_exact_selection_retains_all_batch_leases() -> TestResult {
    let root = tempfile::tempdir()?;
    let project = tempfile::tempdir()?;
    write_project(project.path())?;
    let store = OvenStore::new(root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
    let first = store.publish(&request(project.path(), "first", b"first")?)?;
    let second = store.publish(&request(project.path(), "second", b"second")?)?;
    let selected = PublishedOvenStore::new(root.path())
        .select_payloads_for_execution(&[first.identity.clone(), second.identity.clone()])?;
    let bounded = OvenStore::new(root.path(), OvenStoreLimits::new(1, 1, 1));
    let held = bounded.prune()?;
    assert!(held.removed_entries.is_empty());
    assert_eq!(held.skipped_active_entries.len(), 2);
    for owner in &selected {
        owner.verify_admitted_payload()?;
    }
    drop(selected);
    assert_eq!(bounded.prune()?.removed_entries.len(), 2);
    Ok(())
}

/// Direct lookup cannot silently prefer one of two coordinates claiming the same immutable identity.
#[test]
fn dev7_published_exact_selection_refuses_competing_and_symlinked_coordinates() -> TestResult {
    let root = tempfile::tempdir()?;
    let project = tempfile::tempdir()?;
    write_project(project.path())?;
    let store = OvenStore::new(root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
    let manifest = store.publish(&request(project.path(), "selected", b"original")?)?;
    let original = store.entry_root(&manifest.identity);
    let competing = root.path().join(super::ENTRIES_DIRECTORY).join(format!(
        "{}{}",
        super::entry_directory_name(&manifest.identity),
        super::LOAF_ENTRY_SUFFIX
    ));
    fs::create_dir(&competing)?;
    assert!(
        PublishedOvenStore::new(root.path())
            .select_payloads_for_execution(std::slice::from_ref(&manifest.identity))
            .is_err()
    );
    fs::remove_dir(&competing)?;
    #[cfg(unix)]
    {
        let moved = root.path().join("original-entry");
        fs::rename(&original, &moved)?;
        std::os::unix::fs::symlink(&moved, &original)?;
        assert!(
            PublishedOvenStore::new(root.path())
                .select_payloads_for_execution(&[manifest.identity])
                .is_err()
        );
    }
    Ok(())
}

use super::*;
use crate::selection_test_support::InspectionFixture;

/// Verify absent selection is not a cacheable miss or a retry.
#[test]
fn absent_selection_is_not_a_cacheable_miss_or_a_retry() -> Result<(), Box<dyn std::error::Error>> {
    let context = tempfile::tempdir()?;
    fs::write(context.path().join("Cargo.toml"), "not a selected graph")?;
    let cache = RustMetadataCache::new();
    for query in ["demo::Absent", "demo-crate::Absent", "std::Absent"] {
        let error = match cache.get_or_extract(context.path(), query, &|_| {}) {
            Err(error) => error,
            Ok(_) => return Err("unselected extraction unexpectedly succeeded".into()),
        };
        assert!(matches!(error, RustMetadataError::SelectedInputUnavailable { .. }));
        assert!(NegativeLookup::from_error(&error).is_none());
        assert!(!metadata_extraction_missed(&error));
    }
    assert!(matches!(
        cache.get_cached(context.path(), "demo::Absent"),
        Err(RustMetadataError::SelectedInputUnavailable { .. })
    ));
    assert!(matches!(
        cache.get_cached_or_extract_fast(context.path(), "demo::Absent"),
        Err(RustMetadataError::SelectedInputUnavailable { .. })
    ));
    assert!(matches!(
        cache.get_or_extract_complete(context.path(), "demo::Absent", &|_| {}),
        Err(RustMetadataError::SelectedInputUnavailable { .. })
    ));
    assert!(matches!(
        cache.get_or_extract_with_registry_src_roots(
            context.path(),
            "demo::Absent",
            &[context.path().to_path_buf()],
            &|_| {}
        ),
        Err(RustMetadataError::SelectedInputUnavailable { .. })
    ));
    assert!(!disk_cache_path(context.path()).exists());
    let inner = cache.inner.lock().map_err(|error| error.to_string())?;
    assert!(inner.failed_items.is_empty());
    assert!(inner.fast_failed_items.is_empty());
    assert!(inner.workspaces.is_empty());
    Ok(())
}

/// Verify selected metadata persists and reuses only its complete binding.
#[test]
fn selected_metadata_persists_and_reuses_only_its_complete_binding() -> Result<(), Box<dyn std::error::Error>> {
    let context = tempfile::tempdir()?;
    let fixture = InspectionFixture::new("#![no_std]\npub struct Thing { pub number: u64 }\n")?;
    let cache = RustMetadataCache::new();
    cache.bind_selected_workspace(context.path(), fixture.load()?)?;
    let first = cache.get_or_extract_complete(context.path(), "demo::Thing", &|_| {})?;
    cache.persist_manifest_dir(context.path())?;
    let reopened = RustMetadataCache::new();
    // Even a valid persisted envelope is not enough without selected inputs.
    assert!(matches!(
        reopened.get_cached(context.path(), "demo::Thing"),
        Err(RustMetadataError::SelectedInputUnavailable { .. })
    ));
    reopened.bind_selected_workspace(context.path(), fixture.load()?)?;
    let hit = reopened
        .get_cached(context.path(), "demo::Thing")?
        .ok_or("selected persisted record absent")?;
    assert_eq!(
        serde_json::to_value(first.as_ref())?,
        serde_json::to_value(hit.metadata.as_ref())?
    );
    let changed = InspectionFixture::new("#![no_std]\npub struct Different;\n")?;
    reopened.bind_selected_workspace(context.path(), changed.load()?)?;
    assert!(reopened.get_cached(context.path(), "demo::Thing")?.is_none());
    assert!(
        reopened
            .get_or_extract_complete(context.path(), "demo::Different", &|_| {})
            .is_ok()
    );
    Ok(())
}

/// Verify stable missing item remains local to one selected database.
#[test]
fn stable_missing_item_remains_local_to_one_selected_database() -> Result<(), Box<dyn std::error::Error>> {
    let context = tempfile::tempdir()?;
    let fixture = InspectionFixture::new("#![no_std]\npub struct Thing;\n")?;
    let cache = RustMetadataCache::new();
    cache.bind_selected_workspace(context.path(), fixture.load()?)?;
    assert!(matches!(
        cache.get_or_extract_complete(context.path(), "demo::Absent", &|_| {}),
        Err(RustMetadataError::PathNotResolved(_))
    ));
    cache.persist_manifest_dir(context.path())?;
    let replacement = InspectionFixture::new("#![no_std]\npub struct Absent;\n")?;
    cache.bind_selected_workspace(context.path(), replacement.load()?)?;
    assert!(
        cache
            .get_or_extract_complete(context.path(), "demo::Absent", &|_| {})
            .is_ok()
    );
    Ok(())
}

/// Verify old cargo cache cannot satisfy selected metadata.
#[test]
fn old_cargo_cache_cannot_satisfy_selected_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let context = tempfile::tempdir()?;
    let fixture = InspectionFixture::new("#![no_std]\npub struct Thing;\n")?;
    let cache = RustMetadataCache::new();
    cache.bind_selected_workspace(context.path(), fixture.load()?)?;
    cache.get_or_extract_complete(context.path(), "demo::Thing", &|_| {})?;
    let mut envelope = read_disk_cache(context.path())?.ok_or("persisted cache absent")?;
    envelope.cache_format = 38;
    write_disk_cache(context.path(), &envelope)?;
    let reopened = RustMetadataCache::new();
    reopened.bind_selected_workspace(context.path(), fixture.load()?)?;
    assert!(reopened.get_cached(context.path(), "demo::Thing")?.is_none());
    Ok(())
}

/// Verify raw identifier alias uses the selected cache.
#[test]
fn raw_identifier_alias_uses_the_selected_cache() -> Result<(), Box<dyn std::error::Error>> {
    let context = tempfile::tempdir()?;
    let fixture = InspectionFixture::new("#![no_std]\npub struct r#type;\n")?;
    let cache = RustMetadataCache::new();
    cache.bind_selected_workspace(context.path(), fixture.load()?)?;
    cache.get_or_extract_complete(context.path(), "demo::r#type", &|_| {})?;
    assert!(cache.get_cached(context.path(), "demo::type")?.is_some());
    Ok(())
}

/// Missing standard-library names never select a different crate with a same-spelling item.
#[test]
fn selected_aliases_do_not_reinterpret_std_as_hashbrown_or_core() {
    assert_eq!(
        canonical_path_candidates("std::collections::HashMap"),
        ["std::collections::HashMap"]
    );
    assert_eq!(
        canonical_path_candidates("std::option::Option"),
        ["std::option::Option"]
    );
    assert_eq!(canonical_path_candidates("left-crate::Item"), ["left-crate::Item"]);
}

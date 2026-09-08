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
    fixture.bind(&cache, context.path())?;
    let first = cache.get_or_extract_complete(context.path(), "demo::Thing", &|_| {})?;
    cache.persist_manifest_dir(context.path())?;
    let reopened = RustMetadataCache::new();
    // Even a valid persisted envelope is not enough without selected inputs.
    assert!(matches!(
        reopened.get_cached(context.path(), "demo::Thing"),
        Err(RustMetadataError::SelectedInputUnavailable { .. })
    ));
    fixture.bind(&reopened, context.path())?;
    assert!(
        reopened
            .inner
            .lock()
            .map_err(|error| error.to_string())?
            .workspaces
            .is_empty()
    );
    assert_eq!(fs::read_dir(fixture.output.path())?.count(), 0);
    let hit = reopened
        .get_cached(context.path(), "demo::Thing")?
        .ok_or("selected persisted record absent")?;
    assert_eq!(
        serde_json::to_value(first.as_ref())?,
        serde_json::to_value(hit.metadata.as_ref())?
    );
    assert!(
        reopened
            .inner
            .lock()
            .map_err(|error| error.to_string())?
            .workspaces
            .is_empty()
    );
    assert_eq!(fs::read_dir(fixture.output.path())?.count(), 0);
    let changed = InspectionFixture::new("#![no_std]\npub struct Different;\n")?;
    changed.bind(&reopened, context.path())?;
    assert!(reopened.get_cached(context.path(), "demo::Thing")?.is_none());
    assert!(
        reopened
            .inner
            .lock()
            .map_err(|error| error.to_string())?
            .workspaces
            .is_empty()
    );
    assert_eq!(fs::read_dir(changed.output.path())?.count(), 0);
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
    fixture.bind(&cache, context.path())?;
    assert!(matches!(
        cache.get_or_extract_complete(context.path(), "demo::Absent", &|_| {}),
        Err(RustMetadataError::PathNotResolved(_))
    ));
    cache.persist_manifest_dir(context.path())?;
    let replacement = InspectionFixture::new("#![no_std]\npub struct Absent;\n")?;
    replacement.bind(&cache, context.path())?;
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
    fixture.bind(&cache, context.path())?;
    cache.get_or_extract_complete(context.path(), "demo::Thing", &|_| {})?;
    let mut envelope = read_disk_cache(context.path())?.ok_or("persisted cache absent")?;
    envelope.cache_format = 38;
    write_disk_cache(context.path(), &envelope)?;
    let reopened = RustMetadataCache::new();
    fixture.bind(&reopened, context.path())?;
    assert!(reopened.get_cached(context.path(), "demo::Thing")?.is_none());
    Ok(())
}

/// Verify raw identifier alias uses the selected cache.
#[test]
fn raw_identifier_alias_uses_the_selected_cache() -> Result<(), Box<dyn std::error::Error>> {
    let context = tempfile::tempdir()?;
    let fixture = InspectionFixture::new("#![no_std]\npub struct r#type;\n")?;
    let cache = RustMetadataCache::new();
    fixture.bind(&cache, context.path())?;
    cache.get_or_extract_complete(context.path(), "demo::r#type", &|_| {})?;
    assert!(cache.get_cached(context.path(), "demo::type")?.is_some());
    cache.persist_manifest_dir(context.path())?;
    let reopened = RustMetadataCache::new();
    fixture.bind(&reopened, context.path())?;
    for query in ["demo::type", "demo::r#type"] {
        let hit = reopened
            .get_cached(context.path(), query)?
            .ok_or("raw item spelling absent")?;
        assert_eq!(hit.metadata.canonical_path, query);
    }
    assert!(
        reopened
            .inner
            .lock()
            .map_err(|error| error.to_string())?
            .workspaces
            .is_empty()
    );
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
    assert_eq!(canonical_path_candidates("r#left::Item"), ["r#left::Item"]);
}

/// A database from another selected binding cannot replace the context or satisfy its metadata.
#[test]
fn selected_database_must_match_the_bound_projection() -> Result<(), Box<dyn std::error::Error>> {
    let context = tempfile::tempdir()?;
    let fixture = InspectionFixture::new("#![no_std]\npub struct First;\n")?;
    let other = InspectionFixture::new("#![no_std]\npub struct Other;\n")?;
    let cache = RustMetadataCache::new();
    assert!(matches!(
        cache.bind_selected_workspace(context.path(), fixture.load()?),
        Err(RustMetadataError::SelectedInputUnavailable { .. })
    ));
    fixture.bind(&cache, context.path())?;
    assert!(matches!(
        cache.bind_selected_workspace(context.path(), other.load()?),
        Err(RustMetadataError::InvalidSelectedInput { .. })
    ));
    assert!(
        cache
            .inner
            .lock()
            .map_err(|error| error.to_string())?
            .workspaces
            .is_empty()
    );
    cache.bind_selected_workspace(context.path(), fixture.load()?)?;
    assert!(
        cache
            .get_or_extract_complete(context.path(), "demo::First", &|_| {})
            .is_ok()
    );
    Ok(())
}

/// Equal display names do not let one selected package's definition spelling satisfy another package's query.
#[test]
fn selected_cache_does_not_conflate_equal_display_names() -> Result<(), Box<dyn std::error::Error>> {
    let context = tempfile::tempdir()?;
    let mut first = InspectionFixture::new("#![no_std]\npub struct Item { pub left: u32 }\n")?;
    let second = InspectionFixture::new("#![no_std]\npub struct Item { pub right: bool }\n")?;
    let mut project: serde_json::Value = serde_json::from_slice(&first.inputs.project_json)?;
    let other: serde_json::Value = serde_json::from_slice(&second.inputs.project_json)?;
    project["crates"]
        .as_array_mut()
        .ok_or("crates absent")?
        .push(other["crates"][0].clone());
    first.set_project(project)?;
    first.inputs.sources.extend(second.inputs.sources.clone());
    first.inputs.query_roots.insert("right".to_string(), second.module()?);
    let cache = RustMetadataCache::new();
    first.bind(&cache, context.path())?;
    let right = cache.get_or_extract_complete(context.path(), "right::Item", &|_| {})?;
    assert_eq!(
        right.definition_path.as_deref(),
        Some("demo::Item"),
        "fixture must exercise equal definition spelling across selected roots"
    );
    let RustItemKind::Type(right_type) = &right.kind else {
        return Err("right selected type absent".into());
    };
    assert_eq!(right_type.fields[0].name, "right");
    assert!(
        cache.get_cached(context.path(), "demo::Item")?.is_none(),
        "a different selected query is not a definition-spelling cache hit"
    );
    let left = cache.get_or_extract_complete(context.path(), "demo::Item", &|_| {})?;
    let RustItemKind::Type(left_type) = &left.kind else {
        return Err("left selected type absent".into());
    };
    assert_eq!(left_type.fields[0].name, "left");
    Ok(())
}

/// Retain actual selected source/output trees through database use and release them only with their binding.
#[test]
fn selected_binding_retains_source_owner_until_rebind_or_invalidation() -> Result<(), Box<dyn std::error::Error>> {
    let context = tempfile::tempdir()?;
    let fixture = Arc::new(InspectionFixture::new("#![no_std]\npub struct First;\n")?);
    let first_root = fixture.source.path().to_path_buf();
    let first_owner = Arc::downgrade(&fixture);
    let cache = RustMetadataCache::new();
    cache.bind_selected_project_with_owner(
        context.path(),
        fixture.validate()?,
        fixture.output.path(),
        Arc::clone(&fixture),
    )?;
    drop(fixture);
    assert!(first_owner.upgrade().is_some());
    assert!(first_root.is_dir());
    cache.get_or_extract_complete(context.path(), "demo::First", &|_| {})?;

    let changed = Arc::new(InspectionFixture::new("#![no_std]\npub struct Second;\n")?);
    let second_root = changed.source.path().to_path_buf();
    let second_owner = Arc::downgrade(&changed);
    cache.bind_selected_project_with_owner(
        context.path(),
        changed.validate()?,
        changed.output.path(),
        Arc::clone(&changed),
    )?;
    drop(changed);
    assert!(first_owner.upgrade().is_none());
    assert!(!first_root.exists());
    assert!(cache.get_cached(context.path(), "demo::First")?.is_none());
    cache.get_or_extract_complete(context.path(), "demo::Second", &|_| {})?;
    assert!(second_owner.upgrade().is_some());
    assert!(second_root.is_dir());

    cache.invalidate_manifest_dir(context.path())?;
    assert!(second_owner.upgrade().is_none());
    assert!(!second_root.exists());
    assert!(matches!(
        cache.get_cached(context.path(), "demo::Second"),
        Err(RustMetadataError::SelectedInputUnavailable { .. }),
    ));
    Ok(())
}

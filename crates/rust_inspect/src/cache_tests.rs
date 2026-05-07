use crate::cache_resolve::dependency_manifest_dir_from_lock_with_search_roots;
use super::*;
use incan_core::interop::{RustItemKind, RustTypeInfo, RustVisibility};

/// Build minimal public Rust type metadata for cache round-trip tests.
fn dummy_type_metadata(path: &str) -> RustItemMetadata {
    RustItemMetadata {
        canonical_path: path.to_string(),
        definition_path: None,
        visibility: RustVisibility::Public,
        kind: RustItemKind::Type(RustTypeInfo {
            methods: Vec::new(),
                    implemented_traits: Vec::new(),
            fields: Vec::new(),
            variants: Vec::new(),
        }),
    }
}

#[test]
fn lockfile_registry_fallback_resolves_hyphenated_package_for_underscored_crate_name()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("generated_lock");
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(
        root.join("Cargo.lock"),
        r#"version = 3

[[package]]
name = "foo-bar"
version = "0.1.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#,
    )?;

    let registry_src_root = tmp.path().join("cargo-home").join("registry").join("src");
    let dep_dir = registry_src_root.join("index.crates.io-test").join("foo-bar-0.1.0");
    fs::create_dir_all(dep_dir.join("src"))?;
    fs::write(
        dep_dir.join("Cargo.toml"),
        r#"[package]
name = "foo-bar"
version = "0.1.0"
edition = "2021"

[lib]
name = "foo_bar"
"#,
    )?;
    fs::write(dep_dir.join("src/lib.rs"), "pub fn consume() {}\n")?;

    let resolved = dependency_manifest_dir_from_lock_with_search_roots(&root, "foo_bar", &[registry_src_root])
        .ok_or_else(|| std::io::Error::other("expected Cargo.lock fallback to resolve foo-bar source dir"))?;
    assert_eq!(resolved, dep_dir);
    Ok(())
}

#[test]
fn disk_cache_round_trips_inserted_items() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    let cache = RustMetadataCache::new();
    cache.insert_test_item(tmp.path(), dummy_type_metadata("demo::Thing"))?;
    {
        let inner = cache
            .inner
            .lock()
            .map_err(|_| std::io::Error::other("poisoned cache"))?;
        persist_item_to_disk_cache(&inner, tmp.path(), &dummy_type_metadata("demo::Thing"))?;
    }

    let payload = fs::read_to_string(disk_cache_path(tmp.path()))?;
    assert!(payload.contains("\"demo::Thing\""));

    let cache = RustMetadataCache::new();
    let meta = cache.get_or_extract(tmp.path(), "demo::Thing", &|_| ())?;
    assert_eq!(meta.canonical_path, "demo::Thing");
    Ok(())
}

#[test]
fn disk_cache_invalidates_when_workspace_fingerprint_changes() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    let fingerprint = workspace_fingerprint(tmp.path())?;
    write_disk_cache(
        tmp.path(),
        &DiskCacheEnvelope {
            cache_format: DISK_CACHE_FORMAT,
            inspector_version: INSPECTOR_VERSION.to_string(),
            workspace_fingerprint: fingerprint,
            items: HashMap::from([("demo::Thing".to_string(), dummy_type_metadata("demo::Thing"))]),
            misses: HashMap::new(),
        },
    )?;

    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"probe_changed\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;

    let mut inner = CacheInner::default();
    ensure_disk_cache_loaded(&mut inner, tmp.path())?;
    assert!(
        !inner
            .items
            .contains_key(&(tmp.path().canonicalize()?, "demo::Thing".to_string()))
    );
    Ok(())
}

#[test]
/// Malformed on-disk cache payloads are ignored instead of poisoning later lookups.
fn malformed_disk_cache_is_treated_as_miss() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"incan_test_malformed_rust_inspect_disk_cache\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(disk_cache_path(tmp.path()), "{ definitely not json")?;
    let mut inner = CacheInner::default();
    ensure_disk_cache_loaded(&mut inner, tmp.path())?;
    assert!(inner.items.is_empty());
    Ok(())
}

#[test]
/// Raw identifier definition paths should still match cached canonical aliases.
fn raw_identifier_alias_hits_existing_cached_item() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;

    let cache = RustMetadataCache::new();
    cache.insert_test_item(
        tmp.path(),
        RustItemMetadata {
            canonical_path: "incan_stdlib::async::sync::RawSemaphore".to_string(),
            definition_path: Some("incan_stdlib::r#async::sync::Semaphore".to_string()),
            visibility: RustVisibility::Public,
            kind: RustItemKind::Type(RustTypeInfo {
                methods: Vec::new(),
                    implemented_traits: Vec::new(),
                fields: Vec::new(),
                variants: Vec::new(),
            }),
        },
    )?;

    let hit = cache.get_or_extract(tmp.path(), "incan_stdlib::r#async::sync::RawSemaphore", &|_| ())?;
    assert_eq!(hit.canonical_path, "incan_stdlib::r#async::sync::RawSemaphore");
    Ok(())
}

#[test]
fn repeated_missing_lookup_hits_negative_cache_without_new_workspace_load()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::create_dir_all(tmp.path().join("src"))?;
    fs::write(tmp.path().join("src/lib.rs"), "pub fn keep() {}\n")?;

    let cache = RustMetadataCache::new();
    let query = "std::fs::read_to_string";

    let first = cache.get_or_extract(tmp.path(), query, &|_| ());
    assert!(matches!(
        first,
        Err(RustMetadataError::CrateNotFound(_))
            | Err(RustMetadataError::PathNotResolved(_))
            | Err(RustMetadataError::UnsupportedMacro(_))
    ));

    let root = tmp.path().canonicalize()?;
    let workspaces_after_first = {
        let inner = cache
            .inner
            .lock()
            .map_err(|_| std::io::Error::other("poisoned cache"))?;
        assert!(inner
            .failed_items
            .contains_key(&(root.clone(), query.to_string())));
        inner.workspaces.len()
    };

    let second = cache.get_or_extract(tmp.path(), query, &|_| ());
    assert!(matches!(
        second,
        Err(RustMetadataError::CrateNotFound(_))
            | Err(RustMetadataError::PathNotResolved(_))
            | Err(RustMetadataError::UnsupportedMacro(_))
    ));

    let workspaces_after_second = {
        let inner = cache
            .inner
            .lock()
            .map_err(|_| std::io::Error::other("poisoned cache"))?;
        inner.workspaces.len()
    };
    assert_eq!(
        workspaces_after_second, workspaces_after_first,
        "negative-cache hit should avoid loading additional workspaces on repeated misses"
    );
    Ok(())
}

use crate::cache_resolve::dependency_manifest_dir_from_lock_with_search_roots;
use super::*;
use incan_core::interop::{RustItemKind, RustTypeInfo, RustTypeShape, RustVisibility};

/// Build minimal public Rust type metadata for cache round-trip tests.
fn dummy_type_metadata(path: &str) -> RustItemMetadata {
    RustItemMetadata {
        canonical_path: path.to_string(),
        definition_path: None,
        visibility: RustVisibility::Public,
        kind: RustItemKind::Type(RustTypeInfo {
            alias_target: None,
            methods: Vec::new(),
            implemented_traits: Vec::new(),
            fields: Vec::new(),
            variants: Vec::new(),
        }),
    }
}

/// Build minimal public Rust type metadata that records its defining module path.
fn dummy_reexported_type_metadata(path: &str, definition_path: &str) -> RustItemMetadata {
    RustItemMetadata {
        canonical_path: path.to_string(),
        definition_path: Some(definition_path.to_string()),
        visibility: RustVisibility::Public,
        kind: RustItemKind::Type(RustTypeInfo {
            alias_target: None,
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

/// Inserted metadata should survive a disk-cache round trip through a fresh cache instance.
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
        persist_item_to_disk_cache(&inner, tmp.path().canonicalize()?.as_path())?;
    }

    let payload = fs::read_to_string(disk_cache_path(tmp.path()))?;
    assert!(payload.contains("\"demo::Thing\""));

    let cache = RustMetadataCache::new();
    let meta = cache.get_or_extract(tmp.path(), "demo::Thing", &|_| ())?;
    assert_eq!(meta.canonical_path, "demo::Thing");
    Ok(())
}

/// Disk-cache entries are ignored when the generated workspace inputs change.
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
            inspector_version: format!("cache-format-{DISK_CACHE_FORMAT}"),
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

/// Malformed on-disk cache payloads are ignored instead of poisoning later lookups.
#[test]
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

/// Package version labels do not invalidate a cache when the format and workspace inputs still match.
#[test]
fn disk_cache_does_not_invalidate_on_package_version_label() -> Result<(), Box<dyn std::error::Error>> {
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
            inspector_version: "0.3.0-old-rc-label".to_string(),
            workspace_fingerprint: fingerprint,
            items: HashMap::from([("demo::Thing".to_string(), dummy_type_metadata("demo::Thing"))]),
            misses: HashMap::new(),
        },
    )?;

    let mut inner = CacheInner::default();
    ensure_disk_cache_loaded(&mut inner, tmp.path())?;
    assert!(
        inner
            .items
            .contains_key(&(tmp.path().to_path_buf(), "demo::Thing".to_string())),
        "rust-inspect metadata compatibility is controlled by DISK_CACHE_FORMAT, not package version labels"
    );
    Ok(())
}

/// Legacy rc-versioned fingerprints remain readable so rc bumps do not force needless re-extraction.
#[test]
fn disk_cache_accepts_legacy_versioned_workspace_fingerprint() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    let old_version = "0.3.0-rc43";
    let fingerprint = legacy_versioned_workspace_fingerprint(tmp.path(), old_version)?;
    write_disk_cache(
        tmp.path(),
        &DiskCacheEnvelope {
            cache_format: DISK_CACHE_FORMAT,
            inspector_version: old_version.to_string(),
            workspace_fingerprint: fingerprint,
            items: HashMap::from([("demo::Thing".to_string(), dummy_type_metadata("demo::Thing"))]),
            misses: HashMap::new(),
        },
    )?;

    let mut inner = CacheInner::default();
    ensure_disk_cache_loaded(&mut inner, tmp.path())?;
    assert!(
        inner
            .items
            .contains_key(&(tmp.path().to_path_buf(), "demo::Thing".to_string())),
        "rc-bumped toolchains should reuse old versioned rust-inspect caches when dependency inputs still match"
    );
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
                alias_target: None,
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
/// Definition paths should reuse cached public re-export metadata instead of forcing another extraction.
fn definition_path_alias_hits_existing_cached_reexport() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;

    let cache = RustMetadataCache::new();
    cache.insert_test_item(
        tmp.path(),
        dummy_reexported_type_metadata("bridge::ScalarUDF", "bridge::udf::ScalarUDF"),
    )?;
    {
        let inner = cache
            .inner
            .lock()
            .map_err(|_| std::io::Error::other("poisoned cache"))?;
        assert_eq!(
            inner
                .definition_aliases
                .get(&(tmp.path().canonicalize()?, "bridge::udf::ScalarUDF".to_string()))
                .map(String::as_str),
            Some("bridge::ScalarUDF"),
            "definition-path aliases should be indexed when metadata enters the cache"
        );
    }

    let hit = cache
        .get_cached(tmp.path(), "bridge::udf::ScalarUDF")?
        .ok_or_else(|| std::io::Error::other("expected definition-path cache alias hit"))?;
    assert_eq!(hit.metadata.canonical_path, "bridge::udf::ScalarUDF");
    assert_eq!(
        hit.metadata.definition_path.as_deref(),
        Some("bridge::udf::ScalarUDF")
    );
    assert!(hit.alias_used);

    let extracted = cache.get_or_extract(tmp.path(), "bridge::udf::ScalarUDF", &|_| ())?;
    assert_eq!(extracted.canonical_path, "bridge::udf::ScalarUDF");
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

/// Dependency manifest root lookup misses are cached per generated workspace.
#[test]
fn dependency_manifest_resolution_is_cached_per_manifest_root() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;

    let root = tmp.path().canonicalize()?;
    let mut inner = CacheInner::default();
    let crate_name = "definitely_missing_dependency";

    assert!(resolve_dependency_manifest_dir(&mut inner, &root, crate_name, Some(&[])).is_none());
    assert!(inner
        .dependency_manifest_dirs
        .contains_key(&(root.clone(), crate_name.to_string())));
    let cached_entries = inner.dependency_manifest_dirs.len();

    assert!(resolve_dependency_manifest_dir(&mut inner, &root, crate_name, Some(&[])).is_none());
    assert_eq!(
        inner.dependency_manifest_dirs.len(),
        cached_entries,
        "repeat dependency-root lookups should use the in-memory resolution cache"
    );
    Ok(())
}

/// Dependency manifest lookup cache keys normalize hyphenated package names and underscored crate names.
#[test]
fn dependency_manifest_resolution_cache_normalizes_crate_spelling() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;

    let root = tmp.path().canonicalize()?;
    let mut inner = CacheInner::default();

    assert!(resolve_dependency_manifest_dir(&mut inner, &root, "foo_bar", Some(&[])).is_none());
    let cached_entries = inner.dependency_manifest_dirs.len();
    assert!(resolve_dependency_manifest_dir(&mut inner, &root, "foo-bar", Some(&[])).is_none());
    assert_eq!(
        inner.dependency_manifest_dirs.len(),
        cached_entries,
        "hyphen and underscore crate spellings should share dependency-root resolution cache entries"
    );
    assert!(inner
        .dependency_manifest_dirs
        .contains_key(&(root, "foo_bar".to_string())));
    Ok(())
}

/// Non-root crate misses do not force the generated root workspace to reload with build-script out-dirs.
#[test]
fn root_out_dir_workspace_is_skipped_for_non_root_crate_misses() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"root-probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::create_dir_all(tmp.path().join("src"))?;
    fs::write(tmp.path().join("src/lib.rs"), "pub fn keep() {}\n")?;

    let cache = RustMetadataCache::new();
    let query = "external_crate::Missing";

    let result = cache.get_or_extract(tmp.path(), query, &|_| ());
    assert!(matches!(
        result,
        Err(RustMetadataError::CrateNotFound(_))
            | Err(RustMetadataError::PathNotResolved(_))
            | Err(RustMetadataError::UnsupportedMacro(_))
    ));

    let root = tmp.path().canonicalize()?;
    let inner = cache
        .inner
        .lock()
        .map_err(|_| std::io::Error::other("poisoned cache"))?;
    assert!(
        !inner.workspaces.contains_key(&(root, true)),
        "a dependency or stdlib miss should not force the expensive root out-dir workspace route"
    );
    Ok(())
}

/// Dependency items generated into `OUT_DIR` should resolve through the root workspace that checked those build scripts.
#[test]
fn dependency_generated_out_dir_items_resolve_through_root_workspace() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("root");
    let dep = tmp.path().join("generated-dep");
    let helper = tmp.path().join("helper-crate");
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(dep.join("src"))?;
    fs::create_dir_all(helper.join("src"))?;
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"root\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ngenerated-dep = { path = \"../generated-dep\" }\n",
    )?;
    fs::write(root.join("src").join("lib.rs"), "pub fn keep() {}\n")?;
    fs::create_dir_all(root.join(".cargo"))?;
    fs::write(
        root.join(".cargo").join("config.toml"),
        format!(
            "[build]\ntarget-dir = \"{}\"\n",
            root.join("target").to_string_lossy().replace('\\', "\\\\")
        ),
    )?;
    fs::write(
        dep.join("Cargo.toml"),
        "[package]\nname = \"generated-dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n\n[lib]\nname = \"generated_dep\"\n\n[dependencies]\nhelper-crate = { path = \"../helper-crate\" }\n",
    )?;
    fs::write(
        helper.join("Cargo.toml"),
        "[package]\nname = \"helper-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nname = \"helper_crate\"\n",
    )?;
    fs::write(
        helper.join("src").join("lib.rs"),
        "pub struct Thing { pub value: String }\n",
    )?;
    fs::write(
        dep.join("build.rs"),
        r#"fn main() {
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    std::fs::write(
        out_dir.join("generated.rs"),
        "pub struct Nested { pub count: u32 }\n\
         pub struct GeneratedThing {\n\
             pub r#type: ::core::option::Option<Nested>,\n\
             pub names: ::std::vec::Vec<::std::string::String>,\n\
             pub helper: helper_crate::Thing,\n\
         }\n\
         pub enum GeneratedChoice {\n\
             Unit,\n\
             Child(nested::Child),\n\
             Count(i32),\n\
             Boxed(::std::boxed::Box<Nested>),\n\
         }\n\
         pub struct EmptyRecord {}\n\
         pub mod nested {\n\
             pub struct Child { pub parent: super::Nested }\n\
         }\n",
    )
    .unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
"#,
    )?;
    fs::write(
        dep.join("src").join("lib.rs"),
        "pub mod generated { include!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\")); }\n",
    )?;
    let status = std::process::Command::new("cargo")
        .arg("check")
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .status()?;
    assert!(status.success(), "fixture cargo check should produce build-script out dirs");

    let cache = RustMetadataCache::new();
    let metadata = cache.get_or_extract(&root, "generated_dep::generated::GeneratedThing", &|_| ())?;
    let RustItemKind::Type(type_info) = &metadata.kind else {
        return Err("expected generated dependency type metadata".into());
    };
    assert_eq!(type_info.fields.len(), 3);
    assert_eq!(type_info.fields[0].name, "type");
    assert_eq!(
        type_info.fields[0].type_display,
        "Option<generated_dep::generated::Nested>"
    );
    assert_eq!(type_info.fields[1].name, "names");
    assert_eq!(type_info.fields[1].type_display, "Vec<String>");
    assert_eq!(type_info.fields[2].name, "helper");
    assert_eq!(type_info.fields[2].type_display, "helper_crate::Thing");

    let nested = cache.get_or_extract(&root, "generated_dep::generated::nested::Child", &|_| ())?;
    let RustItemKind::Type(nested_info) = &nested.kind else {
        return Err("expected generated dependency nested type metadata".into());
    };
    assert_eq!(nested_info.fields.len(), 1);
    assert_eq!(nested_info.fields[0].name, "parent");
    assert_eq!(
        nested_info.fields[0].type_display,
        "generated_dep::generated::Nested"
    );

    let choice = cache.get_or_extract(&root, "generated_dep::generated::GeneratedChoice", &|_| ())?;
    let RustItemKind::Type(choice_info) = &choice.kind else {
        return Err("expected generated dependency enum metadata".into());
    };
    assert_eq!(choice_info.variants.len(), 4);
    let unit = choice_info
        .variants
        .iter()
        .find(|variant| variant.name == "Unit")
        .ok_or("missing Unit variant")?;
    assert!(
        unit.fields.is_empty(),
        "unit generated variants should not be modeled as zero-argument payload constructors"
    );
    let boxed = choice_info
        .variants
        .iter()
        .find(|variant| variant.name == "Boxed")
        .ok_or("missing Boxed variant")?;
    assert_eq!(
        boxed.fields,
        vec![RustTypeShape::RustPath {
            path: "generated_dep::generated::Nested".to_string(),
            args: Vec::new()
        }]
    );
    let empty = cache.get_or_extract(&root, "generated_dep::generated::EmptyRecord", &|_| ())?;
    let RustItemKind::Type(empty_info) = &empty.kind else {
        return Err("expected generated dependency empty struct metadata".into());
    };
    assert!(
        empty_info.fields.is_empty() && empty_info.variants.is_empty(),
        "zero-field generated structs should keep constructible type metadata"
    );
    Ok(())
}

/// Public crate re-exports are recognized as identity routes while private imports are ignored.
#[test]
fn dependency_reexport_alias_candidate_uses_public_crate_reexports() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let dep_root = tmp.path().join("wrapper-crate");
    fs::create_dir_all(dep_root.join("src"))?;
    fs::write(
        dep_root.join("Cargo.toml"),
        "[package]\nname = \"wrapper-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nname = \"wrapper_crate\"\n",
    )?;
    fs::write(
        dep_root.join("src").join("lib.rs"),
        "// re-exported dependency\npub use inner_crate;\npub use renamed_crate as renamed;\nuse private_crate;\n",
    )?;

    let mut inner = CacheInner::default();
    assert_eq!(
        dependency_reexport_alias_candidate(&mut inner, dep_root.as_path(), "wrapper_crate::inner_crate::Thing")
            .as_deref(),
        Some("inner_crate::Thing")
    );
    assert_eq!(
        dependency_reexport_alias_candidate(&mut inner, dep_root.as_path(), "wrapper_crate::renamed::Thing")
            .as_deref(),
        Some("renamed_crate::Thing")
    );
    assert_eq!(
        dependency_reexport_alias_candidate(&mut inner, dep_root.as_path(), "wrapper_crate::private_crate::Thing"),
        None,
        "private use declarations must not become public re-export identity aliases"
    );
    Ok(())
}

/// Missing items reached through a public crate re-export do not repeat the same lookup in the wrapper workspace.
#[test]
fn dependency_reexport_alias_miss_skips_wrapper_workspace() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("root");
    let wrapper_root = tmp.path().join("wrapper-crate");
    let inner_root = tmp.path().join("inner-crate");
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(wrapper_root.join("src"))?;
    fs::create_dir_all(inner_root.join("src"))?;
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"root\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nwrapper-crate = { path = \"../wrapper-crate\" }\n",
    )?;
    fs::write(root.join("src").join("lib.rs"), "pub fn keep() {}\n")?;
    fs::write(
        wrapper_root.join("Cargo.toml"),
        "[package]\nname = \"wrapper-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nname = \"wrapper_crate\"\n\n[dependencies]\ninner-crate = { path = \"../inner-crate\" }\n",
    )?;
    fs::write(wrapper_root.join("src").join("lib.rs"), "pub use inner_crate;\n")?;
    fs::write(
        inner_root.join("Cargo.toml"),
        "[package]\nname = \"inner-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nname = \"inner_crate\"\n",
    )?;
    fs::write(inner_root.join("src").join("lib.rs"), "pub struct Present;\n")?;

    let cache = RustMetadataCache::new();
    let result = cache.get_or_extract(&root, "wrapper_crate::inner_crate::Missing", &|_| ());
    assert!(matches!(
        result,
        Err(RustMetadataError::CrateNotFound(_))
            | Err(RustMetadataError::PathNotResolved(_))
            | Err(RustMetadataError::UnsupportedMacro(_))
    ));

    let wrapper_root = wrapper_root.canonicalize()?;
    let inner = cache
        .inner
        .lock()
        .map_err(|_| std::io::Error::other("poisoned cache"))?;
    assert!(
        !inner.workspaces.contains_key(&(wrapper_root, true)),
        "a miss through a public crate re-export should not repeat the lookup through the wrapper dependency"
    );
    Ok(())
}

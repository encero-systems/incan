use crate::cache_resolve::dependency_manifest_dir_from_lock_with_search_roots;
use super::*;
use incan_core::interop::{RustItemKind, RustTraitAssoc, RustTypeInfo, RustTypeShape, RustVisibility};

/// Build minimal public Rust type metadata for cache round-trip tests.
fn dummy_type_metadata(path: &str) -> RustItemMetadata {
    RustItemMetadata {
        canonical_path: path.to_string(),
        definition_path: None,
        visibility: RustVisibility::Public,
        kind: RustItemKind::Type(RustTypeInfo {
            alias_target: None,
            metadata_completeness: Default::default(),
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
            metadata_completeness: Default::default(),
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
                metadata_completeness: Default::default(),
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

/// Public dependency source functions and aliases should resolve before the expensive rust-analyzer workspace route.
#[test]
fn dependency_source_metadata_resolves_public_reexported_functions_and_aliases_without_workspace_load()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("root");
    let dep = tmp.path().join("source-dep");
    let public_api = tmp.path().join("public-api");
    let arrow = tmp.path().join("arrow");
    let arrow_array = tmp.path().join("arrow-array");
    let datafusion_expr = tmp.path().join("datafusion-expr");
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(dep.join("src"))?;
    fs::create_dir_all(dep.join("src").join("async_api"))?;
    fs::create_dir_all(dep.join("src").join("frame"))?;
    fs::create_dir_all(dep.join("src").join("math"))?;
    fs::create_dir_all(public_api.join("src"))?;
    fs::create_dir_all(arrow.join("src").join("array"))?;
    fs::create_dir_all(arrow_array.join("src").join("array"))?;
    fs::create_dir_all(datafusion_expr.join("src"))?;
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"root\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\npublic-api = { path = \"../public-api\" }\nsource-dep = { path = \"../source-dep\" }\ndatafusion-expr = { path = \"../datafusion-expr\" }\n",
    )?;
    fs::write(root.join("src").join("lib.rs"), "pub fn keep() {}\n")?;
    fs::write(
        public_api.join("Cargo.toml"),
        "[package]\nname = \"public-api\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nname = \"public_api\"\n\n[dependencies]\narrow = { path = \"../arrow\" }\nsource-dep = { path = \"../source-dep\" }\ndatafusion-expr = { path = \"../datafusion-expr\" }\n",
    )?;
    fs::write(
        public_api.join("src").join("lib.rs"),
        "pub use arrow;\npub mod logical_expr { pub use datafusion_expr::*; }\npub mod prelude { pub use source_dep::expr_fn::*; }\n",
    )?;
    fs::write(
        datafusion_expr.join("Cargo.toml"),
        "[package]\nname = \"datafusion-expr\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nname = \"datafusion_expr\"\n",
    )?;
    fs::write(
        datafusion_expr.join("src").join("lib.rs"),
        "pub enum Expr { Literal }\n",
    )?;
    fs::write(
        arrow.join("Cargo.toml"),
        "[package]\nname = \"arrow\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\narrow-array = { path = \"../arrow-array\" }\n",
    )?;
    fs::write(
        arrow.join("src").join("lib.rs"),
        "pub mod array;\npub mod datatypes { pub enum DataType { Utf8 } }\n",
    )?;
    fs::write(
        arrow.join("src").join("array").join("mod.rs"),
        "pub use arrow_array::*;\n",
    )?;
    fs::write(
        arrow_array.join("Cargo.toml"),
        "[package]\nname = \"arrow-array\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nname = \"arrow_array\"\n",
    )?;
    fs::write(
        arrow_array.join("src").join("lib.rs"),
        "pub mod array;\npub use array::*;\n",
    )?;
    fs::write(
        arrow_array.join("src").join("array").join("mod.rs"),
        "use std::sync::Arc;\npub trait Array {}\npub type ArrayRef = Arc<dyn Array>;\n",
    )?;
    fs::write(
        dep.join("Cargo.toml"),
        "[package]\nname = \"source-dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nname = \"source_dep\"\n",
    )?;
    fs::write(
        dep.join("src").join("lib.rs"),
        "pub use function::ScalarFunctionImplementation;\npub use udf::create_udf;\npub mod async_api;\npub mod audio;\npub mod catalog;\npub mod datasource;\npub mod execution;\npub mod expr_fn { pub use super::math::expr_fn::*; }\npub mod frame;\npub mod frame_ext;\npub mod function;\npub mod math;\npub mod table;\npub mod types;\npub mod udf;\n",
    )?;
    fs::write(
        dep.join("src").join("catalog.rs"),
        "pub trait TableProvider {}\n",
    )?;
    fs::write(
        dep.join("src").join("datasource.rs"),
        "pub use crate::catalog::TableProvider;\n",
    )?;
    fs::write(
        dep.join("src").join("execution.rs"),
        r#"use crate::catalog::TableProvider;
use std::sync::Arc;

pub struct SessionContext;

impl SessionContext {
    /// Register a table provider through the catalog-facing trait path.
    pub fn register_table(self, provider: Arc<dyn TableProvider>) -> Result<SessionContext, String> { todo!() }
}
"#,
    )?;
    fs::write(
        dep.join("src").join("table.rs"),
        r#"use crate::datasource::TableProvider;
use std::sync::Arc;

pub struct TableFrame;

impl TableFrame {
    /// Return a table provider through the datasource-facing reexport path.
    pub fn into_view(self) -> Arc<dyn TableProvider> { todo!() }
}
"#,
    )?;
    fs::write(
        dep.join("src").join("udf.rs"),
        r#"use crate::{ScalarFunctionImplementation, types::ScalarUDF};
use arrow::datatypes::DataType;

/// Create a scalar UDF from an inspected callback alias.
pub fn create_udf(
    name: &str,
    input_types: Vec<DataType>,
    fun: ScalarFunctionImplementation,
) -> ScalarUDF {
    ScalarUDF { name: name.to_string(), input_count: input_types.len(), fun }
}
"#,
    )?;
    fs::write(
        dep.join("src").join("math").join("mod.rs"),
        r#"
pub mod expr_fn {
    export_functions!(
        (round, "rounds a value", args,),
        (abs, "absolute value", num)
    );
}
"#,
    )?;
    fs::write(
        dep.join("src").join("async_api").join("mod.rs"),
        "mod plan;\npub use plan::*;\n",
    )?;
    fs::write(
        dep.join("src").join("async_api").join("plan.rs"),
        "use datafusion_expr::Expr;\n\npub async fn load_plan() -> Result<Expr, String> { todo!() }\n",
    )?;
    fs::write(
        dep.join("src").join("audio.rs"),
        r#"
pub struct Data;
pub struct OutputCallbackInfo;
pub struct Device;

impl Device {
    /// Build an output stream from generic Rust callback bounds.
    pub fn build_output_stream_raw<D, E>(
        &self,
        mut data_callback: D,
        mut error_callback: E,
    ) where
        D: FnMut(&mut Data, &OutputCallbackInfo) + Send + 'static,
        E: FnMut(String),
    {
        let mut data = Data;
        let info = OutputCallbackInfo;
        data_callback(&mut data, &info);
        error_callback("boom".to_string());
    }

    /// Build an output stream from inline generic Rust callback bounds.
    pub fn build_output_stream_inline<
        D: FnMut(&mut Data, &OutputCallbackInfo) + Send + 'static,
        E: FnMut(String),
    >(
        &self,
        mut data_callback: D,
        mut error_callback: E,
    ) {
        let mut data = Data;
        let info = OutputCallbackInfo;
        data_callback(&mut data, &info);
        error_callback("boom".to_string());
    }
}
"#,
    )?;
    fs::write(
        dep.join("src").join("function.rs"),
        r#"use crate::types::ColumnarValue;
use std::sync::Arc;

pub trait Message {
    /// Encode this value into bytes.
    fn encode_to_vec(&self) -> Vec<u8>;
    /// Decode this value from bytes.
    fn decode(buf: &[u8]) -> Result<Self>;
}

pub trait FunctionRegistry {
    /// Return a registered scalar callback by name.
    fn udf(&self, name: &str) -> Result<ScalarFunctionImplementation>;
}

pub type Result<T> = std::result::Result<T, String>;
pub type ScalarFunctionImplementation =
    Arc<dyn Fn(&[ColumnarValue]) -> Result<ColumnarValue> + Send + Sync>;
"#,
    )?;
    fs::write(
        dep.join("src").join("types.rs"),
        r#"use arrow::datatypes::DataType;

pub struct ColumnarValue;
pub struct Frame;
pub struct ScalarUDF {
    pub name: String,
    pub input_count: usize,
    pub fun: crate::ScalarFunctionImplementation,
}
pub enum PublicChoice {
    Data(DataType),
    Local(ColumnarValue),
}
"#,
    )?;
    fs::write(
        dep.join("src").join("frame_ext.rs"),
        r#"use crate::types::Frame;

impl Frame {
    /// Write this frame to CSV.
    pub async fn write_csv(self, path: &str) -> Result<Frame, String> { todo!() }

    /// Find a borrowed string inside the frame.
    pub fn lifetime_find<'h>(self, haystack: &'h str) -> Result<Frame, String> { todo!() }

    /// Rename one column while preserving borrowed string argument metadata.
    pub fn with_column_renamed(
        self,
        old_name: impl Into<String>,
        new_name: &str,
    ) -> Result<Frame, String> { todo!() }
}
"#,
    )?;
    fs::write(
        dep.join("src").join("frame").join("mod.rs"),
        r#"mod parquet;

pub trait NestedProvider {}
pub struct NestedFrame;

impl NestedFrame {
    /// Return a nested provider through a dyn trait object.
    pub fn into_view(self) -> std::sync::Arc<dyn NestedProvider> { todo!() }
    /// Return a static borrowed nested frame.
    pub fn static_ref(self) -> Option<&'static NestedFrame> { todo!() }
}
"#,
    )?;
    fs::write(
        dep.join("src").join("frame").join("parquet.rs"),
        r#"use super::NestedFrame;

impl NestedFrame {
    /// Write this nested frame to Parquet.
    pub async fn write_parquet(self, path: &str) -> Result<NestedFrame, String> { todo!() }
}
"#,
    )?;

    let cache = RustMetadataCache::new();
    let function = cache.get_or_extract(&root, "source_dep::create_udf", &|_| ())?;
    let RustItemKind::Function(sig) = &function.kind else {
        return Err("expected source dependency function metadata".into());
    };
    assert_eq!(function.definition_path.as_deref(), Some("source_dep::udf::create_udf"));
    assert_eq!(sig.params.len(), 3);
    assert_eq!(sig.params[0].type_display, "&str");
    assert_eq!(
        sig.params[1].type_display,
        "Vec<public_api::arrow::datatypes::DataType>"
    );
    assert_eq!(
        sig.params[2].type_display,
        "source_dep::ScalarFunctionImplementation"
    );
    assert_eq!(sig.return_type, "source_dep::types::ScalarUDF");

    let array_ref = cache.get_or_extract(&root, "public_api::arrow::array::ArrayRef", &|_| ())?;
    let RustItemKind::Type(type_info) = &array_ref.kind else {
        return Err("expected facade dependency alias metadata".into());
    };
    assert_eq!(
        array_ref.definition_path.as_deref(),
        Some("arrow_array::array::ArrayRef")
    );
    assert_eq!(
        type_info.alias_target.as_deref(),
        Some("std::sync::Arc<dyn arrow_array::array::Array>")
    );

    let macro_function = cache.get_or_extract(&root, "public_api::prelude::round", &|_| ())?;
    let RustItemKind::Function(sig) = &macro_function.kind else {
        return Err("expected facade macro-emitted function metadata".into());
    };
    assert_eq!(
        macro_function.definition_path.as_deref(),
        Some("source_dep::math::expr_fn::round")
    );
    assert_eq!(sig.params.len(), 1);
    assert_eq!(sig.params[0].name.as_deref(), Some("args"));
    assert_eq!(sig.params[0].type_display, "Vec<public_api::logical_expr::Expr>");
    assert_eq!(sig.return_type, "public_api::logical_expr::Expr");

    let async_function = cache.get_or_extract(&root, "source_dep::async_api::load_plan", &|_| ())?;
    let RustItemKind::Function(sig) = &async_function.kind else {
        return Err("expected async source dependency function metadata".into());
    };
    assert_eq!(
        async_function.definition_path.as_deref(),
        Some("source_dep::async_api::plan::load_plan")
    );
    assert!(sig.is_async, "source metadata should preserve async functions reached through glob reexports");
    assert_eq!(
        sig.return_type,
        "Result<public_api::logical_expr::Expr, String>"
    );

    let device = cache.get_or_extract(&root, "source_dep::audio::Device", &|_| ())?;
    let RustItemKind::Type(type_info) = &device.kind else {
        return Err("expected source dependency Device metadata".into());
    };
    let build_output = type_info
        .methods
        .iter()
        .find(|method| method.name == "build_output_stream_raw")
        .ok_or("expected build_output_stream_raw method metadata")?;
    assert_eq!(
        build_output.signature.params[1].type_display,
        "impl FnMut(&mut source_dep::audio::Data, &source_dep::audio::OutputCallbackInfo)"
    );
    assert_eq!(
        build_output.signature.params[2].type_display,
        "impl FnMut(String)"
    );
    let build_output_inline = type_info
        .methods
        .iter()
        .find(|method| method.name == "build_output_stream_inline")
        .ok_or("expected build_output_stream_inline method metadata")?;
    assert_eq!(
        build_output_inline.signature.params[1].type_display,
        "impl FnMut(&mut source_dep::audio::Data, &source_dep::audio::OutputCallbackInfo)"
    );
    assert_eq!(
        build_output_inline.signature.params[2].type_display,
        "impl FnMut(String)"
    );

    let alias = cache.get_or_extract(&root, "source_dep::ScalarFunctionImplementation", &|_| ())?;
    let RustItemKind::Type(type_info) = &alias.kind else {
        return Err("expected source dependency alias metadata".into());
    };
    assert_eq!(
        alias.definition_path.as_deref(),
        Some("source_dep::function::ScalarFunctionImplementation")
    );
    assert_eq!(
        type_info.alias_target.as_deref(),
        Some(
            "std::sync::Arc<dyn Fn(&[source_dep::types::ColumnarValue]) -> source_dep::function::Result<source_dep::types::ColumnarValue> + Send + Sync>"
        )
    );

    let message = cache.get_or_extract(&root, "source_dep::function::Message", &|_| ())?;
    let RustItemKind::Trait(trait_info) = &message.kind else {
        return Err("expected source dependency trait metadata".into());
    };
    assert_eq!(
        message.definition_path.as_deref(),
        Some("source_dep::function::Message")
    );
    let encode = trait_info
        .items
        .iter()
        .find_map(|item| match item {
            RustTraitAssoc::Function { name, signature } if name == "encode_to_vec" => Some(signature),
            _ => None,
        })
        .ok_or("expected encode_to_vec trait method metadata")?;
    assert_eq!(encode.params.len(), 1);
    assert_eq!(encode.params[0].type_display, "&self");
    assert_eq!(encode.return_type, "Vec<u8>");
    let decode = trait_info
        .items
        .iter()
        .find_map(|item| match item {
            RustTraitAssoc::Function { name, signature } if name == "decode" => Some(signature),
            _ => None,
        })
        .ok_or("expected decode trait method metadata")?;
    assert_eq!(decode.params.len(), 1);
    assert_eq!(decode.params[0].type_display, "&[u8]");
    assert_eq!(decode.return_type, "Result<Self>");

    let registry = cache.get_or_extract(&root, "source_dep::function::FunctionRegistry", &|_| ())?;
    let RustItemKind::Trait(registry_info) = &registry.kind else {
        return Err("expected source dependency registry trait metadata".into());
    };
    let udf = registry_info
        .items
        .iter()
        .find_map(|item| match item {
            RustTraitAssoc::Function { name, signature } if name == "udf" => Some(signature),
            _ => None,
        })
        .ok_or("expected udf trait method metadata")?;
    assert_eq!(udf.params.len(), 2);
    assert_eq!(udf.params[0].type_display, "&self");
    assert_eq!(udf.params[1].type_display, "&str");
    assert_eq!(
        udf.return_type,
        "Result<source_dep::function::ScalarFunctionImplementation>"
    );

    let enum_meta = cache.get_or_extract(&root, "source_dep::types::PublicChoice", &|_| ())?;
    let RustItemKind::Type(type_info) = &enum_meta.kind else {
        return Err("expected source dependency enum metadata".into());
    };
    let data_variant = type_info
        .variants
        .iter()
        .find(|variant| variant.name == "Data")
        .ok_or("expected Data variant")?;
    assert_eq!(
        data_variant.fields,
        vec![RustTypeShape::RustPath {
            path: "public_api::arrow::datatypes::DataType".to_string(),
            args: Vec::new(),
        }]
    );

    let frame = cache.get_or_extract(&root, "source_dep::types::Frame", &|_| ())?;
    let RustItemKind::Type(frame_info) = &frame.kind else {
        return Err("expected source dependency frame metadata".into());
    };
    let write_csv = frame_info
        .methods
        .iter()
        .find(|method| method.name == "write_csv")
        .ok_or("expected sibling-module inherent write_csv metadata")?;
    assert!(write_csv.signature.is_async);
    assert_eq!(write_csv.signature.params.len(), 2);
    assert_eq!(write_csv.signature.params[0].name.as_deref(), Some("self"));
    assert_eq!(
        write_csv.signature.params[0].type_display,
        "source_dep::types::Frame"
    );
    assert_eq!(write_csv.signature.params[1].type_display, "&str");
    let renamed = frame_info
        .methods
        .iter()
        .find(|method| method.name == "with_column_renamed")
        .ok_or("expected sibling-module inherent with_column_renamed metadata")?;
    assert_eq!(renamed.signature.params.len(), 3);
    assert_eq!(renamed.signature.params[1].type_display, "implInto<String>");
    assert_eq!(renamed.signature.params[2].type_display, "&str");
    let lifetime_find = frame_info
        .methods
        .iter()
        .find(|method| method.name == "lifetime_find")
        .ok_or("expected borrowed-lifetime inherent method metadata")?;
    assert_eq!(lifetime_find.signature.params[1].type_display, "&str");
    let nested_frame = cache.get_or_extract(&root, "source_dep::frame::NestedFrame", &|_| ())?;
    let RustItemKind::Type(nested_frame_info) = &nested_frame.kind else {
        return Err("expected nested source dependency frame metadata".into());
    };
    let write_parquet = nested_frame_info
        .methods
        .iter()
        .find(|method| method.name == "write_parquet")
        .ok_or("expected sibling-module super-alias inherent write_parquet metadata")?;
    assert!(write_parquet.signature.is_async);
    assert_eq!(
        write_parquet.signature.params[0].type_display,
        "source_dep::frame::NestedFrame"
    );
    assert_eq!(write_parquet.signature.params[1].type_display, "&str");
    let into_view = nested_frame_info
        .methods
        .iter()
        .find(|method| method.name == "into_view")
        .ok_or("expected dyn-return inherent method metadata")?;
    assert_eq!(
        into_view.signature.return_type,
        "std::sync::Arc<dynsource_dep::frame::NestedProvider>"
    );
    let static_ref = nested_frame_info
        .methods
        .iter()
        .find(|method| method.name == "static_ref")
        .ok_or("expected static-lifetime generic return metadata")?;
    assert_eq!(
        static_ref.signature.return_type,
        "Option<&source_dep::frame::NestedFrame>"
    );
    let table_frame = cache.get_or_extract(&root, "source_dep::table::TableFrame", &|_| ())?;
    let RustItemKind::Type(table_frame_info) = &table_frame.kind else {
        return Err("expected source dependency table frame metadata".into());
    };
    let into_view = table_frame_info
        .methods
        .iter()
        .find(|method| method.name == "into_view")
        .ok_or("expected table provider method metadata")?;
    assert_eq!(
        into_view.signature.return_type,
        "std::sync::Arc<dynsource_dep::catalog::TableProvider>"
    );
    let session = cache.get_or_extract(&root, "source_dep::execution::SessionContext", &|_| ())?;
    let RustItemKind::Type(session_info) = &session.kind else {
        return Err("expected source dependency session context metadata".into());
    };
    let register_table = session_info
        .methods
        .iter()
        .find(|method| method.name == "register_table")
        .ok_or("expected catalog provider method metadata")?;
    assert_eq!(
        register_table.signature.params[1].type_display,
        "std::sync::Arc<dynsource_dep::catalog::TableProvider>"
    );

    let inner = cache
        .inner
        .lock()
        .map_err(|_| std::io::Error::other("poisoned cache"))?;
    assert!(
        inner.workspaces.is_empty(),
        "dependency source metadata should not load any rust-analyzer workspaces"
    );
    Ok(())
}

/// Complete metadata extraction must retain declared mutable-reference parameter shape rather than trusting the
/// rust-analyzer display, which can erase `mut` from ordinary reference parameters.
#[test]
fn complete_dependency_metadata_preserves_mutable_reference_parameters() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("root");
    let dep = tmp.path().join("source-dep");
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(dep.join("src"))?;
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"root\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nsource-dep = { path = \"../source-dep\" }\n",
    )?;
    fs::write(root.join("src").join("lib.rs"), "pub fn keep() {}\n")?;
    fs::write(
        dep.join("Cargo.toml"),
        "[package]\nname = \"source-dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nname = \"source_dep\"\n",
    )?;
    fs::write(
        dep.join("src").join("lib.rs"),
        r#"
pub struct Header;
pub struct Builder;

impl Builder {
    /// Append one header through a mutable reference parameter.
    pub fn append_data(&mut self, header: &mut Header) {
        let _ = header;
    }
}
"#,
    )?;

    let cache = RustMetadataCache::new();
    let metadata = cache.get_or_extract_complete(&root, "source_dep::Builder", &|_| ())?;
    let RustItemKind::Type(type_info) = &metadata.kind else {
        return Err("expected complete source dependency Builder metadata".into());
    };
    let append_data = type_info
        .methods
        .iter()
        .find(|method| method.name == "append_data")
        .ok_or("expected append_data method metadata")?;
    assert_eq!(
        append_data.signature.params[1].type_display,
        "&mut source_dep::Header"
    );
    Ok(())
}

/// Source metadata indexes are shared inside one cache instance, but path normalization depends on the consuming root.
#[test]
fn dependency_source_metadata_index_is_keyed_by_root_facing_reexports() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let root_a = tmp.path().join("root-a");
    let root_b = tmp.path().join("root-b");
    let dep = tmp.path().join("source-dep");
    let facade_a = tmp.path().join("facade-a");
    let facade_b = tmp.path().join("facade-b");
    let external = tmp.path().join("external-type");
    for root in [&root_a, &root_b, &dep, &facade_a, &facade_b, &external] {
        fs::create_dir_all(root.join("src"))?;
    }
    fs::write(
        root_a.join("Cargo.toml"),
        "[package]\nname = \"root_a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nsource-dep = { path = \"../source-dep\" }\nfacade-a = { path = \"../facade-a\" }\n",
    )?;
    fs::write(root_a.join("src").join("lib.rs"), "pub fn keep() {}\n")?;
    fs::write(
        root_b.join("Cargo.toml"),
        "[package]\nname = \"root_b\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nsource-dep = { path = \"../source-dep\" }\nfacade-b = { path = \"../facade-b\" }\n",
    )?;
    fs::write(root_b.join("src").join("lib.rs"), "pub fn keep() {}\n")?;
    fs::write(
        facade_a.join("Cargo.toml"),
        "[package]\nname = \"facade-a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nname = \"facade_a\"\n\n[dependencies]\nexternal-type = { path = \"../external-type\" }\n",
    )?;
    fs::write(
        facade_a.join("src").join("lib.rs"),
        "pub mod types { pub use external_type::*; }\n",
    )?;
    fs::write(
        facade_b.join("Cargo.toml"),
        "[package]\nname = \"facade-b\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nname = \"facade_b\"\n\n[dependencies]\nexternal-type = { path = \"../external-type\" }\n",
    )?;
    fs::write(
        facade_b.join("src").join("lib.rs"),
        "pub mod model { pub use external_type::*; }\n",
    )?;
    fs::write(
        external.join("Cargo.toml"),
        "[package]\nname = \"external-type\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nname = \"external_type\"\n",
    )?;
    fs::write(external.join("src").join("lib.rs"), "pub struct External;\n")?;
    fs::write(
        dep.join("Cargo.toml"),
        "[package]\nname = \"source-dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nname = \"source_dep\"\n\n[dependencies]\nexternal-type = { path = \"../external-type\" }\n",
    )?;
    fs::write(
        dep.join("src").join("lib.rs"),
        "use external_type::External;\n\npub fn make() -> External { todo!() }\n",
    )?;

    let cache = RustMetadataCache::new();
    let root_a_function = cache.get_or_extract(&root_a, "source_dep::make", &|_| ())?;
    let RustItemKind::Function(root_a_sig) = &root_a_function.kind else {
        return Err("expected root A source function metadata".into());
    };
    assert_eq!(root_a_sig.return_type, "facade_a::types::External");

    let root_b_function = cache.get_or_extract(&root_b, "source_dep::make", &|_| ())?;
    let RustItemKind::Function(root_b_sig) = &root_b_function.kind else {
        return Err("expected root B source function metadata".into());
    };
    assert_eq!(root_b_sig.return_type, "facade_b::model::External");

    let inner = cache
        .inner
        .lock()
        .map_err(|_| std::io::Error::other("poisoned cache"))?;
    assert_eq!(
        inner.source_public_reexport_paths.len(),
        2,
        "the same dependency source root should have one source index per root-facing reexport view"
    );
    Ok(())
}

/// Public source re-export targets should resolve aliases from their declaring file without treating private imports
/// as public items by themselves.
#[test]
fn dependency_source_metadata_resolves_public_globs_through_local_aliases_without_workspace_load()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("root");
    let dep = tmp.path().join("rustix-sim");
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(dep.join("src").join("fs"))?;
    fs::create_dir_all(dep.join("src").join("backend").join("fs"))?;
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"root\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nrustix-sim = { path = \"../rustix-sim\" }\n",
    )?;
    fs::write(root.join("src").join("lib.rs"), "pub fn keep() {}\n")?;
    fs::write(
        dep.join("Cargo.toml"),
        "[package]\nname = \"rustix-sim\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nname = \"rustix\"\n",
    )?;
    fs::write(
        dep.join("src").join("lib.rs"),
        "mod backend;\npub mod fs;\n",
    )?;
    fs::write(
        dep.join("src").join("fs").join("mod.rs"),
        "mod constants;\npub(crate) mod fd;\npub use constants::*;\npub use fd::*;\n",
    )?;
    fs::write(
        dep.join("src").join("fs").join("constants.rs"),
        "use crate::backend;\n\npub use backend::fs::types::*;\n",
    )?;
    fs::write(
        dep.join("src").join("fs").join("fd.rs"),
        "use crate::backend::fs::types::StatVfs;\n\npub fn fstatvfs() -> StatVfs { todo!() }\n",
    )?;
    fs::write(
        dep.join("src").join("backend").join("mod.rs"),
        "pub mod fs;\n",
    )?;
    fs::write(
        dep.join("src").join("backend").join("fs").join("mod.rs"),
        "pub mod types;\n",
    )?;
    fs::write(
        dep.join("src").join("backend").join("fs").join("types.rs"),
        "pub struct StatVfs { pub blocks: u64 }\n",
    )?;

    let cache = RustMetadataCache::new();
    let hit = cache
        .get_cached_or_extract_fast(&root, "rustix::fs::StatVfs")?
        .ok_or_else(|| std::io::Error::other("expected fast source metadata hit for public glob reexport"))?;
    assert_eq!(hit.metadata.canonical_path, "rustix::fs::StatVfs");
    assert_eq!(
        hit.metadata.definition_path.as_deref(),
        Some("rustix::backend::fs::types::StatVfs")
    );
    let RustItemKind::Type(type_info) = &hit.metadata.kind else {
        return Err("expected source struct metadata".into());
    };
    assert_eq!(type_info.fields.len(), 1);
    assert_eq!(type_info.fields[0].name, "blocks");
    assert_eq!(type_info.fields[0].type_display, "u64");

    let dep = dep.canonicalize()?;
    let inner = cache
        .inner
        .lock()
        .map_err(|_| std::io::Error::other("poisoned cache"))?;
    assert!(
        !inner.workspaces.contains_key(&(dep, true)),
        "fast source metadata should resolve alias-backed public globs without loading the dependency workspace"
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
        dep.join("src").join("lib.rs"),
        "pub mod generated { include!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\")); }\npub mod proto;\n",
    )?;
    fs::write(
        dep.join("src").join("proto.rs"),
        "include!(concat!(env!(\"OUT_DIR\"), \"/external.rs\"));\n",
    )?;
    let out_dir = root.join("target").join("debug").join("build").join("generated_dep-fixture").join("out");
    fs::create_dir_all(&out_dir)?;
    fs::write(
        out_dir.join("generated.rs"),
        r#"pub struct Nested { pub count: u32 }
pub struct GeneratedThing {
    pub r#type: ::core::option::Option<Nested>,
    pub names: ::std::vec::Vec<::std::string::String>,
    pub helper: helper_crate::Thing,
}
pub enum GeneratedChoice {
    Unit,
    Child(nested::Child),
    Count(i32),
    Boxed(::std::boxed::Box<Nested>),
}
pub struct EmptyRecord {}
pub mod nested {
    pub struct Child { pub parent: super::Nested }
}
"#,
    )?;
    fs::write(
        out_dir.join("external.rs"),
        "pub struct ExternalThing { pub label: ::std::string::String }\n",
    )?;

    let cache = RustMetadataCache::new();
    let metadata = cache.get_or_extract(&root, "generated_dep::generated::GeneratedThing", &|_| ())?;
    {
        let inner = cache
            .inner
            .lock()
            .map_err(|_| std::io::Error::other("poisoned cache"))?;
        assert!(
            !inner.workspaces.contains_key(&(dep.canonicalize()?, true)),
            "generated dependency OUT_DIR metadata should be parsed directly before loading the dependency workspace"
        );
    }
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
    let external = cache.get_or_extract(&root, "generated_dep::proto::ExternalThing", &|_| ())?;
    let RustItemKind::Type(external_info) = &external.kind else {
        return Err("expected external module generated metadata".into());
    };
    assert_eq!(external_info.fields.len(), 1);
    assert_eq!(external_info.fields[0].name, "label");
    assert_eq!(external_info.fields[0].type_display, "String");

    let wrong_owner = cache.get_or_extract(&root, "generated_dep::wrong::GeneratedThing", &|_| ());
    assert!(
        matches!(wrong_owner, Err(RustMetadataError::PathNotResolved(_))),
        "generated OUT_DIR fallback must not invent module ownership from a suffix match"
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

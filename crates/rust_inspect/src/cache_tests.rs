use super::*;
use crate::cache_resolve::{
    dependency_manifest_dir_from_lock_with_search_roots, dependency_manifest_dir_from_manifest,
};
use incan_core::interop::{
    RustFunctionSig, RustItemKind, RustParam, RustTraitAssoc, RustTypeInfo, RustTypeShape, RustVisibility,
};

/// Build minimal public Rust type metadata for cache round-trip tests.
fn dummy_type_metadata(path: &str) -> RustItemMetadata {
    RustItemMetadata {
        canonical_path: path.to_string(),
        definition_path: None,
        visibility: RustVisibility::Public,
        kind: RustItemKind::Type(RustTypeInfo {
            type_params: Vec::new(),
            type_param_defaults: Vec::new(),
            mutable_reference_type_params: Vec::new(),
            expanded_derive_traits: Vec::new(),
            has_const_params: false,
            alias_target: None,
            metadata_completeness: Default::default(),
            methods: Vec::new(),
            implemented_traits: Vec::new(),
            fields: Vec::new(),
            variants: Vec::new(),
        }),
    }
}

/// Build minimal public function metadata for complete-cache round-trip tests.
fn dummy_function_metadata(path: &str) -> RustItemMetadata {
    RustItemMetadata {
        canonical_path: path.to_string(),
        definition_path: None,
        visibility: RustVisibility::Public,
        kind: RustItemKind::Function(RustFunctionSig {
            receiver_contract: None,
            type_params: Vec::new(),
            params: vec![RustParam {
                name: Some("value".to_string()),
                type_display: "u64".to_string(),
            }],
            return_type: "u64".to_string(),
            is_async: false,
            is_unsafe: false,
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
            type_params: Vec::new(),
            type_param_defaults: Vec::new(),
            mutable_reference_type_params: Vec::new(),
            expanded_derive_traits: Vec::new(),
            has_const_params: false,
            alias_target: None,
            metadata_completeness: Default::default(),
            methods: Vec::new(),
            implemented_traits: Vec::new(),
            fields: Vec::new(),
            variants: Vec::new(),
        }),
    }
}

/// Dependency-source and generated-source fallbacks must not reinterpret Rust's never type as an owner-relative path.
#[test]
fn source_and_generated_type_displays_preserve_never_type() {
    let external_crates = HashSet::new();
    let aliases = HashMap::new();
    let preferred_external_paths = HashMap::new();
    let source_public_reexports = HashMap::new();
    let errors_module_path = ["errors".to_string()];
    let errors_context = SourceMetadataContext {
        crate_name: "demo",
        module_path: &errors_module_path,
        external_crates: &external_crates,
        aliases: &aliases,
        preferred_external_paths: &preferred_external_paths,
        source_public_reexports: &source_public_reexports,
    };
    let root_context = SourceMetadataContext {
        crate_name: "demo",
        module_path: &[],
        external_crates: &external_crates,
        aliases: &aliases,
        preferred_external_paths: &preferred_external_paths,
        source_public_reexports: &source_public_reexports,
    };

    assert_eq!(
        generated_type_display(
            RUST_NEVER_TYPE_DISPLAY,
            "demo",
            &["errors".to_string()],
            &external_crates,
        ),
        RUST_NEVER_TYPE_DISPLAY,
    );
    assert_eq!(
        source_type_display(RUST_NEVER_TYPE_DISPLAY, &errors_context),
        RUST_NEVER_TYPE_DISPLAY,
    );
    assert_eq!(
        source_type_display("self", &root_context),
        "self",
        "a relative namespace marker with no item tail must fail closed instead of indexing an empty path"
    );
}

/// Source method signatures can reach a public reexport through a `super::` path.
///
/// The reexport key is only available after the relative path has been qualified with its owning crate and module.
#[test]
fn source_type_display_resolves_public_reexport_after_qualifying_super_path() {
    let external_crates = HashSet::new();
    let aliases = HashMap::new();
    let preferred_external_paths = HashMap::new();
    let source_public_reexports = HashMap::from([(
        "datafusion::execution::options::ParquetReadOptions".to_string(),
        "datafusion::datasource::file_format::options::ParquetReadOptions".to_string(),
    )]);
    let module_path = ["execution".to_string(), "context".to_string(), "parquet".to_string()];
    let context = SourceMetadataContext {
        crate_name: "datafusion",
        module_path: &module_path,
        external_crates: &external_crates,
        aliases: &aliases,
        preferred_external_paths: &preferred_external_paths,
        source_public_reexports: &source_public_reexports,
    };

    assert_eq!(
        source_type_display("super::super::options::ParquetReadOptions", &context),
        "datafusion::datasource::file_format::options::ParquetReadOptions"
    );
}

/// Sysroot namespace inspection follows public re-exports and inherited `extern crate` aliases without Cargo.
#[test]
fn sysroot_source_metadata_resolves_public_identity_across_std_alloc_and_core() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    let library_root = tmp.path().join("library");
    let generated_root = tmp.path().join("generated");
    fs::create_dir_all(&generated_root)?;
    for crate_name in ["std", "alloc", "core"] {
        let crate_root = library_root.join(crate_name);
        fs::create_dir_all(crate_root.join("src/io/error"))?;
        fs::write(
            crate_root.join("Cargo.toml"),
            format!("[package]\nname = \"{crate_name}\"\nversion = \"0.0.0\"\nedition = \"2021\"\n"),
        )?;
    }
    fs::write(
        library_root.join("std/src/lib.rs"),
        "extern crate alloc as alloc_crate;\npub mod io;\npub mod time;\n",
    )?;
    fs::write(
        library_root.join("std/src/io/mod.rs"),
        "pub use alloc_crate::io::Error;\npub trait Read { fn take(self, limit: u64); }\n",
    )?;
    fs::write(
        library_root.join("std/src/time.rs"),
        "pub struct SystemTime;\npub const UNIX_EPOCH: SystemTime = SystemTime;\n",
    )?;
    fs::write(library_root.join("alloc/src/lib.rs"), "pub mod io;\n")?;
    fs::write(library_root.join("alloc/src/io/mod.rs"), "pub use core::io::Error;\n")?;
    fs::write(library_root.join("core/src/lib.rs"), "pub mod io;\n")?;
    fs::write(
        library_root.join("core/src/io/mod.rs"),
        "mod error;\npub use error::Error;\n",
    )?;
    fs::write(library_root.join("core/src/io/error.rs"), "pub struct Error;\n")?;

    let mut inner = CacheInner {
        rust_library_source_root: Some(Some(library_root)),
        ..CacheInner::default()
    };
    let metadata = sysroot_source_metadata(&mut inner, &generated_root, "std::io::Error")
        .ok_or_else(|| std::io::Error::other("expected std::io::Error namespace metadata"))?;
    assert_eq!(metadata.canonical_path, "std::io::Error");
    assert_eq!(metadata.definition_path.as_deref(), Some("core::io::error::Error"));
    let RustItemKind::Type(type_info) = metadata.kind else {
        return Err("expected std::io::Error type identity metadata".into());
    };
    assert!(type_info.methods.is_empty());
    assert!(
        sysroot_source_metadata(&mut inner, &generated_root, "std::io::Read").is_none(),
        "identity-only sysroot inspection must not widen legacy trait-method checking"
    );
    let epoch = sysroot_source_metadata(&mut inner, &generated_root, "std::time::UNIX_EPOCH")
        .ok_or_else(|| std::io::Error::other("expected typed sysroot constant metadata"))?;
    let RustItemKind::Constant { type_display } = epoch.kind else {
        return Err("expected std::time::UNIX_EPOCH constant metadata".into());
    };
    assert_eq!(type_display, "std::time::SystemTime");
    Ok(())
}

/// Syntax-only metadata preserves owner type parameters while excluding unsupported lifetime and const parameters.
#[test]
fn generated_type_metadata_preserves_owner_type_parameters() -> Result<(), Box<dyn std::error::Error>> {
    let external_crates = HashSet::new();
    let source = r#"
pub struct Factory<'a, T, const N: usize> {
    pub value: T,
    marker: std::marker::PhantomData<&'a [u8; N]>,
}
"#;
    let info = generated_type_info_from_source(source, &["Factory"], "demo", &[], &external_crates)
        .ok_or_else(|| std::io::Error::other("expected generated Factory metadata"))?;

    assert_eq!(info.type_params, ["T"]);
    assert!(info.has_const_params);
    Ok(())
}

#[test]
/// Tuple-struct syntax records an empty field label so constructors emit positional Rust arguments.
fn generated_tuple_struct_metadata_preserves_positional_constructor_shape() -> Result<(), Box<dyn std::error::Error>> {
    let external_crates = HashSet::new();
    let source = "pub struct ClearColor(pub Color);";
    let info = generated_type_info_from_source(source, &["ClearColor"], "demo", &[], &external_crates)
        .ok_or_else(|| std::io::Error::other("expected generated tuple-struct metadata"))?;

    assert_eq!(info.fields.len(), 1);
    assert_eq!(info.fields[0].name, "");
    assert_eq!(info.fields[0].type_display, "demo::Color");
    Ok(())
}

#[test]
/// Both Cargo's nested registry layout and Oven's exact package root resolve the same locked crate.
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
    let sealed_root =
        dependency_manifest_dir_from_lock_with_search_roots(&root, "foo_bar", std::slice::from_ref(&dep_dir))
            .ok_or_else(|| {
                std::io::Error::other("expected Cargo.lock fallback to accept an exact sealed source root")
            })?;
    assert_eq!(sealed_root, dep_dir);

    let cache = RustMetadataCache::new();
    let hit = cache
        .get_cached_or_extract_fast_with_registry_src_roots(&root, "foo_bar::consume", std::slice::from_ref(&dep_dir))?
        .ok_or_else(|| std::io::Error::other("expected fast metadata from the sealed source root"))?;
    assert_eq!(hit.metadata.canonical_path, "foo_bar::consume");
    assert!(matches!(hit.metadata.kind, RustItemKind::Function(_)));
    Ok(())
}

/// When the lock holds several versions of one package, the fallback resolves the root's declared dependency to the
/// version the root manifest requires and keeps lock order for a crate the root reaches only transitively.
#[test]
fn lock_fallback_selects_the_version_the_root_manifest_requires() -> Result<(), Box<dyn std::error::Error>> {
    // The lock holds two versions of one package: the root's own `substrait = "0.63"` beside the 0.62 that an
    // adapter pins. The older entry sorts first, and only the root's requirement says which one the root links.
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("generated_lock");
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "probe"
version = "0.1.0"
edition = "2021"

[dependencies.substrait]
features = ["protoc"]
version = "0.63"

[dependencies]
adapter = "1"
"#,
    )?;
    fs::write(root.join("src/lib.rs"), "pub fn keep() {}\n")?;
    fs::write(
        root.join("Cargo.lock"),
        r#"version = 3

[[package]]
name = "adapter"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "substrait"
version = "0.62.2"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "substrait"
version = "0.63.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "transitive"
version = "0.2.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "transitive"
version = "0.3.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#,
    )?;
    let registry_src_root = tmp.path().join("cargo-home/registry/src");
    let mut dirs = std::collections::HashMap::new();
    for (name, version) in [
        ("substrait", "0.62.2"),
        ("substrait", "0.63.0"),
        ("transitive", "0.2.0"),
        ("transitive", "0.3.0"),
    ] {
        let dir = registry_src_root
            .join("index.crates.io-test")
            .join(format!("{name}-{version}"));
        fs::create_dir_all(dir.join("src"))?;
        fs::write(
            dir.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n"),
        )?;
        fs::write(dir.join("src/lib.rs"), "pub fn item() {}\n")?;
        dirs.insert((name, version), dir);
    }

    let resolved = dependency_manifest_dir_from_lock_with_search_roots(&root, "substrait", std::slice::from_ref(&registry_src_root))
        .ok_or_else(|| std::io::Error::other("expected the lock fallback to resolve substrait"))?;
    assert_eq!(
        resolved,
        dirs[&("substrait", "0.63.0")],
        "the root's requirement selects 0.63.0 over the older entry that sorts first"
    );
    // A crate the root reaches only transitively keeps lock order: nothing in the root says which one it links.
    let transitive = dependency_manifest_dir_from_lock_with_search_roots(&root, "transitive", &[registry_src_root])
        .ok_or_else(|| std::io::Error::other("expected the lock fallback to resolve transitive"))?;
    assert_eq!(transitive, dirs[&("transitive", "0.2.0")]);
    Ok(())
}

/// A sealed Oven source root has no nested `Cargo.lock`; a public facade re-export must therefore resolve its
/// external target through the generated root's locked selection, never through ambient Cargo sources.
#[test]
fn sealed_registry_sources_resolve_nested_public_reexports_from_generated_lock()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("generated_lock");
    let facade = tmp.path().join("facade-0.1.0");
    let inner = tmp.path().join("inner-0.1.0");
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(facade.join("src"))?;
    fs::create_dir_all(inner.join("src"))?;
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(root.join("src/lib.rs"), "pub fn keep() {}\n")?;
    fs::write(
        root.join("Cargo.lock"),
        r#"version = 3

[[package]]
name = "facade"
version = "0.1.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "inner"
version = "0.1.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#,
    )?;
    fs::write(
        facade.join("Cargo.toml"),
        "[package]\nname = \"facade\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ninner = \"0.1.0\"\n",
    )?;
    fs::write(facade.join("src/lib.rs"), "pub use inner::*;\n")?;
    fs::write(
        inner.join("Cargo.toml"),
        "[package]\nname = \"inner\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(inner.join("src/lib.rs"), "pub struct Vec2;\n")?;

    let cache = RustMetadataCache::new();
    let hit = cache
        .get_cached_or_extract_fast_with_registry_src_roots(&root, "facade::Vec2", &[facade.clone(), inner.clone()])?
        .ok_or_else(|| std::io::Error::other("expected sealed facade re-export metadata"))?;
    assert_eq!(hit.metadata.canonical_path, "facade::Vec2");
    assert_eq!(hit.metadata.definition_path.as_deref(), Some("inner::Vec2"));
    Ok(())
}

/// Public trait methods from a path dependency must retain their callable shapes on the fast namespace route.
#[test]
fn source_route_records_boxed_variant_payloads_as_the_semantic_type_with_their_carrier()
-> Result<(), Box<dyn std::error::Error>> {
    // A registry crate reached without rust-analyzer spells a recursive payload as `Box<T>`. The prelude `Box` is
    // not an item of the owning module: it must canonicalize like the HIR route (semantic `T` plus a `Boxed`
    // carrier) rather than owner-join into `inner::Box` and escape the payload stripper (#1229).
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("generated_lock");
    let inner = tmp.path().join("inner-0.1.0");
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(inner.join("src"))?;
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(root.join("src/lib.rs"), "pub fn keep() {}\n")?;
    fs::write(
        root.join("Cargo.lock"),
        "version = 3\n\n[[package]]\nname = \"inner\"\nversion = \"0.1.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n",
    )?;
    fs::write(
        inner.join("Cargo.toml"),
        "[package]\nname = \"inner\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(
        inner.join("src/lib.rs"),
        "pub struct WindowFunction;\npub enum Expr {\n    WindowFunction(Box<WindowFunction>),\n    Literal(i64),\n}\n",
    )?;

    let cache = RustMetadataCache::new();
    let hit = cache
        .get_cached_or_extract_fast_with_registry_src_roots(&root, "inner::Expr", std::slice::from_ref(&inner))?
        .ok_or_else(|| std::io::Error::other("expected sealed source enum metadata"))?;
    let incan_core::interop::RustItemKind::Type(info) = &hit.metadata.kind else {
        return Err(std::io::Error::other("expected a type item").into());
    };
    let boxed = info
        .variants
        .iter()
        .find(|variant| variant.name == "WindowFunction")
        .ok_or_else(|| std::io::Error::other("WindowFunction variant missing"))?;
    assert_eq!(
        boxed.fields,
        vec![incan_core::interop::RustTypeShape::RustPath {
            path: "inner::WindowFunction".to_string(),
            args: Vec::new(),
        }]
    );
    assert_eq!(
        boxed.field_carriers,
        vec![incan_core::interop::RustPayloadCarrier::Boxed]
    );
    let plain = info
        .variants
        .iter()
        .find(|variant| variant.name == "Literal")
        .ok_or_else(|| std::io::Error::other("Literal variant missing"))?;
    assert_eq!(
        plain.field_carriers,
        vec![incan_core::interop::RustPayloadCarrier::Direct]
    );
    Ok(())
}

/// With two sealed build units of one package, the generated-code route reads the unit whose recorded version matches
/// the inspected dependency, even when the other unit sorts first.
#[test]
fn direct_workspace_reads_the_sealed_build_unit_of_the_inspected_version() -> Result<(), Box<dyn std::error::Error>> {
    // A closure can hold two build units of one package (IncQL's `substrait` beside the one a DataFusion adapter
    // pins). Both seal a `gen.rs`; only the unit built from the inspected dependency's version defines its items.
    // The wrong unit is named so it sorts first, so this passes only because the recorded version filters it out.
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("generated_lock");
    let inner = tmp.path().join("inner-0.1.0");
    let wrong_out = tmp.path().join("plan/target/debug/build/inner-0000000000000000/out");
    let right_out = tmp.path().join("plan/target/debug/build/inner-ffffffffffffffff/out");
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(inner.join("src"))?;
    fs::create_dir_all(&wrong_out)?;
    fs::create_dir_all(&right_out)?;
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(root.join("src/lib.rs"), "pub fn keep() {}\n")?;
    fs::write(
        root.join("Cargo.lock"),
        "version = 3\n\n[[package]]\nname = \"inner\"\nversion = \"0.1.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n",
    )?;
    fs::write(
        inner.join("Cargo.toml"),
        "[package]\nname = \"inner\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(
        inner.join("src/lib.rs"),
        "include!(concat!(env!(\"OUT_DIR\"), \"/gen.rs\"));\n",
    )?;
    fs::write(
        wrong_out.join("gen.rs"),
        "pub mod kind {\n    pub enum Kind {\n        Other(u8),\n        Empty,\n    }\n}\n",
    )?;
    fs::write(
        right_out.join("gen.rs"),
        "pub struct Node;\npub mod kind {\n    pub enum Kind {\n        Node(::prost::alloc::boxed::Box<super::Node>),\n        Empty,\n    }\n}\n",
    )?;
    crate::loader::write_oven_generated_out_dirs(
        &root,
        &[
            crate::loader::SealedGeneratedOutDir {
                out_dir: wrong_out,
                version: Some("0.2.0".to_string()),
            },
            crate::loader::SealedGeneratedOutDir {
                out_dir: right_out,
                version: Some("0.1.0".to_string()),
            },
        ],
    )?;

    let cache = RustMetadataCache::new();
    let hit = cache
        .get_cached_or_extract_fast_with_registry_src_roots(&root, "inner::kind::Kind", std::slice::from_ref(&inner))?
        .ok_or_else(|| std::io::Error::other("expected generated enum metadata through the sealed out dir"))?;
    let incan_core::interop::RustItemKind::Type(info) = &hit.metadata.kind else {
        return Err(std::io::Error::other("expected a type item").into());
    };
    let mut names = info.variants.iter().map(|variant| variant.name.as_str()).collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(names, vec!["Empty", "Node"], "the 0.1.0 unit defines the inspected enum: {names:?}");
    Ok(())
}

/// A direct-inspection workspace resolves a prost-style generated `oneof` enum, carrier included, through the sealed
/// build-script output the installer records for it.
#[test]
fn direct_workspace_reads_sealed_build_script_output_for_generated_enums() -> Result<(), Box<dyn std::error::Error>> {
    // A normal Oven command never runs Cargo, so a dependency whose items live in build-script output (prost's
    // generated modules) is visible only through the sealed plan directories the installer records. Without that
    // route the generated `oneof` enum below simply does not exist for inspection.
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("generated_lock");
    let inner = tmp.path().join("inner-0.1.0");
    let out_dir = tmp
        .path()
        .join("plan/target/aarch64-apple-darwin/debug/build/inner/0123abcd0123abcd/out");
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(inner.join("src"))?;
    fs::create_dir_all(&out_dir)?;
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(root.join("src/lib.rs"), "pub fn keep() {}\n")?;
    fs::write(
        root.join("Cargo.lock"),
        "version = 3\n\n[[package]]\nname = \"inner\"\nversion = \"0.1.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n",
    )?;
    fs::write(
        inner.join("Cargo.toml"),
        "[package]\nname = \"inner\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(
        inner.join("src/lib.rs"),
        "include!(concat!(env!(\"OUT_DIR\"), \"/gen.rs\"));\n",
    )?;
    // Spelled the way prost writes a recursive oneof: a nested module, a `super::` payload path, and the storage
    // carrier through its own re-export of `Box`.
    fs::write(
        out_dir.join("gen.rs"),
        "pub struct Node;\npub mod kind {\n    pub enum Kind {\n        Node(::prost::alloc::boxed::Box<super::Node>),\n        Empty,\n    }\n}\n",
    )?;
    crate::loader::write_oven_generated_out_dirs(
        &root,
        &[crate::loader::SealedGeneratedOutDir {
            out_dir: out_dir.clone(),
            version: None,
        }],
    )?;

    let cache = RustMetadataCache::new();
    let hit = cache
        .get_cached_or_extract_fast_with_registry_src_roots(&root, "inner::kind::Kind", std::slice::from_ref(&inner))?
        .ok_or_else(|| std::io::Error::other("expected generated enum metadata through the sealed out dir"))?;
    let incan_core::interop::RustItemKind::Type(info) = &hit.metadata.kind else {
        return Err(std::io::Error::other("expected a type item").into());
    };
    let boxed = info
        .variants
        .iter()
        .find(|variant| variant.name == "Node")
        .ok_or_else(|| std::io::Error::other("Node variant missing"))?;
    assert_eq!(
        boxed.fields,
        vec![incan_core::interop::RustTypeShape::RustPath {
            path: "inner::Node".to_string(),
            args: Vec::new(),
        }]
    );
    assert_eq!(
        boxed.field_carriers,
        vec![incan_core::interop::RustPayloadCarrier::Boxed]
    );
    Ok(())
}

/// The owning crate of a sealed build-script output directory is read from Oven's `build/<crate>/<hash>/out` layout and
/// from Cargo's `build/<crate>-<hash>/out` layout alike.
#[test]
fn sealed_out_dir_owner_is_read_from_both_build_layouts() {
    use std::path::Path;
    assert_eq!(
        super::sealed_out_dir_crate_name(Path::new(
            "/loaf/target/aarch64-apple-darwin/debug/build/substrait/9741e2/out"
        )),
        Some("substrait".to_string())
    );
    assert_eq!(
        super::sealed_out_dir_crate_name(Path::new("/target/debug/build/substrait-9741e23407182c1c/out")),
        Some("substrait".to_string())
    );
    assert_eq!(super::sealed_out_dir_crate_name(Path::new("/nowhere/out")), None);
}

#[test]
fn fast_source_metadata_retains_cross_crate_trait_method_signatures() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("generated_lock");
    let prost = tmp.path().join("prost");
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(prost.join("src"))?;
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "probe"
version = "0.1.0"
edition = "2021"

[dependencies]
prost = { path = "../prost" }
"#,
    )?;
    fs::write(root.join("src/lib.rs"), "pub fn keep() {}\n")?;
    fs::write(
        prost.join("Cargo.toml"),
        "[package]\nname = \"prost\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(
        prost.join("src/lib.rs"),
        r#"pub trait Buf {}

pub trait Message: Sized {
    /// Decode one message from an accepted buffer implementation.
    fn decode(buf: impl Buf) -> Self;
}
"#,
    )?;

    let cache = RustMetadataCache::new();
    let hit = cache
        .get_cached_or_extract_fast(&root, "prost::Message")?
        .ok_or_else(|| std::io::Error::other("expected fast trait metadata from dependency source"))?;
    let RustItemKind::Trait(info) = &hit.metadata.kind else {
        return Err("expected source trait metadata".into());
    };
    let decode = info
        .items
        .iter()
        .find_map(|item| match item {
            RustTraitAssoc::Function { name, signature } if name == "decode" => Some(signature),
            RustTraitAssoc::Function { .. } | RustTraitAssoc::TypeAlias { .. } | RustTraitAssoc::Constant { .. } => {
                None
            }
        })
        .ok_or_else(|| std::io::Error::other("expected Message::decode metadata"))?;
    assert_eq!(decode.params.len(), 1);
    assert_eq!(decode.params[0].type_display, "implBuf");
    Ok(())
}

/// Compiler-authored generated manifests resolve path dependencies without starting Cargo metadata discovery.
#[test]
fn manifest_path_dependency_resolves_renamed_package_without_cargo() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("generated_lock");
    let dependency = tmp.path().join("source-dep");
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(dependency.join("src"))?;
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "probe"
version = "0.1.0"
edition = "2021"

[dependencies]
renamed_dep = { package = "source-dep", path = "../source-dep" }
"#,
    )?;
    fs::write(
        dependency.join("Cargo.toml"),
        "[package]\nname = \"source-dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    let dependency = dependency.canonicalize()?;

    assert_eq!(
        dependency_manifest_dir_from_manifest(&root, "renamed_dep"),
        Some(dependency.clone())
    );
    assert_eq!(
        dependency_manifest_dir_from_manifest(&root, "source_dep"),
        Some(dependency)
    );
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

/// Complete type metadata must not persist downstream consumer-only implementations as intrinsic type facts.
#[test]
fn complete_metadata_excludes_root_only_cross_crate_implementations() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let trait_owner = tmp.path().join("trait-owner");
    let type_owner = tmp.path().join("type-owner");
    for root in [tmp.path(), trait_owner.as_path(), type_owner.as_path()] {
        fs::create_dir_all(root.join("src"))?;
    }
    fs::write(
        tmp.path().join("Cargo.toml"),
        r#"[package]
name = "solver_root"
version = "0.1.0"
edition = "2021"

[dependencies]
trait-owner = { path = "trait-owner" }
"#,
    )?;
    fs::write(tmp.path().join("src/lib.rs"), "pub fn load_root() {}\n")?;
    fs::write(
        trait_owner.join("Cargo.toml"),
        r#"[package]
name = "trait-owner"
version = "0.1.0"
edition = "2021"

[dependencies]
type-owner = { path = "../type-owner" }
"#,
    )?;
    fs::write(
        trait_owner.join("src/lib.rs"),
        "pub trait Marker {}\nimpl Marker for type_owner::Thing {}\n",
    )?;
    fs::write(
        type_owner.join("Cargo.toml"),
        "[package]\nname = \"type-owner\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(type_owner.join("src/lib.rs"), "pub struct Thing;\n")?;

    // Seed and reload a dependency-type record to reproduce the cross-process consumer path.
    let cache = RustMetadataCache::new();
    cache.get_or_extract_complete(tmp.path(), "type_owner::Thing", &|_| ())?;
    let fresh_cache = RustMetadataCache::new();
    let cached = fresh_cache
        .get_cached(tmp.path(), "type_owner::Thing")?
        .ok_or("complete metadata was not persisted")?;
    let RustItemKind::Type(info) = &cached.metadata.kind else {
        return Err("expected type metadata".into());
    };
    assert!(
        !info
            .implemented_traits
            .iter()
            .any(|implementation| implementation.path == "trait_owner::Marker")
    );
    Ok(())
}

/// Complete extraction must reuse a semantic function record from a fresh disk cache without reopening the
/// generated Rust workspace. Functions have no partial metadata state analogous to incomplete type methods.
#[test]
fn complete_disk_cache_reuses_semantic_function_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    let root = tmp.path().canonicalize()?;
    let query = "demo::transform";
    let cache = RustMetadataCache::new();
    cache.insert_test_item(root.as_path(), dummy_function_metadata(query))?;
    {
        let mut inner = cache
            .inner
            .lock()
            .map_err(|_| std::io::Error::other("poisoned cache"))?;
        inner.complete_items.insert((root.clone(), query.to_string()));
        persist_item_to_disk_cache(&inner, root.as_path())?;
    }

    let cache = RustMetadataCache::new();
    let metadata = cache.get_or_extract_complete(root.as_path(), query, &|_| ())?;
    assert_eq!(metadata.canonical_path, query);
    assert!(matches!(metadata.kind, RustItemKind::Function(_)));
    let inner = cache
        .inner
        .lock()
        .map_err(|_| std::io::Error::other("poisoned cache"))?;
    assert!(
        inner.workspaces.is_empty(),
        "a complete cached function must not reopen rust-analyzer"
    );
    Ok(())
}

/// Complete extraction must reuse a persisted stable miss rather than reopening a workspace that cannot satisfy it.
#[test]
fn complete_disk_cache_reuses_stable_negative_lookup() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    let root = tmp.path().canonicalize()?;
    let query = "missing_crate::symbol";
    let cache = RustMetadataCache::new();
    {
        let mut inner = cache
            .inner
            .lock()
            .map_err(|_| std::io::Error::other("poisoned cache"))?;
        inner.failed_items.insert(
            (root.clone(), query.to_string()),
            NegativeLookup::CrateNotFound("missing_crate".to_string()),
        );
        persist_negative_to_disk_cache(&inner, root.as_path())?;
    }

    let cache = RustMetadataCache::new();
    let result = cache.get_or_extract_complete(root.as_path(), query, &|_| ());
    assert!(matches!(result, Err(RustMetadataError::CrateNotFound(crate_name)) if crate_name == "missing_crate"));
    let inner = cache
        .inner
        .lock()
        .map_err(|_| std::io::Error::other("poisoned cache"))?;
    assert!(
        inner.workspaces.is_empty(),
        "a cached stable miss must not reopen rust-analyzer"
    );
    Ok(())
}

/// A deferred complete extraction must flush its completeness evidence once for a later compiler process.
#[test]
fn deferred_complete_function_extraction_flushes_without_reopening_workspace() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    fs::create_dir_all(tmp.path().join("src"))?;
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(
        tmp.path().join("src/lib.rs"),
        "/// Preserve the value.\npub fn transform(value: u64) -> u64 { value }\n",
    )?;
    let root = tmp.path().canonicalize()?;
    let query = "probe::transform";

    let first = RustMetadataCache::new();
    let extracted = first.get_or_extract_complete_deferred_persist(root.as_path(), query, &|_| ())?;
    assert!(matches!(extracted.kind, RustItemKind::Function(_)));
    assert!(
        !disk_cache_path(root.as_path()).is_file(),
        "a deferred complete lookup must not rewrite the cache snapshot by itself"
    );
    first.persist_manifest_dir(root.as_path())?;
    drop(first);

    let second = RustMetadataCache::new();
    let reused = second.get_or_extract_complete(root.as_path(), query, &|_| ())?;
    assert_eq!(reused, extracted);
    let inner = second
        .inner
        .lock()
        .map_err(|_| std::io::Error::other("poisoned cache"))?;
    assert!(
        inner.workspaces.is_empty(),
        "the fresh compiler process must select the persisted complete function record"
    );
    Ok(())
}

/// A stable miss discovered by complete extraction must also persist across compiler processes.
#[test]
fn complete_negative_extraction_round_trips_without_reopening_workspace() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::create_dir_all(tmp.path().join("src"))?;
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(tmp.path().join("src/lib.rs"), "pub fn present() {}\n")?;
    let root = tmp.path().canonicalize()?;
    let query = "probe::missing";

    let first = RustMetadataCache::new();
    let initial = first.get_or_extract_complete(root.as_path(), query, &|_| ());
    assert!(matches!(initial, Err(RustMetadataError::PathNotResolved(path)) if path == query));
    drop(first);

    let second = RustMetadataCache::new();
    let reused = second.get_or_extract_complete(root.as_path(), query, &|_| ());
    assert!(matches!(reused, Err(RustMetadataError::PathNotResolved(path)) if path == query));
    let inner = second
        .inner
        .lock()
        .map_err(|_| std::io::Error::other("poisoned cache"))?;
    assert!(
        inner.workspaces.is_empty(),
        "the fresh compiler process must select the persisted stable miss"
    );
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
            complete_items: HashSet::new(),
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

/// Ownership metadata added by the current cache format must never be read from an older record with serde defaults,
/// because that would silently turn a supported composite borrowing contract into a cache-dependent failure.
#[test]
fn disk_cache_rejects_pre_mutable_reference_metadata_format() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    write_disk_cache(
        tmp.path(),
        &DiskCacheEnvelope {
            cache_format: 21,
            inspector_version: "cache-format-21".to_string(),
            workspace_fingerprint: workspace_fingerprint(tmp.path())?,
            items: HashMap::from([("demo::Thing".to_string(), dummy_type_metadata("demo::Thing"))]),
            complete_items: HashSet::from(["demo::Thing".to_string()]),
            misses: HashMap::new(),
        },
    )?;

    let root = tmp.path().canonicalize()?;
    let mut inner = CacheInner::default();
    let report = ensure_disk_cache_loaded(&mut inner, root.as_path())?;
    assert_eq!(report.reason, "miss.cache_format_changed");
    assert!(
        !inner.items.contains_key(&(root, "demo::Thing".to_string())),
        "a pre-v22 record must be re-extracted rather than supplying empty tuple-composition metadata"
    );
    Ok(())
}

/// `Cargo.lock` does not content-checksum a `path = "..."` dependency, so editing that dependency's own source must
/// still change the workspace fingerprint or the disk cache would keep serving pre-edit extracted metadata.
#[test]
fn workspace_fingerprint_changes_when_path_dependency_source_changes() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let dep_dir = tmp.path().join("local_dep");
    fs::create_dir_all(dep_dir.join("src"))?;
    fs::write(
        dep_dir.join("Cargo.toml"),
        "[package]\nname = \"local_dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(dep_dir.join("src/lib.rs"), "pub fn hello() -> i32 { 1 }\n")?;
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nlocal_dep = { path = \"local_dep\" }\n",
    )?;

    let before = workspace_fingerprint(tmp.path())?;
    fs::write(dep_dir.join("src/lib.rs"), "pub fn hello() -> i32 { 2 }\n")?;
    let after = workspace_fingerprint(tmp.path())?;

    assert_ne!(
        before, after,
        "editing a local path dependency's source must change the workspace fingerprint"
    );
    Ok(())
}

/// The compiler injects `incan_derive`/`incan_stdlib`/`incan_stdlib_<component>` path dependencies into every
/// generated project. Editing their source (a rare, compiler-development-only scenario) must not force every
/// generated project's fingerprint to walk the whole stdlib tree on every cache load; excluded by package-name
/// prefix regardless of the dependency's own directory size.
#[test]
fn workspace_fingerprint_ignores_compiler_owned_path_dependencies() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let dep_dir = tmp.path().join("incan_stdlib");
    fs::create_dir_all(dep_dir.join("src"))?;
    fs::write(
        dep_dir.join("Cargo.toml"),
        "[package]\nname = \"incan_stdlib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(dep_dir.join("src/lib.rs"), "pub fn hello() -> i32 { 1 }\n")?;
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nincan_stdlib = { path = \"incan_stdlib\" }\n",
    )?;

    let before = workspace_fingerprint(tmp.path())?;
    fs::write(dep_dir.join("src/lib.rs"), "pub fn hello() -> i32 { 2 }\n")?;
    let after = workspace_fingerprint(tmp.path())?;

    assert_eq!(
        before, after,
        "editing a compiler-owned incan_* path dependency must not change the workspace fingerprint"
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
            complete_items: HashSet::new(),
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
            complete_items: HashSet::new(),
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
                type_params: Vec::new(),
                type_param_defaults: Vec::new(),
                mutable_reference_type_params: Vec::new(),
                expanded_derive_traits: Vec::new(),
                has_const_params: false,
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
    assert_eq!(hit.metadata.definition_path.as_deref(), Some("bridge::udf::ScalarUDF"));
    assert!(hit.alias_used);

    let extracted = cache.get_or_extract(tmp.path(), "bridge::udf::ScalarUDF", &|_| ())?;
    assert_eq!(extracted.canonical_path, "bridge::udf::ScalarUDF");
    Ok(())
}

/// A second lookup of an item that is absent from the workspace is answered from the negative cache without loading
/// the workspace again.
#[test]
fn repeated_missing_lookup_hits_negative_cache_without_new_workspace_load() -> Result<(), Box<dyn std::error::Error>> {
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
        assert!(inner.failed_items.contains_key(&(root.clone(), query.to_string())));
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
    assert!(
        inner
            .dependency_manifest_dirs
            .contains_key(&(root.clone(), crate_name.to_string()))
    );
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
    assert!(
        inner
            .dependency_manifest_dirs
            .contains_key(&(root, "foo_bar".to_string()))
    );
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
    fs::write(dep.join("src").join("catalog.rs"), "pub trait TableProvider {}\n")?;
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
use std::fs::File;

pub struct ColumnarValue;
pub struct Defaulted<OwnerName, F = File, Wrapped = Option<OwnerName>> {
    marker: std::marker::PhantomData<(OwnerName, F, Wrapped)>,
}
pub struct Frame;
pub const DEFAULT_FRAME: Frame = Frame;
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
    /// Return a child using receiver lifetime elision.
    pub fn child(&self, key: &str) -> Option<&NestedFrame> { todo!() }
    /// The same normalized return type can instead borrow an unrelated argument.
    pub fn other<'a>(&self, other: &'a NestedFrame) -> Option<&'a NestedFrame> { todo!() }
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
    assert_eq!(sig.params[2].type_display, "source_dep::ScalarFunctionImplementation");
    assert_eq!(sig.return_type, "source_dep::types::ScalarUDF");

    let default_frame = cache.get_or_extract(&root, "source_dep::types::DEFAULT_FRAME", &|_| ())?;
    let RustItemKind::Constant { type_display } = &default_frame.kind else {
        return Err("expected source dependency DEFAULT_FRAME constant metadata".into());
    };
    assert_eq!(type_display, "source_dep::types::Frame");

    let defaulted = cache.get_or_extract(&root, "source_dep::types::Defaulted", &|_| ())?;
    let RustItemKind::Type(defaulted_info) = &defaulted.kind else {
        return Err("expected source dependency Defaulted metadata".into());
    };
    assert_eq!(defaulted_info.type_params, ["OwnerName", "F", "Wrapped"]);
    assert_eq!(
        defaulted_info.type_param_defaults,
        [
            None,
            Some("std::fs::File".to_string()),
            Some("Option<OwnerName>".to_string()),
        ],
        "syntax-only metadata must resolve imported defaults without reinterpreting owner parameters as types"
    );

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
    assert!(
        sig.is_async,
        "source metadata should preserve async functions reached through glob reexports"
    );
    assert_eq!(sig.return_type, "Result<public_api::logical_expr::Expr, String>");

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
    assert_eq!(build_output.signature.params[2].type_display, "impl FnMut(String)");
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
    assert_eq!(write_csv.signature.params[0].type_display, "source_dep::types::Frame");
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
    assert!(
        !static_ref
            .signature
            .receiver_contract
            .is_some_and(|contract| contract.returns_receiver_borrow)
    );
    let child = nested_frame_info
        .methods
        .iter()
        .find(|method| method.name == "child")
        .ok_or("expected receiver child metadata")?;
    let other = nested_frame_info
        .methods
        .iter()
        .find(|method| method.name == "other")
        .ok_or("expected unrelated lifetime metadata")?;
    assert_eq!(child.signature.return_type, other.signature.return_type);
    assert!(
        child
            .signature
            .receiver_contract
            .is_some_and(|contract| contract.shared && contract.returns_receiver_borrow)
    );
    assert!(
        other
            .signature
            .receiver_contract
            .is_some_and(|contract| contract.shared && !contract.returns_receiver_borrow)
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
mod writer {
    use super::Header;

    pub struct Builder;

    impl Builder {
        /// Append one header through a mutable reference parameter.
        pub fn append_data(&mut self, header: &mut Header) {
            let _ = header;
        }
    }
}

pub use writer::Builder;
"#,
    )?;

    let source_cache = RustMetadataCache::new();
    let source_metadata = source_cache.get_or_extract(&root, "source_dep::Builder", &|_| ())?;
    let RustItemKind::Type(source_type_info) = &source_metadata.kind else {
        return Err("expected source dependency Builder metadata".into());
    };
    assert!(
        !source_type_info.metadata_completeness.has_methods(),
        "syntax-derived method indexes must not claim an authoritative full method surface"
    );
    let source_append_data = source_type_info
        .methods
        .iter()
        .find(|method| method.name == "append_data")
        .ok_or("expected source-derived append_data method metadata")?;
    assert_eq!(
        source_append_data.signature.params[1].type_display,
        "&mut source_dep::Header"
    );
    let source_inner = source_cache
        .inner
        .lock()
        .map_err(|_| std::io::Error::other("poisoned source metadata cache"))?;
    assert!(
        source_inner.workspaces.is_empty(),
        "source-derived inherent method metadata should not load a rust-analyzer workspace"
    );
    drop(source_inner);

    let complete_cache = RustMetadataCache::new();
    let metadata = complete_cache.get_or_extract_complete(&root, "source_dep::Builder", &|_| ())?;
    let RustItemKind::Type(type_info) = &metadata.kind else {
        return Err("expected complete source dependency Builder metadata".into());
    };
    let append_data = type_info
        .methods
        .iter()
        .find(|method| method.name == "append_data")
        .ok_or("expected append_data method metadata")?;
    assert_eq!(append_data.signature.params[1].type_display, "&mut source_dep::Header");
    let repeated = complete_cache.get_or_extract_complete(&root, "source_dep::Builder", &|_| ())?;
    assert!(
        std::sync::Arc::ptr_eq(&metadata, &repeated),
        "a second complete lookup must reuse the complete in-memory type record"
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
    fs::write(dep.join("src").join("lib.rs"), "mod backend;\npub mod fs;\n")?;
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
    fs::write(dep.join("src").join("backend").join("mod.rs"), "pub mod fs;\n")?;
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
    assert!(
        !disk_cache_path(&root).is_file(),
        "fast metadata discovery must defer its cache snapshot until the owning preparation batch finishes"
    );
    cache.persist_manifest_dir(&root)?;
    let snapshot = fs::read_to_string(disk_cache_path(&root))?;
    assert!(
        snapshot.contains("rustix::fs::StatVfs"),
        "the final batch flush must retain source-derived metadata for the next process"
    );

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

/// Dependency items generated into `OUT_DIR` should resolve through the root workspace that checked those build
/// scripts.
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
    let out_dir = root
        .join("target")
        .join("debug")
        .join("build")
        .join("generated_dep-fixture")
        .join("out");
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
    assert_eq!(nested_info.fields[0].type_display, "generated_dep::generated::Nested");

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
        dependency_reexport_alias_candidate(&mut inner, dep_root.as_path(), "wrapper_crate::renamed::Thing").as_deref(),
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

/// Receiver contracts come from explicit Rust implementations; inherent and shadowed names never inherit a guess.
#[test]
fn source_prelude_methods_preserve_actual_receiver_and_name_resolution() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("probe");
    let dependency = temporary.path().join("fixture");
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(dependency.join("src"))?;
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "probe"
version = "0.1.0"
[dependencies]
fixture = { path = "../fixture" }
"#,
    )?;
    fs::write(root.join("src/lib.rs"), "")?;
    fs::write(
        dependency.join("Cargo.toml"),
        r#"[package]
name = "fixture"
version = "0.1.0"
"#,
    )?;
    fs::write(
        dependency.join("src/lib.rs"),
        r#"
pub struct Shared;
impl Clone for Shared { fn clone(&self) -> Self { Shared } }
pub struct Consumed;
impl Clone for Consumed { fn clone(&self) -> Self { Consumed } }
impl Consumed { pub fn clone(self) -> Self { self } }
#[derive(Clone)]
pub struct Derived;
pub mod shadow {
    pub trait Clone { fn clone(self) -> Self; }
    pub struct Local;
    impl Clone for Local { fn clone(self) -> Self { self } }
}
pub mod qualified_shadow {
    pub mod std { pub mod clone { pub trait Clone { fn clone(self) -> Self; } } }
    pub struct Local;
    impl std::clone::Clone for Local { fn clone(self) -> Self { self } }
}
"#,
    )?;
    let cache = RustMetadataCache::new();
    for (path, expected) in [
        ("fixture::Shared", Some(true)),
        ("fixture::Consumed", Some(false)),
        ("fixture::Derived", None),
        ("fixture::shadow::Local", None),
        ("fixture::qualified_shadow::Local", None),
    ] {
        let hit = cache
            .get_cached_or_extract_fast(&root, path)?
            .ok_or("missing fixture metadata")?;
        let RustItemKind::Type(info) = &hit.metadata.kind else {
            return Err("expected fixture type".into());
        };
        let contract = info
            .methods
            .iter()
            .find(|method| method.name == "clone")
            .and_then(|method| method.signature.receiver_contract);
        assert_eq!(contract.map(|contract| contract.shared), expected, "{path}");
    }
    let inner = cache
        .inner
        .lock()
        .map_err(|_| std::io::Error::other("poisoned cache"))?;
    assert!(
        inner.workspaces.is_empty(),
        "source proof must not load the Rust sysroot"
    );
    Ok(())
}

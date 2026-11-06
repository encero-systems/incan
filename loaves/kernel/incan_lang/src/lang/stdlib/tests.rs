use super::*;
use crate::lang::keywords::KeywordId;

fn segs(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

#[test]
/// Ordinary package admission and publication ownership share the module catalog.
fn known_stdlib_modules_cover_existing_surface_paths() {
    assert!(is_known_stdlib_module(&segs(&["std", "primitives"])));
    assert!(is_known_stdlib_module(&segs(&["std", "primitives", "async"])));
    assert!(is_known_stdlib_module(&segs(&["std", "primitives", "async", "task"])));
    assert!(is_known_stdlib_module(&segs(&["std", "primitives", "vocab"])));
    let policy = crate::lang::standard_packages::standard_package_namespace_policy("incan_stdlib_primitives");
    assert_eq!(
        policy.map(|policy| policy.namespace_roots.as_slice()),
        Some(["primitives"].as_slice())
    );
    assert_eq!(
        find_namespace("primitives").and_then(|namespace| namespace.package),
        Some("incan_stdlib_primitives")
    );
    assert!(is_known_stdlib_module(&segs(&["std", "testing"])));
    assert!(is_known_stdlib_module(&segs(&["std", "web"])));
    assert!(is_known_stdlib_module(&segs(&["std", "web", "app"])));
    assert!(is_known_stdlib_module(&segs(&["std", "web", "routing"])));
    assert!(is_known_stdlib_module(&segs(&["std", "web", "request"])));
    assert!(is_known_stdlib_module(&segs(&["std", "web", "response"])));
    assert!(is_known_stdlib_module(&segs(&["std", "async"])));
    assert!(is_known_stdlib_module(&segs(&["std", "async", "prelude"])));
    assert!(is_known_stdlib_module(&segs(&["std", "async", "time"])));
    assert!(is_known_stdlib_module(&segs(&["std", "json"])));
    assert!(is_known_stdlib_module(&segs(&["std", "serde", "json"])));
    assert!(is_known_stdlib_module(&segs(&["std", "reflection"])));
    assert!(is_known_stdlib_module(&segs(&["std", "result"])));
    assert!(is_known_stdlib_module(&segs(&["std", "environ"])));
    assert!(is_known_stdlib_module(&segs(&["std", "fs"])));
    assert!(is_known_stdlib_module(&segs(&["std", "datetime"])));
    assert!(is_known_stdlib_module(&segs(&["std", "datetime", "runtime"])));
    assert!(is_known_stdlib_module(&segs(&["std", "datetime", "civil"])));
    assert!(is_known_stdlib_module(&segs(&[
        "std",
        "datetime",
        "civil",
        "intervals"
    ])));
    assert!(is_known_stdlib_module(&segs(&["std", "datetime", "civil", "naive"])));
    assert!(is_known_stdlib_module(&segs(&["std", "datetime", "civil", "offset"])));
    assert!(is_known_stdlib_module(&segs(&["std", "graph"])));
    assert!(is_known_stdlib_module(&segs(&["std", "uuid"])));
    assert!(is_known_stdlib_module(&segs(&["std", "regex"])));
    assert!(is_known_stdlib_module(&segs(&["std", "regex", "_core"])));
    assert!(is_known_stdlib_module(&segs(&["std", "regex", "_replacement"])));
    assert!(is_known_stdlib_module(&segs(&["std", "regex", "types"])));
    assert!(is_known_stdlib_module(&segs(&["std", "io"])));
    assert!(is_known_stdlib_module(&segs(&["std", "encoding"])));
    assert!(is_known_stdlib_module(&segs(&["std", "encoding", "_shared"])));
    assert!(is_known_stdlib_module(&segs(&["std", "encoding", "hex"])));
    assert!(is_known_stdlib_module(&segs(&["std", "encoding", "base32"])));
    assert!(is_known_stdlib_module(&segs(&["std", "encoding", "base64"])));
    assert!(is_known_stdlib_module(&segs(&["std", "encoding", "base85"])));
    assert!(is_known_stdlib_module(&segs(&["std", "encoding", "base58"])));
    assert!(is_known_stdlib_module(&segs(&["std", "encoding", "bech32"])));
    assert!(is_known_stdlib_module(&segs(&["std", "checksum"])));
    assert!(is_known_stdlib_module(&segs(&["std", "hash"])));
    assert!(is_known_stdlib_module(&segs(&["std", "compression"])));
    assert!(is_known_stdlib_module(&segs(&["std", "compression", "_core"])));
    assert!(is_known_stdlib_module(&segs(&["std", "compression", "_auto"])));
    assert!(is_known_stdlib_module(&segs(&["std", "compression", "gzip"])));
    assert!(is_known_stdlib_module(&segs(&["std", "compression", "snappy", "raw"])));
    assert!(is_known_stdlib_module(&segs(&["std", "tempfile"])));
    assert!(is_known_stdlib_module(&segs(&["std", "collections"])));
    assert!(is_known_stdlib_module(&segs(&["std", "rust"])));
    assert!(is_known_stdlib_module(&segs(&["std", "builtins"])));
}

/// Async library imports do not activate language-owned keywords.
#[test]
fn async_imports_do_not_activate_language_keywords() {
    let std_async = soft_keywords_for_import(&segs(&["std", "async", "time"]));
    let primitive_async = soft_keywords_for_import(&segs(&["std", "primitives", "async", "time"]));
    let unrelated_primitive = soft_keywords_for_import(&segs(&["std", "primitives", "vocab"]));

    assert!(!std_async.contains(&KeywordId::Async));
    assert!(!std_async.contains(&KeywordId::Await));
    assert!(!primitive_async.contains(&KeywordId::Async));
    assert!(!primitive_async.contains(&KeywordId::Await));
    assert!(!unrelated_primitive.contains(&KeywordId::Async));
    assert!(!unrelated_primitive.contains(&KeywordId::Await));
}

#[test]
fn graph_constructor_types_are_registry_owned() {
    assert!(is_graph_constructor_type("DiGraph"));
    assert!(is_graph_constructor_type("Dag"));
    assert!(is_graph_constructor_type("MultiDiGraph"));
    assert!(!is_graph_constructor_type("NodeId"));
    assert!(!is_graph_constructor_type("EdgeId"));
}

/// #1561: a namespace's `prelude` path names the namespace; `std.prelude` stays a module of its own.
#[test]
fn a_namespace_prelude_path_names_the_namespace() {
    assert_eq!(
        stdlib_prelude_module_namespace(&segs(&["std", "async", "prelude"])),
        Some(segs(&["std", "async"]).as_slice())
    );
    assert_eq!(
        stdlib_prelude_module_namespace(&segs(&["std", "traits", "prelude"])),
        Some(segs(&["std", "traits"]).as_slice())
    );
    assert_eq!(stdlib_prelude_module_namespace(&segs(&["std", "prelude"])), None);
    assert_eq!(stdlib_prelude_module_namespace(&segs(&["std", "async", "time"])), None);
    assert_eq!(
        stdlib_prelude_module_namespace(&segs(&["app", "models", "prelude"])),
        None
    );
}

#[test]
/// Unknown roots remain outside the catalog even when ordinary package ownership is projected.
fn unknown_stdlib_modules_are_rejected() {
    assert!(!is_known_stdlib_module(&segs(&["std", "nope"])));
    assert!(!is_known_stdlib_module(&segs(&["std", "f64", "consts"])));
    assert!(!is_known_stdlib_module(&segs(&["std", "web", "missing"])));
    assert!(!is_known_stdlib_module(&segs(&["std", "math", "extra"])));
    assert!(!is_known_stdlib_module(&segs(&["std", "collections", "deque"])));
    assert!(!is_known_stdlib_module(&segs(&["std", "datetime", "civil", "missing"])));
}

#[test]
fn stub_paths_follow_namespace_conventions() {
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "testing"])),
        Some("stdlib/testing.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "web"])),
        Some("stdlib/web/prelude.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "web", "app"])),
        Some("stdlib/web/app.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "async", "prelude"])),
        Some("stdlib/async/prelude.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "fs"])),
        Some("stdlib/fs/prelude.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "fs", "path"])),
        Some("stdlib/fs/path.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "graph"])),
        Some("stdlib/graph.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "uuid"])),
        Some("stdlib/uuid.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "regex"])),
        Some("stdlib/regex/prelude.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "regex", "_core"])),
        Some("stdlib/regex/_core.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "datetime"])),
        Some("stdlib/datetime/prelude.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "datetime", "runtime"])),
        Some("stdlib/datetime/runtime.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "datetime", "civil", "naive"])),
        Some("stdlib/datetime/civil/naive.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "io"])),
        Some("stdlib/io.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "environ"])),
        Some("stdlib/environ.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "encoding"])),
        Some("stdlib/encoding/prelude.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "encoding", "base64"])),
        Some("stdlib/encoding/base64.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "hash"])),
        Some("stdlib/hash/prelude.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "checksum"])),
        Some("stdlib/checksum.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "compression"])),
        Some("stdlib/compression/prelude.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "compression", "_core"])),
        Some("stdlib/compression/_core.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "compression", "_auto"])),
        Some("stdlib/compression/_auto.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "compression", "gzip"])),
        Some("stdlib/compression/gzip.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "compression", "snappy", "raw"])),
        Some("stdlib/compression/snappy/raw.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "tempfile"])),
        Some("stdlib/tempfile.incn".to_string())
    );
    assert_eq!(
        stdlib_stub_path(&segs(&["std", "collections"])),
        Some("stdlib/collections.incn".to_string())
    );
}

#[test]
fn typechecker_only_namespace_std_rust_has_no_stub_path() {
    assert_eq!(stdlib_stub_path(&segs(&["std", "rust"])), None);
    assert_eq!(stdlib_stub_path(&segs(&["std", "builtins"])), None);
    assert!(is_typechecker_only_stdlib(&segs(&["std", "rust"])));
    assert!(is_typechecker_only_stdlib(&segs(&["std", "builtins"])));
    assert!(!is_typechecker_only_stdlib(&segs(&["std", "testing"])));
    assert!(!is_typechecker_only_stdlib(&segs(&["std", "async"])));
    assert!(!is_typechecker_only_stdlib(&segs(&["not", "stdlib"]))); // non-stdlib path
}

#[test]
fn known_modules_hint_is_registry_driven_and_sorted() {
    let hint = known_stdlib_modules_for_hint();
    assert!(hint.windows(2).all(|w| w[0] <= w[1]));
    assert!(hint.contains(&"std.derives".to_string()));
    assert!(hint.contains(&"std.fs".to_string()));
    assert!(hint.contains(&"std.graph".to_string()));
    assert!(hint.contains(&"std.io".to_string()));
    assert!(hint.contains(&"std.uuid".to_string()));
    assert!(hint.contains(&"std.hash".to_string()));
    assert!(hint.contains(&"std.tempfile".to_string()));
    assert!(hint.contains(&"std.rust".to_string()));
    assert!(hint.contains(&"std.web.app".to_string()));
    assert!(hint.contains(&"std.async.prelude".to_string()));
    assert!(hint.contains(&"std.datetime.civil.naive".to_string()));
    assert!(hint.contains(&"std.builtins".to_string()));
    assert!(hint.contains(&"std.collections".to_string()));
}

#[test]
fn trait_method_module_lookup_preserves_derive_fallback_surface() {
    assert_eq!(
        trait_method_module_segments(traits::as_str(TraitId::Clone)),
        Some(segs(&["std", "derives", "copying"]))
    );
    assert_eq!(
        trait_method_module_segments(derives::as_str(DeriveId::Copy)),
        Some(segs(&["std", "derives", "copying"]))
    );
    assert_eq!(
        trait_method_module_segments(traits::as_str(TraitId::Default)),
        Some(segs(&["std", "derives", "copying"]))
    );
    assert_eq!(
        trait_method_module_segments(traits::as_str(TraitId::Debug)),
        Some(segs(&["std", "derives", "string"]))
    );
    assert_eq!(
        trait_method_module_segments(traits::as_str(TraitId::Display)),
        Some(segs(&["std", "derives", "string"]))
    );
    assert_eq!(
        trait_method_module_segments(traits::as_str(TraitId::Eq)),
        Some(segs(&["std", "derives", "comparison"]))
    );
    assert_eq!(
        trait_method_module_segments(traits::as_str(TraitId::PartialEq)),
        Some(segs(&["std", "derives", "comparison"]))
    );
    assert_eq!(
        trait_method_module_segments(traits::as_str(TraitId::Ord)),
        Some(segs(&["std", "derives", "comparison"]))
    );
    assert_eq!(
        trait_method_module_segments(traits::as_str(TraitId::PartialOrd)),
        Some(segs(&["std", "derives", "comparison"]))
    );
    assert_eq!(
        trait_method_module_segments(traits::as_str(TraitId::Hash)),
        Some(segs(&["std", "derives", "comparison"]))
    );
}

#[test]
fn trait_method_module_lookup_leaves_other_builtin_traits_unmapped() {
    assert_eq!(trait_method_module_segments(traits::as_str(TraitId::From)), None);
    assert_eq!(trait_method_module_segments(traits::as_str(TraitId::Into)), None);
    assert_eq!(trait_method_module_segments("Serialize"), None);
}

#[test]
fn stdlib_json_trait_lookup_covers_source_and_qualified_names_only() {
    for name in ["Serialize", "json.Serialize", "std.serde.json.Serialize"] {
        assert_eq!(stdlib_json_trait_id(name), Some(StdlibJsonTraitId::Serialize));
    }

    for name in ["Deserialize", "json.Deserialize", "std.serde.json.Deserialize"] {
        assert_eq!(stdlib_json_trait_id(name), Some(StdlibJsonTraitId::Deserialize));
    }

    // An import alias is a binding, resolved by identity rather than by spelling (#1712).
    assert_eq!(stdlib_json_trait_id("JsonSerialize"), None);
    assert_eq!(stdlib_json_trait_id("JsonDeserialize"), None);
    let json_module = ["std", "serde", "json"].map(String::from);
    assert_eq!(
        stdlib_json_trait_id_for_identity(&json_module, "Serialize"),
        Some(StdlibJsonTraitId::Serialize)
    );
    assert_eq!(stdlib_json_trait_id_for_identity(&json_module, "JsonSerialize"), None);
    assert_eq!(stdlib_json_trait_id("yaml.Serialize"), None);
    assert_eq!(stdlib_json_trait_scope_import_id("Serialize"), None);
    assert_eq!(stdlib_json_trait_scope_import_id("JsonSerialize"), None);
    assert_eq!(
        stdlib_json_trait_scope_import_id("json.Serialize"),
        Some(StdlibJsonTraitId::Serialize)
    );
    let json_trait_module = vec!["std".to_string(), "serde".to_string(), "json".to_string()];
    assert!(is_stdlib_json_trait_module_path(&json_trait_module));
    assert_eq!(
        stdlib_json_trait_id_for_identity(&json_trait_module, "Serialize"),
        Some(StdlibJsonTraitId::Serialize)
    );
    assert_eq!(
        stdlib_json_trait_id_for_identity(&json_trait_module, "Deserialize"),
        Some(StdlibJsonTraitId::Deserialize)
    );
    assert_eq!(
        stdlib_json_trait_id_for_identity(&json_trait_module, "JsonSerialize"),
        None
    );
    assert_eq!(stdlib_json_trait_id_for_identity(&segs(&["main"]), "Serialize"), None);
    assert_eq!(
        stdlib_json_trait_id_for_identity(&segs(&["std", "toml"]), "Serialize"),
        None
    );
    let serialize_path = vec![
        "std".to_string(),
        "serde".to_string(),
        "json".to_string(),
        "Serialize".to_string(),
    ];
    assert_eq!(
        stdlib_json_trait_id_from_path(&serialize_path),
        Some(StdlibJsonTraitId::Serialize)
    );
}

#[test]
fn stdlib_registry_keeps_phase_023_metadata() {
    let async_ns = find_namespace("async");
    let reflection_ns = find_namespace("reflection");
    let fs_ns = find_namespace("fs");
    let tempfile_ns = find_namespace("tempfile");
    let traits_ns = find_namespace("traits");
    let graph_ns = find_namespace("graph");
    let uuid_ns = find_namespace("uuid");
    let hash_ns = find_namespace("hash");
    let checksum_ns = find_namespace("checksum");
    let datetime_ns = find_namespace("datetime");
    let collections_ns = find_namespace("collections");
    let compression_ns = find_namespace("compression");

    assert_eq!(async_ns.and_then(|ns| ns.facet), Some(facets::ASYNC));
    // The facet's layout decides the module path: async is its own crate root, json is a module of data,
    // strings a module of core.
    assert!(facets::namespace_is_facet_root("async"));
    assert!(facets::namespace_is_facet_root("testing"));
    assert!(!facets::namespace_is_facet_root("json"));
    assert!(!facets::namespace_is_facet_root("collections"));
    assert_eq!(facets::for_component("stdlib-web"), Some(facets::WEB));
    assert_eq!(facets::for_component("stdlib-system"), None);
    assert!(facets::path_names_a_facet("incan_std_primitives::strings::str_len"));
    assert!(!facets::path_names_a_facet("incan_stdlib_core::anything"));
    assert_eq!(reflection_ns.map(|ns| ns.submodules.is_empty()), Some(true));
    assert_eq!(fs_ns.map(|ns| ns.submodules.contains(&"path")), Some(true));
    assert_eq!(fs_ns.and_then(|ns| ns.facet), None);
    assert_eq!(tempfile_ns.map(|ns| ns.submodules.is_empty()), Some(true));
    assert_eq!(tempfile_ns.and_then(|ns| ns.facet), None);
    assert_eq!(traits_ns.map(|ns| ns.submodules.contains(&"prelude")), Some(true));
    assert_eq!(graph_ns.map(|ns| ns.facet), Some(None));
    assert_eq!(graph_ns.map(|ns| ns.submodules.is_empty()), Some(true));
    assert_eq!(uuid_ns.map(|ns| ns.facet), Some(None));
    assert_eq!(uuid_ns.map(|ns| ns.submodules.is_empty()), Some(true));
    assert_eq!(uuid_ns.map(|ns| ns.typechecker_only), Some(false));
    assert_eq!(collections_ns.and_then(|ns| ns.facet), Some(facets::DATA));
    assert_eq!(collections_ns.map(|ns| ns.submodules.is_empty()), Some(true));
    assert_eq!(collections_ns.map(|ns| ns.typechecker_only), Some(false));
    assert_eq!(hash_ns.map(|ns| ns.facet), Some(None));
    assert_eq!(hash_ns.map(|ns| ns.submodules.contains(&"prelude")), Some(true));
    assert_eq!(hash_ns.map(|ns| ns.submodules.contains(&"_core")), Some(true));
    assert_eq!(hash_ns.map(|ns| ns.submodules.contains(&"_streaming")), Some(true));
    assert_eq!(hash_ns.map(|ns| ns.typechecker_only), Some(false));
    assert_eq!(checksum_ns.map(|ns| ns.facet), Some(None));
    assert_eq!(checksum_ns.map(|ns| ns.submodules.is_empty()), Some(true));
    assert_eq!(checksum_ns.map(|ns| ns.typechecker_only), Some(false));
    assert_eq!(compression_ns.map(|ns| ns.facet), Some(None));
    assert_eq!(compression_ns.map(|ns| ns.submodules.contains(&"_core")), Some(true));
    assert_eq!(compression_ns.map(|ns| ns.submodules.contains(&"_auto")), Some(true));
    assert_eq!(compression_ns.map(|ns| ns.submodules.contains(&"gzip")), Some(true));
    assert_eq!(
        compression_ns.map(|ns| ns.submodules.contains(&"snappy.raw")),
        Some(true)
    );
    assert_eq!(datetime_ns.map(|ns| ns.facet), Some(None));
    assert_eq!(datetime_ns.map(|ns| ns.submodules.contains(&"civil.naive")), Some(true));
}

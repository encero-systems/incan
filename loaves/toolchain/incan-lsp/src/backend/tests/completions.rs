//! Completion and hover surfaces over checked metadata: public package and stdlib module completions,
//! provider-plan-driven stdlib surfaces, component-aware stdlib navigation, checked provider item metadata,
//! package-feature projections, `std.collections` and `std.environ` items, unchecked-lookup hovers and the builtin
//! `list.repeat` helper.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::sync::Arc;

use super::{
    ValueTypeFact, ValueTypeKind, builtin_list_member_completions, builtin_list_repeat_hover,
    declaration_active_for_lsp, inactive_declaration_note_at_offset, pub_library_import_item_completions,
    pub_library_module_completions, stdlib_import_item_completions, stdlib_import_item_hover, stdlib_location_for_path,
    stdlib_module_completions, unchecked_lookup_hover, value_type_kind,
};
use incan_frontend::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
};
use incan_frontend::ast::Span;
use incan_frontend::library_manifest::LibraryManifest;
use incan_frontend::library_manifest_index::{
    LibraryArtifactKind, LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
};
use incan_frontend::symbols::SymbolKind as FrontendSymbolKind;
use incan_frontend::{lexer, parser};
use incan_provider::{NamespaceAuthority, ProviderIdentity, ProviderPlan, ProviderProvenance, ProviderRecord};
use tower_lsp::lsp_types::{CompletionItemKind, Url};

fn parse_source(source: &str) -> Result<incan_frontend::ast::Program, String> {
    let tokens = lexer::lex(source).map_err(|err| format!("lexer failed: {err:?}"))?;
    parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))
}

fn value_type_facts_from_source(source: &str) -> Result<Vec<ValueTypeFact>, String> {
    let ast = parse_source(source)?;
    let mut checker = incan_frontend::typechecker::TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errors| format!("typecheck failed: {errors:?}"))?;
    let mut facts = Vec::new();
    for symbol in checker.symbols.all_symbols() {
        match &symbol.kind {
            FrontendSymbolKind::Variable(info) => facts.push(ValueTypeFact {
                name: symbol.name.clone(),
                span: symbol.span,
                kind: value_type_kind(&info.ty),
            }),
            FrontendSymbolKind::Static(info) => facts.push(ValueTypeFact {
                name: symbol.name.clone(),
                span: symbol.span,
                kind: value_type_kind(&info.ty),
            }),
            _ => {}
        }
    }
    Ok(facts)
}

fn public_module_completion_index() -> Result<LibraryManifestIndex, String> {
    let mut modules = Vec::new();
    for (source, module_path) in [
        (
            "pub const DEFAULT_SIZE: int = 4\n\
                 pub def build_index(size: int = DEFAULT_SIZE) -> int:\n  return size\n\
                 def pack_bits(size: int) -> int:\n  return size\n",
            vec!["hyperquant".to_string(), "index".to_string()],
        ),
        (
            "pub def search(index: int) -> int:\n  return index\n",
            vec!["hyperquant".to_string(), "search".to_string()],
        ),
    ] {
        let ast = parse_source(source)?;
        let mut checker = incan_frontend::typechecker::TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        checker
            .check_program(&ast)
            .map_err(|errors| format!("public completion fixture typecheck failed: {errors:?}"))?;
        modules.push(collect_checked_api_metadata(&ast, &checker, module_path));
    }
    let mut manifest = LibraryManifest::new("modulelib", "0.1.0");
    manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules,
        public_namespaces: Vec::new(),
    });
    Ok(LibraryManifestIndex::from_entries(HashMap::from([(
        "modulelib".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(
                "modulelib",
                "modulelib",
                std::path::PathBuf::from("/tmp/modulelib"),
            ),
        },
    )])))
}

#[test]
fn public_package_completions_follow_checked_module_metadata_issue948() -> Result<(), String> {
    let index = public_module_completion_index()?;
    let packages = pub_library_module_completions("from pub::", &index)
        .ok_or_else(|| "expected compiled package completions".to_string())?;
    assert!(packages.iter().any(|item| item.label == "modulelib"));

    let modules = pub_library_module_completions("from pub::modulelib.", &index)
        .ok_or_else(|| "expected public module completions".to_string())?;
    assert!(modules.iter().any(|item| item.label == "hyperquant"));

    let root_items = pub_library_import_item_completions("from pub::modulelib import ", &index)
        .ok_or_else(|| "expected public package root item completions".to_string())?;
    assert!(
        root_items
            .iter()
            .any(|item| { item.label == "hyperquant" && item.kind == Some(CompletionItemKind::MODULE) })
    );

    let nested_items = pub_library_import_item_completions("from pub::modulelib.hyperquant import ", &index)
        .ok_or_else(|| "expected public module item completions".to_string())?;
    assert!(nested_items.iter().any(|item| item.label == "build_index"));
    assert!(nested_items.iter().any(|item| item.label == "DEFAULT_SIZE"));
    assert!(nested_items.iter().any(|item| item.label == "search"));
    assert!(
        nested_items
            .iter()
            .any(|item| { item.label == "index" && item.kind == Some(CompletionItemKind::MODULE) })
    );
    assert!(!nested_items.iter().any(|item| item.label == "pack_bits"));
    Ok(())
}

#[test]
fn stdlib_module_completions_include_std_fs() -> Result<(), String> {
    let items = stdlib_module_completions("from std.", None)
        .ok_or_else(|| "expected stdlib completions for `from std.`".to_string())?;
    assert!(
        items
            .iter()
            .any(|item| item.label == "fs" && item.detail.as_deref() == Some("std.fs module")),
        "expected std.fs to be exposed through stdlib registry completions: {items:?}"
    );
    Ok(())
}

#[test]
fn stdlib_module_completions_include_std_environ() -> Result<(), String> {
    let items = stdlib_module_completions("from std.en", None)
        .ok_or_else(|| "expected stdlib completions for `from std.en`".to_string())?;
    assert!(
        items.iter().any(|item| {
            item.label == "environ"
                && item.kind == Some(CompletionItemKind::MODULE)
                && item.detail.as_deref() == Some("std.environ module")
        }),
        "expected std.environ root-module completion: {items:?}"
    );
    Ok(())
}

#[test]
fn stdlib_module_completions_follow_the_project_provider_plan() -> Result<(), String> {
    let plan = ProviderPlan::for_in_memory_sdk_modules(
        LibraryManifestIndex::default(),
        [
            vec!["future".to_string()],
            vec!["future".to_string(), "tools".to_string()],
        ],
    );
    let items = stdlib_module_completions("from std.", Some(&plan))
        .ok_or_else(|| "expected provider-aware stdlib completions".to_string())?;
    let future = items
        .iter()
        .find(|item| item.label == "future")
        .ok_or_else(|| format!("expected active std.future completion: {items:?}"))?;
    assert!(
        future
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("enabled by SDK component `in-memory-source`")),
        "expected active provider provenance in completion detail: {future:?}"
    );
    assert!(
        !items.iter().any(|item| item.label == "web"),
        "provider-aware completion must not advertise an unknown SDK module: {items:?}"
    );
    assert!(
        items.iter().any(|item| item.label == "rust"),
        "compiler-owned symbolic modules must remain discoverable: {items:?}"
    );
    let nested = stdlib_module_completions("from std.future.", Some(&plan))
        .ok_or_else(|| "expected provider-aware submodule completions".to_string())?;
    assert!(
        nested.iter().any(|item| item.label == "tools"),
        "provider submodules must not require compiler registry entries: {nested:?}"
    );
    Ok(())
}

#[test]
fn component_aware_stdlib_navigation_targets_the_relocatable_provider_manifest()
-> Result<(), Box<dyn std::error::Error>> {
    let artifact = tempfile::tempdir()?;
    let manifest_path = artifact.path().join("stdlib-future.incnlib");
    fs::write(&manifest_path, "{}")?;
    let manifest = Arc::new(LibraryManifest::new("stdlib-future", "0.5.0"));
    let provider = ProviderRecord {
        identity: ProviderIdentity {
            name: "stdlib-future".to_string(),
            version: "0.5.0".to_string(),
            digest: format!("sha256:{}", "0".repeat(64)),
            feature_projection: BTreeSet::new(),
        },
        provenance: ProviderProvenance::Sdk {
            sdk_identity: "incan@0.5.0".to_string(),
            component_id: "stdlib-future".to_string(),
            inventory_path: Some(artifact.path().join("sdk-inventory.json")),
        },
        authority: NamespaceAuthority::SdkReserved,
        namespace_claims: BTreeSet::from([vec!["std".to_string(), "future".to_string()]]),
        available: true,
        enabled: true,
        manifest: Some(manifest),
        artifact: Some(LibraryArtifactMetadata {
            dependency_key: "stdlib-future".to_string(),
            manifest_name: "stdlib-future".to_string(),
            manifest_path: manifest_path.clone(),
            crate_root: artifact.path().to_path_buf(),
            cargo_toml_path: artifact.path().join("Cargo.toml"),
            crate_lib_path: artifact.path().join("src/lib.rs"),
            kind: LibraryArtifactKind::Materialized,
        }),
        implementation_facets: Vec::new(),
    };
    let plan = ProviderPlan::new(LibraryManifestIndex::default(), vec![provider], [])?;

    let location = stdlib_location_for_path(&["std".to_string(), "future".to_string()], Some(&plan))
        .ok_or("expected provider-backed stdlib location")?;

    assert_eq!(
        location.uri,
        Url::from_file_path(&manifest_path).map_err(|_| "invalid manifest path")?
    );
    assert_eq!(location.range.start.line, 0);
    Ok(())
}

#[test]
fn component_aware_stdlib_navigation_never_falls_back_to_the_producer_checkout() {
    let plan = ProviderPlan::for_in_memory_sdk_modules(LibraryManifestIndex::default(), [vec!["future".to_string()]]);

    assert!(
        stdlib_location_for_path(&["std".to_string(), "future".to_string()], Some(&plan)).is_none(),
        "a component-aware provider without a materialized artifact must not expose CARGO_MANIFEST_DIR"
    );
}

#[test]
fn stdlib_item_completions_use_checked_provider_metadata_without_source_lookup() -> Result<(), String> {
    let provider_ast = parse_source(
        "pub def greet(name: str) -> str:\n    \"\"\"Return a provider-owned greeting.\"\"\"\n    return name\n",
    )?;
    let mut checker = incan_frontend::typechecker::TypeChecker::new();
    checker
        .check_program(&provider_ast)
        .map_err(|errors| format!("typecheck failed: {errors:?}"))?;
    let mut manifest = LibraryManifest::new("incan-stdlib-future", "0.5.0");
    manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![incan_frontend::api_metadata::collect_checked_api_metadata(
            &provider_ast,
            &checker,
            vec!["future".to_string()],
        )],
        public_namespaces: Vec::new(),
    });
    let plan = ProviderPlan::for_in_memory_sdk_manifest(LibraryManifestIndex::default(), manifest);

    let items = stdlib_import_item_completions("from std.future import gre", Some(&plan))
        .ok_or_else(|| "expected provider-backed item completions".to_string())?;
    let greet = items
        .iter()
        .find(|item| item.label == "greet")
        .ok_or_else(|| format!("missing provider-backed `greet` completion: {items:?}"))?;
    assert_eq!(greet.kind, Some(CompletionItemKind::FUNCTION));
    assert!(
        greet
            .detail
            .as_deref()
            .is_some_and(|detail| detail.starts_with("def greet(name: str) -> str — enabled by SDK component")),
        "provider-backed signature or provenance was lost: {greet:?}"
    );
    let docs = greet
        .documentation
        .as_ref()
        .map(|documentation| format!("{documentation:?}"))
        .unwrap_or_default();
    assert!(
        docs.contains("Return a provider-owned greeting."),
        "provider docstring was not preserved: {docs}"
    );

    let consumer_source = "from std.future import greet\n";
    let consumer_ast = parse_source(consumer_source)?;
    let start = consumer_source
        .find("greet")
        .ok_or_else(|| "expected greet in consumer fixture".to_string())?;
    let (markdown, span) = stdlib_import_item_hover(&consumer_ast, consumer_source, start, Some(&plan))
        .ok_or_else(|| "expected provider-backed greet hover".to_string())?;
    assert_eq!(span, Span::new(start, start + "greet".len()));
    assert!(
        markdown.contains("Return a provider-owned greeting."),
        "provider hover lost checked docs: {markdown}"
    );
    assert!(
        !markdown.contains("loaves/stdlib") && !markdown.contains(env!("CARGO_MANIFEST_DIR")),
        "component-aware hover must not expose a producer-checkout path: {markdown}"
    );
    Ok(())
}

#[test]
fn declaration_completion_projection_follows_active_package_features() -> Result<(), String> {
    let ast = parse_source("when feature(\"json\"):\n    const JSON_ENABLED = true\n\nconst ALWAYS_ENABLED = true\n")?;
    let json_decl = ast
        .declarations
        .iter()
        .find(|decl| !decl.required_features.is_empty())
        .ok_or_else(|| "expected a feature-conditioned declaration".to_string())?;
    assert!(!declaration_active_for_lsp(
        json_decl,
        &std::collections::BTreeSet::new()
    ));
    assert!(declaration_active_for_lsp(
        json_decl,
        &std::collections::BTreeSet::from(["json".to_string()])
    ));
    assert_eq!(
        inactive_declaration_note_at_offset(&ast, json_decl.span.start, &std::collections::BTreeSet::new()).as_deref(),
        Some("inactive; requires `json`.")
    );
    Ok(())
}

#[test]
fn stdlib_collections_import_completions_include_ordinal_map_surface() -> Result<(), String> {
    let items = stdlib_import_item_completions("from std.collections import Ord", None)
        .ok_or_else(|| "expected std.collections item completions".to_string())?;
    assert!(
        items.iter().any(|item| {
            item.label == "OrdinalMap"
                && item.kind == Some(CompletionItemKind::CLASS)
                && item.detail.as_deref() == Some("OrdinalMap: stdlib class")
        }),
        "expected OrdinalMap public item completion: {items:?}"
    );
    assert!(
        items
            .iter()
            .any(|item| item.label == "OrdinalKey" && item.kind == Some(CompletionItemKind::INTERFACE)),
        "expected OrdinalKey public item completion: {items:?}"
    );
    Ok(())
}

#[test]
fn stdlib_collections_item_hover_documents_ordinal_map() -> Result<(), String> {
    let source = "from std.collections import OrdinalMap\n";
    let ast = parse_source(source)?;
    let start = source
        .find("OrdinalMap")
        .ok_or_else(|| "expected OrdinalMap in fixture".to_string())?;
    let (markdown, span) = stdlib_import_item_hover(&ast, source, start, None)
        .ok_or_else(|| "expected OrdinalMap import item hover".to_string())?;
    assert_eq!(span, Span::new(start, start + "OrdinalMap".len()));
    assert!(
        markdown.contains("from std.collections import OrdinalMap"),
        "expected import signature in hover markdown: {markdown}"
    );
    assert!(
        markdown.contains("lookup verifies canonical key bytes"),
        "expected exact lookup detail in hover markdown: {markdown}"
    );
    Ok(())
}

#[test]
fn stdlib_environ_completions_cover_the_public_module_surface() -> Result<(), String> {
    let items = stdlib_import_item_completions("from std.environ import ", None)
        .ok_or_else(|| "expected std.environ item completions".to_string())?;
    assert_eq!(items.len(), 7, "expected one completion per public item: {items:?}");
    let completions = items
        .iter()
        .map(|item| (item.label.as_str(), item.kind))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(
        completions.keys().copied().collect::<Vec<_>>(),
        vec![
            "EnvironError",
            "EnvironErrorKind",
            "args",
            "get",
            "get_as",
            "get_optional",
            "get_or"
        ]
    );
    assert_eq!(completions.get("EnvironError"), Some(&Some(CompletionItemKind::CLASS)));
    assert_eq!(
        completions.get("EnvironErrorKind"),
        Some(&Some(CompletionItemKind::ENUM))
    );
    for function in ["args", "get", "get_as", "get_optional", "get_or"] {
        assert_eq!(
            completions.get(function),
            Some(&Some(CompletionItemKind::FUNCTION)),
            "expected {function} function completion: {items:?}"
        );
    }
    let get_as = items
        .iter()
        .find(|item| item.label == "get_as")
        .ok_or_else(|| "missing get_as completion".to_string())?;
    assert_eq!(get_as.detail.as_deref(), Some("get_as: 2 stdlib function overloads"));
    Ok(())
}

#[test]
fn stdlib_environ_get_as_hover_shows_both_signatures() -> Result<(), String> {
    let source = "from std.environ import get_as\n";
    let ast = parse_source(source)?;
    let start = source
        .find("get_as")
        .ok_or_else(|| "expected get_as in fixture".to_string())?;
    let (markdown, span) = stdlib_import_item_hover(&ast, source, start, None)
        .ok_or_else(|| "expected get_as import hover".to_string())?;
    assert_eq!(span, Span::new(start, start + "get_as".len()));
    assert!(
        markdown.contains("def get_as[T with TryFrom[str]](key: str) -> Result[Option[T], EnvironError]"),
        "expected optional get_as signature: {markdown}"
    );
    assert!(
        markdown.contains("def get_as[T with TryFrom[str]](key: str, default: T) -> Result[T, EnvironError]"),
        "expected defaulted get_as signature: {markdown}"
    );
    assert!(
        markdown.contains("conversion failures"),
        "expected source docstring in get_as hover: {markdown}"
    );
    assert!(
        markdown.contains("never fall back to `default`"),
        "expected second overload docstring in get_as hover: {markdown}"
    );
    Ok(())
}

#[test]
fn stdlib_environ_error_hovers_include_type_kind_and_docs() -> Result<(), String> {
    for (name, kind, docs) in [
        ("EnvironError", "stdlib class", "observed environment value"),
        (
            "EnvironErrorKind",
            "stdlib enum",
            "Stable categories for environment read failures",
        ),
    ] {
        let source = format!("from std.environ import {name}\n");
        let ast = parse_source(&source)?;
        let start = source.find(name).ok_or_else(|| format!("expected {name} in fixture"))?;
        let (markdown, span) = stdlib_import_item_hover(&ast, &source, start, None)
            .ok_or_else(|| format!("expected {name} import hover"))?;
        assert_eq!(span, Span::new(start, start + name.len()));
        assert!(
            markdown.contains(&format!("from std.environ import {name}")),
            "expected import signature in {name} hover: {markdown}"
        );
        assert!(
            markdown.contains(&format!("*{kind}*")),
            "expected {kind} label in {name} hover: {markdown}"
        );
        assert!(
            markdown.contains(docs),
            "expected source docs in {name} hover: {markdown}"
        );
    }
    Ok(())
}

#[test]
fn unchecked_lookup_hover_warns_about_missing_key_semantics() -> Result<(), String> {
    let source = "columns.get_unchecked(\"missing\")";
    let start = source
        .find("get_unchecked")
        .ok_or_else(|| "expected get_unchecked in fixture".to_string())?;
    let value_types = vec![ValueTypeFact {
        name: "columns".to_string(),
        span: Span::new(0, "columns".len()),
        kind: ValueTypeKind::OrdinalMap,
    }];
    let markdown = unchecked_lookup_hover(
        source,
        &value_types,
        "get_unchecked",
        Span::new(start, start + "get_unchecked".len()),
    )
    .ok_or_else(|| "expected unchecked hover".to_string())?;
    assert!(
        markdown.contains("get_unchecked(...)"),
        "expected unchecked signature in hover markdown: {markdown}"
    );
    assert!(
        markdown.contains("Missing-key behavior is implementation-specific."),
        "expected missing-key warning in hover markdown: {markdown}"
    );
    Ok(())
}

#[test]
fn unchecked_lookup_hover_ignores_non_ordinal_map_receivers() -> Result<(), String> {
    let source = "cache.get_unchecked(\"missing\")";
    let start = source
        .find("get_unchecked")
        .ok_or_else(|| "expected get_unchecked in fixture".to_string())?;
    let value_types = vec![ValueTypeFact {
        name: "cache".to_string(),
        span: Span::new(0, "cache".len()),
        kind: ValueTypeKind::Other,
    }];
    let markdown = unchecked_lookup_hover(
        source,
        &value_types,
        "get_unchecked",
        Span::new(start, start + "get_unchecked".len()),
    );
    assert!(
        markdown.is_none(),
        "unchecked lookup hover should be scoped to OrdinalMap receivers"
    );
    Ok(())
}

#[test]
fn unchecked_lookup_hover_uses_nearest_preceding_receiver_type() -> Result<(), String> {
    let source = "cache = ordinal\ncache = plain\ncache.get_unchecked(\"missing\")";
    let start = source
        .find("get_unchecked")
        .ok_or_else(|| "expected get_unchecked in fixture".to_string())?;
    let value_types = vec![
        ValueTypeFact {
            name: "cache".to_string(),
            span: Span::new(0, "cache".len()),
            kind: ValueTypeKind::OrdinalMap,
        },
        ValueTypeFact {
            name: "cache".to_string(),
            span: Span::new(16, 21),
            kind: ValueTypeKind::Other,
        },
    ];
    let markdown = unchecked_lookup_hover(
        source,
        &value_types,
        "get_unchecked",
        Span::new(start, start + "get_unchecked".len()),
    );
    assert!(
        markdown.is_none(),
        "nearest same-name binding should drive unchecked lookup hover"
    );
    Ok(())
}

#[test]
fn unchecked_lookup_hover_ignores_member_chain_receivers() -> Result<(), String> {
    let source = "columns = ordinal\nother.columns.get_unchecked(\"missing\")";
    let start = source
        .find("get_unchecked")
        .ok_or_else(|| "expected get_unchecked in fixture".to_string())?;
    let value_types = vec![ValueTypeFact {
        name: "columns".to_string(),
        span: Span::new(0, "columns".len()),
        kind: ValueTypeKind::OrdinalMap,
    }];
    let markdown = unchecked_lookup_hover(
        source,
        &value_types,
        "get_unchecked",
        Span::new(start, start + "get_unchecked".len()),
    );
    assert!(
        markdown.is_none(),
        "member chains should wait for real receiver typing instead of matching the final identifier"
    );
    Ok(())
}

#[test]
fn unchecked_lookup_hover_uses_typechecked_document_facts() -> Result<(), String> {
    let source = "from std.collections import OrdinalMap, OrdinalMapError\n\ndef run() -> Result[None, OrdinalMapError]:\n    columns: OrdinalMap[str] = OrdinalMap.from_keys([\"id\", \"status\"])?\n    columns.get_unchecked(\"id\")\n    columns.get_many_unchecked([\"status\"])\n    return Ok(None)\n";
    let value_types = value_type_facts_from_source(source)?;
    for method in ["get_unchecked", "get_many_unchecked"] {
        let start = source
            .find(method)
            .ok_or_else(|| format!("expected {method} in fixture"))?;
        let markdown = unchecked_lookup_hover(source, &value_types, method, Span::new(start, start + method.len()))
            .ok_or_else(|| format!("expected unchecked hover for {method}"))?;
        assert!(
            markdown.contains("*unchecked lookup*"),
            "expected unchecked lookup warning in hover markdown: {markdown}"
        );
    }
    Ok(())
}

#[test]
fn builtin_list_member_completions_include_repeat() -> Result<(), String> {
    let items = builtin_list_member_completions("let xs = list.re")
        .ok_or_else(|| "expected built-in list member completions".to_string())?;
    assert!(
        items.iter().any(|item| {
            item.label == "repeat" && item.detail.as_deref() == Some("list.repeat[T](value: T, count: int) -> list[T]")
        }),
        "expected list.repeat completion: {items:?}"
    );
    Ok(())
}

#[test]
fn builtin_list_repeat_hover_documents_helper() -> Result<(), String> {
    let source = "let xs = list.repeat(0, 3)";
    let start = source
        .find("repeat")
        .ok_or_else(|| "expected repeat in fixture".to_string())?;
    let markdown = builtin_list_repeat_hover(source, "repeat", Span::new(start, start + "repeat".len()))
        .ok_or_else(|| "expected list.repeat hover".to_string())?;
    assert!(
        markdown.contains("list.repeat[T](value: T, count: int) -> list[T]"),
        "expected signature in hover markdown: {markdown}"
    );
    assert!(
        markdown.contains("Negative counts raise `ValueError`."),
        "expected negative-count detail in hover markdown: {markdown}"
    );
    Ok(())
}

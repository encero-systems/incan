//! A Loaf's public namespaces, read by name from the installed set (RFC 179).

use super::typechecker_reads::catalog;
use super::{installed_library, read_declaration_in, read_summary};

/// After inventory admission, installed and path Loaves supply the same namespace record without the full manifest.
#[test]
fn a_consumer_reads_one_public_namespace_record_alone() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let mut manifest = catalog()?;
    let set = installed_library(root.path(), &manifest)?;
    let dir = root.path().join("debug");
    read_summary(&set, &dir, "prices")?.ok_or("the Loaf is installed")?;
    let admitted = set.reads();
    let (namespaces, _) = read_declaration_in(&set, &dir, "prices", "public-namespace", "money")?
        .ok_or("the namespace record is installed")?;
    assert_eq!(namespaces.len(), 1, "one namespace at the path `money`");
    assert_eq!(namespaces[0]["path"], serde_json::json!(["money"]));
    let records = set.reads().records - admitted.records;
    assert_eq!(
        records, 1,
        "the consumer read {records} records; it needs the namespace's record alone"
    );

    let directory = root.path().join("path-dependency");
    std::fs::create_dir(&directory)?;
    let private = tempfile::tempdir()?;
    let _home = super::super::super::manifest_input::select_test_home(private.path());
    let _records = super::thread_selection::select(super::RecordSet::new(
        root.path().join("unrelated-installation"),
        Vec::new(),
    ));
    let declared = crate::typechecker::TypeChecker::new().loaf_install_record("prices", &manifest);
    let install = manifest.prepare_install_record(&declared, false)?;
    let content = manifest.to_json_string()?;
    std::fs::write(directory.join("prices.incnlib"), &content)?;
    std::fs::write(
        directory.join("prices.incnfacts"),
        manifest.encode_loaf_facts(&content, Some(install))?,
    )?;
    let open = || {
        super::super::super::lazy_entry::IndexedLoaf::open(
            "prices".into(),
            crate::loaf_index::LoafNamespace::Dependency { key: "prices".into() },
            directory.clone(),
            "prices",
        )
    };
    let cold = open()?;
    assert!(cold.was_demanded(), "the first admission establishes the inventory");
    let warm = open()?;
    let selected = warm.public_namespace(&["money".into()])?;
    assert_eq!(selected.as_ref(), namespaces.first());
    assert!(
        !warm.was_demanded(),
        "namespace selection reread the whole path manifest"
    );
    check_path_rust_reexport_without_manifest()?;
    Ok(())
}

/// A path dependency's namespaced and root Rust re-exports use its named facts and selected ABI.
fn check_path_rust_reexport_without_manifest() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::Arc;

    use crate::library_manifest::{LIBRARY_MANIFEST_FORMAT, LibraryManifest, OWNERSHIP_MANIFEST_FORMAT};
    use crate::loaf_index::LoafIndex;
    use crate::typechecker::TypeChecker;

    let root = tempfile::tempdir()?;
    let private = tempfile::tempdir()?;
    let _home = super::super::super::manifest_input::select_test_home(private.path());
    let _records = super::thread_selection::select(super::RecordSet::new(
        root.path().join("unrelated-installation"),
        Vec::new(),
    ));
    let (mut manifest, root_checker, abi_cache) = rust_reexport_provider(None, false)?;
    let facts_path = root.path().join("probe.incnfacts");
    let publish = |manifest: &mut LibraryManifest| -> Result<(), Box<dyn std::error::Error>> {
        let install = root_checker.loaf_install_record("probe", manifest);
        let publication = manifest.prepare_install_record(&install, true)?;
        let content = manifest.to_json_string()?;
        std::fs::write(root.path().join("probe.incnlib"), &content)?;
        std::fs::write(root.path().join("probe.rustabi"), &abi_cache)?;
        std::fs::write(&facts_path, manifest.encode_loaf_facts(&content, Some(publication))?)?;
        Ok(())
    };
    publish(&mut manifest)?;
    let open = || LoafIndex::deferred([("probe".into(), "probe".into(), root.path().to_owned(), false)]);
    let check = |loaves: &Arc<LoafIndex>, source: &str| {
        let mut checker = TypeChecker::new();
        checker.set_loaves(Arc::clone(loaves));
        let program = parse_reexport_program(&format!(
            "from {source} import answer\n\ndef main() -> int:\n    return answer()\n"
        ))?;
        checker.check_program(&program).map_err(|errors| format!("{errors:?}"))
    };
    assert_eq!(open()?.demanded_fixture_manifests(), ["probe"]);
    for source in ["pub::probe.keys", "pub::probe"] {
        let loaves = Arc::new(open()?);
        check(&loaves, source)?;
        assert!(
            loaves.demanded_fixture_manifests().is_empty(),
            "Rust alias from {source} demanded the whole path manifest"
        );
    }

    let mut damaged: serde_json::Value = serde_json::from_slice(&std::fs::read(&facts_path)?)?;
    damaged["manifest_digest"] = serde_json::Value::String("0".repeat(64));
    std::fs::write(&facts_path, serde_json::to_vec(&damaged)?)?;
    let loaves = Arc::new(open()?);
    let Err(error) = check(&loaves, "pub::probe") else {
        return Err("the checker accepted damaged install facts".into());
    };
    assert!(
        error.contains("sidecar names a different manifest or schema"),
        "{error}"
    );

    manifest.manifest_format = OWNERSHIP_MANIFEST_FORMAT;
    publish(&mut manifest)?;
    assert_eq!(open()?.demanded_fixture_manifests(), ["probe"]);
    let loaves = Arc::new(open()?);
    check(&loaves, "pub::probe")?;
    assert_eq!(loaves.demanded_fixture_manifests(), ["probe"]);

    manifest.manifest_format = LIBRARY_MANIFEST_FORMAT;
    let api = manifest.contract_metadata.api.as_mut().ok_or("missing test API")?;
    api.modules[1].module_path = vec!["answer".into()];
    api.public_namespaces.clear();
    manifest
        .contract_metadata
        .identity_graph
        .exports
        .retain(|identity| identity.public_path.len() == 2);
    publish(&mut manifest)?;
    assert_eq!(open()?.demanded_fixture_manifests(), ["probe"]);
    let loaves = Arc::new(open()?);
    let Err(error) = check(&loaves, "pub::probe") else {
        return Err("the checker bound a root Rust re-export with a same-named child namespace".into());
    };
    assert!(
        error.contains("`answer` is ambiguous in public module `pub::probe`"),
        "{error}"
    );
    Ok(())
}

/// A package-root Rust import can install a different root alias's checked type projection without the facade manifest.
#[test]
fn a_root_rust_import_reads_a_projected_nominal_alias_without_the_facade_manifest()
-> Result<(), Box<dyn std::error::Error>> {
    use std::sync::Arc;

    use crate::library_manifest::TypeRef;
    use crate::loaf_index::LoafIndex;
    use crate::typechecker::TypeChecker;

    let root = tempfile::tempdir()?;
    let private = tempfile::tempdir()?;
    let _home = super::super::super::manifest_input::select_test_home(private.path());
    let _records = super::thread_selection::select(super::RecordSet::new(
        root.path().join("unrelated-installation"),
        Vec::new(),
    ));
    let owner = super::typechecker_reads::catalog_with("\npub type MappedView = newtype int\n")?;
    let owner_dir = root.path().join("prices");
    std::fs::create_dir(&owner_dir)?;
    std::fs::write(owner_dir.join("prices.incnlib"), owner.to_json_string()?)?;
    let owner = LoafIndex::dependency("prices", "prices", &owner_dir)?;
    let provider_identity = owner.artifact.identity.clone();
    let (mut facade, producer, abi) = rust_reexport_provider(Some(owner), false)?;
    let record = producer.loaf_install_record("probe", &facade);
    let alias = record
        .aliases
        .iter()
        .find(|alias| alias.name == "MappedView")
        .ok_or("missing checked facade alias")?;
    let Some(TypeRef::Named {
        origin: Some(origin), ..
    }) = &alias.projected_type
    else {
        return Err("missing checked nominal alias projection".into());
    };
    assert_eq!(origin.provider, provider_identity);
    assert_eq!(origin.canonical.declaration_name, "MappedView");
    assert!(record.canonical_types.iter().any(|row| row.public_name == "MappedView"));
    let publication = facade.prepare_install_record(&record, true)?;
    let content = facade.to_json_string()?;
    let facade_dir = root.path().join("probe");
    std::fs::create_dir(&facade_dir)?;
    std::fs::write(facade_dir.join("probe.incnlib"), &content)?;
    std::fs::write(facade_dir.join("probe.rustabi"), abi)?;
    std::fs::write(
        facade_dir.join("probe.incnfacts"),
        facade.encode_loaf_facts(&content, Some(publication))?,
    )?;

    let open = || {
        LoafIndex::deferred([
            ("prices".into(), "prices".into(), owner_dir.clone(), false),
            ("probe".into(), "probe".into(), facade_dir.clone(), false),
        ])
    };
    assert!(open()?.demanded_fixture_manifests().contains(&"probe"));
    let loaves = Arc::new(open()?);
    let mut consumer = TypeChecker::new();
    consumer.set_loaves(Arc::clone(&loaves));
    let program = parse_reexport_program("from pub::probe import answer\n\ndef main() -> int:\n    return answer()\n")?;
    consumer
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let demands = loaves.demanded_fixture_manifests();
    let name = crate::typechecker::canonical_public_library_type_name("probe", "MappedView");
    let snapshot = consumer.dependency_interface_snapshot();
    let nominal = snapshot
        .tables
        .iter()
        .find(|table| table.name == "transitive_public_types")
        .ok_or("missing transitive type layouts")?;
    assert!(
        nominal.entries.iter().any(|row| {
            row.windows(name.len()).any(|bytes| bytes == name.as_bytes())
                && row.windows(b"Newtype".len()).any(|bytes| bytes == b"Newtype")
        }),
        "the facade's MappedView newtype layout was not installed"
    );
    assert!(
        !demands.contains(&"probe"),
        "projected nominal alias demanded the whole facade manifest: {demands:?}"
    );
    Ok(())
}

/// A checked body may pass a named path-Loaf Rust text constant without opening the provider's full manifest.
#[test]
fn a_named_rust_text_constant_keeps_the_path_facade_deferred() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::Arc;

    use crate::loaf_index::LoafIndex;
    use crate::symbols::SymbolKind;
    use crate::typechecker::TypeChecker;
    use incan_lang::interop::RustItemKind;

    let root = tempfile::tempdir()?;
    let private = tempfile::tempdir()?;
    let _home = super::super::super::manifest_input::select_test_home(private.path());
    let _records = super::thread_selection::select(super::RecordSet::new(
        root.path().join("unrelated-installation"),
        Vec::new(),
    ));
    let (mut manifest, producer, abi) = rust_reexport_provider(None, true)?;
    let record = producer.loaf_install_record("probe", &manifest);
    let publication = manifest.prepare_install_record(&record, true)?;
    let content = manifest.to_json_string()?;
    std::fs::write(root.path().join("probe.incnlib"), &content)?;
    std::fs::write(root.path().join("probe.rustabi"), abi)?;
    std::fs::write(
        root.path().join("probe.incnfacts"),
        manifest.encode_loaf_facts(&content, Some(publication))?,
    )?;
    let open = || LoafIndex::deferred([("probe".into(), "probe".into(), root.path().to_owned(), false)]);
    assert_eq!(open()?.demanded_fixture_manifests(), ["probe"]);
    let loaves = Arc::new(open()?);
    let mut consumer = TypeChecker::new();
    consumer.set_loaves(Arc::clone(&loaves));
    let program = parse_reexport_program(
        "from pub::probe import answer\nfrom pub::probe.checker_records import CHECKER_PROFILE\n\n\
         def consume(profile: str) -> str:\n    return profile\n\n\
         def main() -> str:\n    if answer() == 0:\n        return consume(CHECKER_PROFILE)\n    return \"other\"\n",
    )?;
    consumer
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let symbol = consumer
        .lookup_symbol("CHECKER_PROFILE")
        .ok_or("missing checked Rust constant")?;
    let SymbolKind::RustItem(item) = &symbol.kind else {
        return Err("the path Loaf lost the selected Rust constant identity".into());
    };
    let metadata = item.metadata.as_ref().ok_or("missing checked Rust constant metadata")?;
    assert_eq!(metadata.canonical_path, "native::CHECKER_PROFILE");
    assert!(matches!(&metadata.kind, RustItemKind::Constant { type_display } if type_display == "&str"));
    assert!(
        loaves.demanded_fixture_manifests().is_empty(),
        "the checked Rust constant use demanded the whole path manifest"
    );
    Ok(())
}

/// Refresh a source bridge's public alias from the exact admitted root Rust type.
#[test]
fn a_source_bridge_refreshes_a_root_rust_type_without_the_whole_manifest() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::Arc;

    use crate::ast::ImportPath;
    use crate::loaf_index::LoafIndex;
    use crate::symbols::{RustImportBindingKind, SymbolKind};
    use crate::typechecker::TypeChecker;
    use incan_lang::interop::RustItemKind;
    use incan_semantics_core::body_ir::rust_item_identity;

    let root = tempfile::tempdir()?;
    let private = tempfile::tempdir()?;
    let _home = super::super::super::manifest_input::select_test_home(private.path());
    let _records = super::thread_selection::select(super::RecordSet::new(
        root.path().join("unrelated-installation"),
        Vec::new(),
    ));
    let (mut manifest, producer, abi) = rust_reexport_provider(None, true)?;
    let record = producer.loaf_install_record("probe", &manifest);
    let publication = manifest.prepare_install_record(&record, true)?;
    let content = manifest.to_json_string()?;
    std::fs::write(root.path().join("probe.incnlib"), &content)?;
    std::fs::write(root.path().join("probe.rustabi"), abi)?;
    std::fs::write(
        root.path().join("probe.incnfacts"),
        manifest.encode_loaf_facts(&content, Some(publication))?,
    )?;

    let open = || LoafIndex::deferred([("selected".into(), "probe".into(), root.path().to_owned(), false)]);
    let bridge = parse_reexport_program("pub from pub::selected import NativeCarrier as Output\n")?;
    let bridge_path = ImportPath::simple(vec!["bridge".to_owned()]);
    let consumer =
        parse_reexport_program("from bridge import Output\n\ndef keep(value: Output) -> Output:\n    return value\n")?;
    let check = |loaves: &Arc<LoafIndex>| -> Result<_, Box<dyn std::error::Error>> {
        let mut checker = TypeChecker::new();
        checker.set_loaves(Arc::clone(loaves));
        checker.import_module(&bridge, "bridge");
        let demands = loaves.demanded_fixture_manifests();
        checker
            .check_program(&consumer)
            .map_err(|errors| format!("{errors:?}"))?;
        let Some(SymbolKind::RustItem(item)) = checker.dependency_member_symbol_for_path(&bridge_path, "Output") else {
            return Err("the source bridge did not retain its checked Rust type".into());
        };
        let canonical = checker
            .type_info()
            .resolved_import_identity("Output")
            .ok_or("the consumer lost the checked public alias identity")?;
        Ok((
            item,
            canonical.clone(),
            demands.into_iter().map(str::to_owned).collect::<Vec<_>>(),
        ))
    };
    let cold = Arc::new(open()?);
    assert_eq!(cold.demanded_fixture_manifests(), ["selected"]);
    let (expected, expected_canonical, _) = check(&cold)?;
    let direct = open()?;
    let (target, recorded_canonical) = direct
        .root_rust_reexport_target("selected", "NativeCarrier")?
        .ok_or("the selected records omitted the root Rust re-export")?;
    assert_eq!(
        target,
        ["rust".to_owned(), "native".to_owned(), "NativeCarrier".to_owned()]
    );
    assert_eq!(recorded_canonical, expected_canonical);
    assert!(direct.root_rust_reexport_target("selected", "Missing")?.is_none());
    assert!(direct.demanded_fixture_manifests().is_empty());
    let warm = Arc::new(open()?);
    let (actual, canonical, demands) = check(&warm)?;
    assert_eq!(actual, expected);
    assert_eq!(canonical, expected_canonical);
    assert_eq!(actual.path, "native::NativeCarrier");
    assert_eq!(actual.binding, RustImportBindingKind::FromImport);
    assert_eq!(
        canonical,
        rust_item_identity("native::NativeCarrier").ok_or("missing canonical Rust item")?
    );
    let metadata = actual.metadata.ok_or("missing selected ABI metadata")?;
    assert!(
        matches!(&metadata.kind, RustItemKind::Type(info) if info.fields.iter().any(|field| field.name == "member")),
        "the lookup lost the declaring Rust type layout"
    );
    assert_eq!(
        demands,
        Vec::<String>::new(),
        "the source bridge's checked root member demanded the selected whole manifest"
    );
    Ok(())
}

/// Parse a source fixture through the normal consumer frontend.
fn parse_reexport_program(source: &str) -> Result<crate::ast::Program, String> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))
}

/// Build the checked provider and ABI used by each path-Loaf admission control.
fn rust_reexport_provider(
    foreign: Option<crate::loaf_index::LoafEntry>,
    include_native_surfaces: bool,
) -> Result<
    (
        crate::library_manifest::LibraryManifest,
        crate::typechecker::TypeChecker,
        Vec<u8>,
    ),
    Box<dyn std::error::Error>,
> {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;

    use crate::api_metadata::{
        CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
        materialize_checked_api_public_namespaces,
    };
    use crate::library_manifest::{LibraryManifest, LibraryRustAbi};
    use crate::loaf_index::LoafIndex;
    use crate::rust_abi_cache::encode_rust_abi_cache;
    use crate::typechecker::TypeChecker;
    use incan_lang::interop::{RustFunctionSig, RustItemKind, RustItemMetadata, RustVisibility};
    use sha2::{Digest, Sha256};

    let item = RustItemMetadata {
        canonical_path: "native::answer".into(),
        definition_path: None,
        visibility: RustVisibility::Public,
        kind: RustItemKind::Function(RustFunctionSig {
            receiver_contract: None,
            type_params: Vec::new(),
            bounds: Vec::new(),
            params: Vec::new(),
            return_type: "i64".into(),
            is_async: false,
            is_unsafe: false,
        }),
    };
    let mut native_items = vec![item];
    if include_native_surfaces {
        native_items.push(native_carrier_fixture());
        native_items.push(RustItemMetadata {
            canonical_path: "native::CHECKER_PROFILE".into(),
            definition_path: None,
            visibility: RustVisibility::Public,
            kind: RustItemKind::Constant {
                type_display: "&str".into(),
            },
        });
    }
    let native_facts: BTreeMap<_, _> = native_items
        .iter()
        .map(|item| (item.canonical_path.clone(), item.clone()))
        .collect();
    let namespace_source = if include_native_surfaces {
        "pub from rust::native import answer, CHECKER_PROFILE\n"
    } else {
        "pub from rust::native import answer\n"
    };
    let namespace_program = parse_reexport_program(namespace_source)?;
    let root_program = if foreign.is_some() {
        parse_reexport_program("pub from rust::native import answer\npub from pub::prices.money import MappedView\n")?
    } else if include_native_surfaces {
        parse_reexport_program("pub from rust::native import answer, NativeCarrier\n")?
    } else {
        parse_reexport_program("pub from rust::native import answer\n")?
    };
    let check_producer = |program: &crate::ast::Program, path: &[String]| {
        let mut producer_index = LoafIndex::new(foreign.iter().cloned()).map_err(|error| error.to_string())?;
        producer_index.add_rust_facts(&BTreeSet::new(), native_facts.clone());
        let mut checker = TypeChecker::new();
        checker.set_current_package_identity(Some("probe".into()));
        checker.set_current_module_path(Some(path.to_vec()));
        checker.set_loaves(Arc::new(producer_index));
        checker.check_program(program).map_err(|errors| format!("{errors:?}"))?;
        Ok::<_, String>(checker)
    };
    let root_path = vec!["lib".to_owned()];
    let namespace_path = vec![
        if include_native_surfaces {
            "checker_records"
        } else {
            "keys"
        }
        .to_owned(),
    ];
    let root_checker = check_producer(&root_program, &root_path)?;
    let namespace_checker = check_producer(&namespace_program, &namespace_path)?;
    let root_exports = crate::library_exports::collect_checked_public_exports(&root_program, &root_checker);
    let namespace_exports =
        crate::library_exports::collect_checked_public_exports(&namespace_program, &namespace_checker);
    let mut api = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![
            collect_checked_api_metadata(&root_program, &root_checker, root_path.clone()),
            collect_checked_api_metadata(&namespace_program, &namespace_checker, namespace_path.clone()),
        ],
        public_namespaces: Vec::new(),
    };
    materialize_checked_api_public_namespaces(&mut api)?;
    let mut manifest = LibraryManifest::from_checked_exports("probe", "1.0.0", &root_exports);
    manifest.contract_metadata.identity_graph.extend_checked_api_exports(
        "probe",
        &api,
        &[(root_path, root_exports), (namespace_path, namespace_exports)],
    )?;
    manifest.contract_metadata.api = Some(api);
    let abi_cache = encode_rust_abi_cache(&native_facts)?;
    let mut abi = LibraryRustAbi::from_items(native_items).ok_or("producer lost its explicit Rust item")?;
    abi.cache_digest = Some(format!("sha256:{:x}", Sha256::digest(&abi_cache)));
    manifest.rust_abi = Some(abi);
    Ok((manifest, root_checker, abi_cache))
}

/// One root Rust carrier with field metadata that must remain in its declaring scope.
fn native_carrier_fixture() -> incan_lang::interop::RustItemMetadata {
    use incan_lang::interop::{
        RustFieldInfo, RustItemKind, RustItemMetadata, RustTypeInfo, RustTypeShape, RustVisibility,
    };
    RustItemMetadata {
        canonical_path: "native::NativeCarrier".into(),
        definition_path: None,
        visibility: RustVisibility::Public,
        kind: RustItemKind::Type(RustTypeInfo {
            fields: vec![RustFieldInfo {
                name: "member".into(),
                type_display: "Vec<u8>".into(),
                type_shape: RustTypeShape::Bytes,
            }],
            ..RustTypeInfo::default()
        }),
    }
}

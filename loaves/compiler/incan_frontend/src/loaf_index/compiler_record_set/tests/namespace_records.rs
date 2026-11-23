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
    let install = manifest.prepare_install_record(&declared)?;
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

/// A path dependency carrying a Rust re-export must resolve from its named facts and the already checked Rust item.
fn check_path_rust_reexport_without_manifest() -> Result<(), Box<dyn std::error::Error>> {
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

    let root = tempfile::tempdir()?;
    let private = tempfile::tempdir()?;
    let _home = super::super::super::manifest_input::select_test_home(private.path());
    let _records = super::thread_selection::select(super::RecordSet::new(
        root.path().join("unrelated-installation"),
        Vec::new(),
    ));
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
    let supply_native = |loaves: &mut LoafIndex| {
        loaves.add_rust_facts(
            &BTreeSet::new(),
            BTreeMap::from([(item.canonical_path.clone(), item.clone())]),
        );
    };
    let parse = |source: &str| -> Result<crate::ast::Program, String> {
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))
    };
    let producer = parse("pub from rust::native import answer\n")?;
    let path = vec!["keys".to_owned()];
    let mut producer_index = LoafIndex::default();
    supply_native(&mut producer_index);
    let mut checker = TypeChecker::new();
    checker.set_current_package_identity(Some("probe".into()));
    checker.set_current_module_path(Some(path.clone()));
    checker.set_loaves(Arc::new(producer_index));
    checker
        .check_program(&producer)
        .map_err(|errors| format!("{errors:?}"))?;
    let exports = crate::library_exports::collect_checked_public_exports(&producer, &checker);
    let mut api = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![collect_checked_api_metadata(&producer, &checker, path.clone())],
        public_namespaces: Vec::new(),
    };
    materialize_checked_api_public_namespaces(&mut api)?;
    let mut manifest = LibraryManifest::new("probe", "1.0.0");
    manifest
        .contract_metadata
        .identity_graph
        .extend_checked_api_exports("probe", &api, &[(path, exports)])?;
    manifest.contract_metadata.api = Some(api);
    let abi_cache = encode_rust_abi_cache(&BTreeMap::from([(item.canonical_path.clone(), item.clone())]))?;
    let mut abi = LibraryRustAbi::from_items(vec![item]).ok_or("producer lost its explicit Rust item")?;
    abi.cache_digest = Some(format!("sha256:{:x}", Sha256::digest(&abi_cache)));
    manifest.rust_abi = Some(abi);
    let install = checker.loaf_install_record("probe", &manifest);
    let publication = manifest.prepare_install_record(&install)?;
    let content = manifest.to_json_string()?;
    std::fs::write(root.path().join("probe.incnlib"), &content)?;
    std::fs::write(root.path().join("probe.rustabi"), abi_cache)?;
    std::fs::write(
        root.path().join("probe.incnfacts"),
        manifest.encode_loaf_facts(&content, Some(publication))?,
    )?;
    let open = || LoafIndex::deferred([("probe".into(), "probe".into(), root.path().to_owned(), false)]);
    let cold = open()?;
    assert_eq!(cold.demanded_fixture_manifests(), ["probe"]);
    let loaves = Arc::new(open()?);
    let mut checker = TypeChecker::new();
    checker.set_loaves(Arc::clone(&loaves));
    let program = parse("from pub::probe.keys import answer\n\ndef main() -> int:\n    return answer()\n")?;
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    assert!(
        loaves.demanded_fixture_manifests().is_empty(),
        "Rust alias demanded the whole path manifest"
    );
    Ok(())
}

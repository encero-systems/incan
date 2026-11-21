//! A Rust item re-exported through a chain of `pub from` modules is one public declaration wherever it is imported.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use crate::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
    materialize_api_alias_projections, materialize_checked_api_public_namespaces,
};
use crate::ast::Program;
use crate::library_exports::{CheckedNamedExport, collect_checked_public_exports};
use crate::library_manifest::{LibraryManifest, LibraryRustAbi};
use crate::loaf_index::{LoafEntry, LoafIndex};
use crate::typechecker::TypeChecker;
use incan_lang::interop::{
    RustFieldInfo, RustFunctionSig, RustItemKind, RustItemMetadata, RustParam, RustTypeInfo, RustTypeShape,
    RustVisibility,
};
use incan_semantics_core::{CanonicalSymbolId, IncanType, SymbolOrigin};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// The Rust crate `native`'s record `RecordKey { value: i64 }` and function `read_key(key: &RecordKey) -> i64`.
fn native_items() -> Vec<RustItemMetadata> {
    let function = RustItemMetadata {
        canonical_path: "native::read_key".into(),
        definition_path: None,
        visibility: RustVisibility::Public,
        kind: RustItemKind::Function(RustFunctionSig {
            receiver_contract: None,
            type_params: Vec::new(),
            bounds: Vec::new(),
            params: vec![RustParam {
                name: Some("key".into()),
                type_display: "&native::RecordKey".into(),
            }],
            return_type: "i64".into(),
            is_async: false,
            is_unsafe: false,
        }),
    };
    let record = RustItemMetadata {
        canonical_path: "native::RecordKey".into(),
        definition_path: None,
        visibility: RustVisibility::Public,
        kind: RustItemKind::Type(RustTypeInfo {
            fields: vec![RustFieldInfo {
                name: "value".into(),
                type_display: "i64".into(),
                type_shape: RustTypeShape::Int,
            }],
            ..RustTypeInfo::default()
        }),
    };
    vec![function, record]
}

/// Check `source` as the module `module` of `package` against `loaves` and publish it as that package's manifest, with
/// `rust_abi` as the Rust items the package ships.
fn published_module(
    package: &str,
    module: &str,
    source: &str,
    loaves: LoafIndex,
    rust_abi: Vec<RustItemMetadata>,
) -> TestResult<LibraryManifest> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec![module.to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_package_identity(Some(package.into()));
    checker.set_current_module_path(Some(path.clone()));
    checker.set_loaves(Arc::new(loaves));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{package}.{module}: {errors:?}"))?;
    let exports = collect_checked_public_exports(&program, &checker);
    let mut api = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![collect_checked_api_metadata(&program, &checker, path.clone())],
        public_namespaces: Vec::new(),
    };
    materialize_api_alias_projections(&mut api.modules);
    materialize_checked_api_public_namespaces(&mut api)?;
    let mut manifest = LibraryManifest::new(package, "0.1.0");
    manifest
        .contract_metadata
        .identity_graph
        .extend_checked_api_exports(package, &api, &[(path, exports)])?;
    manifest.contract_metadata.api = Some(api);
    manifest.rust_abi = LibraryRustAbi::from_items(rust_abi);
    Ok(manifest)
}

/// Check `source` as the root module of `package` against `loaves` and publish its root exports as that package's
/// manifest, as a Loaf whose `lib.incn` re-exports names publishes them.
fn published_root(package: &str, source: &str, loaves: LoafIndex) -> TestResult<LibraryManifest> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["lib".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_package_identity(Some(package.into()));
    checker.set_current_module_path(Some(path.clone()));
    checker.set_loaves(Arc::new(loaves));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{package}: {errors:?}"))?;
    let exports = collect_checked_public_exports(&program, &checker);
    let mut api = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![collect_checked_api_metadata(&program, &checker, path.clone())],
        public_namespaces: Vec::new(),
    };
    materialize_api_alias_projections(&mut api.modules);
    materialize_checked_api_public_namespaces(&mut api)?;
    let mut manifest = LibraryManifest::from_checked_exports(package, "0.1.0", &exports);
    manifest
        .contract_metadata
        .identity_graph
        .extend_checked_api_exports(package, &api, &[(path, exports)])?;
    manifest.contract_metadata.api = Some(api);
    Ok(manifest)
}

/// Write `manifest` where the index reads a dependency's compiled artifact and admit it under its own name.
fn installed(root: &Path, manifest: &LibraryManifest) -> TestResult<LoafEntry> {
    let directory = root.join(&manifest.name);
    std::fs::create_dir(&directory)?;
    std::fs::write(
        directory.join(format!("{}.incnlib", manifest.name)),
        manifest.to_json_string()?,
    )?;
    Ok(LoafIndex::dependency(&manifest.name, &manifest.name, &directory)?)
}

/// An index holding only the Rust facts of the crate `native`.
fn native_facts() -> LoafIndex {
    let mut facts = LoafIndex::default();
    facts.add_rust_facts(
        &BTreeSet::new(),
        native_items()
            .into_iter()
            .map(|item| (item.canonical_path.clone(), item))
            .collect::<BTreeMap<_, _>>(),
    );
    facts
}

/// `models.keys` publishes the Rust crate's items with `pub from rust::native import …` and ships their metadata;
/// `facade.store` publishes them again with `pub from pub::models.keys import …`, and `rootfacade` does so at its
/// package root. Returns the installed Loaves.
fn reexport_chain(root: &Path) -> TestResult<[LoafEntry; 3]> {
    let models = published_module(
        "models",
        "keys",
        "pub from rust::native import RecordKey, read_key\n",
        native_facts(),
        native_items(),
    )?;
    let models_entry = installed(root, &models)?;
    let facade = published_module(
        "facade",
        "store",
        "pub from pub::models.keys import RecordKey, read_key\n",
        LoafIndex::new([models_entry.clone()])?,
        Vec::new(),
    )?;
    let root_facade = published_root(
        "rootfacade",
        "pub from pub::models.keys import RecordKey, read_key\n",
        LoafIndex::new([models_entry.clone()])?,
    )?;
    Ok([installed(root, &facade)?, models_entry, installed(root, &root_facade)?])
}

/// Check a consumer of the facades that reaches the Rust items through both re-exports, and return its Body IR.
fn check_consumer(source: &str) -> TestResult<incan_semantics_core::body_ir::BodyIrModule> {
    let root = tempfile::tempdir()?;
    let loaves = reexport_chain(root.path())?;
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_loaves(Arc::new(LoafIndex::new(loaves)?));
    checker.check_program(&program).map_err(|errors| {
        let messages = errors.iter().map(|error| error.message.as_str()).collect::<Vec<_>>();
        format!("{messages:?}")
    })?;
    let facts = checker.into_type_info();
    let providers = crate::body_ir::ProviderOperationCatalog::new();
    let context = crate::body_ir::BodyIrContext {
        modules: crate::body_ir::ContextModules::default(),
        provider_operations: &providers,
        registry_package: None,
        library_package: None,
        include_inline_tests: false,
    };
    Ok(crate::body_ir::build_body_ir_module(&program, &[], &facts, &context))
}

/// A function and a type of a Rust crate, re-exported by one Loaf's module and again by another's, import by name from
/// the second module as the Rust items themselves, not as an ambiguous member.
#[test]
fn rust_items_through_two_reexports_import_from_the_outer_module() -> TestResult {
    check_consumer(
        "from pub::facade.store import RecordKey, read_key\n\ndef keep(key: RecordKey) -> RecordKey:\n    return key\n",
    )?;
    Ok(())
}

/// The same Rust items resolve when the outer module is imported as a namespace and its members are named through it.
#[test]
fn rust_items_through_two_reexports_resolve_through_the_outer_namespace() -> TestResult {
    check_consumer(
        "from pub::facade import store\n\ndef keep(key: store.RecordKey) -> store.RecordKey:\n    return key\n",
    )?;
    Ok(())
}

/// A record of the Rust type, built through each route and kept in a local, and the Rust function called through each
/// route on it, lower to Body IR at the Rust item's own type: no local is left without a type.
#[test]
fn rust_items_through_two_reexports_lower_to_body_ir() -> TestResult {
    let module = check_consumer(
        "from pub::facade.store import RecordKey, read_key\nfrom pub::facade import store\n\ndef main() -> int:\n    first = RecordKey(value=5)\n    second = store.RecordKey(value=6)\n    return read_key(first) + store.read_key(second)\n",
    )?;
    for body in &module.bodies {
        for local in &body.locals {
            assert!(
                !matches!(local.ty, IncanType::Unknown),
                "{}: local {:?} {:?} at {:?} has no type",
                body.name,
                local.id,
                local.name,
                local.span
            );
        }
        let rendered = format!("{body:?}");
        assert!(!rendered.contains("Unsupported"), "{}: {rendered}", body.name);
    }
    Ok(())
}

/// The same Rust items re-exported again at a Loaf's package root import by name from the root and resolve through the
/// package namespace, as the Rust items themselves, and lower to Body IR with every local typed.
#[test]
fn rust_items_through_a_package_root_reexport_lower_to_body_ir() -> TestResult {
    let module = check_consumer(
        "from pub::rootfacade import RecordKey, read_key\nimport pub::rootfacade as rf\n\ndef main() -> int:\n    first = RecordKey(value=5)\n    second = rf.RecordKey(value=6)\n    return read_key(first) + rf.read_key(second)\n",
    )?;
    for body in &module.bodies {
        for local in &body.locals {
            assert!(
                !matches!(local.ty, IncanType::Unknown),
                "{}: local {:?} {:?} at {:?} has no type",
                body.name,
                local.id,
                local.name,
                local.span
            );
        }
        let rendered = format!("{body:?}");
        assert!(!rendered.contains("Unsupported"), "{}: {rendered}", body.name);
    }
    Ok(())
}

/// Check `source` as the module `module` of the library `package`, beside the library's source modules `siblings`,
/// against the Rust facts of the crate `native`.
fn checked_library_module(
    package: &str,
    module: &str,
    source: &str,
    siblings: &[(&str, &Program)],
) -> TestResult<(Program, TypeChecker)> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_package_identity(Some(package.into()));
    checker.set_current_module_path(Some(vec![module.to_owned()]));
    checker.set_loaves(Arc::new(native_facts()));
    for (name, _) in siblings {
        checker.register_dependency_module_path_segments(name, vec![(*name).to_owned()]);
    }
    checker
        .check_with_imports(&program, siblings)
        .map_err(|errors| format!("{package}.{module}: {errors:?}"))?;
    Ok((program, checker))
}

/// The canonical identity a module's checked exports publish for `name`.
fn published_identity(exports: &[CheckedNamedExport], name: &str) -> Option<CanonicalSymbolId> {
    exports
        .iter()
        .find(|export| export.name == name)
        .and_then(|export| export.identity.canonical.clone())
}

/// A library module that re-exports a Rust item another module of the same library re-exports from its crate (std.web's
/// prelude taking `Html` from `std.web.response`, which takes it with `pub from rust::incan_std_web`) publishes the
/// Rust item's own canonical identity: the chain ends at the `rust::` import. The library's manifest, with both
/// modules, is valid, and a consumer importing the items from the second module checks and lowers with every local
/// typed.
#[test]
fn a_rust_item_a_library_reexports_through_a_second_module_keeps_its_identity() -> TestResult {
    let (vendor, vendor_checker) = checked_library_module(
        "web",
        "vendor",
        "pub from rust::native import RecordKey, read_key\n",
        &[],
    )?;
    let (facade, facade_checker) = checked_library_module(
        "web",
        "facade",
        "pub from vendor import RecordKey, read_key\n",
        &[("vendor", &vendor)],
    )?;
    let direct = collect_checked_public_exports(&vendor, &vendor_checker);
    let through = collect_checked_public_exports(&facade, &facade_checker);
    for name in ["RecordKey", "read_key"] {
        let expected = published_identity(&direct, name).ok_or_else(|| format!("vendor: no identity for {name}"))?;
        assert!(
            matches!(expected.origin, SymbolOrigin::RustCrate(_)),
            "{name}: {expected:?}"
        );
        assert_eq!(
            published_identity(&through, name),
            Some(expected),
            "facade: {name} publishes the Rust item's identity"
        );
    }

    let mut api = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![
            collect_checked_api_metadata(&vendor, &vendor_checker, vec!["vendor".to_owned()]),
            collect_checked_api_metadata(&facade, &facade_checker, vec!["facade".to_owned()]),
        ],
        public_namespaces: Vec::new(),
    };
    materialize_api_alias_projections(&mut api.modules);
    materialize_checked_api_public_namespaces(&mut api)?;
    let mut manifest = LibraryManifest::new("web", "0.1.0");
    manifest.contract_metadata.identity_graph.extend_checked_api_exports(
        "web",
        &api,
        &[
            (vec!["vendor".to_owned()], direct),
            (vec!["facade".to_owned()], through),
        ],
    )?;
    manifest.contract_metadata.api = Some(api);
    manifest.rust_abi = LibraryRustAbi::from_items(native_items());
    let root = tempfile::tempdir()?;
    // Reading the artifact back validates the manifest's identity graph.
    let web = installed(root.path(), &manifest)?;

    let source = "from pub::web.facade import RecordKey, read_key\n\ndef main() -> int:\n    key = RecordKey(value=5)\n    return read_key(key)\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_loaves(Arc::new(LoafIndex::new([web])?));
    checker.check_program(&program).map_err(|errors| {
        let messages = errors.iter().map(|error| error.message.as_str()).collect::<Vec<_>>();
        format!("consumer: {messages:?}")
    })?;
    let facts = checker.into_type_info();
    let providers = crate::body_ir::ProviderOperationCatalog::new();
    let context = crate::body_ir::BodyIrContext {
        modules: crate::body_ir::ContextModules::default(),
        provider_operations: &providers,
        registry_package: None,
        library_package: None,
        include_inline_tests: false,
    };
    let module = crate::body_ir::build_body_ir_module(&program, &[], &facts, &context);
    for body in &module.bodies {
        for local in &body.locals {
            assert!(
                !matches!(local.ty, IncanType::Unknown),
                "{}: local {:?} has no type",
                body.name,
                local.name
            );
        }
    }
    Ok(())
}

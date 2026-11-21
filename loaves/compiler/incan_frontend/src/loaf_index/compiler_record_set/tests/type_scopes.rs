//! A Loaf's nominal type scope, read by module from the installed set (RFC 179).

use std::sync::Arc;

use super::super::thread_selection;
use super::typechecker_reads::{CODECS, catalog_with, manifest_import_reads};
use super::{content_name, installed_library, read_declaration_in, read_summary};
use crate::api_metadata::{
    ApiAlias, ApiDeclaration, CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage,
    collect_checked_api_metadata, materialize_checked_api_public_namespaces,
};
use crate::library_manifest::LibraryManifest;
use crate::library_manifest::compiler_records::RecordFamily;
use crate::loaf_index::LoafIndex;

/// The record family of a module's type scope.
const TYPE_SCOPE: &str = "type-scope";

/// The Loaf `prices` whose module `money` also declares the model `Receipt` and `receipt`, which returns one.
fn catalog_with_a_model() -> Result<LibraryManifest, Box<dyn std::error::Error>> {
    catalog_with(
        "\npub model Receipt:\n    pub cents: int\n\npub def receipt(cents: int) -> Receipt:\n    return Receipt(cents=cents)\n",
    )
}

/// After the index's summary, reading the type scope of the module `money` by its path opens its one record and
/// nothing else, and the record names the declaration `Receipt` spells.
#[test]
fn a_consumer_reads_one_modules_type_scope_record_alone() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let set = installed_library(root.path(), &catalog_with_a_model()?)?;
    let dir = root.path().join("debug");
    read_summary(&set, &dir, "prices")?.ok_or("the Loaf is installed")?;
    let indexed = set.reads();
    let (scopes, _) =
        read_declaration_in(&set, &dir, "prices", TYPE_SCOPE, "money")?.ok_or("the type scope record is installed")?;
    let [scope] = scopes.as_slice() else {
        return Err(format!("one type scope record for `money`, found {}", scopes.len()).into());
    };
    let mut scope = scope.clone();
    let identity = scope["names"]["Receipt"]
        .as_object_mut()
        .and_then(|declared| declared.remove("identity"));
    assert!(identity.is_some(), "the scope names the identity of `Receipt`: {scope}");
    assert_eq!(
        scope,
        serde_json::json!({
            "module": ["money"],
            "standard": false,
            "names": { "Receipt": { "module": ["money"], "name": "Receipt", "scoped": true } },
        }),
        "the scope of `money`: `Receipt` names the module's own model"
    );
    let (records, bytes) = (set.reads().records - indexed.records, set.reads().bytes - indexed.bytes);
    assert_eq!(
        records, 1,
        "the consumer read {records} records, {bytes} bytes; it needs the type scope record alone"
    );
    Ok(())
}

/// A program that calls a function whose signature names a type of the Loaf is checked with that module's type scope
/// read from its own record.
#[test]
fn a_program_reads_the_type_scope_of_the_module_it_calls_into() -> Result<(), Box<dyn std::error::Error>> {
    let read = manifest_import_reads(
        &catalog_with_a_model()?,
        "from pub::prices import money\n\ndef main() -> int:\n    return money.receipt(5).cents\n",
    )?;
    let scope = format!("/{}/", TYPE_SCOPE);
    let named = format!("/{}", content_name(b"money"));
    assert!(
        read.iter().any(|key| key.contains(&scope) && key.ends_with(&named)),
        "checking reads the type scope of `money` by name; it read {read:?}"
    );
    Ok(())
}

/// A Loaf `package` whose module `encoding::_shared` declares the model `EncodingError`, whose module
/// `encoding::base64` re-exports it and declares `b64decode`, and whose module `encoding::hex` re-exports it from
/// `encoding::base64` and declares `unhexlify`; both functions return it. Each re-export spells its target below
/// `root`: nothing for a package's own paths, `std` for a standard package's.
fn reexporting(package: &str, root: &[&str]) -> Result<LibraryManifest, Box<dyn std::error::Error>> {
    let mut api = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: Vec::new(),
        public_namespaces: Vec::new(),
    };
    let model = "pub model EncodingError:\n    pub detail: str\n";
    let mut exports = Vec::new();
    for (module, function, target) in [
        ("_shared", "", ""),
        ("base64", "b64decode", "_shared"),
        ("hex", "unhexlify", "base64"),
    ] {
        let mut source = model.to_owned();
        if !function.is_empty() {
            source.push_str(&format!(
                "\npub def {function}(text: str) -> Result[bytes, EncodingError]:\n    return Err(EncodingError(detail=text))\n"
            ));
        }
        let tokens = crate::lexer::lex(&source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let path = vec!["encoding".to_owned(), module.to_owned()];
        let mut checker = crate::typechecker::TypeChecker::new();
        checker.set_current_package_identity(Some(package.into()));
        checker.set_current_module_path(Some(path.clone()));
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        let mut checked = collect_checked_api_metadata(&program, &checker, path.clone());
        let published = crate::library_exports::collect_checked_public_exports(&program, &checker);
        if !target.is_empty() {
            // The module re-exports the model rather than declaring it: `pub from <root>.encoding.<target> import
            // EncodingError`.
            for declaration in &mut checked.declarations {
                if let ApiDeclaration::Model(declared) = declaration {
                    *declaration = ApiDeclaration::Alias(ApiAlias {
                        name: declared.name.clone(),
                        anchor: declared.anchor.clone(),
                        target_path: root
                            .iter()
                            .copied()
                            .chain(["encoding", target, "EncodingError"])
                            .map(str::to_owned)
                            .collect(),
                        is_public: true,
                        projected_function: None,
                        projected_type: None,
                    });
                }
            }
        }
        exports.push((path, published));
        api.modules.push(checked);
    }
    materialize_checked_api_public_namespaces(&mut api)?;
    let mut manifest = LibraryManifest::new(package, "0.1.0");
    manifest
        .contract_metadata
        .identity_graph
        .extend_checked_api_exports(package, &api, &exports)?;
    manifest.contract_metadata.api = Some(api);
    Ok(manifest)
}

/// The type scope record of the module `encoding::hex` of the installed `manifest`, without the identity a later set
/// names for the declaration.
fn hex_scope(manifest: &LibraryManifest) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let set = installed_library(root.path(), manifest)?;
    let dir = root.path().join("debug");
    read_summary(&set, &dir, &manifest.name)?.ok_or("the Loaf is installed")?;
    let (scopes, _) = read_declaration_in(&set, &dir, &manifest.name, TYPE_SCOPE, "encoding::hex")?
        .ok_or("the type scope record is installed")?;
    let [scope] = scopes.as_slice() else {
        return Err(format!("one type scope record for `encoding::hex`, found {}", scopes.len()).into());
    };
    let mut scope = scope.clone();
    if let Some(declared) = scope["names"]["EncodingError"].as_object_mut() {
        declared.remove("identity");
    }
    Ok(scope)
}

/// A spelling that reaches its declaration through re-exports in two other modules names the declaring module.
#[test]
fn a_type_scope_follows_a_reexport_through_two_modules() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        hex_scope(&reexporting("prices", &[])?)?,
        serde_json::json!({
            "module": ["encoding", "hex"],
            "standard": false,
            "names": { "EncodingError": { "module": ["encoding", "_shared"], "name": "EncodingError", "scoped": true } },
        }),
    );
    Ok(())
}

/// A standard package spells its own modules below `std`; its type scope follows those spellings as its own.
#[test]
fn a_standard_packages_type_scope_follows_its_std_spellings() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        hex_scope(&reexporting(CODECS, &["std"])?)?,
        serde_json::json!({
            "module": ["encoding", "hex"],
            "standard": true,
            "names": { "EncodingError": { "module": ["encoding", "_shared"], "name": "EncodingError", "scoped": true } },
        }),
    );
    Ok(())
}

/// A program that calls a function returning a model its module re-exports through another module is checked from
/// the function's records, the type scopes of the modules on the way and the model's own records: it reads the
/// model's layout by its declaring module's name and never the Loaf's contract metadata.
#[test]
fn a_program_checks_a_function_naming_a_reexported_model_from_its_records() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let _selection = thread_selection::select(installed_library(root.path(), &reexporting(CODECS, &["std"])?)?);
    let index = LoafIndex::deferred([(CODECS.to_owned(), CODECS.to_owned(), root.path().join("debug"), true)])?;
    let source = "from std.encoding import hex\n\ndef main() -> str:\n    match hex.unhexlify(\"00\"):\n        Ok(data) => return \"\"\n        Err(error) => return error.detail\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let exports: Arc<[crate::module::ExportedSymbol]> = Arc::from(crate::module::exported_symbols(&program));
    let mut checker = crate::typechecker::TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".to_owned()]));
    checker.set_loaves(Arc::new(index));
    checker
        .check_with_imports_and_exports(&program, &[], Default::default())
        .map_err(|errors| format!("{errors:?}"))?;
    checker.seal_current_module_dependency_interface("main", &program, exports, true);
    let read = thread_selection::read_keys();
    let layout = format!("/{}/", RecordFamily::Layout.as_str());
    let named = format!("/{}", content_name(b"encoding::_shared::EncodingError"));
    assert!(
        read.iter().any(|key| key.contains(&layout) && key.ends_with(&named)),
        "checking reads `EncodingError`'s layout by its declaring module's name; it read {read:?}"
    );
    let contract = format!("/{}/", RecordFamily::ContractMetadata.as_str());
    let whole = read.iter().filter(|key| key.contains(&contract)).collect::<Vec<_>>();
    assert!(
        whole.is_empty(),
        "checking read the Loaf's contract metadata: {whole:?}"
    );
    Ok(())
}

/// The primitive provider is mandatory for a program, even when its expressions never use asynchronous tasks.
/// Checking a plain value must neither open its task scope nor hydrate its entire contract.
#[test]
fn ordinary_body_does_not_open_the_task_error_provider_contract() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let package = "incan_stdlib_primitives";
    let manifest = LibraryManifest::new(package, "0.1.0");
    let _selection = thread_selection::select(installed_library(root.path(), &manifest)?);
    let index = LoafIndex::deferred([(package.to_owned(), package.to_owned(), root.path().join("debug"), true)])?;
    let source = "def main() -> bool:\n    return 0 in [0, 1]\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = crate::typechecker::TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".to_owned()]));
    checker.set_loaves(Arc::new(index));
    checker
        .check_with_imports_and_exports(&program, &[], Default::default())
        .map_err(|errors| format!("{errors:?}"))?;
    let read = thread_selection::read_keys();
    assert!(
        read.iter()
            .all(|key| !key.contains("/contract-metadata/") && !key.contains("/type-scope/")),
        "{read:?}"
    );
    Ok(())
}

//! What the typechecker reads of an installed Loaf a program imports from (RFC 179).

use std::sync::Arc;

use super::super::thread_selection;
use super::{content_name, installed_library};
use crate::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
    materialize_checked_api_public_namespaces,
};
use crate::library_manifest::LibraryManifest;
use crate::library_manifest::compiler_records::RecordFamily;
use crate::loaf_index::LoafIndex;

/// How many public functions the module declares beside the one the program calls.
const OTHER_FUNCTIONS: usize = 60;

/// A Loaf `prices` whose module `money` declares `price`, with a defaulted parameter, and many other public functions.
fn catalog() -> Result<LibraryManifest, Box<dyn std::error::Error>> {
    let mut source = "pub def price(cents: int, rounded: bool = true) -> int:\n    return cents\n".to_owned();
    for index in 0..OTHER_FUNCTIONS {
        source.push_str(&format!(
            "\npub def discount_{index}(cents: int, percent: int = 10) -> int:\n    return cents - percent\n"
        ));
    }
    let tokens = crate::lexer::lex(&source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = crate::typechecker::TypeChecker::new();
    checker.set_current_package_identity(Some("prices".into()));
    checker.set_current_module_path(Some(vec!["money".to_owned()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let module = vec!["money".to_owned()];
    let exports = crate::library_exports::collect_checked_public_exports(&program, &checker);
    let mut api = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![collect_checked_api_metadata(&program, &checker, module.clone())],
        public_namespaces: Vec::new(),
    };
    materialize_checked_api_public_namespaces(&mut api)?;
    let mut manifest = LibraryManifest::new("prices", "0.1.0");
    manifest
        .contract_metadata
        .identity_graph
        .extend_checked_api_exports("prices", &api, &[(module, exports)])?;
    manifest.contract_metadata.api = Some(api);
    Ok(manifest)
}

/// A program that calls one function of an installed Loaf's module is checked from that function's own records:
/// checking reads `money::price`'s signature by name, and neither the Loaf's contract metadata nor any record of the
/// declarations the program does not use.
#[test]
fn a_program_checks_an_imported_function_from_its_declaration_records() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let manifest = catalog()?;
    let named = |declaration: &str| format!("/{}", content_name(declaration.as_bytes()));
    let selection = thread_selection::select(installed_library(root.path(), &manifest)?);
    let index = LoafIndex::deferred([(
        "prices".to_owned(),
        "prices".to_owned(),
        root.path().join("debug"),
        false,
    )])?;
    let source = "from pub::prices import money\n\ndef main() -> int:\n    return money.price(1999)\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = crate::typechecker::TypeChecker::new();
    checker.set_loaves(Arc::new(index));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let read = thread_selection::read_keys();
    let reads = selection.0.reads();
    let signature = format!("/{}/", RecordFamily::DeclarationSignature.as_str());
    assert!(
        read.iter()
            .any(|key| key.contains(&signature) && key.ends_with(&named("money::price"))),
        "checking reads `price`'s signature by name; it read {} records, {} bytes: {read:?}",
        reads.records,
        reads.bytes
    );
    let contract = format!("/{}/", RecordFamily::ContractMetadata.as_str());
    let unused = read
        .iter()
        .filter(|key| {
            key.contains(&contract)
                || (0..OTHER_FUNCTIONS).any(|index| key.ends_with(&named(&format!("money::discount_{index}"))))
        })
        .collect::<Vec<_>>();
    assert!(
        unused.is_empty(),
        "checking read records the program does not use, {} bytes in all: {unused:?}",
        reads.bytes
    );
    Ok(())
}

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

/// A Loaf `prices` whose module `money` declares `price`, with a defaulted parameter, `unit`, without parameters, the
/// constant `RATE` and many other public functions.
pub(super) fn catalog() -> Result<LibraryManifest, Box<dyn std::error::Error>> {
    let mut source = "pub def price(cents: int, rounded: bool = true) -> int:\n    return cents\n".to_owned();
    source.push_str("\npub def unit() -> int:\n    return 100\n");
    source.push_str("\npub const RATE: int = 7\n");
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

/// The package a program reaches as `std.encoding`.
const CODECS: &str = "incan_stdlib_codecs";

/// A standard Loaf whose namespace `encoding` holds the modules `base64`, declaring `b64encode`, and `hex`, each beside
/// many other functions, so the parent namespace `encoding` publishes both modules' members.
fn codecs() -> Result<LibraryManifest, Box<dyn std::error::Error>> {
    let mut api = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: Vec::new(),
        public_namespaces: Vec::new(),
    };
    let mut exports = Vec::new();
    for (module, first) in [("base64", "b64encode"), ("hex", "hexlify")] {
        let mut source = format!("pub def {first}(data: bytes) -> str:\n    return \"\"\n");
        for index in 0..OTHER_FUNCTIONS {
            source.push_str(&format!(
                "\npub def {module}_{index}(data: bytes) -> str:\n    return \"\"\n"
            ));
        }
        let tokens = crate::lexer::lex(&source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let path = vec!["encoding".to_owned(), module.to_owned()];
        let mut checker = crate::typechecker::TypeChecker::new();
        checker.set_current_package_identity(Some(CODECS.into()));
        checker.set_current_module_path(Some(path.clone()));
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        exports.push((
            path.clone(),
            crate::library_exports::collect_checked_public_exports(&program, &checker),
        ));
        api.modules.push(collect_checked_api_metadata(&program, &checker, path));
    }
    materialize_checked_api_public_namespaces(&mut api)?;
    let mut manifest = LibraryManifest::new(CODECS, "0.1.0");
    manifest
        .contract_metadata
        .identity_graph
        .extend_checked_api_exports(CODECS, &api, &exports)?;
    manifest.contract_metadata.api = Some(api);
    Ok(manifest)
}

/// Check `from std.encoding import base64` and a call of `base64.b64encode` the way the driver checks a program's
/// module, sealing its interface after the check, and return the keys of the records it read, in reading order.
fn standard_import_reads() -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let _selection = thread_selection::select(installed_library(root.path(), &codecs()?)?);
    let index = LoafIndex::deferred([(CODECS.to_owned(), CODECS.to_owned(), root.path().join("debug"), true)])?;
    let source =
        "from std.encoding import base64\n\ndef main() -> str:\n    return base64.b64encode(b\"two loaves\")\n";
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
    Ok(thread_selection::read_keys())
}

/// A program importing a standard module and calling one of its functions is checked, and its interface sealed, from
/// that module's records: it reads `b64encode`'s signature by name and never the Loaf's contract metadata.
#[test]
fn a_program_checks_a_standard_module_import_from_its_records() -> Result<(), Box<dyn std::error::Error>> {
    let read = standard_import_reads()?;
    let signature = format!("/{}/", RecordFamily::DeclarationSignature.as_str());
    let named = format!("/{}", content_name(b"encoding::base64::b64encode"));
    assert!(
        read.iter().any(|key| key.contains(&signature) && key.ends_with(&named)),
        "checking reads `b64encode`'s signature by name; it read {read:?}"
    );
    let contract = format!("/{}/", RecordFamily::ContractMetadata.as_str());
    let whole = read.iter().filter(|key| key.contains(&contract)).collect::<Vec<_>>();
    assert!(
        whole.is_empty(),
        "checking read the Loaf's contract metadata: {whole:?}"
    );
    Ok(())
}

/// Checking a program and sealing its interface read each record they use once: the index keeps every namespace and
/// declaration record it has read for the rest of the check.
#[test]
fn a_program_reads_each_record_it_checks_with_once() -> Result<(), Box<dyn std::error::Error>> {
    let read = standard_import_reads()?;
    let mut again = read
        .iter()
        .filter(|key| read.iter().filter(|other| other == key).count() > 1)
        .collect::<Vec<_>>();
    again.dedup();
    assert!(
        again.is_empty(),
        "checking read these records more than once: {again:?}; all reads: {read:?}"
    );
    Ok(())
}

/// Importing the namespace `encoding.base64` reads its own record, never that of its parent `encoding`, which publishes
/// every member of its child modules: the child's record says whether the parent publishes a member of its name.
#[test]
fn importing_a_child_namespace_reads_no_record_of_its_parent() -> Result<(), Box<dyn std::error::Error>> {
    let read = standard_import_reads()?;
    let namespace = format!("/{}/", RecordFamily::PublicNamespace.as_str());
    let parent = format!("/{}", content_name(b"encoding"));
    let child = format!("/{}", content_name(b"encoding::base64"));
    assert!(
        read.iter().any(|key| key.contains(&namespace) && key.ends_with(&child)),
        "the import reads `encoding::base64`'s record; it read {read:?}"
    );
    let parents = read
        .iter()
        .filter(|key| key.contains(&namespace) && key.ends_with(&parent))
        .collect::<Vec<_>>();
    assert!(
        parents.is_empty(),
        "the import read the parent namespace's record: {parents:?}"
    );
    Ok(())
}

/// Check `source`, a program importing from the installed Loaf `prices`, and return the keys of the records it read.
fn prices_import_reads(source: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let _selection = thread_selection::select(installed_library(root.path(), &catalog()?)?);
    let index = LoafIndex::deferred([(
        "prices".to_owned(),
        "prices".to_owned(),
        root.path().join("debug"),
        false,
    )])?;
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = crate::typechecker::TypeChecker::new();
    checker.set_loaves(Arc::new(index));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    Ok(thread_selection::read_keys())
}

/// A program that calls a function without parameters, which has no default record, is checked from the function's
/// signature record: it reads `money::unit`'s signature by name and never the Loaf's contract metadata.
#[test]
fn a_program_checks_an_imported_function_without_parameters_from_its_records() -> Result<(), Box<dyn std::error::Error>>
{
    let read = prices_import_reads("from pub::prices import money\n\ndef main() -> int:\n    return money.unit()\n")?;
    let signature = format!("/{}/", RecordFamily::DeclarationSignature.as_str());
    let named = format!("/{}", content_name(b"money::unit"));
    assert!(
        read.iter().any(|key| key.contains(&signature) && key.ends_with(&named)),
        "checking reads `unit`'s signature by name; it read {read:?}"
    );
    let contract = format!("/{}/", RecordFamily::ContractMetadata.as_str());
    let whole = read.iter().filter(|key| key.contains(&contract)).collect::<Vec<_>>();
    assert!(
        whole.is_empty(),
        "checking read the Loaf's contract metadata: {whole:?}"
    );
    Ok(())
}

/// A program that uses a constant of an installed Loaf's module is checked from the constant's own records: it reads
/// `money::RATE`'s signature by name and never the Loaf's contract metadata.
#[test]
fn a_program_checks_an_imported_constant_from_its_declaration_records() -> Result<(), Box<dyn std::error::Error>> {
    let read = prices_import_reads("from pub::prices import money\n\ndef main() -> int:\n    return money.RATE\n")?;
    let signature = format!("/{}/", RecordFamily::DeclarationSignature.as_str());
    let named = format!("/{}", content_name(b"money::RATE"));
    assert!(
        read.iter().any(|key| key.contains(&signature) && key.ends_with(&named)),
        "checking reads `RATE`'s signature by name; it read {read:?}"
    );
    let contract = format!("/{}/", RecordFamily::ContractMetadata.as_str());
    let whole = read.iter().filter(|key| key.contains(&contract)).collect::<Vec<_>>();
    assert!(
        whole.is_empty(),
        "checking read the Loaf's contract metadata: {whole:?}"
    );
    Ok(())
}

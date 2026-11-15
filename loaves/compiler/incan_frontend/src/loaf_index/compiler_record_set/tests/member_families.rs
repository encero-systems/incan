//! The member families a program's check settles from an installed Loaf's own records (RFC 179).

use super::content_name;
use super::typechecker_reads::{catalog, catalog_with, manifest_import_reads};
use crate::library_manifest::LibraryManifest;
use crate::library_manifest::compiler_records::RecordFamily;

/// The records of `read` that hold the Loaf's whole contract metadata.
fn contract_metadata(read: &[String]) -> Vec<&String> {
    let contract = format!("/{}/", RecordFamily::ContractMetadata.as_str());
    read.iter().filter(|key| key.contains(&contract)).collect()
}

/// A program that calls an overloaded function of an installed Loaf's module is checked from the overloads' own
/// records, each default record matched to its overload: it reads `money::scale`'s signatures by name and never the
/// Loaf's contract metadata.
#[test]
fn a_program_checks_an_imported_overloaded_function_from_its_declaration_records()
-> Result<(), Box<dyn std::error::Error>> {
    let overloads = "\npub def scale(cents: int) -> int:\n    return cents\n\npub def scale(cents: float, factor: float = 2.0) -> float:\n    return cents * factor\n";
    let read = manifest_import_reads(
        &catalog_with(overloads)?,
        "from pub::prices import money\n\ndef main() -> float:\n    return money.scale(1.5) + money.scale(2.5, 3.0)\n",
    )?;
    let signature = format!("/{}/", RecordFamily::DeclarationSignature.as_str());
    let named = format!("/{}", content_name(b"money::scale"));
    let signatures = read
        .iter()
        .filter(|key| key.contains(&signature) && key.ends_with(&named))
        .count();
    assert_eq!(
        signatures, 2,
        "checking reads both of `scale`'s signatures by name; it read {read:?}"
    );
    let whole = contract_metadata(&read);
    assert!(
        whole.is_empty(),
        "checking read the Loaf's contract metadata: {whole:?}"
    );
    Ok(())
}

/// The Loaf `prices` of [`catalog`], whose package root also exports the enum `Tier`.
fn catalog_exporting_an_enum() -> Result<LibraryManifest, Box<dyn std::error::Error>> {
    let source = "pub enum Tier:\n    Basic\n    Plus\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = crate::typechecker::TypeChecker::new();
    checker.set_current_package_identity(Some("prices".into()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let exports = crate::library_exports::collect_checked_public_exports(&program, &checker);
    let root = LibraryManifest::from_checked_exports("prices", "0.1.0", &exports);
    let mut manifest = catalog()?;
    manifest.exports = root.exports;
    manifest
        .contract_metadata
        .identity_graph
        .exports
        .extend(root.contract_metadata.identity_graph.exports);
    Ok(manifest)
}

/// A program that imports a namespace from the package root of an installed Loaf exporting an enum is checked from the
/// Loaf's records: the names of the enum's variants are record names, so the records prove the root exports nothing
/// named like the namespace, and checking never reads the Loaf's contract metadata.
#[test]
fn a_program_imports_a_namespace_from_the_root_of_a_loaf_exporting_an_enum_from_its_records()
-> Result<(), Box<dyn std::error::Error>> {
    let read = manifest_import_reads(
        &catalog_exporting_an_enum()?,
        "from pub::prices import money\n\ndef main() -> int:\n    return money.price(1999)\n",
    )?;
    let signature = format!("/{}/", RecordFamily::DeclarationSignature.as_str());
    let named = format!("/{}", content_name(b"money::price"));
    assert!(
        read.iter().any(|key| key.contains(&signature) && key.ends_with(&named)),
        "checking reads `price`'s signature by name; it read {read:?}"
    );
    let whole = contract_metadata(&read);
    assert!(
        whole.is_empty(),
        "checking read the Loaf's contract metadata: {whole:?}"
    );
    Ok(())
}

/// A program that imports a variant of an installed Loaf's exported enum from its package root is still bound to that
/// variant: the records name it, so the namespace route leaves the import to the Loaf's manifest.
#[test]
fn a_program_imports_an_enum_variant_from_the_root_of_a_loaf() -> Result<(), Box<dyn std::error::Error>> {
    manifest_import_reads(
        &catalog_exporting_an_enum()?,
        "from pub::prices import Tier, Plus\n\ndef main() -> Tier:\n    return Plus\n",
    )?;
    Ok(())
}

/// A program that calls a function returning nothing is checked from the function's own records: `None` is no type a
/// provider's remapping can change, so checking reads `money::note`'s signature by name and never the Loaf's contract
/// metadata.
#[test]
fn a_program_checks_an_imported_function_returning_nothing_from_its_declaration_records()
-> Result<(), Box<dyn std::error::Error>> {
    let read = manifest_import_reads(
        &catalog_with("\npub def note(cents: int) -> None:\n    pass\n")?,
        "from pub::prices import money\n\ndef main() -> None:\n    money.note(5)\n",
    )?;
    let signature = format!("/{}/", RecordFamily::DeclarationSignature.as_str());
    let named = format!("/{}", content_name(b"money::note"));
    assert!(
        read.iter().any(|key| key.contains(&signature) && key.ends_with(&named)),
        "checking reads `note`'s signature by name; it read {read:?}"
    );
    let whole = contract_metadata(&read);
    assert!(
        whole.is_empty(),
        "checking read the Loaf's contract metadata: {whole:?}"
    );
    Ok(())
}

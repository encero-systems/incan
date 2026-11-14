//! The member families a program's check settles from an installed Loaf's own records (RFC 179).

use super::content_name;
use super::typechecker_reads::{catalog_with, manifest_import_reads};
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

//! A Loaf's nominal type scope, read by module from the installed set (RFC 179).

use super::typechecker_reads::{catalog_with, manifest_import_reads};
use super::{content_name, installed_library, read_declaration_in, read_summary};
use crate::library_manifest::LibraryManifest;

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
    assert_eq!(
        *scope,
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

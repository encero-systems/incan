//! Canonical storage identity for same-spelled declarations imported from distinct modules.

use super::identities_and_imports::build_with_published_imports;
use super::{body_named, build};

/// Same-spelled owners reached through distinct checked imports must not collapse to one local storage type.
#[test]
fn same_named_imported_constructor_results_keep_distinct_carriers() -> Result<(), Box<dyn std::error::Error>> {
    let first_source = "pub model Product:\n    pub value: int\n";
    let second_source = "pub model Product:\n    pub value: str\n";
    let first = build(first_source, &["first"])?;
    let second = build(second_source, &["second"])?;
    let module = build_with_published_imports(
        "from first import Product as FirstProduct\nfrom second import Product as SecondProduct\n\ndef main() -> None:\n    first = FirstProduct(value=1)\n    second = SecondProduct(value=\"two\")\n    println(first.value)\n    println(second.value)\n",
        &["consumer"],
        &[
            ("first", &["first"], first_source),
            ("second", &["second"], second_source),
        ],
        &[first, second],
    )?;
    let main = body_named(&module, "main")?;
    let first = main
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("first"))
        .ok_or("missing first binding")?;
    let second = main
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("second"))
        .ok_or("missing second binding")?;
    assert_ne!(
        first.ty,
        second.ty,
        "{}\n{:?}",
        main.render_snapshot(),
        main.named_type_identities
    );
    Ok(())
}

//! Original trait annotations remain distinct from concrete model witnesses across Body IR refinement passes.

use super::{body_named, build};

/// Recording the original result must never relabel its concrete carrier as the source trait.
#[test]
fn trait_return_refinement_preserves_the_checked_model_owner() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "trait Named:\n  pass\n\nmodel Item with Named:\n  number: int\n\ndef make() -> Named:\n  return Item(number=3)\n\ndef main() -> None:\n  value = make()\n  println(7)\n",
        &["return_pass_order"],
    )?;
    let producer = body_named(&module, "make")?;
    let owner = module
        .nominal_declarations
        .iter()
        .find(|owner| owner.name == "Item")
        .ok_or("missing checked model owner")?;
    assert_eq!(producer.named_type_identities.get("Item"), Some(&owner.canonical));
    let source_trait = module.trait_declarations.first().ok_or("missing source trait")?;
    assert_eq!(producer.named_type_identities.get("Named"), Some(source_trait));
    Ok(())
}

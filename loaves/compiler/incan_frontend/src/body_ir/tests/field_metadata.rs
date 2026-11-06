//! Compiler-generated reflection keeps its canonical operation and declaration-owned field values.

use super::{bir, build};

/// Public models expose only public fields, retaining each visible field's immutable metadata and defaults.
#[test]
fn generated_metadata_retains_values_and_operation() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "pub model Vault:\n    secret: str = \"sealed\"\n    pub label: str\n\ndef main() -> None:\n    vault = Vault(label=\"public\")\n    fields = vault.__fields__()\n",
        &["m", "field_metadata"],
    )?;
    let declaration = module
        .nominal_declarations
        .iter()
        .find(|item| item.name == "Vault")
        .ok_or("missing Vault")?;
    let encoded = serde_json::to_value(declaration)?;
    assert_eq!(encoded["reflection_metadata"][0]["name"], "label");
    assert_eq!(encoded["reflection_metadata"].as_array().map(Vec::len), Some(1));
    assert_eq!(encoded["reflection_metadata"][0]["type_name"], "str");
    assert_eq!(encoded["reflection_metadata"][0]["has_default"], false);
    let target = module
        .bodies
        .iter()
        .flat_map(|body| &body.block.stmts)
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                ..
            } if target.name == "__fields__" => Some(target),
            _ => None,
        })
        .ok_or("missing reflection call")?;
    assert_eq!(serde_json::to_value(target)?["reflection"], "Metadata");
    Ok(())
}

/// A user-written member with the reflection spelling retains its source implementation.
#[test]
fn source_field_metadata_method_keeps_its_identity() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "model Record:\n    value: int\n    def __fields__(self) -> int:\n        return self.value\n\ndef main() -> None:\n    println(Record(value=2).__fields__())\n",
        &["m", "field_metadata_source"],
    )?;
    let target = module
        .bodies
        .iter()
        .flat_map(|body| &body.block.stmts)
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                ..
            } if target.name == "__fields__" => Some(target),
            _ => None,
        })
        .ok_or("missing source member")?;
    assert!(target.canonical.is_some());
    assert!(target.reflection.is_none());
    Ok(())
}

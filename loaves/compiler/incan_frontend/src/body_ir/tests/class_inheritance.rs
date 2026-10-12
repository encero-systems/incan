//! Checked inheritance ownership and dispatch retained separately from original source bodies.

use super::{IncanType, build};

const SOURCE: &str = "class Base:\n  pub root: int\n  def score(self) -> int:\n    return self.root\n\nclass Child extends Base:\n  pub child: int\n  def score(self) -> int:\n    return self.root + self.child\n\nclass Leaf extends Child:\n  pub leaf: int\n  def read(self) -> int:\n    return self.score() + self.leaf\n";

/// Parent-first fields retain their ancestor identities while every concrete layout validates independently.
#[test]
fn inherited_class_fields_keep_their_declaring_identities() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(SOURCE, &["inheritance_fields"])?;
    let base = module
        .nominal_declarations
        .iter()
        .find(|item| item.name == "Base")
        .ok_or("base missing")?;
    let child = module
        .nominal_declarations
        .iter()
        .find(|item| item.name == "Child")
        .ok_or("child missing")?;
    let leaf = module
        .nominal_declarations
        .iter()
        .find(|item| item.name == "Leaf")
        .ok_or("leaf missing")?;
    assert_eq!(child.fields, ["root", "child"]);
    assert_eq!(leaf.fields, ["root", "child", "leaf"]);
    assert_eq!(child.field_identities[0], base.field_identities[0]);
    assert_eq!(leaf.field_identities[0], base.field_identities[0]);
    assert_eq!(leaf.field_identities[1], child.field_identities[1]);
    assert!(module.is_well_formed_nominal_declaration(base));
    assert!(module.is_well_formed_nominal_declaration(child));
    assert!(module.is_well_formed_nominal_declaration(leaf));
    Ok(())
}

/// Ancestor slots record the nearest checked override without inventing descendant source body identities.
#[test]
fn inherited_class_slots_select_the_nearest_checked_override() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(SOURCE, &["inheritance_slots"])?;
    let source_slot = module
        .bodies
        .iter()
        .find(|body| {
            body.name == "score"
                && body
                    .locals
                    .first()
                    .is_some_and(|local| local.ty == IncanType::Named("Base".to_string()))
        })
        .and_then(|body| body.canonical.as_ref())
        .ok_or("ancestor slot missing")?;
    let implementation = module
        .bodies
        .iter()
        .find(|body| {
            body.name == "score"
                && body
                    .locals
                    .first()
                    .is_some_and(|local| local.ty == IncanType::Named("Child".to_string()))
        })
        .and_then(|body| body.canonical.as_ref())
        .ok_or("child implementation missing")?;
    let encoded = serde_json::to_value(&module)?;
    let declarations = encoded["nominal_declarations"]
        .as_array()
        .ok_or("nominal records missing")?;
    let leaf = declarations
        .iter()
        .find(|item| item["name"] == "Leaf")
        .ok_or("leaf record missing")?;
    assert_eq!(leaf["class_inheritance"]["parent"]["declaration_name"], "Child");
    let methods = leaf["class_inheritance"]["methods"]
        .as_array()
        .ok_or("inherited slots missing")?;
    let slot = serde_json::to_value(source_slot)?;
    let body = serde_json::to_value(implementation)?;
    assert!(
        methods
            .iter()
            .any(|method| method["method"] == slot && method["implementation"] == body)
    );
    assert_eq!(module.bodies.iter().filter(|body| body.name == "score").count(), 2);
    Ok(())
}

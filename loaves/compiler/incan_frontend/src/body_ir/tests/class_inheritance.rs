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

/// Malformed layouts cannot omit, reorder or change ancestor storage, forge slots, or attach class edges to newtypes.
#[test]
fn malformed_class_inheritance_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(SOURCE, &["inheritance_malformed"])?;
    let child = module
        .nominal_declarations
        .iter()
        .find(|item| item.name == "Child")
        .ok_or("child missing")?;
    let mut missing = child.clone();
    missing.fields.clear();
    missing.field_identities.clear();
    missing.field_types.clear();
    missing.field_public.clear();
    assert!(!module.is_well_formed_nominal_declaration(&missing));
    let mut changed = child.clone();
    changed.field_public[0] = !changed.field_public[0];
    assert!(!module.is_well_formed_nominal_declaration(&changed));
    let mut reordered = child.clone();
    reordered.fields.reverse();
    reordered.field_identities.reverse();
    reordered.field_types.reverse();
    reordered.field_public.reverse();
    assert!(!module.is_well_formed_nominal_declaration(&reordered));
    let mut encoded = serde_json::to_value(child)?;
    encoded["class_inheritance"]["methods"] = serde_json::json!([]);
    let missing_slots = serde_json::from_value(encoded)?;
    assert!(!module.is_well_formed_nominal_declaration(&missing_slots));
    let tokens = build("type Token = newtype int\n", &["inheritance_newtype"])?;
    let token = tokens.nominal_declarations.first().ok_or("newtype missing")?;
    assert!(tokens.is_well_formed_nominal_declaration(token));
    let mut forged = serde_json::to_value(token)?;
    forged["class_inheritance"] = serde_json::to_value(child)?["class_inheritance"].clone();
    let forged = serde_json::from_value(forged)?;
    assert!(!tokens.is_well_formed_nominal_declaration(&forged));
    Ok(())
}

/// Source-local inheritance remains explicitly uncovered by package publication until its complete closure is proven.
#[test]
fn inherited_class_package_publication_is_uncovered() -> Result<(), Box<dyn std::error::Error>> {
    use incan_semantics_core::SymbolOrigin;
    use incan_semantics_core::executable_representation::{
        CoverageReason, DeclarationCoverage, SurfaceReader, build_surface,
    };
    use std::collections::BTreeSet;

    let mut module = build(SOURCE, &["inheritance_publication"])?;
    let mut child = module
        .nominal_declarations
        .iter()
        .find(|item| item.name == "Child")
        .ok_or("child missing")?
        .clone();
    child.canonical.origin = SymbolOrigin::Package {
        library: "inheritance_probe".to_owned(),
        module_path: vec!["inheritance_publication".to_owned()],
    };
    let identity = child.canonical.clone();
    module.nominal_declarations = vec![child];
    module.bodies.clear();
    let bytes = build_surface(
        &[module],
        "inheritance_probe",
        "0.1.0",
        &BTreeSet::from([identity.clone()]),
        &BTreeSet::new(),
    )?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(matches!(
        reader.index().coverage(&identity)?,
        DeclarationCoverage::Uncovered(CoverageReason::UnsupportedConstruct)
    ));
    Ok(())
}

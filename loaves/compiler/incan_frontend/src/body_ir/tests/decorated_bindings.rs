//! Original callable and initialized replacement contracts for RFC 036's direct route.

use super::*;

/// A decorated function retains an executable original body and a separately typed replacement initializer.
#[test]
fn decorated_function_retains_original_body_and_replacement() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_functions_and_projections/user_defined_decorators.incn"
    );
    let module = build(source, &["m", "decorated_binding"])?;
    let label = body_named(&module, "label")?;
    assert_eq!(label.first_representation_gap(), None, "{}", label.render_snapshot());
    assert_eq!(label.return_type, IncanType::Primitive(IncanPrimitiveType::Int));
    let facts = serde_json::to_value(&module)?;
    let bindings = facts.get("decorated_bindings").and_then(serde_json::Value::as_array);
    assert_eq!(bindings.map(Vec::len), Some(1), "{facts}");
    let original = &facts["decorated_bindings"][0]["original_body"];
    assert_eq!(
        original["return_type"],
        serde_json::to_value(&IncanType::Primitive(IncanPrimitiveType::Str))?
    );
    assert_eq!(
        body_named(&module, "run")?.return_type,
        IncanType::Primitive(IncanPrimitiveType::Int)
    );
    let [storage] = module.static_declarations.as_slice() else {
        return Err("expected one initialized decorated replacement".into());
    };
    assert!(module.is_well_formed_static_declaration(storage));
    let mut malformed = module.clone();
    malformed.static_declarations[0].ty = IncanType::Primitive(IncanPrimitiveType::Bool);
    assert!(!malformed.is_well_formed_static_declaration(&malformed.static_declarations[0]));
    Ok(())
}

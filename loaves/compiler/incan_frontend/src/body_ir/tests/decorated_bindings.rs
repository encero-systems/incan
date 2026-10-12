//! Original callable and initialized replacement contracts for RFC 036's direct route.

use super::*;

/// A decorated method retains its receiver on the original and on the initialized replacement signature.
#[test]
fn decorated_method_retains_original_receiver_and_replacement() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_functions_and_projections/user_defined_method_decorators.incn"
    );
    let module = build(source, &["m", "decorated_method"])?;
    let [binding] = module.decorated_bindings.as_slice() else {
        return Err("expected the decorated method original".into());
    };
    assert_eq!(binding.original_body.first_representation_gap(), None);
    assert_eq!(
        binding.original_body.return_type,
        IncanType::Primitive(IncanPrimitiveType::Str)
    );
    assert!(module.is_well_formed_decorated_binding(binding));
    assert_eq!(binding.original_body.params[0].name, "self");
    let [storage] = module.static_declarations.as_slice() else {
        return Err("expected the initialized method replacement".into());
    };
    assert!(module.is_well_formed_static_declaration(storage));
    Ok(())
}

/// A mutable method original and replacement retain the exclusive receiver passing contract.
#[test]
fn decorated_mutable_method_retains_receiver_passing() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_functions_and_projections/user_defined_mutable_method_decorators.incn"
    );
    let module = build(source, &["m", "decorated_mutable_method"])?;
    let [binding] = module.decorated_bindings.as_slice() else {
        return Err("expected the mutable decorated method original".into());
    };
    assert!(binding.original_body.params[0].mutable);
    assert!(module.is_well_formed_decorated_binding(binding));
    let wrapper = body_named(&module, "bump")?;
    assert!(wrapper.params[0].mutable);
    assert_eq!(wrapper.first_representation_gap(), None);
    Ok(())
}

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

/// An extern's delegation must not bypass an unproven user-defined replacement binding.
#[test]
fn decorated_extern_keeps_a_named_refusal() -> Result<(), Box<dyn std::error::Error>> {
    let source = "rust.module(\"incan_std_testing\")\n\ndef keep(func: (str) -> None) -> (str) -> None:\n    return func\n\n@keep\n@rust.extern\ndef fail_t(msg: str) -> None:\n    ...\n";
    let module = build(source, &["m", "decorated_extern"])?;
    let body = body_named(&module, "fail_t")?;
    assert_eq!(
        body.first_representation_gap(),
        Some("declaration `fail_t` rebound by a user-defined decorator")
    );
    assert!(body.extern_delegation.is_none());
    assert!(module.static_declarations.is_empty());
    Ok(())
}

/// A factory expression retains its checked call before the returned decorator is applied to the original.
#[test]
fn decorator_factory_retains_the_initialized_replacement() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/lowering_expressions/decorator_declared_surface_issue1453.incn"
    );
    let module = build(source, &["m", "decorator_factory"])?;
    let body = body_named(&module, "twice")?;
    assert_eq!(body.first_representation_gap(), None, "{}", body.render_snapshot());
    assert_eq!(module.static_declarations.len(), 2);
    for storage in &module.static_declarations {
        assert!(module.is_well_formed_static_declaration(storage));
    }
    Ok(())
}

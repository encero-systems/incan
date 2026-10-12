//! Original callable and initialized replacement contracts for RFC 036's direct route.

use super::*;

/// A checked derive adoption retains the source trait slot and its default implementation for the concrete model.
#[test]
fn derived_trait_default_retains_its_implementation() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"import codec
@derive(codec)
model Item:
    value: int

def main() -> None:
    println(Item(value=1).tag())
"#;
    let provider = "__derives__ = [Named]\n\npub trait Named:\n    def tag(self) -> str:\n        return \"named\"\n";
    let program = parser::parse(&lexer::lex(source).map_err(|errors| format!("{errors:?}"))?)
        .map_err(|errors| format!("{errors:?}"))?;
    let provider = parser::parse(&lexer::lex(provider).map_err(|errors| format!("{errors:?}"))?)
        .map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    let module_path = vec!["m".to_string(), "derived_trait".to_string()];
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_with_imports(&program, &[("codec", &provider)])
        .map_err(|errors| format!("{errors:?}"))?;
    let module = build_body_ir_module_v0(&program, &module_path, checker.type_info());
    let implementation = module
        .trait_implementations
        .iter()
        .find(|implementation| implementation.owner.declaration_name == "Item")
        .ok_or("missing checked derive adoption")?;
    assert_eq!(implementation.method.declaration_name, "tag");
    assert_eq!(implementation.method, implementation.implementation);
    Ok(())
}

/// A direct call to a planned shared replacement keeps the caller's non-Copy owner for its scope-exit drop.
#[test]
fn shared_method_replacement_call_keeps_the_owner() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"class Box:
    pub text: str
    @choose
    def label(self, number: int) -> str:
        return "original"

def parse(box: Box, number: int) -> str:
    return f"{box.text}:{number}"

def choose(func: (Box, int) -> str) -> (Box, int) -> str:
    return parse

def main() -> None:
    box = Box(text="v")
    println(parse(box, 3))
"#;
    let module = build(source, &["m", "shared_receiver_owner"])?;
    let main = body_named(&module, "main")?;
    let owner = main
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("box"))
        .ok_or("missing box owner")?;
    let read = main
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                args,
                ..
            } if target.name == "parse" => match args.first() {
                Some(bir::ArgumentElement::One(bir::Operand::Place(read))) => Some(read),
                _ => None,
            },
            _ => None,
        })
        .ok_or("missing shared replacement call")?;
    assert_eq!(read.fact, bir::OwnershipFact::Borrow);
    assert!(!read.last_use);
    assert!(
        main.block
            .stmts
            .iter()
            .any(|statement| matches!(statement.kind, bir::StatementKind::Drop { local } if local == owner.id))
    );
    Ok(())
}

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
    let decorator = body_named(&module, "as_int")?;
    let bir::StaticInitializer::Computed(initializer) = &storage.initial else {
        return Err("expected an executable decorator initializer".into());
    };
    let forwarded = initializer
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Closure { params, body, .. },
                ..
            } if body.function_item && body.original_callable.is_none() => Some(params),
            _ => None,
        })
        .ok_or("expected a named decorator function item")?;
    assert_eq!(forwarded[0].ty, decorator.params[0].ty);
    assert!(matches!(body_named(&module, "parse")?.params[0].ty, IncanType::Ref(_)));
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

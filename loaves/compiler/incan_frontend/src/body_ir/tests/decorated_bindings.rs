//! Original callable and initialized replacement contracts for RFC 036's direct route.

use super::*;

/// A function with type parameters keeps its decorator initializer as a per-instantiation template, not module storage.
#[test]
fn generic_decorator_retains_an_initializer_template() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def keep[F]() -> (F) -> F:\n    return (func) => func\n\n@keep()\ndef label[T]() -> str:\n    return \"label\"\n\ndef main() -> None:\n    println(label[int]())\n",
        &["m", "generic_decorator"],
    )?;
    assert_eq!(module.decorated_bindings.len(), 1, "{module:#?}");
    let binding = module.decorated_bindings.first().ok_or("missing generic original")?;
    let json = serde_json::to_value(binding)?;
    assert_eq!(json["initializer"]["type_parameters"], serde_json::json!(["T"]));
    assert!(binding.initializer.is_some());
    assert!(serde_json::to_string(binding)?.contains("\"PerInstantiation\""));

    assert_eq!(binding.original_body.type_parameters, vec!["T"]);
    assert!(module.is_well_formed_decorated_binding(binding));
    assert!(
        module.static_declarations.is_empty(),
        "a generic decorator cannot initialize at module entry"
    );
    Ok(())
}

/// A generic initializer cannot borrow another declaration's source identity, even with the same callable signature.
#[test]
fn generic_decorator_initializer_requires_its_original_identity() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def keep[F]() -> (F) -> F:\n    return (func) => func\n\n@keep()\ndef label[T]() -> str:\n    return \"label\"\n\ndef other() -> str:\n    return \"other\"\n",
        &["m", "generic_decorator_identity"],
    )?;
    let binding = module.decorated_bindings.first().ok_or("missing generic original")?;
    assert!(module.is_well_formed_decorated_binding(binding));
    let mut json = serde_json::to_value(binding)?;
    json["initializer"]["decl_id"] = serde_json::to_value(&body_named(&module, "other")?.decl_id)?;
    let corrupted: bir::DecoratedBinding = serde_json::from_value(json)?;
    assert!(!module.is_well_formed_decorated_binding(&corrupted));
    Ok(())
}

/// Imported builtin Clone retains the model's required native marker even when the checked trait has no macro paths.
#[test]
fn imported_builtin_clone_retains_the_native_marker() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std.derives.copying import Clone\n\n@derive(Clone)\nmodel Counter:\n    value: int\n",
        &["m", "imported_builtin_clone"],
    )?;
    let model = module
        .nominal_declarations
        .iter()
        .find(|model| model.canonical.declaration_name == "Counter")
        .ok_or("missing checked Counter declaration")?;
    assert!(model.derives.iter().any(|derive| derive == "Clone"), "{model:?}");
    Ok(())
}

/// A facade-adopted JSON protocol's late native selection reaches the same derive record consumed by Body IR.
#[test]
fn facade_json_adoption_reaches_the_native_derive_record() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from facade import Serialize\n\nmodel Payload with Serialize:\n    value: int\n\ndef main() -> None:\n    println(Payload(value=1).to_json())\n";
    let facade = "pub from std.serde.json import Serialize\n";
    let program = parser::parse(&lexer::lex(source).map_err(|errors| format!("{errors:?}"))?)
        .map_err(|errors| format!("{errors:?}"))?;
    let facade = parser::parse(&lexer::lex(facade).map_err(|errors| format!("{errors:?}"))?)
        .map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    let path = vec!["m".to_string(), "facade_json_adoption".to_string()];
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_with_imports(&program, &[("facade", &facade)])
        .map_err(|errors| format!("{errors:?}"))?;
    let module = build_body_ir_module_v0(&program, &path, checker.type_info());
    let model = module
        .nominal_declarations
        .iter()
        .find(|model| model.canonical.declaration_name == "Payload")
        .ok_or("missing checked Payload declaration")?;
    assert!(
        model.derives.iter().any(|derive| derive == "serde::Serialize"),
        "{model:?}"
    );
    Ok(())
}
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
    let binding = module.decorated_bindings.first().ok_or("module original missing")?;
    assert!(binding.initializer.is_none());
    assert!(serde_json::to_string(binding)?.contains("\"ModuleStorage\""));
    assert!(
        serde_json::to_value(binding)?
            .as_object()
            .ok_or("binding must be a record")?
            .contains_key("initializer")
    );

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
        serde_json::to_value(IncanType::Primitive(IncanPrimitiveType::Str))?
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

/// A generic factory is instantiated independently at each checked function or method application.
#[test]
fn generic_decorator_factory_retains_closed_applications() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_functions_and_projections/decorator_factory_on_function_and_method.incn"
    );
    let module = build(source, &["m", "generic_decorator_factory"])?;
    assert_eq!(module.decorated_bindings.len(), 2);
    assert_eq!(module.static_declarations.len(), 2);
    for storage in &module.static_declarations {
        let bir::StaticInitializer::Computed(initializer) = &storage.initial else {
            return Err("expected a once-initialized replacement".into());
        };
        let target = initializer
            .block
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                bir::StatementKind::Call {
                    callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                    ..
                } if target.name == "preserve" => Some(target),
                _ => None,
            })
            .ok_or("missing checked factory call")?;
        assert_eq!(target.type_args.len(), 1, "{}", initializer.render_snapshot());
        assert_eq!(
            target.type_args[0], storage.ty,
            "factory instance must retain the exact free or shared carrier"
        );
        assert!(!format!("{:?}", target.type_args).contains("TypeVar"));
        assert!(!format!("{:?}", initializer.locals).contains("TypeVar"));
        assert!(module.is_well_formed_static_declaration(storage));
    }
    Ok(())
}

/// Factory arguments fixed by values survive application inference of the remaining callable parameter.
#[test]
fn generic_decorator_factory_keeps_partially_inferred_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"def preserve[Tag, F](tag: Tag) -> ((F) -> F):
    return (func) => func

@preserve(7)
def increment(value: int) -> int:
    return value + 1

def main() -> None:
    println(increment(2))
"#;
    let module = build(source, &["m", "partial_factory_inference"])?;
    let [storage] = module.static_declarations.as_slice() else {
        return Err("expected one initialized replacement".into());
    };
    let bir::StaticInitializer::Computed(initializer) = &storage.initial else {
        return Err("expected an executable factory initializer".into());
    };
    let target = initializer
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                ..
            } if target.name == "preserve" => Some(target),
            _ => None,
        })
        .ok_or("missing checked factory instance")?;
    assert_eq!(
        target.type_args,
        vec![IncanType::Primitive(IncanPrimitiveType::Int), storage.ty.clone()]
    );
    Ok(())
}

/// A factory's nested closure signature retains its declared binder for later instance substitution.
#[test]
fn generic_factory_closure_keeps_parameter_placeholders() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_functions_and_projections/decorator_factory_on_function_and_method.incn"
    );
    let module = build(source, &["m", "factory_closure_parameters"])?;
    let factory = body_named(&module, "preserve")?;
    let signature = factory
        .locals
        .iter()
        .find_map(|local| match &local.ty {
            IncanType::Function { params, return_type } => Some((params, return_type)),
            _ => None,
        })
        .ok_or("missing factory closure signature")?;
    assert_eq!(signature.0[0].ty, IncanType::TypeVar("F".to_string()));
    assert_eq!(**signature.1, IncanType::TypeVar("F".to_string()));
    Ok(())
}

/// Rest parameters retain their checked packed frame types and source roles on originals and replacement wrappers.
#[test]
fn decorated_rest_parameters_retain_owned_packs() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_functions_and_projections/decorated_variadic_function.incn"
    );
    let module = build(source, &["m", "decorated_rest_parameters"])?;
    let [binding] = module.decorated_bindings.as_slice() else {
        return Err("expected one original rest-parameter function".into());
    };
    let original = &binding.original_body;
    let wrapper = body_named(&module, "decorated_total")?;
    assert!(module.body_has_canonical_direct_call_id(original));
    assert!(
        module.is_well_formed_decorated_binding(binding),
        "initialization={:?}; original={:?}; wrapper={:?}; storage={:?}",
        binding.initialization,
        original.params,
        wrapper.params,
        module
            .static_declarations
            .iter()
            .map(|storage| &storage.ty)
            .collect::<Vec<_>>()
    );
    for body in [original, wrapper] {
        assert_eq!(body.params.len(), 4);
        assert_eq!(
            body.params[2].ty,
            IncanType::Generic {
                base: "List".to_string(),
                args: vec![IncanType::Primitive(IncanPrimitiveType::Int)],
            }
        );
        assert_eq!(
            body.params[3].ty,
            IncanType::Generic {
                base: "Dict".to_string(),
                args: vec![
                    IncanType::Primitive(IncanPrimitiveType::Str),
                    IncanType::Primitive(IncanPrimitiveType::Str)
                ],
            }
        );
        let facts = serde_json::to_value(body)?;
        assert_eq!(facts["params"][2]["kind"], "RestPositional");
        assert_eq!(facts["params"][3]["kind"], "RestKeyword");
    }
    let main = body_named(&module, "main")?;
    let mut call_count = 0;
    for statement in &main.block.stmts {
        if let bir::StatementKind::Call {
            callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
            args,
            ..
        } = &statement.kind
            && target.name == "decorated_total"
        {
            call_count += 1;
            assert_eq!(args.len(), 4);
            assert_eq!(target.binding, bir::ArgumentBinding::resolved_positional(4));
            for (index, base) in [(2, "List"), (3, "Dict")] {
                let bir::ArgumentElement::One(bir::Operand::Place(read)) = &args[index] else {
                    return Err("rest argument must select its owned pack local".into());
                };
                let local = read.place.local_id().ok_or("rest pack must be frame-local")?;
                assert!(matches!(&main.locals[local.index()].ty,
                    IncanType::Generic { base: actual, .. } if actual == base));
            }
        }
    }
    assert_eq!(call_count, 2);
    Ok(())
}

/// The combined parameter roles, native union fields, enum identities, publication context, and checked reconstruction
/// policy and complete delegation positions reject older positional layouts before decoding.
#[test]
fn combined_body_ir_shape_refuses_older_wire_versions() -> Result<(), Box<dyn std::error::Error>> {
    use incan_semantics_core::executable_representation::{
        EXECUTABLE_REPRESENTATION_VERSION, ExecutableRepresentationError, decode_module,
    };
    const { assert!(EXECUTABLE_REPRESENTATION_VERSION >= 18) };
    for version in [9, 10, 11, 12, 13, 14, 15, 16, 17] {
        assert!(matches!(
            decode_module(&[version]),
            Err(ExecutableRepresentationError::UnsupportedVersion { found, supported })
                if found == u32::from(version) && supported == EXECUTABLE_REPRESENTATION_VERSION
        ));
    }
    Ok(())
}

/// Rest packs are callee-owned and must be dropped on return, while ordinary collections keep caller ownership.
#[test]
fn rest_packs_drop_without_dropping_ordinary_collections() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def total(ordinary: List[int], *xs: int, **labels: str) -> int:\n    return 1\n";
    let module = build(source, &["m", "rest_ownership"])?;
    let body = body_named(&module, "total")?;
    let drops: Vec<_> = body
        .block
        .stmts
        .iter()
        .filter_map(|statement| match statement.kind {
            bir::StatementKind::Drop { local } => Some(local),
            _ => None,
        })
        .collect();
    assert!(!drops.contains(&body.params[0].local));
    assert!(drops.contains(&body.params[1].local));
    assert!(drops.contains(&body.params[2].local));
    Ok(())
}

/// An open declaration element is not a concrete pack fact, even when the call itself inferred a type argument.
#[test]
fn generic_rest_calls_refuse_unretained_pack_elements() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def total[T](*xs: T) -> int:\n    return 1\n\ndef main() -> None:\n    println(total(1))\n";
    let module = build(source, &["m", "generic_rest"])?;
    assert!(
        body_named(&module, "main")?
            .render_snapshot()
            .contains("rest call with an unretained concrete pack element")
    );
    Ok(())
}

/// Checked tuple spreads preserve the established pack binding, while unretained omitted inputs remain refused.
#[test]
fn rest_calls_preserve_checked_spreads_and_refuse_unretained_defaults() -> Result<(), Box<dyn std::error::Error>> {
    let prefix = "def total(a: int = 1, *xs: int) -> int:\n    return a\n\ndef main() -> None:\n";
    let module = build(
        &format!("{prefix}    println(total(1, *(2, 3)))\n"),
        &["m", "checked_rest_spread"],
    )?;
    let main = body_named(&module, "main")?;
    let snapshot = main.render_snapshot();
    assert!(!snapshot.contains("unsupported("), "{snapshot}");
    let target = named_targets(&module, "main")
        .into_iter()
        .find(|target| target.name == "total")
        .ok_or("missing total call")?;
    let bir::ArgumentBinding::Resolved {
        arguments,
        defaulted_slots,
    } = &target.binding
    else {
        return Err("checked rest spread must retain its declared slots".into());
    };
    assert_eq!(
        arguments.iter().map(|argument| argument.slot).collect::<Vec<_>>(),
        [0, 1]
    );
    assert!(defaulted_slots.is_empty());
    let module = build(
        &format!("{prefix}    println(total())\n"),
        &["m", "omitted_rest_default"],
    )?;
    assert!(
        body_named(&module, "main")?
            .render_snapshot()
            .contains("omitted ordinary argument")
    );
    Ok(())
}

/// Named JSON snapshots default an absent ordinary role; binary snapshots use the separately bumped wire version.
#[test]
fn ordinary_parameter_json_defaults_its_role() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def one(value: int) -> int:\n    return value\n",
        &["m", "parameter_json"],
    )?;
    let parameter = &body_named(&module, "one")?.params[0];
    let mut json = serde_json::to_value(parameter)?;
    json.as_object_mut()
        .ok_or("parameter JSON must be an object")?
        .remove("kind");
    assert_eq!(serde_json::from_value::<bir::CallableParam>(json)?, *parameter);
    Ok(())
}

/// A signature using its checked type parameter keeps invocation-scoped application and no module storage.
#[test]
fn generic_decorator_with_open_signature_keeps_invocation_policy() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def preserve[F]() -> ((F) -> F):\n    println(\"factory\")\n    return (func) => func\n\n@preserve()\npub def echo[T](value: T) -> T:\n    return value\n";
    let module = build(source, &["m", "open_generic_decorator"])?;
    let binding = module
        .decorated_bindings
        .first()
        .ok_or("open generic original missing")?;
    assert_eq!(binding.original_body.type_parameters, ["T"]);
    assert!(module.is_well_formed_decorated_binding(binding));
    assert!(serde_json::to_string(binding)?.contains("\"PerInvocation\""));
    let json = serde_json::to_value(binding)?;
    assert!(
        json.as_object()
            .ok_or("decorated binding must be a record")?
            .contains_key("initializer")
    );
    assert!(module.static_declarations.is_empty());
    Ok(())
}

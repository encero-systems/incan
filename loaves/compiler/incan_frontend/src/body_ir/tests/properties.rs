//! Checked property getter bodies and reads for direct execution.

use super::{IncanPrimitiveType, IncanType, SemanticSourceTargetKind, bir, body_named, build};

/// Original property programs keep concrete getter frames and locals under project-aware nominal retention.
#[test]
fn original_properties_keep_checked_project_types() -> Result<(), Box<dyn std::error::Error>> {
    for (name, source) in [
        (
            "computed_properties",
            include_str!(
                "../../../../incan_test_support/fixtures/behavior/snapshots_models_and_classes/computed_properties.incn"
            ),
        ),
        (
            "method_classmethod_staticmethod_property",
            include_str!(
                "../../../../incan_test_support/fixtures/behavior/snapshots_functions_and_projections/method_classmethod_staticmethod_property.incn"
            ),
        ),
        (
            "public_model_static_method_and_property",
            include_str!(
                "../../../../incan_test_support/fixtures/behavior/snapshots_functions_and_projections/public_model_static_method_and_property.incn"
            ),
        ),
    ] {
        let tokens = super::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = super::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let path = vec!["m".to_owned(), name.to_owned()];
        let mut checker = super::TypeChecker::new();
        checker.set_current_module_path(Some(path.clone()));
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        let module = super::build_body_ir_module_v0_with_executable_context(&program, &path, checker.type_info(), &[]);
        for body in &module.bodies {
            assert_ne!(
                body.return_type,
                IncanType::Unknown,
                "{name}: {}",
                body.render_snapshot()
            );
            for local in &body.locals {
                assert_ne!(local.ty, IncanType::Unknown, "{name}: {}", body.render_snapshot());
            }
            assert!(
                !body.render_snapshot().contains("unsupported("),
                "{name}: {}",
                body.render_snapshot()
            );
        }
    }
    Ok(())
}

/// A conflicting carrier binding cannot select an owner, and a same-named foreign getter frame remains refused.
#[test]
fn project_property_refuses_a_foreign_getter_owner() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_models_and_classes/computed_properties.incn"
    );
    let tokens = super::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = super::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["m".to_owned(), "conflicting_property_owner".to_owned()];
    let mut checker = super::TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let mut facts = checker.type_info().clone();
    let owner = facts
        .declarations
        .named_type_identities
        .get("Money")
        .cloned()
        .ok_or("missing checked Money owner")?;
    let key = bir::nominal_carrier_key(&owner).ok_or("missing checked Money carrier")?;
    let mut foreign = owner.clone();
    foreign.origin = incan_semantics_core::SymbolOrigin::Module(vec!["foreign".to_owned()]);
    facts
        .declarations
        .named_type_identities
        .insert(key.clone(), foreign.clone());
    assert!(super::super::parameter_types::retained_nominal_identity(&key, &facts).is_none());
    facts.declarations.named_type_identities.remove(&key);
    facts
        .declarations
        .named_type_identities
        .insert("ForeignMoney".into(), foreign);
    for binding in facts.declarations.body_ir_property_bindings.values_mut() {
        if binding
            .callable
            .identity
            .as_ref()
            .is_some_and(|identity| identity.declaration_name == "dollars")
        {
            binding.receiver_type = crate::symbols::ResolvedType::Named("ForeignMoney".into());
        }
    }
    let module = super::build_body_ir_module_v0_with_executable_context(&program, &path, &facts, &[]);
    let snapshot = body_named(&module, "property_total")?.render_snapshot();
    assert!(
        snapshot.contains("computed property owner differs from checked getter frame"),
        "{snapshot}"
    );
    Ok(())
}

/// A computed property of a model lowers to a receiver-only getter body carrying the property's own identity, and a
/// property read is a call of that getter on the borrowed receiver.
#[test]
fn computed_properties_lower_to_getter_bodies_their_reads_call() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "model Account:\n  cents: int\n\n  property dollars -> int:\n    return self.cents // 100\n\n",
        "def main() -> None:\n  account = Account(cents=1250)\n  println(account.dollars)\n",
    );
    let module = build(source, &["m", "computed_property"])?;
    let getter = body_named(&module, "dollars")?;
    let canonical = getter.canonical.as_ref().ok_or("getter identity missing")?;
    assert_eq!(canonical.kind, SemanticSourceTargetKind::Property);
    assert!(module.body_has_canonical_direct_call_id(getter));
    let getter_rendered = getter.render_snapshot();
    assert!(!getter_rendered.contains("unsupported("), "{getter_rendered}");
    assert_eq!(getter.params.len(), 1);
    assert_eq!(getter.return_type, IncanType::Primitive(IncanPrimitiveType::Int));
    let rendered = body_named(&module, "main")?.render_snapshot();
    assert!(rendered.contains("call method:dollars(borrow("), "{rendered}");
    Ok(())
}

/// Getter identities admit only immutable zero-argument receiver frames and matching property implementation kinds.
#[test]
fn getter_identity_requires_immutable_zero_argument_frame() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/snapshots_models_and_classes/computed_properties.incn"
        ),
        &["m", "getter_identity"],
    )?;
    let index = module
        .bodies
        .iter()
        .position(|body| body.name == "dollars")
        .ok_or("missing getter")?;
    assert!(module.body_has_canonical_direct_call_id(&module.bodies[index]));
    let mut extra_parameter = module.clone();
    let parameter = extra_parameter.bodies[index]
        .params
        .first()
        .ok_or("missing receiver parameter")?
        .clone();
    extra_parameter.bodies[index].params.push(parameter);
    assert!(!extra_parameter.body_has_canonical_direct_call_id(&extra_parameter.bodies[index]));
    let mut mutable = module.clone();
    mutable.bodies[index].locals[0].origin = bir::LocalOrigin::Receiver { mutable: true };
    mutable.bodies[index].params[0].mutable = true;
    assert!(!mutable.body_has_canonical_direct_call_id(&mutable.bodies[index]));
    let implementation = module
        .trait_implementations
        .iter()
        .find(|implementation| implementation.method.kind == SemanticSourceTargetKind::Property)
        .ok_or("missing property implementation")?;
    assert!(module.is_well_formed_trait_implementation(implementation));
    let mut mismatch = implementation.clone();
    mismatch.implementation.kind = SemanticSourceTargetKind::Method;
    assert!(!module.is_well_formed_trait_implementation(&mismatch));
    Ok(())
}

/// A trait-required nominal property retains its checked getter, receiver and canonical access target.
#[test]
fn nominal_property_reads_call_checked_getters() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/snapshots_models_and_classes/computed_properties.incn"
        ),
        &["m", "computed_properties"],
    )?;
    let getter = body_named(&module, "dollars")?;
    assert_eq!(getter.return_type, IncanType::Primitive(IncanPrimitiveType::Int));
    let canonical = getter.canonical.as_ref().ok_or("missing getter identity")?;
    assert_eq!(canonical.kind, SemanticSourceTargetKind::Property);
    let receiver = getter.locals.first().ok_or("missing getter receiver")?;
    assert!(receiver.identity.is_some());
    assert!(matches!(receiver.origin, bir::LocalOrigin::Receiver { mutable: false }));
    let caller = body_named(&module, "property_total")?;
    for body in &module.bodies {
        assert!(
            body.locals.iter().all(|local| local.ty != IncanType::Unknown),
            "{}",
            body.render_snapshot()
        );
    }
    assert!(
        caller.block.stmts.iter().any(|statement| matches!(
            &statement.kind,
            bir::StatementKind::Call { callee: bir::Callee::Method(target), .. }
                if target.canonical.as_ref() == Some(canonical)
        )),
        "{}",
        caller.render_snapshot()
    );
    let label = body_named(&module, "label")?
        .canonical
        .as_ref()
        .ok_or("missing label getter identity")?;
    assert!(
        module
            .trait_implementations
            .iter()
            .any(
                |implementation| implementation.method.kind == SemanticSourceTargetKind::Property
                    && &implementation.implementation == label
            )
    );
    Ok(())
}

/// Generic model and class getter bodies carry their checked owner frames and immutable receiver identities.
#[test]
fn generic_and_class_property_bodies_keep_owner_frames() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "model Box[T with Clone]:\n    pub content: T\n    pub property selected -> T:\n        return self.content\n\nclass Label:\n    pub value: int\n    pub property text -> str:\n        return f\"n={self.value}\"\n",
        &["m", "property_owner_frames"],
    )?;
    let generic = body_named(&module, "selected")?;
    assert_eq!(generic.return_type, IncanType::TypeVar("T".into()));
    assert!(generic.type_parameters.is_empty());
    let receiver = generic.locals.first().ok_or("missing generic getter receiver")?;
    assert!(matches!(&receiver.ty, IncanType::Generic { base, args }
        if base == "Box" && args == &[IncanType::TypeVar("T".into())]));
    assert!(receiver.identity.is_some());
    let class = body_named(&module, "text")?;
    assert_eq!(class.return_type, IncanType::Primitive(IncanPrimitiveType::Str));
    assert_eq!(
        class.canonical.as_ref().ok_or("missing class getter identity")?.kind,
        SemanticSourceTargetKind::Property
    );
    assert!(class.locals.first().is_some_and(|local| local.identity.is_some()));
    Ok(())
}

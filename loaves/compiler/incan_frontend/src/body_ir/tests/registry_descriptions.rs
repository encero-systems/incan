//! Checked method descriptions and once-only source-local registry initialization.

use super::*;

const SOURCE: &str =
    include_str!("../../../../incan_test_support/fixtures/behavior/snapshots_stdlib/std_registry_methods.incn");

/// Function descriptions execute after both original statics, under their checked mutable global receivers.
#[test]
fn function_descriptions_retain_ordered_module_effects() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!("../../../../incan_test_support/fixtures/behavior/snapshots_stdlib/std_registry_functions.incn"),
        &["main"],
    )?;
    assert_eq!(module.static_declarations.len(), 3, "{}", module.render_snapshot());
    let effects = module
        .static_declarations
        .last()
        .ok_or("missing registration effects")?;
    let bir::StaticInitializer::Computed(body) = &effects.initial else {
        return Err("registration effects need a once-only computed body".into());
    };
    assert!(
        effects.canonical.declaration_span.start < effects.canonical.declaration_span.end,
        "the synthetic effect must retain a physical source character"
    );
    let registrations = body
        .block
        .stmts
        .iter()
        .filter_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                args,
                ..
            } if target.name == "_describe_function" => Some((target, args)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(registrations.len(), 2, "{}", body.render_snapshot());
    for (registration, expected) in registrations.iter().zip(["functions", "targets"]) {
        assert!(registration.0.canonical.is_some());
        assert!(
            matches!(registration.1.first(), Some(bir::ArgumentElement::One(bir::Operand::Place(read)))
            if read.fact == bir::OwnershipFact::MutBorrow
                && read.place.global().is_some_and(|global| global.identity.declaration_name == expected))
        );
    }
    assert!(
        !body.render_snapshot().contains("unsupported("),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}

/// Corrupted checked description spans never grant registration effects to a same-spelled decorator.
#[test]
fn function_description_requires_checked_spans() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        include_str!("../../../../incan_test_support/fixtures/behavior/snapshots_stdlib/std_registry_functions.incn");
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let path = vec!["main".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut info = checker.type_info().clone();
    for description in &mut info.registry.descriptions {
        description.descriptor_span.0 += 1;
    }
    let module = build_body_ir_module_v0(&program, &path, &info);
    assert_eq!(module.static_declarations.len(), 2);
    assert!(body_named(&module, "main")?.render_snapshot().contains("unsupported("));
    Ok(())
}

/// Imported description facts cannot silently omit the loaded-module registration effect.
#[test]
fn imported_function_descriptions_remain_refused() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        include_str!("../../../../incan_test_support/fixtures/behavior/snapshots_stdlib/std_registry_functions.incn");
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let path = vec!["main".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut info = checker.type_info().clone();
    let entry = info
        .registry
        .descriptions
        .first_mut()
        .ok_or("missing function description")?;
    entry.registry = crate::typechecker::RegistryDescriptionRegistry::Imported {
        module_path: vec!["foreign".to_owned()],
        binding: "functions".to_owned(),
        public: true,
    };
    let module = build_body_ir_module_v0(&program, &path, &info);
    assert!(body_named(&module, "main")?.render_snapshot().contains("unsupported("));
    Ok(())
}

/// The invocation supplies the package name; both approved constructors retain exact identities and argument bytes.
#[test]
fn registry_subject_materializers_keep_compilation_context() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        include_str!("../../../../incan_test_support/fixtures/behavior/snapshots_stdlib/std_registry_subjects.incn");
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let path = vec!["main".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let module =
        build_body_ir_module_v0_with_registry_package(&program, &path, checker.type_info(), &[], "checked-package");
    for entry in &checker.type_info().registry.explicit_entries {
        let storage = module
            .static_declarations
            .iter()
            .find(|storage| storage.canonical.declaration_name == entry.entry_name)
            .ok_or("missing explicit entry storage")?;
        let bir::StaticInitializer::Computed(body) = &storage.initial else {
            return Err("expected explicit entry body".into());
        };
        let (target, arguments) = body
            .block
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                bir::StatementKind::Call {
                    callee: bir::Callee::Method(target),
                    args,
                    ..
                } if target.canonical.as_ref() == Some(&entry.entry_method_identity) => Some((target, args)),
                _ => None,
            })
            .ok_or("missing checked entry call")?;
        assert!(
            matches!(&target.binding, bir::ArgumentBinding::Resolved { arguments, defaulted_slots }
            if arguments.len() == 3 && defaulted_slots.is_empty())
        );
        assert!(
            matches!(arguments.first(), Some(bir::ArgumentElement::One(bir::Operand::Place(read)))
            if read.fact == bir::OwnershipFact::MutBorrow)
        );
        let args = body
            .block
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                bir::StatementKind::Call {
                    callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                    args,
                    ..
                } if target.canonical.as_ref() == Some(&entry.checked_constructor_identity) => Some(args),
                _ => None,
            })
            .ok_or_else(|| format!("missing checked subject: {}", body.render_snapshot()))?;
        let expected = if entry.subject_kind == incan_semantics_core::SemanticRegistrySubjectKind::Package {
            "checked-package"
        } else {
            "main"
        };
        assert_eq!(
            args,
            &[bir::ArgumentElement::One(bir::Operand::Constant(bir::Constant::Str(
                expected.to_owned()
            )))]
        );
    }
    Ok(())
}

/// A global enum field is read before the right operand, retaining a local value for tag comparison.
#[test]
fn registry_subject_enum_reads_are_evaluated_before_comparison() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!("../../../../incan_test_support/fixtures/behavior/snapshots_stdlib/std_registry_subjects.incn"),
        &["main"],
    )?;
    for statement in &body_named(&module, "main")?.block.stmts {
        if let bir::StatementKind::Assign {
            rvalue: bir::Rvalue::BinaryOp(_, left, right),
            ..
        } = &statement.kind
        {
            for operand in [left, right] {
                assert!(
                    !matches!(operand, bir::Operand::Place(read) if read.place.global().is_some()),
                    "{}",
                    module.render_snapshot()
                );
            }
        }
    }
    Ok(())
}

/// Explicit entries consume the approved constructor identity rather than the public placeholder.
#[test]
fn explicit_registry_subjects_retain_checked_materializers() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!("../../../../incan_test_support/fixtures/behavior/snapshots_stdlib/std_registry_subjects.incn"),
        &["main"],
    )?;
    let snapshot = module
        .static_declarations
        .iter()
        .filter_map(|storage| match &storage.initial {
            bir::StaticInitializer::Computed(body) => Some(body.render_snapshot()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(snapshot.contains("_checked_current_unit"), "{snapshot}");
    assert!(snapshot.contains("_checked_package"), "{snapshot}");
    Ok(())
}

/// A described method stays callable while its sole registry initializer retains the checked registration.
#[test]
fn described_method_registration_is_retained_once() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(SOURCE, &["main"])?;
    assert!(
        module
            .nominal_declarations
            .iter()
            .any(|owner| owner.name == "Normalizer"),
        "{}",
        module.render_snapshot()
    );
    let [storage] = module.static_declarations.as_slice() else {
        return Err("expected one registry static".into());
    };
    let bir::StaticInitializer::Computed(body) = &storage.initial else {
        return Err("expected computed registry initialization".into());
    };
    let registrations = body
        .block
        .stmts
        .iter()
        .filter_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                args,
                ..
            } if target.name == "_describe_method" => Some((target, args)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [(target, arguments)] = registrations.as_slice() else {
        return Err(format!("expected one retained registration: {}", body.render_snapshot()).into());
    };
    assert!(target.canonical.is_some());
    assert!(
        matches!(arguments.first(), Some(bir::ArgumentElement::One(bir::Operand::Place(receiver)))
        if receiver.fact == bir::OwnershipFact::MutBorrow)
    );
    assert_eq!(
        arguments.last(),
        Some(&bir::ArgumentElement::One(bir::Operand::Constant(bir::Constant::Str(
            "main.Normalizer.normalize".to_owned()
        ))))
    );
    assert!(
        !body_named(&module, "normalize")?
            .render_snapshot()
            .contains("unsupported(")
    );
    Ok(())
}

/// A checked registry declaration closes the storage, factory owner and initializer return consistently.
#[test]
fn registry_static_keeps_its_checked_declared_carrier() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(SOURCE, &["main"])?;
    let storage = module.static_declarations.first().ok_or("missing registry static")?;
    let expected = IncanType::Generic {
        base: "Registry".to_owned(),
        args: vec![
            IncanType::Named("MethodId".to_owned()),
            IncanType::Named("MethodSpec".to_owned()),
        ],
    };
    assert_eq!(storage.ty, expected);
    let bir::StaticInitializer::Computed(body) = &storage.initial else {
        return Err("expected computed registry initializer".into());
    };
    assert_eq!(body.return_type, expected);
    let factory = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                ..
            } if target.canonical.as_ref().is_some_and(|identity| {
                identity.declaration_name == "define" && identity.kind == SemanticSourceTargetKind::Method
            }) =>
            {
                Some(target)
            }
            _ => None,
        })
        .ok_or("missing checked registry factory")?;
    assert_eq!(factory.receiver_type.as_ref(), Some(&expected));
    Ok(())
}

/// A checked registry getter dispatches to its compiled SDK method under the same canonical identity.
#[test]
fn registry_getter_keeps_its_sdk_delegation() -> Result<(), Box<dyn std::error::Error>> {
    let tokens = lexer::lex(SOURCE).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let path = vec!["main".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let checked = checker.type_info();
    let module = build_body_ir_module_v0(&program, &path, checked);
    let target = body_named(&module, "main")?
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                ..
            } if target.name == "loaded_entries" => Some(target),
            _ => None,
        })
        .ok_or("missing registry getter")?;
    let calls = library_call_delegations(checked);
    assert!(
        calls
            .iter()
            .any(|call| Some(&call.canonical) == target.canonical.as_ref()),
        "target={target:?}; calls={calls:?}"
    );
    Ok(())
}

/// Another executable declaration decorator can observe registration order, so it excludes initializer fusion.
#[test]
fn registry_registration_refuses_other_declaration_initializers() -> Result<(), Box<dyn std::error::Error>> {
    let source = format!(
        "{SOURCE}\n\ndef observe(callback: () -> str) -> () -> str:\n    println(len(methods.loaded_entries()))\n    return callback\n\n@observe\ndef other() -> str:\n    return \"other\"\n"
    );
    let tokens = lexer::lex(&source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let path = vec!["main".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    assert!(
        !checker
            .type_info()
            .declarations
            .decorated_function_bindings_by_span
            .is_empty()
    );
    let module = build_body_ir_module_v0(&program, &path, checker.type_info());
    assert!(
        !module
            .nominal_declarations
            .iter()
            .any(|owner| owner.name == "Normalizer")
    );
    Ok(())
}

/// An attachment whose materialization span differs from checking cannot admit the nominal method.
#[test]
fn described_method_requires_exact_attachment_facts() -> Result<(), Box<dyn std::error::Error>> {
    let tokens = lexer::lex(SOURCE).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let path = vec!["main".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checked = checker.type_info().clone();
    let description = checked
        .registry
        .descriptions
        .first_mut()
        .ok_or("missing checked description")?;
    description.key_span.0 += 1;
    let module = build_body_ir_module_v0(&program, &path, &checked);
    assert!(
        !module
            .nominal_declarations
            .iter()
            .any(|owner| owner.name == "Normalizer")
    );
    Ok(())
}

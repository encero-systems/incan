//! Declaration-owned newtype payload projections and checked single-field model validation.

use super::build;

/// Checked ingress targets keep their exact owning carrier when the four original programs use project-aware lowering.
#[test]
fn original_newtype_ingress_retains_qualified_targets() -> Result<(), Box<dyn std::error::Error>> {
    let mut failures = Vec::new();
    for (name, source) in [
        (
            "static_factory_named_like_field",
            include_str!(
                "../../../../incan_test_support/fixtures/behavior/snapshots_functions_and_projections/static_factory_named_like_field.incn"
            ),
        ),
        (
            "validated_newtype_hook_and_coercion",
            include_str!(
                "../../../../incan_test_support/fixtures/behavior/snapshots_functions_and_projections/validated_newtype_hook_and_coercion.incn"
            ),
        ),
        (
            "constrained_newtype_validation",
            include_str!(
                "../../../../incan_test_support/fixtures/behavior/snapshots_newtypes_and_serde/constrained_newtype_validation.incn"
            ),
        ),
        (
            "newtype_implicit_coercion",
            include_str!(
                "../../../../incan_test_support/fixtures/behavior/snapshots_newtypes_and_serde/newtype_implicit_coercion.incn"
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
            let snapshot = body.render_snapshot();
            if snapshot.contains("validated-newtype coercion target") {
                failures.push(format!("{name}: {snapshot}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}

/// An equal source leaf in another module cannot replace a checked ingress step's actual local owner.
#[test]
fn qualified_ingress_refuses_a_foreign_owner_with_the_same_name() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_functions_and_projections/validated_newtype_hook_and_coercion.incn"
    );
    let tokens = super::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = super::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["m".to_owned(), "foreign_ingress_target".to_owned()];
    let mut checker = super::TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let original = super::build_body_ir_module_v0_with_executable_context(&program, &path, checker.type_info(), &[]);
    let mut facts = checker.type_info().clone();
    let mut changed = 0;
    for coercion in facts.expressions.validated_newtype_coercions.values_mut() {
        let Some(step) = coercion.steps.last() else {
            continue;
        };
        let owner = original
            .nominal_declarations
            .iter()
            .find(|owner| owner.name == step.newtype_name)
            .ok_or("checked ingress owner absent")?;
        let mut foreign = owner.canonical.clone();
        foreign.origin = incan_semantics_core::SymbolOrigin::Module(vec!["foreign".to_owned()]);
        let key = super::bir::nominal_carrier_key(&foreign).ok_or("foreign carrier absent")?;
        coercion.target_type = crate::symbols::ResolvedType::Named(key);
        changed += 1;
    }
    assert!(changed > 0);
    let module = super::build_body_ir_module_v0_with_executable_context(&program, &path, &facts, &[]);
    assert!(
        module
            .bodies
            .iter()
            .any(|body| body.render_snapshot().contains("validated-newtype coercion target"))
    );
    Ok(())
}

/// Retained conversion owners remain valid after their inner nominal type becomes an identity-derived carrier key.
#[test]
fn composed_conversion_owner_retains_valid_qualified_carrier() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import std.traits.convert as convert\n\ntype Port = newtype int\ntype WrappedPort = newtype Port\n\ndef main() -> None:\n    pass\n";
    let tokens = super::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = super::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["m".to_owned(), "conversion_carrier".to_owned()];
    let mut checker = super::TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let module = super::build_body_ir_module_v0_with_executable_context(&program, &path, checker.type_info(), &[]);
    let inner = module
        .nominal_declarations
        .iter()
        .find(|owner| owner.name == "Port")
        .ok_or("Port absent")?;
    let outer = module
        .nominal_declarations
        .iter()
        .find(|owner| owner.name == "WrappedPort")
        .ok_or("WrappedPort absent")?;
    let key = incan_semantics_core::body_ir::nominal_carrier_key(&inner.canonical).ok_or("carrier absent")?;
    assert_eq!(outer.field_types, [super::IncanType::Named(key.clone())]);
    assert_eq!(outer.named_type_identities.get(&key), Some(&inner.canonical));
    assert_eq!(
        outer.newtype_string_conversion,
        super::bir::NewtypeStringConversion::Automatic
    );
    assert!(module.is_well_formed_nominal_declaration(outer), "{outer:#?}");
    let mut changed = outer.clone();
    changed.named_type_identities.insert(key, outer.canonical.clone());
    assert!(!module.is_well_formed_nominal_declaration(&changed));
    Ok(())
}

/// The original public EntryId accessor retains its owning layout under project carrier qualification.
#[test]
fn package_newtype_payload_projection_keeps_its_owner() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/cli_dependencies/dependency_reexported_type_identity/deps/recall/src/ids.incn"
    );
    let tokens = super::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = super::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["ids".to_owned()];
    let mut checker = super::TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker.set_current_package_identity(Some("recall".to_owned()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    for project in [false, true] {
        let module = if project {
            super::build_body_ir_module_v0_with_executable_context(&program, &path, checker.type_info(), &[])
        } else {
            super::build_body_ir_module_v0(&program, &path, checker.type_info())
        };
        let declaration = module.nominal_declarations.first().ok_or("missing EntryId layout")?;
        let body = super::body_named(&module, "entry_id_text")?;
        let read = body
            .block
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                super::bir::StatementKind::Return {
                    value: Some(super::bir::Operand::Place(read)),
                } => Some(read),
                _ => None,
            })
            .ok_or("missing original wrapped-value return")?;
        assert!(
            matches!(read.place.projection.last(), Some(super::bir::PlaceElem::Field { canonical: Some(owner), .. }) if owner == &declaration.canonical),
            "project={project}: {:?}; layout={declaration:?}; body={}",
            read.place.projection,
            body.render_snapshot()
        );
        let public = std::collections::BTreeSet::from([
            declaration.canonical.clone(),
            body.canonical.clone().ok_or("missing public accessor identity")?,
        ]);
        let bytes = incan_semantics_core::executable_representation::build_surface(
            std::slice::from_ref(&module),
            "recall",
            "0.1.0",
            &public,
            &std::collections::BTreeSet::new(),
        )?;
        let reader = incan_semantics_core::executable_representation::SurfaceReader::open(&bytes)?;
        let identity = body.canonical.as_ref().ok_or("missing public accessor identity")?;
        assert!(
            reader.covers(identity),
            "project={project}: {:?}",
            reader.index().coverage(identity)?
        );
    }
    Ok(())
}

/// Missing layouts, stale declaration identities, absent checked owners, and conflicting name bindings cannot
/// authorize a qualified wrapped-value slot from its spelling.
#[test]
fn project_newtype_payload_rejects_missing_or_conflicting_layouts() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/cli_dependencies/dependency_reexported_type_identity/deps/recall/src/ids.incn"
    );
    let tokens = super::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = super::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["ids".to_owned()];
    let mut checker = super::TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker.set_current_package_identity(Some("recall".to_owned()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let owner = checker
        .type_info()
        .declarations
        .named_type_identities
        .get("EntryId")
        .cloned()
        .ok_or("missing checked EntryId owner")?;
    let span = checker
        .type_info()
        .declarations
        .declaration_identities
        .iter()
        .find_map(|(span, identity)| (identity == &owner).then_some(*span))
        .ok_or("missing original declaration span")?;
    for mutation in 0..4 {
        let mut facts = checker.type_info().clone();
        match mutation {
            0 => {
                facts
                    .declarations
                    .declaration_identities
                    .remove(&span)
                    .ok_or("missing original declaration identity")?;
            }
            1 => {
                let mut conflicting = owner.clone();
                conflicting.declaration_name = "OtherId".to_owned();
                facts.declarations.declaration_identities.insert(span, conflicting);
            }
            2 => {
                facts
                    .declarations
                    .named_type_identities
                    .remove("EntryId")
                    .ok_or("missing original checked owner")?;
            }
            3 => {
                let key = super::bir::nominal_carrier_key(&owner).ok_or("missing original owner key")?;
                let mut conflicting = owner.clone();
                conflicting.declaration_name = "OtherId".to_owned();
                facts.declarations.named_type_identities.insert(key, conflicting);
            }
            _ => return Err("unknown owner mutation".into()),
        }
        let module = super::build_body_ir_module_v0_with_executable_context(&program, &path, &facts, &[]);
        let body = super::body_named(&module, "entry_id_text")?;
        let read = body
            .block
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                super::bir::StatementKind::Return {
                    value: Some(super::bir::Operand::Place(read)),
                } => Some(read),
                _ => None,
            })
            .ok_or("missing original payload read")?;
        assert!(
            matches!(read.place.projection.last(), Some(super::bir::PlaceElem::Field { name, canonical: None, structural: false }) if name == "0"),
            "mutation={mutation}: {:?}; body={}",
            read.place.projection,
            body.render_snapshot()
        );
    }
    Ok(())
}

/// Both original generic payload reads retain their checked owner and lexical type parameter in project mode.
#[test]
fn project_generic_newtype_payloads_keep_the_checked_owner() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_newtypes_and_serde/newtype_generic.incn"
    );
    let tokens = super::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = super::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["generic_payloads".to_owned()];
    let mut checker = super::TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker.set_current_package_identity(Some("holders".to_owned()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    for project in [false, true] {
        let module = if project {
            super::build_body_ir_module_v0_with_executable_context(&program, &path, checker.type_info(), &[])
        } else {
            super::build_body_ir_module_v0(&program, &path, checker.type_info())
        };
        let declaration = module.nominal_declarations.first().ok_or("missing Box layout")?;
        assert_eq!(declaration.type_parameters, ["T"]);
        let body = super::body_named(&module, "duplicate")?;
        let mut reads = 0;
        for statement in &body.block.stmts {
            if let super::bir::StatementKind::Assign {
                rvalue: super::bir::Rvalue::Aggregate(super::bir::AggregateKind::Tuple, values),
                ..
            } = &statement.kind
            {
                for value in values {
                    if let super::bir::ArgumentElement::One(super::bir::Operand::Place(read)) = value {
                        assert!(
                            matches!(read.place.projection.last(), Some(super::bir::PlaceElem::Field { canonical: Some(owner), .. }) if owner == &declaration.canonical),
                            "project={project}: {:?}; body={}",
                            read.place.projection,
                            body.render_snapshot()
                        );
                        reads += 1;
                    }
                }
            }
        }
        assert_eq!(reads, 2, "project={project}: {}", body.render_snapshot());
    }
    Ok(())
}

/// A sole supplied model field uses a distinct aggregate extraction, preserving fail-fast callers and locals.
#[test]
fn single_field_newtype_ingress_retains_aggregate_extraction() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/snapshots_newtypes_and_serde/newtype_implicit_coercion.incn"
        ),
        &["m", "single_field_ingress"],
    )?;
    let main = super::body_named(&module, "main")?;
    let snapshot = main.render_snapshot();
    assert!(snapshot.contains("newtype_field_hook_extract"), "{snapshot}");
    assert!(snapshot.contains("newtype_hook_extract"), "{snapshot}");
    assert!(!snapshot.contains("unsupported("), "{snapshot}");
    Ok(())
}

/// A constructor with two checked fields still refuses until supplied-field collection is represented.
#[test]
fn multiple_checked_model_fields_remain_refused() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"type Positive = newtype int:
    def from_underlying(value: int) -> Result[Self, ValidationError]:
        return Ok(Positive(value))

model Pair:
    left: Positive
    right: Positive

def main() -> None:
    pair = Pair(left=1, right=2)
    println(pair.left.0)
"#,
        &["m", "multiple_checked_fields"],
    )?;
    let main = super::body_named(&module, "main")?;
    let snapshot = main.render_snapshot();
    assert!(
        snapshot.contains("checked-newtype aggregate-field validation"),
        "{snapshot}"
    );
    assert!(!snapshot.contains("newtype_field_hook_extract"), "{snapshot}");
    Ok(())
}

/// A checked coercion alone cannot authorize field extraction after its canonical model layout is missing.
#[test]
fn single_field_newtype_ingress_requires_canonical_field_layout() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_newtypes_and_serde/newtype_implicit_coercion.incn"
    );
    let tokens = super::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = super::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["m".to_owned(), "missing_checked_field".to_owned()];
    let mut checker = super::TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let mut facts = checker.type_info().clone();
    assert!(!facts.declarations.member_declaration_identities.is_empty());
    facts.declarations.member_declaration_identities.clear();
    let module = super::build_body_ir_module_v0(&program, &path, &facts);
    let snapshot = super::body_named(&module, "main")?.render_snapshot();
    assert!(
        snapshot.contains("checked-newtype aggregate-field validation"),
        "{snapshot}"
    );
    assert!(!snapshot.contains("newtype_field_hook_extract"), "{snapshot}");
    Ok(())
}

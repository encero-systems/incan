//! Declaration-bound newtype layouts, generic method frames, and selected trait implementations.

use super::{IncanType, build};

/// A generic tuple wrapper retains the declaring binder as a placeholder authorized by its canonical layout.
#[test]
fn generic_newtype_layout_retains_its_owner_parameter() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"type Box[T] = newtype T:
    @staticmethod
    def wrap(value: T) -> Self:
        return Box(value)

    def duplicate(self) -> Tuple[T, T]:
        return (self.0, self.0)

def main() -> None:
    println(Box.wrap(42).duplicate()[0])
"#,
        &["m", "newtype_parameters"],
    )?;
    let [declaration] = module.nominal_declarations.as_slice() else {
        return Err("expected one retained newtype layout".into());
    };
    assert_eq!(declaration.type_parameters, ["T"]);
    assert_eq!(declaration.type_parameter_count, 1);
    assert_eq!(declaration.fields, ["0"]);
    assert_eq!(declaration.field_types, [IncanType::TypeVar("T".into())]);
    assert!(module.is_well_formed_nominal_declaration(declaration));
    Ok(())
}

/// Same-named methods targeting distinct traits keep both selected implementations on a raw newtype.
#[test]
fn newtype_trait_targets_retain_distinct_implementations() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/snapshots_traits_and_generics/newtype_trait_targets.incn"
        ),
        &["m", "newtype_trait_targets"],
    )?;
    assert_eq!(module.nominal_declarations.len(), 1);
    assert_eq!(module.trait_implementations.len(), 2);
    assert_ne!(
        module.trait_implementations[0].implementation,
        module.trait_implementations[1].implementation
    );
    Ok(())
}

/// A generic validation hook stays refused until its declaration-bound checked ingress can be specialized.
#[test]
fn generic_checked_newtypes_remain_outside_raw_layout_admission() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"type Checked[T] = newtype T:
    def from_underlying(value: T) -> Result[Self, ValidationError]:
        return Ok(Checked(value))

def main() -> None:
    pass
"#,
        &["m", "checked_newtype_boundary"],
    )?;
    assert!(module.nominal_declarations.is_empty());
    Ok(())
}
/// Accepted constructor and method selections retain different closed owner arguments at each use.
#[test]
fn generic_newtype_uses_retain_closed_owner_types() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/snapshots_newtypes_and_serde/newtype_generic.incn"
        ),
        &["m", "closed_newtype_uses"],
    )?;
    let main = super::body_named(&module, "main")?;
    let snapshot = main.render_snapshot();
    for name in ["b", "boxed"] {
        let local = main
            .locals
            .iter()
            .find(|local| local.name.as_deref() == Some(name))
            .ok_or("expected original fixture binding")?;
        assert_eq!(local.ty.to_string(), "Box[int]", "{name}: {snapshot}");
    }
    let pair = main
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("pair"))
        .ok_or("expected duplicate payload binding")?;
    assert!(
        matches!(&pair.ty, IncanType::Tuple(elements) if elements.len() == 2 && elements.iter().all(|element| element.to_string() == "int")),
        "{snapshot}"
    );
    assert!(snapshot.contains("Box[int]"), "{snapshot}");
    assert!(snapshot.contains("Box[str]"), "{snapshot}");
    assert!(!snapshot.contains("Box[Unknown]"), "{snapshot}");
    assert!(!snapshot.contains("unsupported("), "{snapshot}");
    for body in &module.bodies {
        assert!(
            !body.render_snapshot().contains("unsupported("),
            "{}",
            body.render_snapshot()
        );
    }
    Ok(())
}

/// Damaged constructor, method and storage authority invalidates the entire source-local refinement transaction.
#[test]
fn generic_newtype_uses_refuse_changed_checked_facts() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_newtypes_and_serde/newtype_generic.incn"
    );
    let tokens = super::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = super::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["m".to_owned(), "native_newtype_guards".to_owned()];
    let mut checker = super::TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    for mutation in 0..8 {
        let mut facts = checker.type_info().clone();
        match mutation {
            0 => facts.calls.constructor_field_bindings.clear(),
            1 => facts.references.resolved_write_types.clear(),
            2 => facts.references.resolved_identities.clear(),
            3 => facts.declarations.newtype_construction.clear(),
            4 => facts.declarations.method_bindings_by_span.clear(),
            5 => {
                let (key, binding) = facts
                    .declarations
                    .method_bindings_by_span
                    .iter()
                    .find(|(_, binding)| {
                        binding
                            .identity
                            .as_ref()
                            .is_some_and(|identity| identity.declaration_name == "wrap")
                    })
                    .map(|(key, binding)| (*key, binding.clone()))
                    .ok_or("expected checked wrap method binding")?;
                facts
                    .declarations
                    .method_bindings_by_span
                    .insert((key.0 + 1, key.1 + 1), binding);
            }
            6 => facts.declarations.named_type_identities.clear(),
            7 => facts.declarations.declaration_identities.clear(),
            _ => return Err("unknown guard mutation".into()),
        }
        let module = super::build_body_ir_module_v0(&program, &path, &facts);
        let main = super::body_named(&module, "main")?;
        let snapshot = main.render_snapshot();
        let boxed = main
            .locals
            .iter()
            .find(|local| local.name.as_deref() == Some("boxed"))
            .ok_or("expected guarded storage binding")?;
        assert!(
            !matches!(&boxed.ty, IncanType::Generic { base, args }
                if base == "Box" && matches!(args.as_slice(), [IncanType::Primitive(_)])),
            "mutation {mutation}: {snapshot}"
        );
    }
    Ok(())
}

/// A conflicting later write cannot preserve a previously closed owner instance in the same storage slot.
#[test]
fn generic_newtype_conflicting_storage_stays_refused() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "type Box[T] = newtype T\n\ndef main() -> None:\n    mut value = Box(1)\n    println(value.0)\n    value = Box(\"text\")\n    println(value.0)\n",
        &["m", "native_newtype_conflict"],
    )?;
    let main = super::body_named(&module, "main")?;
    let value = main
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("value"))
        .ok_or("expected conflicting storage binding")?;
    assert!(
        matches!(&value.ty, IncanType::Generic { base, args }
        if base == "Box" && matches!(args.as_slice(), [IncanType::Unknown])),
        "{}",
        main.render_snapshot()
    );
    Ok(())
}

/// A shared field identity cannot replace a missing atomic generic payload proof, even if owner metadata is absent.
#[test]
fn generic_newtype_payload_does_not_restore_shared_field_proof() -> Result<(), Box<dyn std::error::Error>> {
    let source = "type Box[T] = newtype T\n\ndef read(value: Box[int]) -> None:\n    payload = value.0\n\ndef main() -> None:\n    pass\n";
    let tokens = super::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = super::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["m".to_owned(), "native_newtype_payload_guard".to_owned()];
    let mut checker = super::TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let start = source.find("value.0").ok_or("expected payload projection")?;
    let owner = checker
        .type_info()
        .declarations
        .named_type_identities
        .get("Box")
        .cloned()
        .ok_or("expected canonical Box owner")?;
    for remove_owner in [false, true] {
        let mut facts = checker.type_info().clone();
        facts
            .references
            .resolved_identities
            .insert((start, start + "value.0".len()), owner.clone());
        if remove_owner {
            assert!(facts.declarations.named_type_identities.remove("Box").is_some());
        }
        let module = super::build_body_ir_module_v0(&program, &path, &facts);
        let body = super::body_named(&module, "read")?;
        let mut projections = 0;
        for statement in &body.block.stmts {
            if let super::bir::StatementKind::Assign {
                rvalue: super::bir::Rvalue::Use(super::bir::Operand::Place(read)),
                ..
            } = &statement.kind
            {
                for projection in &read.place.projection {
                    if let super::bir::PlaceElem::Field {
                        name,
                        canonical,
                        structural,
                    } = projection
                        && name == "0"
                    {
                        assert!(canonical.is_none(), "{projection:?}");
                        assert!(!structural, "{projection:?}");
                        projections += 1;
                    }
                }
            }
        }
        assert_eq!(projections, 1, "{}", body.render_snapshot());
    }
    Ok(())
}

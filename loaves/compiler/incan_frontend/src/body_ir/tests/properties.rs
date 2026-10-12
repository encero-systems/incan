//! Checked property getter bodies and reads for direct execution.

use super::{IncanPrimitiveType, IncanType, SemanticSourceTargetKind, bir, body_named, build};

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

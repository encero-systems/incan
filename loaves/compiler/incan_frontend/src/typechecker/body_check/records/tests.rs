//! Tests of restoring a checked body: positions, restored identities and the members a restore binds.

use super::*;

/// A restored body can name a field of a sibling module's model that it reaches only through a call's return
/// type (`outcome = check_program(...)`, then `outcome.code`), with no lexical import of the model. When the
/// restore positions do not bind that field, restoring its reference must refuse or bind the field's own
/// declaration; it must never mint the field at the model's span, which lowering cannot find.
#[test]
fn sibling_member_reached_through_a_return_type_is_never_minted_at_its_model() -> Result<(), Box<dyn std::error::Error>>
{
    let (program, checker, owner) = fixture("def view() -> int:\n    return 1\n", "view")?;
    let mut positions = Positions::new(&checker, &program, owner, false)?;
    let origin = incan_semantics_core::SymbolOrigin::Module(vec!["engine_programs".into()]);
    let model = CanonicalSymbolId {
        namespace: SymbolNamespace::OrdinaryLexical,
        origin: origin.clone(),
        declaration_name: "CheckOutcome".into(),
        kind: SemanticSourceTargetKind::Model,
        scope_discriminant: None,
        declaration_span: Span::new(5619, 6111).into(),
    };
    let field = CanonicalSymbolId {
        namespace: SymbolNamespace::Member,
        origin,
        declaration_name: "code".into(),
        kind: SemanticSourceTargetKind::Field,
        scope_discriminant: None,
        declaration_span: Span::new(5746, 5759).into(),
    };
    let model_key = StableDeclarationId::from_canonical(&model, None, None)
        .ok_or("missing model identity")?
        .render_compact();
    positions.type_identities.insert(model_key.clone(), model.clone());
    let member_key = stable_member_identity(&field, &model, 0)?;
    let row = [member_key.clone(), model_key, "0".into(), "field".into(), "code".into()];
    let mut local = BodyLocal {
        owner,
        ..Default::default()
    };
    let mut identities = BTreeMap::new();
    if restore_identity_context(&positions, &mut local, &mut identities, owner, &mut row.iter()).is_ok() {
        assert_eq!(
            identities.get(&member_key),
            Some(&field),
            "the restored field was minted at its model's span"
        );
    }
    Ok(())
}

/// A member of a nominal of the body's own module that the restore positions do not bind is compiler generated,
/// such as a string enum's `from_value`; its identity is the nominal's own, and the restore keeps it.
#[test]
fn own_module_generated_member_restores_at_its_nominal() -> Result<(), Box<dyn std::error::Error>> {
    let (program, checker, owner) = fixture("def view() -> int:\n    return 1\n", "view")?;
    let mut positions = Positions::new(&checker, &program, owner, false)?;
    let nominal = CanonicalSymbolId {
        namespace: SymbolNamespace::OrdinaryLexical,
        origin: checker.symbols.declaration_origin(),
        declaration_name: "GraphRecordKind".into(),
        kind: SemanticSourceTargetKind::Enum,
        scope_discriminant: None,
        declaration_span: Span::new(40, 120).into(),
    };
    let generated = nominal.generated_member("from_value", SemanticSourceTargetKind::Method);
    let nominal_key = StableDeclarationId::from_canonical(&nominal, None, None)
        .ok_or("missing enum identity")?
        .render_compact();
    positions.type_identities.insert(nominal_key.clone(), nominal.clone());
    let member_key = stable_member_identity(&generated, &nominal, 0)?;
    let row = [
        member_key.clone(),
        nominal_key,
        "0".into(),
        "method".into(),
        "from_value".into(),
    ];
    let mut local = BodyLocal {
        owner,
        ..Default::default()
    };
    let mut identities = BTreeMap::new();
    restore_identity_context(&positions, &mut local, &mut identities, owner, &mut row.iter())?;
    assert_eq!(identities.get(&member_key), Some(&generated));
    Ok(())
}

#[test]
fn physical_inputs_ignore_duplicate_nominal_and_member_bindings() -> Result<(), Box<dyn std::error::Error>> {
    let (program, checker, owner) = fixture(
        "model Record:\n    value: int\n\ndef take(item: Record) -> int:\n    return item.value\n",
        "take",
    )?;
    let local = &checker.checked_body(owner).ok_or("missing checked body")?.local;
    let records = [
        checker
            .available_declaration_record(owner)
            .ok_or("missing declaration record")?,
        checker.available_local_record(owner).ok_or("missing local record")?,
        checker.available_final_record(owner).ok_or("missing final record")?,
    ];
    let mut positions = Positions::for_local(&checker, &program, owner, local)?;
    let (&here, context) = local
        .references
        .stable_declaration_contexts
        .iter()
        .next()
        .ok_or("missing checked member context")?;
    let member = local
        .references
        .resolved_identities
        .get(&here)
        .ok_or("missing checked member")?;
    let member_key = positions.contextual_identity(member, context)?;
    let nominal_key = StableDeclarationId::from_canonical(&context.owner, None, None)
        .ok_or("missing nominal identity")?
        .render_compact();
    assert_eq!(positions.type_identities.get(&nominal_key), Some(&context.owner));

    positions.identities.remove(&nominal_key);
    positions.identities.remove(&member_key);
    let before = positions
        .physical_inputs_in(checker.type_info(), local, records, None)
        .ok_or("missing witness")?;
    for (key, identity) in [(nominal_key, &context.owner), (member_key, member)] {
        positions.identities.insert(key.clone(), identity.clone());
        let duplicate = positions.physical_inputs_in(checker.type_info(), local, records, None);
        assert_eq!(duplicate, Some(before), "{key}");
        positions
            .identities
            .get_mut(&key)
            .ok_or("missing duplicate")?
            .declaration_span
            .start += 1;
        let conflict = positions.physical_inputs_in(checker.type_info(), local, records, None);
        assert!(conflict.is_some());
        assert_ne!(conflict, Some(before), "{key}");
        positions.identities.remove(&key);
    }
    Ok(())
}

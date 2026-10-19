//! Source comparison slots retain the native equality ABI without changing ordinary method parameters.

use super::{IncanType, build};

/// A canonical carrier keeps the selected Eq ABI; a conflicting checked binding cannot borrow another owner's proof.
#[test]
fn equality_carrier_keys_require_the_exact_checked_owner() -> Result<(), Box<dyn std::error::Error>> {
    let original = build(
        "from std.derives.comparison import Eq\n\nmodel Key with Eq:\n    value: int\n    def __eq__(self, other: Key) -> bool:\n        return self.value == other.value\n",
        &["m", "canonical_comparison"],
    )?;
    let owner = original
        .nominal_declarations
        .first()
        .ok_or("missing Key")?
        .canonical
        .clone();
    let key = incan_semantics_core::body_ir::nominal_carrier_key(&owner).ok_or("missing nominal carrier key")?;
    for foreign_binding in [false, true] {
        let mut module = original.clone();
        module.nominal_declarations[0].comparison_equality = None;
        let body = module
            .bodies
            .iter_mut()
            .find(|body| body.name == "__eq__")
            .ok_or("missing Eq body")?;
        let parameter = body.params.get_mut(1).ok_or("missing other operand")?;
        let owned = IncanType::Named(key.clone());
        parameter.ty = owned.clone();
        body.locals
            .get_mut(parameter.local.index())
            .ok_or("missing parameter local")?
            .ty = owned.clone();
        let mut binding = owner.clone();
        if foreign_binding {
            binding.declaration_name.push_str("_foreign");
        }
        body.named_type_identities.insert(key.clone(), binding);
        super::super::source_comparison::retain_equality_borrows(&mut module);
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == "__eq__")
            .ok_or("missing Eq body")?;
        let expected = if foreign_binding {
            owned
        } else {
            IncanType::Ref(Box::new(owned))
        };
        assert_eq!(body.params[1].ty, expected);
        assert_eq!(
            module.nominal_declarations[0].comparison_equality.is_some(),
            !foreign_binding
        );
    }
    Ok(())
}

/// A checked enum equality body retains the same borrowed standard ABI without inventing another declaration fact.
#[test]
fn enum_equality_retains_checked_borrowed_abi() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/lowering_declarations/comparison_trait_adopters_of_every_kind.incn"
        ),
        &["m", "enum_comparison_abi"],
    )?;
    let owner = module
        .enum_declarations
        .iter()
        .find(|owner| owner.name == "Level")
        .ok_or("missing Level")?;
    let record = module
        .trait_implementations
        .iter()
        .find(|record| {
            record.owner == owner.canonical
                && super::super::source_comparison::is_standard_slot(&record.method, "__eq__")
        })
        .ok_or("missing checked enum equality slot")?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.canonical.as_ref() == Some(&record.implementation))
        .ok_or("missing enum equality body")?;
    let parameter = body.params.get(1).ok_or("missing enum other operand")?;
    assert_eq!(parameter.ty, IncanType::Ref(Box::new(IncanType::Named("Level".into()))));
    assert_eq!(body.locals[parameter.local.index()].ty, parameter.ty);
    Ok(())
}

/// Checked standard equality on retained class and newtype owners carries the same borrowed other operand as models.
#[test]
fn class_and_newtype_equality_retain_checked_abi() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/lowering_declarations/comparison_trait_adopters_of_every_kind.incn"
        ),
        &["m", "comparison_owner_kinds"],
    )?;
    for name in ["Rank", "Account"] {
        let owner = module
            .nominal_declarations
            .iter()
            .find(|owner| owner.name == name)
            .ok_or("missing owner")?;
        let implementation = module
            .trait_implementations
            .iter()
            .find(|record| {
                record.owner == owner.canonical
                    && super::super::source_comparison::is_standard_slot(&record.method, "__eq__")
            })
            .ok_or_else(|| format!("missing checked equality implementation for {name}"))?;
        let body = module
            .bodies
            .iter()
            .find(|body| body.canonical.as_ref() == Some(&implementation.implementation))
            .ok_or("missing selected equality body")?;
        assert_eq!(
            owner.comparison_equality.as_ref(),
            Some(&implementation.implementation),
            "{name}: {:?}",
            body.params
        );
        let parameter = body.params.get(1).ok_or("missing other operand")?;
        assert_eq!(parameter.ty, IncanType::Ref(Box::new(IncanType::Named(name.into()))));
        assert_eq!(body.locals[parameter.local.index()].ty, parameter.ty);
        assert!(
            module.is_well_formed_nominal_declaration(owner),
            "{name}: the retained comparison witness must pass kernel identity validation"
        );
    }
    let tag = module
        .nominal_declarations
        .iter()
        .find(|owner| owner.name == "Tag")
        .ok_or("missing Tag")?;
    assert!(
        tag.comparison_equality.is_none(),
        "a source Same slot is not standard Eq"
    );
    Ok(())
}

/// A checked standard Eq implementation borrows its other operand, as legacy's native PartialEq body does.
#[test]
fn standard_equality_body_retains_borrowed_other() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std.derives.comparison import Eq\n\nmodel Key with Eq:\n    value: int\n    def __eq__(self, other: Key) -> bool:\n        return self.value == other.value\n\ndef main() -> None:\n    println(Key(value=1) == Key(value=1))\n",
        &["m", "source_comparison"],
    )?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "__eq__")
        .ok_or("missing equality body")?;
    assert_eq!(body.params.len(), 2);
    assert_eq!(
        body.params[1].ty,
        IncanType::Ref(Box::new(IncanType::Named("Key".into())))
    );
    assert_eq!(body.locals[body.params[1].local.index()].ty, body.params[1].ty);
    let owner = module
        .nominal_declarations
        .iter()
        .find(|owner| owner.name == "Key")
        .ok_or("missing Key")?;
    assert_eq!(
        serde_json::to_value(owner)?["comparison_equality"],
        serde_json::to_value(&body.canonical)?
    );
    Ok(())
}

/// An ordinary same-spelled method grants no checked standard comparison ABI.
#[test]
fn ordinary_equality_spelling_keeps_owned_parameter() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "model Key:\n    value: int\n    def __eq__(self, other: Key) -> bool:\n        return self.value == other.value\n\ndef main() -> None:\n    println(Key(value=1).__eq__(Key(value=1)))\n",
        &["m", "ordinary_comparison"],
    )?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "__eq__")
        .ok_or("missing ordinary member")?;
    assert_eq!(body.params[1].ty, IncanType::Named("Key".into()));
    Ok(())
}

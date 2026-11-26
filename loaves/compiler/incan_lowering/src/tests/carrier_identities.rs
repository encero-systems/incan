//! A keyed product names a standard carrier by the checked type it lays out, never by its admission position.

use incan_mir_plan_native::record_types::{CheckedPrimitiveType, CheckedType};
use incan_mir_plan_native::{Carrier, EnumDeclaration, EnumVariant, PlanType, source_span};

use crate::nominal_records::carrier_nominal;

/// An admitted `Option` carrier at the native name its admission position gives it.
fn option_carrier(name: &str, payload: CheckedType, field: PlanType) -> EnumDeclaration {
    let variants = vec![
        EnumVariant {
            name: "None".into(),
            fields: Vec::new(),
        },
        EnumVariant {
            name: "Some".into(),
            fields: vec![field],
        },
    ];
    let mut declaration = EnumDeclaration::new(
        name.into(),
        false,
        variants,
        vec!["Clone".into()],
        source_span("carriers.incn", 1, 1),
    );
    declaration.carrier = Carrier::Option;
    declaration.semantic_key = Some(CheckedType::Generic("Option".into(), vec![payload]));
    declaration
}

/// Another module's carrier, admitted first, renumbers this one; its identity and digest stay the same.
#[test]
fn a_carrier_keeps_its_identity_when_another_module_admits_one_first() {
    let integer = CheckedType::Primitive(CheckedPrimitiveType::Int);
    let before = carrier_nominal(&option_carrier("__IncanCarrier0", integer.clone(), PlanType::Int));
    let after = carrier_nominal(&option_carrier("__IncanCarrier1", integer, PlanType::Int));
    assert!(
        !before.identity.is_empty(),
        "an Option carrier with a checked type is named"
    );
    assert_eq!(
        before.identity, after.identity,
        "a carrier's identity must not follow its admission position"
    );
    assert_eq!(before.digest, after.digest);
}

/// `Option[int]` and `Option[str]` at the same native name are different declarations to a product.
#[test]
fn carriers_of_one_shape_but_different_payloads_are_distinct() {
    let integer = carrier_nominal(&option_carrier(
        "__IncanCarrier0",
        CheckedType::Primitive(CheckedPrimitiveType::Int),
        PlanType::Int,
    ));
    let text = carrier_nominal(&option_carrier(
        "__IncanCarrier0",
        CheckedType::Primitive(CheckedPrimitiveType::Str),
        PlanType::String,
    ));
    assert_ne!(
        integer.identity, text.identity,
        "a product must not bind Option[int] to Option[str]"
    );
}

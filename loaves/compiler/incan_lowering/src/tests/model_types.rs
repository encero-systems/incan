//! A checked model type resolves only through one authenticated nominal owner, or one unique legacy spelling.

use std::collections::BTreeMap;
use std::sync::Arc;

use incan_mir_plan_native::{Carrier, EnumDeclaration, ListLeaf, PlanType};
use incan_semantics_core::body_ir::{
    BodyIrModule, NewtypeDeserialize, NewtypeStringConversion, NominalDeclaration, native_enum_declaration,
    native_opaque_nominal, nominal_carrier_key, rust_item_identity,
};
use incan_semantics_core::{
    CanonicalSymbolId, CompilerNodeId, HirSourceSpan, IncanType, SemanticSourceTargetKind, SymbolNamespace,
    SymbolOrigin, generic,
};

use incan_semantics_core::semantic_digest::{enum_layout_witness, model_layout_witness};

use crate::models::model_type;
use crate::native_type_identity::{argument_identities, constructor_index};
use crate::nominal_instances::{NominalInstance, close_layout};
use crate::options::retain_carrier;
use crate::symbols::model_symbol;

fn identity(name: &str) -> CanonicalSymbolId {
    CanonicalSymbolId::module_declaration(
        vec!["app".into()],
        name,
        SemanticSourceTargetKind::Model,
        HirSourceSpan::new(0, 10),
    )
}

/// A package-owned model identity whose carrier key differs from its source declaration name.
fn imported_identity(name: &str) -> CanonicalSymbolId {
    CanonicalSymbolId {
        namespace: SymbolNamespace::OrdinaryLexical,
        origin: SymbolOrigin::Package {
            library: "internal_models".into(),
            module_path: vec!["compiler_records".into()],
        },
        declaration_name: name.into(),
        kind: SemanticSourceTargetKind::Model,
        scope_discriminant: None,
        declaration_span: HirSourceSpan::new(0, 10),
    }
}

fn carrier_key(owner: &CanonicalSymbolId) -> Result<String, String> {
    nominal_carrier_key(owner).ok_or_else(|| "test identity has no nominal carrier key".to_owned())
}

/// One complete fieldless source model exposing only the identity facts each control varies.
pub(super) fn nominal(
    owner: CanonicalSymbolId,
    name: &str,
    identities: &[(&str, &CanonicalSymbolId)],
) -> NominalDeclaration {
    let named: BTreeMap<String, CanonicalSymbolId> = identities
        .iter()
        .map(|(key, identity)| ((*key).to_owned(), (*identity).clone()))
        .collect();
    NominalDeclaration {
        record_package_identity: None,
        trait_adoptions: Vec::new(),
        native_item: String::new(),
        class_inheritance: None,
        native_type_arguments: Vec::new(),
        direct_declaration_id: CompilerNodeId::declaration_span("app", 0, 10),
        canonical: owner,
        name: name.into(),
        fields: Vec::new(),
        field_identities: Vec::new(),
        field_types: Vec::new(),
        owned_callable_fields: Vec::new(),
        field_public: Vec::new(),
        field_aliases: Vec::new(),
        reflection_fields: None,
        reflection_metadata: None,
        comparison_equality: None,
        public: false,
        has_field_defaults: false,
        field_default_body: None,
        validated_constructor: None,
        method_partial_defaults: Vec::new(),
        display_method: None,
        newtype_deserialize: NewtypeDeserialize::Unselected,
        newtype_string_conversion: NewtypeStringConversion::Unselected,
        derives: Vec::new(),
        named_type_identities: Arc::new(named),
        type_parameters: Vec::new(),
        type_parameter_count: 0,
    }
}

fn resolved(declarations: &[NominalDeclaration], name: &str) -> Result<PlanType, String> {
    model_type(
        &constructor_index(declarations),
        declarations,
        &IncanType::Named(name.into()),
        "",
    )
}

/// Resolve an imported carrier by its authenticated owner while preserving unique legacy source-name lookup.
#[test]
fn model_type_selects_exact_imported_carrier_and_legacy_name() -> Result<(), String> {
    let owner = imported_identity("CanonicalSymbolId");
    let carrier = carrier_key(&owner)?;
    let target = nominal(owner.clone(), "CanonicalSymbolId", &[(&carrier, &owner)]);
    let decoy_owner = identity("Shadow");
    let decoy = nominal(decoy_owner.clone(), &carrier, &[(&carrier, &decoy_owner)]);
    let before = vec![decoy.clone(), target.clone()];
    assert!(resolved(&before, &carrier)? == PlanType::Model(1, model_symbol("CanonicalSymbolId")));
    let after = vec![target, decoy];
    assert!(resolved(&after, &carrier)? == PlanType::Model(0, model_symbol("CanonicalSymbolId")));
    let legacy = vec![nominal(identity("LegacyPacket"), "LegacyPacket", &[])];
    assert!(resolved(&legacy, "LegacyPacket")? == PlanType::Model(0, model_symbol("LegacyPacket")));
    Ok(())
}

/// Refuse carrier aliases without one closed self-bound owner and legacy spellings with multiple declarations.
#[test]
fn model_type_refuses_unauthenticated_or_ambiguous_nominal_owners() -> Result<(), String> {
    let owner = imported_identity("CanonicalSymbolId");
    let carrier = carrier_key(&owner)?;
    let missing = vec![nominal(owner.clone(), "CanonicalSymbolId", &[])];
    assert!(resolved(&missing, &carrier).is_err());
    let foreign = imported_identity("ForeignCanonicalSymbolId");
    let foreign_binding = vec![nominal(owner.clone(), "CanonicalSymbolId", &[(&carrier, &foreign)])];
    assert!(resolved(&foreign_binding, &carrier).is_err());
    assert!(resolved(&[], &carrier).is_err());
    let decoy_owner = identity("Shadow");
    let no_owner = vec![nominal(decoy_owner.clone(), &carrier, &[(&carrier, &decoy_owner)])];
    assert!(resolved(&no_owner, &carrier).is_err());
    let mut open = nominal(owner.clone(), "CanonicalSymbolId", &[(&carrier, &owner)]);
    open.type_parameters = vec!["T".into()];
    open.type_parameter_count = 1;
    assert!(resolved(&[open], &carrier).is_err());
    let authenticated = nominal(owner.clone(), "CanonicalSymbolId", &[(&carrier, &owner)]);
    assert!(resolved(&[authenticated.clone(), authenticated.clone()], &carrier).is_err());
    let unbound_duplicate = nominal(owner.clone(), "CanonicalSymbolId", &[]);
    for mixed in [
        vec![authenticated.clone(), unbound_duplicate.clone()],
        vec![unbound_duplicate, authenticated],
    ] {
        assert!(resolved(&mixed, &carrier).is_err());
    }
    let foreign_carrier = carrier_key(&foreign)?;
    let wrong_carrier = vec![nominal(owner, "CanonicalSymbolId", &[(&foreign_carrier, &foreign)])];
    assert!(resolved(&wrong_carrier, &foreign_carrier).is_err());
    let legacy_duplicate = vec![
        nominal(identity("LegacyPacket"), "LegacyPacket", &[]),
        nominal(identity("OtherLegacyPacket"), "LegacyPacket", &[]),
    ];
    assert!(resolved(&legacy_duplicate, "LegacyPacket").is_err());
    Ok(())
}

/// Retain `Option[List[model]]` only through the same exact imported carrier authority scalar layouts use.
#[test]
fn nested_option_list_retains_exact_imported_model_carrier() -> Result<(), String> {
    let owner = imported_identity("CanonicalSymbolId");
    let carrier = carrier_key(&owner)?;
    let ty = generic(
        "Option".into(),
        vec![generic("List".into(), vec![IncanType::Named(carrier.clone())])],
    );
    let mut module = BodyIrModule::declarations_only("app".into());
    module.nominal_declarations = vec![nominal(owner.clone(), "CanonicalSymbolId", &[(&carrier, &owner)])];
    let constructors = constructor_index(&module.nominal_declarations);
    let mut intrinsics: Vec<EnumDeclaration> = Vec::new();
    retain_carrier(&constructors, &module, &mut intrinsics, &ty, "src/main.incn")?;
    assert_eq!(intrinsics.len(), 1);
    assert!(intrinsics[0].carrier == Carrier::Option);
    assert!(intrinsics[0].variants[0].fields.is_empty());
    let model = ListLeaf::Model(0, model_symbol("CanonicalSymbolId"));
    assert!(intrinsics[0].variants[1].fields == vec![PlanType::List(model, 1)]);
    let mut rejected = BodyIrModule::declarations_only("app".into());
    rejected.nominal_declarations = vec![nominal(owner, "CanonicalSymbolId", &[])];
    let rejected_index = constructor_index(&rejected.nominal_declarations);
    let mut rejected_intrinsics: Vec<EnumDeclaration> = Vec::new();
    assert!(
        retain_carrier(
            &rejected_index,
            &rejected,
            &mut rejected_intrinsics,
            &ty,
            "src/main.incn"
        )
        .is_err()
    );
    assert!(rejected_intrinsics.is_empty());
    Ok(())
}

/// Nested specializations survive a rebound source carrier; meaningful changes to arguments or fields still miss.
#[test]
fn stored_layout_identity_follows_nested_arguments_across_a_source_move() -> Result<(), String> {
    let owner = identity("Wrapper");
    let first = imported_identity("Packet");
    let mut second = imported_identity("Packet");
    second.declaration_span = HirSourceSpan::new(27, 44);
    let (first_key, second_key) = (carrier_key(&first)?, carrier_key(&second)?);
    let mut before = nominal(owner.clone(), "generated_before", &[(&first_key, &first)]);
    let mut after = nominal(owner, "generated_after", &[(&second_key, &second)]);
    before.native_type_arguments = vec![generic("Option".into(), vec![IncanType::Named(first_key)])];
    after.native_type_arguments = vec![generic("Option".into(), vec![IncanType::Named(second_key)])];
    after.direct_declaration_id = CompilerNodeId::declaration_span("app", 27, 44);
    let witness = |model: &NominalDeclaration| model_layout_witness(model).map_err(|error| error.to_string());
    let same = witness(&after)?;
    assert!(witness(&before)? == same);
    after.derives = vec!["Clone".into()];
    let changed = witness(&after)?;
    assert!(changed.0 == same.0 && changed.1 != same.1);
    after.native_type_arguments = vec![generic(
        "Option".into(),
        vec![IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Str)],
    )];
    assert!(witness(&after)?.0 != same.0);
    Ok(())
}

/// A closed native layout over a source model, as `Rc[RefCell[Packet]]` lowers to, names its source arguments by
/// declaration, so it has a layout witness and a product can name it; a different argument changes the witness.
#[test]
fn a_closed_native_layout_names_its_source_arguments_by_declaration() -> Result<(), String> {
    let packet = imported_identity("Packet");
    let header = imported_identity("Header");
    let (packet_key, header_key) = (carrier_key(&packet)?, carrier_key(&header)?);
    let declarations = vec![
        nominal(packet.clone(), "Packet", &[(&packet_key, &packet)]),
        nominal(header.clone(), "Header", &[(&header_key, &header)]),
    ];
    let shared = rust_item_identity("std::rc::Rc").ok_or("no Rc item identity")?;
    let template = native_opaque_nominal(&shared, 1).ok_or("no Rc template")?;
    let closed = |argument: &str| -> Result<NominalDeclaration, String> {
        let arguments = vec![IncanType::Named(argument.to_owned())];
        close_layout(&NominalInstance {
            declaration: template.clone(),
            argument_identities: argument_identities(&declarations, &arguments),
            arguments,
            ancestors: Vec::new(),
        })
    };
    let witness = |model: &NominalDeclaration| model_layout_witness(model).map_err(|error| error.to_string());
    let over_packet = witness(&closed(&packet_key)?)?;
    assert!(witness(&closed(&packet_key)?)? == over_packet);
    assert!(
        witness(&closed(&header_key)?)?.0 != over_packet.0,
        "a different argument kept the specialization"
    );
    Ok(())
}

/// Retaining a type whose carriers a pass already holds appends nothing, for an option, a result and a union.
///
/// A carrier inventory pass remembers each type it has retained and skips it in later frames; that is exact only
/// while a second retention of the same type is a no-op, which this test pins.
#[test]
fn a_retained_carrier_type_is_retained_again_without_a_new_carrier() -> Result<(), String> {
    let owner = imported_identity("CanonicalSymbolId");
    let carrier = carrier_key(&owner)?;
    let mut module = BodyIrModule::declarations_only("app".into());
    module.nominal_declarations = vec![nominal(owner.clone(), "CanonicalSymbolId", &[(&carrier, &owner)])];
    let constructors = constructor_index(&module.nominal_declarations);
    let int = IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Int);
    let text = IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Str);
    let types = [
        generic("Option".into(), vec![IncanType::Named(carrier)]),
        generic("Result".into(), vec![text.clone(), int.clone()]),
        generic(incan_lang::lang::types::UNION_TYPE_NAME.into(), vec![int, text]),
    ];
    let mut intrinsics: Vec<EnumDeclaration> = Vec::new();
    for ty in &types {
        retain_carrier(&constructors, &module, &mut intrinsics, ty, "src/main.incn")?;
    }
    let first = intrinsics.len();
    assert_eq!(first, 3, "each type should add one carrier");
    for ty in types.iter().rev() {
        retain_carrier(&constructors, &module, &mut intrinsics, ty, "src/main.incn")?;
    }
    assert_eq!(intrinsics.len(), first, "retaining a held type appended a carrier");
    Ok(())
}

/// A changed variant payload changes the version of the same closed enum specialization.
#[test]
fn native_enum_layout_authenticates_its_variant_payload() -> Result<(), String> {
    let payload = |name: &str| vec![("Some".to_owned(), vec![IncanType::Named(name.into())])];
    let Some(before) = native_enum_declaration("std::option::Option", Vec::new(), payload("X")) else {
        return Err("native enum fixture could not be constructed".into());
    };
    let Some(after) = native_enum_declaration("std::option::Option", Vec::new(), payload("Y")) else {
        return Err("changed native enum fixture could not be constructed".into());
    };
    let a = enum_layout_witness(&before).map_err(|error| error.to_string())?;
    let b = enum_layout_witness(&after).map_err(|error| error.to_string())?;
    assert!(a.0 == b.0 && a.1 != b.1);
    Ok(())
}

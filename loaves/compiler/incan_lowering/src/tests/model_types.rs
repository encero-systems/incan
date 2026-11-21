//! A checked model type resolves only through one authenticated nominal owner, or one unique legacy spelling.

use std::collections::BTreeMap;
use std::sync::Arc;

use incan_mir_plan_native::PlanType;
use incan_semantics_core::body_ir::{
    NewtypeDeserialize, NewtypeStringConversion, NominalDeclaration, nominal_carrier_key,
};
use incan_semantics_core::{
    CanonicalSymbolId, CompilerNodeId, HirSourceSpan, IncanType, SemanticSourceTargetKind, SymbolNamespace,
    SymbolOrigin,
};

use crate::models::model_type;
use crate::native_type_identity::constructor_index;
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
fn nominal(owner: CanonicalSymbolId, name: &str, identities: &[(&str, &CanonicalSymbolId)]) -> NominalDeclaration {
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

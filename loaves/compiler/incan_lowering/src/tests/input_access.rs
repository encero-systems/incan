//! Checked input modes survive native representation normalization and distinguish source instance symbols.
//!
//! The reference's initial `independent_result` contract is called by nothing but itself and its test, so the Rust
//! lowering keeps only `independent_result_with_layout`, which the last test here covers.

use std::collections::BTreeMap;
use std::sync::Arc;

use incan_semantics_core::body_ir::{
    Block, Body, CheckedInputProfile, EnumDeclaration, EnumVariantDeclaration, NominalDeclaration, ScopeId,
    nominal_carrier_key,
};
use incan_semantics_core::{
    CanonicalSymbolId, CompilerNodeId, HirSourceSpan, IncanCallableParam, IncanCallableParamKind, IncanInputAccess,
    IncanPrimitiveType as Primitive, IncanType, SemanticSourceTargetKind, function_type, generic,
};

use crate::decls::ViewLifetimeLayout;
use crate::native_type_identity::{constructor_index, representation_arguments};
use crate::result_requirements::{independent_result_with_layout, validate_result_requirements};
use crate::symbols::type_component;
use crate::type_parameters::source_nominal_owner;

const MODES: [IncanInputAccess; 4] = [
    IncanInputAccess::Inferred,
    IncanInputAccess::Shared,
    IncanInputAccess::Exclusive,
    IncanInputAccess::Owned,
];

/// Vary only the checked access mode, retaining equal value types and ordinary binding flags.
fn callable_with_access(access: IncanInputAccess) -> IncanType {
    let parameter = IncanCallableParam {
        name: None,
        ty: IncanType::Primitive(Primitive::Int),
        kind: IncanCallableParamKind::Normal,
        has_default: false,
        is_partial_preset: false,
        is_mut: false,
        native_owned: false,
        input_access: access,
    };
    function_type(vec![parameter], IncanType::Primitive(Primitive::Unit))
}

/// All four modes have distinct deterministic keys; no other signature field differs.
#[test]
fn callable_symbols_distinguish_input_access() {
    for (left, mode) in MODES.iter().enumerate() {
        let key = type_component(&callable_with_access(*mode));
        assert_eq!(key, type_component(&callable_with_access(*mode)));
        for other in &MODES[left + 1..] {
            assert_ne!(key, type_component(&callable_with_access(*other)));
        }
    }
}

/// Removing source-only binding metadata must retain shared, exclusive and owned access facts.
#[test]
fn native_callable_normalization_preserves_input_access() {
    let declarations: Vec<NominalDeclaration> = Vec::new();
    let constructors = constructor_index(&declarations);
    for mode in MODES {
        let normalized = representation_arguments(&declarations, &constructors, &[callable_with_access(mode)]);
        assert_eq!(normalized.len(), 1);
        let IncanType::Function { params, .. } = &normalized[0] else {
            panic!("a callable stays a callable");
        };
        assert_eq!(params.len(), 1);
        assert!(params[0].input_access == mode);
        assert!(!params[0].is_mut);
        assert!(!params[0].native_owned);
    }
}

/// A missing spelling needs a checked carrier, and a direct checked alias wins.
#[test]
fn source_nominal_lookup_respects_direct_and_legacy_carriers() -> Result<(), String> {
    let source = CanonicalSymbolId::module_declaration(
        vec!["local_packet".into()],
        "Payload",
        SemanticSourceTargetKind::Model,
        HirSourceSpan::new(0, 10),
    );
    let mut scope = BTreeMap::new();
    scope.insert("Payload".to_owned(), source.clone());
    assert!(source_nominal_owner(&scope, "Payload"));
    assert!(!source_nominal_owner(&scope, "Optional"));
    let Some(key) = nominal_carrier_key(&source) else {
        return Err("source declaration has no carrier".into());
    };
    assert!(source_nominal_owner(&scope, &key));
    let shadow = CanonicalSymbolId::module_declaration(
        vec!["local_packet".into()],
        "Plain",
        SemanticSourceTargetKind::TypeAlias,
        HirSourceSpan::new(11, 20),
    );
    scope.insert(key.clone(), shadow);
    assert!(!source_nominal_owner(&scope, &key));
    Ok(())
}

fn source_identity(name: &str, kind: SemanticSourceTargetKind) -> CanonicalSymbolId {
    CanonicalSymbolId::module_declaration(
        vec!["std".into(), "result_requirements".into()],
        name,
        kind,
        HirSourceSpan::new(0, 10),
    )
}

/// One exact nongeneric source enum with one payload variant and its own identity map.
fn source_enum(name: &str, fields: Vec<IncanType>) -> EnumDeclaration {
    let canonical = source_identity(name, SemanticSourceTargetKind::Enum);
    let mut identities = BTreeMap::new();
    identities.insert(name.to_owned(), canonical.clone());
    EnumDeclaration {
        record_package_identity: None,
        native_item: String::new(),
        direct_declaration_id: CompilerNodeId::declaration_span("std::result_requirements", 0, 10),
        canonical,
        name: name.into(),
        public: false,
        type_parameters: Vec::new(),
        native_type_arguments: Vec::new(),
        variants: vec![EnumVariantDeclaration {
            direct_declaration_id: CompilerNodeId::declaration_span("std::result_requirements", 11, 20),
            canonical: source_identity(&format!("{name}Value"), SemanticSourceTargetKind::Variant),
            name: format!("{name}Value"),
            fields,
        }],
        derives: Vec::new(),
        named_type_identities: Arc::new(identities),
    }
}

/// The identity map binding each name to its enum's canonical identity.
fn identities(enums: &[&EnumDeclaration]) -> BTreeMap<String, CanonicalSymbolId> {
    enums
        .iter()
        .map(|declaration| (declaration.name.clone(), declaration.canonical.clone()))
        .collect()
}

fn checked(ty: &IncanType, identities: &BTreeMap<String, CanonicalSymbolId>, enums: &[EnumDeclaration]) -> bool {
    let declarations: Vec<NominalDeclaration> = Vec::new();
    let mut layouts = ViewLifetimeLayout::start(&declarations, enums);
    independent_result_with_layout(ty, identities, &declarations, enums, &mut layouts)
}

fn named(name: &str) -> IncanType {
    IncanType::Named(name.into())
}

fn result(ok: IncanType, error: IncanType) -> IncanType {
    generic("Result".into(), vec![ok, error])
}

/// Owned nominal recursion closes through exact source identities.
#[test]
fn result_requirements_close_owned_source_declarations() {
    let text = IncanType::Primitive(Primitive::Str);
    let bytes = IncanType::Primitive(Primitive::Bytes);
    let unit = IncanType::Primitive(Primitive::Unit);
    let value = source_enum(
        "Value",
        vec![text.clone(), generic("List".into(), vec![named("Value")])],
    );
    let fault = source_enum("CodecFault", vec![text.clone(), IncanType::Primitive(Primitive::Int)]);
    let ids = identities(&[&value, &fault]);
    let enums = vec![value.clone(), fault.clone()];
    assert!(checked(&result(named("Value"), named("CodecFault")), &ids, &enums));
    assert!(checked(&result(bytes.clone(), named("CodecFault")), &ids, &enums));
    assert!(checked(&result(unit, named("CodecFault")), &ids, &enums));
    let empty = BTreeMap::new();
    assert!(checked(
        &result(generic("List".into(), vec![bytes]), text.clone()),
        &empty,
        &[]
    ));
    assert!(!checked(&generic("Result".into(), vec![text.clone()]), &empty, &[]));
    assert!(!checked(
        &generic("Option".into(), vec![text.clone(), text.clone()]),
        &empty,
        &[]
    ));
    assert!(!checked(
        &result(IncanType::TypeVar("E".into()), text.clone()),
        &empty,
        &[]
    ));
    assert!(!checked(&result(named("E"), text.clone()), &empty, &[]));
    let list_of_two = generic("List".into(), vec![text.clone(), text.clone()]);
    assert!(!checked(&result(list_of_two, text.clone()), &empty, &[]));
    let dictionary = generic("Dict".into(), vec![text.clone(), text.clone()]);
    assert!(!checked(&result(dictionary, text.clone()), &empty, &[]));
    assert!(!checked(
        &result(IncanType::RustInteropPath("rust::String".into()), text),
        &empty,
        &[]
    ));
}

/// Every unresolved, ambiguous or repointed nominal edge refuses.
#[test]
fn result_requirements_refuse_ambiguous_or_repointed_declarations() {
    let text = IncanType::Primitive(Primitive::Str);
    let mut value = source_enum("Value", Vec::new());
    value.variants[0].fields = vec![text.clone(), generic("List".into(), vec![named("Value")])];
    let fault = source_enum("CodecFault", vec![text.clone(), IncanType::Primitive(Primitive::Int)]);
    let ids = identities(&[&value, &fault]);
    let both = result(named("Value"), named("CodecFault"));
    assert!(!checked(&both, &ids, &[value.clone(), value.clone(), fault.clone()]));
    let missing = identities(&[&fault]);
    assert!(!checked(&both, &missing, &[value.clone(), fault.clone()]));
    let mut foreign = value.clone();
    foreign.canonical = CanonicalSymbolId::module_declaration(
        vec!["other".into()],
        "Value",
        SemanticSourceTargetKind::Enum,
        HirSourceSpan::new(0, 10),
    );
    foreign.named_type_identities = Arc::new(identities(&[&foreign]));
    let mut repointed = ids.clone();
    repointed.insert("Value".into(), foreign.canonical.clone());
    assert!(!checked(&both, &repointed, &[value.clone(), fault.clone(), foreign]));
    assert!(!checked(&both, &repointed, &[value.clone(), fault.clone()]));
    let mut alias_cycle = value.clone();
    alias_cycle.variants[0].fields = vec![named("Wrong")];
    let mut alias_scope = alias_cycle.named_type_identities.as_ref().clone();
    alias_scope.insert("Wrong".into(), alias_cycle.canonical.clone());
    alias_cycle.named_type_identities = Arc::new(alias_scope);
    assert!(!checked(&both, &ids, &[alias_cycle, fault]));
}

/// A payload that borrows, directly, through a wrapper or inside a collection, cannot discharge the requirement.
#[test]
fn result_requirements_refuse_borrowed_payloads() {
    let text = IncanType::Primitive(Primitive::Str);
    let borrowed = source_enum("BorrowedPayload", vec![IncanType::Ref(Box::new(text.clone()))]);
    let borrowed_ids = identities(&[&borrowed]);
    assert!(!checked(
        &result(named("BorrowedPayload"), text.clone()),
        &borrowed_ids,
        &[borrowed.clone()]
    ));
    let borrowed_mut = source_enum("BorrowedMutPayload", vec![IncanType::RefMut(Box::new(text.clone()))]);
    let borrowed_mut_ids = identities(&[&borrowed_mut]);
    assert!(!checked(
        &result(named("BorrowedMutPayload"), text.clone()),
        &borrowed_mut_ids,
        &[borrowed_mut]
    ));
    for wrapper in ["Borrowed", "BorrowedMut", "Reference"] {
        let name = format!("{wrapper}Payload");
        let wrapped = source_enum(&name, vec![generic(wrapper.into(), vec![text.clone()])]);
        let wrapped_ids = identities(&[&wrapped]);
        assert!(!checked(&result(named(&name), text.clone()), &wrapped_ids, &[wrapped]));
    }
    let hidden_list = generic("List".into(), vec![IncanType::Ref(Box::new(text.clone()))]);
    let hidden = source_enum("HiddenBorrow", vec![hidden_list]);
    let hidden_ids = identities(&[&hidden]);
    assert!(!checked(
        &result(named("HiddenBorrow"), text.clone()),
        &hidden_ids,
        &[hidden]
    ));
    let (left, mut right) = mutual_pair(&text);
    let cycle_ids = identities(&[&left]);
    assert!(!checked(
        &result(named("Left"), text.clone()),
        &cycle_ids,
        &[left.clone(), right.clone()]
    ));
    right.variants[0].fields = vec![named("Left")];
    assert!(checked(
        &result(named("Left"), text.clone()),
        &cycle_ids,
        &[left, right]
    ));
    let mut open = source_enum("Open", vec![text.clone()]);
    open.type_parameters = vec!["T".into()];
    let open_ids = identities(&[&open]);
    assert!(!checked(&result(named("Open"), text), &open_ids, &[open]));
}

/// `Left` holds `Right`, and `Right` holds `Left` beside a borrowed text.
fn mutual_pair(text: &IncanType) -> (EnumDeclaration, EnumDeclaration) {
    let mut left = source_enum("Left", vec![named("Right")]);
    let mut right = source_enum("Right", vec![named("Left"), IncanType::Ref(Box::new(text.clone()))]);
    let mut left_scope = left.named_type_identities.as_ref().clone();
    left_scope.insert("Right".into(), right.canonical.clone());
    left.named_type_identities = Arc::new(left_scope);
    let mut right_scope = right.named_type_identities.as_ref().clone();
    right_scope.insert("Left".into(), left.canonical.clone());
    right.named_type_identities = Arc::new(right_scope);
    (left, right)
}

/// A complete empty Body carrying one checked result requirement.
fn required_body(name: &str, ty: IncanType, identities: BTreeMap<String, CanonicalSymbolId>) -> Body {
    Body {
        nominal_instantiation: Vec::new(),
        trait_default_instantiation: None,
        type_parameters: Vec::new(),
        decl_id: CompilerNodeId::declaration("result_requirements", name),
        direct_call_id: CompilerNodeId::declaration_span("result_requirements", 0, 10),
        canonical: None,
        name: name.into(),
        span: HirSourceSpan::new(0, 10),
        return_type: IncanType::Primitive(Primitive::Unit),
        callable_representation: None,
        checked_input_profile: Some(CheckedInputProfile {
            result_requirements: vec![(HirSourceSpan::new(3, 17), ty)],
            version: 3,
            owner: source_identity(name, SemanticSourceTargetKind::Function),
            parameters: Vec::new(),
            owned_initializers: Vec::new(),
        }),
        explicit_input_bindings: Vec::new(),
        returned_view: None,
        named_type_identities: Arc::new(identities),
        locals: Vec::new(),
        params: Vec::new(),
        param_locals: Vec::new(),
        scopes: Vec::new(),
        block: Block {
            scope: ScopeId(0),
            stmts: Vec::new(),
        },
        runtime_requirements: Vec::new(),
        panic_facts: Vec::new(),
        is_async: false,
        retired_rust_facade_wire: None,
        native_attributes: Vec::new(),
    }
}

/// A body's retained requirements validate through the same proof, with the body's own identity map.
#[test]
fn bodies_validate_their_result_requirements() -> Result<(), String> {
    let text = IncanType::Primitive(Primitive::Str);
    let bytes = IncanType::Primitive(Primitive::Bytes);
    let declarations: Vec<NominalDeclaration> = Vec::new();
    let list = result(generic("List".into(), vec![bytes]), text.clone());
    validate_result_requirements(&required_body("list_result", list, BTreeMap::new()), &declarations, &[])?;
    let value = source_enum(
        "Value",
        vec![text.clone(), generic("List".into(), vec![named("Value")])],
    );
    let fault = source_enum("CodecFault", vec![text.clone(), IncanType::Primitive(Primitive::Int)]);
    let ids = identities(&[&value, &fault]);
    let enums = vec![value.clone(), fault.clone()];
    let both = result(named("Value"), named("CodecFault"));
    let nominal = required_body("nominal_result", both.clone(), ids.clone());
    validate_result_requirements(&nominal, &declarations, &enums)?;
    let mut repointed = ids;
    repointed.insert(
        "Value".into(),
        CanonicalSymbolId::module_declaration(
            vec!["other".into()],
            "Value",
            SemanticSourceTargetKind::Enum,
            HirSourceSpan::new(0, 10),
        ),
    );
    let wrong = required_body("wrong_nominal", both, repointed);
    assert!(validate_result_requirements(&wrong, &declarations, &enums).is_err());
    let borrowed = source_enum("BorrowedPayload", vec![IncanType::Ref(Box::new(text.clone()))]);
    let borrowed_body = required_body(
        "borrowed_nominal",
        result(named("BorrowedPayload"), text.clone()),
        identities(&[&borrowed]),
    );
    assert!(validate_result_requirements(&borrowed_body, &declarations, &[borrowed]).is_err());
    let (left, right) = mutual_pair(&text);
    let cycle = required_body("borrow_cycle", result(named("Left"), text), identities(&[&left]));
    assert!(validate_result_requirements(&cycle, &declarations, &[left, right]).is_err());
    Ok(())
}

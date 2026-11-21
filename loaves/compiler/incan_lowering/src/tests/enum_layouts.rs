//! Closed enum layouts are selected from checked uses: the first generic enum declaration whose carrier name a closed
//! use spells is the one instantiated, and a use with another number of arguments is refused.

use std::collections::BTreeMap;
use std::sync::Arc;

use incan_semantics_core::body_ir::{BodyIrModule, EnumDeclaration, EnumVariantDeclaration, NominalDeclaration};
use incan_semantics_core::{
    CanonicalSymbolId, CompilerNodeId, HirSourceSpan, IncanPrimitiveType, IncanType, SemanticSourceTargetKind,
};

use crate::enum_instances::specialized_enum_layouts;

fn identity(name: &str, kind: SemanticSourceTargetKind) -> CanonicalSymbolId {
    CanonicalSymbolId::module_declaration(
        vec!["app".into(), "layouts".into()],
        name,
        kind,
        HirSourceSpan::new(0, 10),
    )
}

/// An enum named `name` with one payload variant holding `fields`, generic over `parameters`.
fn source_enum(name: &str, parameters: &[&str], fields: Vec<IncanType>) -> EnumDeclaration {
    let canonical = identity(name, SemanticSourceTargetKind::Enum);
    let mut identities = BTreeMap::new();
    identities.insert(name.to_owned(), canonical.clone());
    EnumDeclaration {
        record_package_identity: None,
        native_item: String::new(),
        direct_declaration_id: CompilerNodeId::declaration_span("app::layouts", 0, 10),
        canonical,
        name: name.into(),
        public: false,
        type_parameters: parameters.iter().map(|parameter| (*parameter).to_owned()).collect(),
        native_type_arguments: Vec::new(),
        variants: vec![EnumVariantDeclaration {
            direct_declaration_id: CompilerNodeId::declaration_span("app::layouts", 11, 20),
            canonical: identity(&format!("{name}Value"), SemanticSourceTargetKind::Variant),
            name: format!("{name}Value"),
            fields,
        }],
        derives: Vec::new(),
        named_type_identities: Arc::new(identities),
    }
}

/// A model whose one field has type `field`.
fn holder(field: IncanType) -> NominalDeclaration {
    let mut declaration = NominalDeclaration {
        record_package_identity: None,
        trait_adoptions: Vec::new(),
        native_item: String::new(),
        class_inheritance: None,
        native_type_arguments: Vec::new(),
        direct_declaration_id: CompilerNodeId::declaration_span("app::layouts", 30, 40),
        canonical: identity("Holder", SemanticSourceTargetKind::Model),
        name: "Holder".into(),
        fields: vec!["value".into()],
        field_identities: Vec::new(),
        field_aliases: Vec::new(),
        field_types: Vec::new(),
        owned_callable_fields: Vec::new(),
        field_public: vec![true],
        reflection_fields: None,
        display_method: None,
        newtype_deserialize: Default::default(),
        newtype_string_conversion: Default::default(),
        reflection_metadata: None,
        comparison_equality: None,
        public: true,
        has_field_defaults: false,
        field_default_body: None,
        validated_constructor: None,
        method_partial_defaults: Vec::new(),
        derives: Vec::new(),
        named_type_identities: Default::default(),
        type_parameters: Vec::new(),
        type_parameter_count: 0,
    };
    declaration.field_types = vec![field];
    declaration
}

fn module(enums: Vec<EnumDeclaration>, field: IncanType) -> BodyIrModule {
    let mut module = BodyIrModule::declarations_only("app::layouts".into());
    module.enum_declarations = enums;
    module.nominal_declarations = vec![holder(field)];
    module
}

fn closed(base: &str, arguments: Vec<IncanType>) -> IncanType {
    IncanType::Generic {
        base: base.into(),
        args: arguments,
    }
}

/// A closed use instantiates the first generic declaration of its carrier name, never a nongeneric one before it,
/// and a use with another argument count is refused at that declaration.
#[test]
fn a_closed_use_instantiates_the_first_generic_enum_of_its_name() -> Result<(), String> {
    let plain = source_enum("Choice", &[], vec![IncanType::Primitive(IncanPrimitiveType::Int)]);
    let generic = source_enum("Choice", &["T"], vec![IncanType::TypeVar("T".into())]);
    let later = source_enum("Choice", &["T", "U"], vec![IncanType::TypeVar("U".into())]);
    let base = generic.carrier_name();
    let used = module(
        vec![plain.clone(), generic.clone(), later.clone()],
        closed(&base, vec![IncanType::Primitive(IncanPrimitiveType::Int)]),
    );
    let layouts = specialized_enum_layouts(&used)?;
    // The nongeneric declaration is its own layout; the use closes the first generic one, over one argument.
    assert_eq!(
        layouts.len(),
        2,
        "layouts: {:?}",
        layouts.iter().map(|layout| &layout.name).collect::<Vec<_>>()
    );
    assert!(layouts[0] == plain);
    assert!(layouts[1].canonical == generic.canonical);
    assert_eq!(
        layouts[1].native_type_arguments,
        vec![IncanType::Primitive(IncanPrimitiveType::Int)]
    );
    assert_eq!(
        layouts[1].variants[0].fields,
        vec![IncanType::Primitive(IncanPrimitiveType::Int)]
    );
    let arity = module(
        vec![plain, generic, later],
        closed(
            &base,
            vec![
                IncanType::Primitive(IncanPrimitiveType::Int),
                IncanType::Primitive(IncanPrimitiveType::Str),
            ],
        ),
    );
    assert_eq!(
        specialized_enum_layouts(&arity).err(),
        Some(format!("unsupported Body IR enum type argument arity on {base}"))
    );
    // A use of a name no generic enum carries closes nothing.
    let unused = module(
        vec![source_enum("Other", &["T"], Vec::new())],
        closed(&base, vec![IncanType::Primitive(IncanPrimitiveType::Int)]),
    );
    assert!(specialized_enum_layouts(&unused)?.is_empty());
    Ok(())
}

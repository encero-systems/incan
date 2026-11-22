//! The one native-to-record compiler adapter copies every available field and attaches missing boundary facts.
//!
//! The reference's authored and native plan source spans are one type in Rust, so its span round trip has no
//! counterpart here.

use std::collections::BTreeMap;

use incan_mir_plan_native::record_types::{
    CheckedCallableParam, CheckedCallableParamKind, CheckedInputAccess, CheckedPrimitiveType, CheckedType,
    DeclarationKind, SymbolOrigin,
};
use incan_mir_plan_native::{EnumDeclaration, PlanType, semantic_type_matches, source_span};
use incan_semantics_core::{
    CanonicalSymbolId as NativeCanonicalSymbolId, HirSourceSpan as NativeHirSourceSpan, IncanCallableParam,
    IncanCallableParamKind, IncanInputAccess, IncanPrimitiveType, IncanType, NumericTypeId, RuntimeBuiltinType,
    SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin as NativeSymbolOrigin, function_type, generic,
    native_array_type,
};

use crate::compiler_record_adapters::{
    authored_plan_checked_type, authored_plan_symbol_id, canonical_symbol_id, checked_type, native_canonical_symbol_id,
    native_checked_type,
};

fn boxed(ty: IncanType) -> Box<IncanType> {
    Box::new(ty)
}

fn readable() -> NativeCanonicalSymbolId {
    NativeCanonicalSymbolId::module_declaration(
        vec!["contracts".into()],
        "Readable",
        SemanticSourceTargetKind::Trait,
        NativeHirSourceSpan::new(4, 12),
    )
}

/// Copy one native canonical identity while attaching owned file text and selected package version explicitly.
#[test]
fn compiler_record_adapters() {
    let native = NativeCanonicalSymbolId::module_declaration(
        vec!["billing".into()],
        "Invoice",
        SemanticSourceTargetKind::Model,
        NativeHirSourceSpan::new(12, 19),
    );
    let identity = canonical_symbol_id(&native, "src/invoice.incn", "1.4.0");
    assert_eq!(identity.declaration_name, "Invoice");
    assert!(identity.kind == DeclarationKind::Model);
    assert_eq!(identity.declaration_span.file, "src/invoice.incn");
    assert_eq!(identity.declaration_span.start, 12);
    assert_eq!(identity.declaration_span.end, 19);
    assert!(identity.origin == SymbolOrigin::Module(vec!["billing".into()]));
    let reference = IncanType::Ref(boxed(IncanType::TypeVar("T".into())));
    assert!(
        checked_type(&reference, "src/main.incn", "1.4.0") == CheckedType::Ref(vec![CheckedType::TypeVar("T".into())])
    );
    let decimal = IncanType::Decimal {
        precision: 38,
        scale: 4,
    };
    assert!(checked_type(&decimal, "src/main.incn", "1.4.0") == CheckedType::Decimal(38, 4));
    let trait_object = IncanType::NativeTraitObject {
        path: "std::fmt::Debug".into(),
        send: true,
        sync: false,
    };
    assert!(
        checked_type(&trait_object, "src/main.incn", "1.4.0")
            == CheckedType::NativeTraitObject("std::fmt::Debug".into(), true, false)
    );
    let pointer = IncanType::NativePointer {
        mutable: true,
        pointee: boxed(IncanType::NativeCChar),
    };
    assert!(
        checked_type(&pointer, "src/main.incn", "1.4.0")
            == CheckedType::NativePointer(true, vec![CheckedType::NativeCChar])
    );
}

/// Complete direct and nested package trait identities from the selected artifact map and refuse an absent owner.
#[test]
fn plan_package_versions_are_authenticated() -> Result<(), String> {
    let native = NativeCanonicalSymbolId {
        namespace: SymbolNamespace::OrdinaryLexical,
        origin: NativeSymbolOrigin::Package {
            library: "contracts".into(),
            module_path: vec!["traits".into()],
        },
        declaration_name: "Readable".into(),
        kind: SemanticSourceTargetKind::Trait,
        scope_discriminant: None,
        declaration_span: NativeHirSourceSpan::new(4, 12),
    };
    let mut source_files = BTreeMap::new();
    source_files.insert(
        "pub::contracts::traits".to_owned(),
        "registry/contracts/traits.incn".to_owned(),
    );
    let mut versions = BTreeMap::new();
    versions.insert("contracts".to_owned(), "1.4.0".to_owned());
    let authored = authored_plan_symbol_id(&native, "src/main.incn", &source_files, &versions)?;
    assert_eq!(authored.declaration_span.file, "registry/contracts/traits.incn");
    let SymbolOrigin::Package(package, path) = &authored.origin else {
        return Err("a package trait keeps its package origin".into());
    };
    assert_eq!(package.name, "contracts");
    assert_eq!(package.version, "1.4.0");
    assert_eq!(path, &vec!["traits".to_owned()]);
    let wrapper = generic("Wrapper".into(), vec![IncanType::Trait(native.clone())]);
    let nested = authored_plan_checked_type(&wrapper, "src/main.incn", &source_files, &versions)?;
    let CheckedType::Generic(_, arguments) = &nested else {
        return Err("a generic stays generic".into());
    };
    let CheckedType::Trait(identity) = &arguments[0] else {
        return Err("a nested trait stays a trait".into());
    };
    let SymbolOrigin::Package(package, _) = &identity.origin else {
        return Err("a nested package trait keeps its package origin".into());
    };
    assert_eq!(package.version, "1.4.0");
    assert!(authored_plan_symbol_id(&native, "src/main.incn", &source_files, &BTreeMap::new()).is_err());
    Ok(())
}

/// Every native type alternative, with nested callable, binder, ownership, array and trait controls.
fn native_types() -> Vec<IncanType> {
    let parameter = IncanCallableParam {
        name: Some("value".into()),
        ty: IncanType::TypeVar("T".into()),
        kind: IncanCallableParamKind::RestKeyword,
        has_default: true,
        is_partial_preset: true,
        is_mut: true,
        native_owned: true,
        input_access: IncanInputAccess::Exclusive,
    };
    let reference = || IncanType::Ref(boxed(IncanType::TypeVar("T".into())));
    vec![
        IncanType::Never,
        IncanType::Primitive(IncanPrimitiveType::Numeric(NumericTypeId::I64)),
        IncanType::Named("Invoice".into()),
        generic("Result".into(), vec![IncanType::TypeVar("T".into()), reference()]),
        IncanType::Decimal {
            precision: 38,
            scale: 4,
        },
        function_type(vec![parameter], reference()),
        IncanType::TypeToken(boxed(IncanType::Named("Invoice".into()))),
        IncanType::Tuple(vec![IncanType::Primitive(IncanPrimitiveType::Bool)]),
        IncanType::TypeVar("T".into()),
        IncanType::SelfType,
        reference(),
        IncanType::RefMut(boxed(IncanType::TypeVar("T".into()))),
        IncanType::RustInteropPath("std::path::PathBuf".into()),
        IncanType::NativeBuiltin(RuntimeBuiltinType::ValidationError),
        IncanType::Unsupported("opaque".into()),
        IncanType::Infer,
        IncanType::Unknown,
        native_array_type(IncanType::Primitive(IncanPrimitiveType::Bytes), u64::MAX),
        IncanType::NativeTraitObject {
            path: "std::fmt::Debug".into(),
            send: true,
            sync: false,
        },
        IncanType::NativePointer {
            mutable: true,
            pointee: boxed(IncanType::NativeCChar),
        },
        IncanType::NativeCChar,
        IncanType::Trait(readable()),
    ]
}

/// Convert every native type alternative and back, keeping each nested control.
#[test]
fn all_native_checked_types_convert() -> Result<(), String> {
    let authored: Vec<CheckedType> = native_types()
        .iter()
        .map(|ty| checked_type(ty, "src/main.incn", "1.4.0"))
        .collect();
    assert_eq!(authored.len(), 22);
    for ty in &authored {
        let restored = native_checked_type(ty)?;
        assert!(checked_type(&restored, "src/main.incn", "1.4.0") == *ty);
    }
    let variable = || CheckedType::TypeVar("T".into());
    assert!(authored[3] == CheckedType::Generic("Result".into(), vec![variable(), CheckedType::Ref(vec![variable()])]));
    let parameter = CheckedCallableParam {
        name: Some("value".into()),
        ty: variable(),
        kind: CheckedCallableParamKind::RestKeyword,
        has_default: true,
        is_partial_preset: true,
        is_mut: true,
        native_owned: true,
        input_access: CheckedInputAccess::Exclusive,
    };
    assert!(authored[5] == CheckedType::Function(vec![parameter], vec![CheckedType::Ref(vec![variable()])]));
    assert!(
        authored[17] == CheckedType::NativeArray(vec![CheckedType::Primitive(CheckedPrimitiveType::Bytes)], u64::MAX)
    );
    let CheckedType::Trait(identity) = &authored[21] else {
        return Err("a trait converts to a trait".into());
    };
    assert_eq!(identity.declaration_name, "Readable");
    assert_eq!(identity.declaration_span.file, "src/main.incn");
    Ok(())
}

/// Reconstruct an exact 32-byte expansion identity instead of dropping or stringifying its native fixed array.
#[test]
fn expanded_canonical_identity_round_trips() -> Result<(), String> {
    let native = NativeCanonicalSymbolId::module_declaration(
        vec!["generated".into()],
        "expanded",
        SemanticSourceTargetKind::Function,
        NativeHirSourceSpan::new(20, 40).with_expansion(Some([7u8; 32])),
    );
    let authored = canonical_symbol_id(&native, "src/generated.incn", "1.4.0");
    let restored = native_canonical_symbol_id(&authored)?;
    assert!(canonical_symbol_id(&restored, "src/generated.incn", "1.4.0") == authored);
    Ok(())
}

/// Keep trait identity and enum carrier meaning typed even when diagnostic source text deliberately disagrees.
#[test]
fn plan_facts_use_authored_records() {
    let authored = canonical_symbol_id(&readable(), "src/main.incn", "1.4.0");
    let PlanType::Trait(retained) = PlanType::Trait(authored.clone()) else {
        panic!("a trait plan type keeps its identity");
    };
    assert!(authored == retained);
    let semantic = CheckedType::Generic(
        "Result".into(),
        vec![
            CheckedType::RefMut(vec![CheckedType::TypeVar("T".into())]),
            CheckedType::TypeVar("T".into()),
        ],
    );
    let mut declaration = EnumDeclaration::new(
        "__IncanCarrier0".into(),
        false,
        Vec::new(),
        Vec::new(),
        source_span("src/main.incn", 1, 1),
    );
    declaration.semantic_key = Some(semantic.clone());
    declaration.source_type = "diagnostic text is not identity".into();
    assert!(semantic_type_matches(&declaration, &semantic));
}

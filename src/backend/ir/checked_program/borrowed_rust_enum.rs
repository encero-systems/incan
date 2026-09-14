//! Borrowed imported enums retain constructor resolution and reference ownership (#1448).

use crate::backend::IrCodegen;
use crate::frontend::typechecker::TypeChecker;
use crate::frontend::{lexer, parser};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Both reference modes admit imported payload patterns and keep a second match usable without cloning.
#[test]
fn borrowed_rust_enum_codegen_preserves_shared_and_mutable_subjects() -> TestResult {
    for reference in ["&", "&mut "] {
        let source = format!(
            r#"
from rust::toml import Value as RawValue

pub def inspect(value: {reference}RawValue) -> bool:
    match value:
        RawValue.String(text) =>
            _ = text
        _ => pass
    match value:
        RawValue.String(_) => return true
        _ => return false
"#
        );
        let tokens = lexer::lex(&source).map_err(|error| format!("lex: {error:?}"))?;
        let ast = parser::parse(&tokens).map_err(|error| format!("parse: {error:?}"))?;
        let mut checker = TypeChecker::new();
        checker
            .check_program(&ast)
            .map_err(|error| format!("check: {error:?}"))?;
        let mut generator = IrCodegen::new();
        generator.set_prechecked_type_info(checker.type_info().clone(), std::collections::HashMap::new());
        let rust = generator
            .try_generate(&ast)
            .map_err(|error| format!("generate: {error:?}"))?;
        assert!(rust.contains("RawValue::String"), "{rust}");
        assert_eq!(rust.matches("match value {").count(), 2, "{rust}");
        assert!(
            !rust.contains("value.clone()"),
            "a borrowed scrutinee must not clone its referent: {rust}"
        );
    }
    Ok(())
}

/// Unwrapping a borrowed subject must not erase the existing rusttype qualifier check.
#[test]
fn borrowed_rust_enum_keeps_mismatched_rusttype_qualifier_rejected() -> TestResult {
    let source = r#"
from rust::toml import Value as RawValue

type Value = rusttype RawValue:
    def noop(self) -> None:
        ...

type Other = rusttype RawValue:
    def noop(self) -> None:
        ...

pub def inspect(value: &Value) -> None:
    match value:
        Other.String(_) => pass
        _ => pass
"#;
    let tokens = lexer::lex(source).map_err(|error| format!("lex: {error:?}"))?;
    let ast = parser::parse(&tokens).map_err(|error| format!("parse: {error:?}"))?;
    let mut checker = TypeChecker::new();
    let errors = checker
        .check_program(&ast)
        .err()
        .ok_or("mismatched qualifier was accepted")?;
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("does not resolve for this match")),
        "{errors:?}"
    );
    Ok(())
}

/// Precise metadata and metadata-free fallback both preserve payload reference types at their use sites.
#[test]
fn borrowed_rust_enum_payload_types_preserve_reference_modes() -> TestResult {
    use crate::frontend::ast::Span;
    use crate::frontend::symbols::{ResolvedType, RustImportBindingKind, RustItemInfo, Symbol, SymbolKind};
    use incan_core::interop::{
        RustItemKind, RustItemMetadata, RustTypeInfo, RustTypeShape, RustVariantInfo, RustVisibility,
    };
    let cases = [
        (None, "payload"),
        (Some(RustTypeShape::Str), "payload"),
        (
            Some(RustTypeShape::Tuple(vec![RustTypeShape::Str, RustTypeShape::Str])),
            "(payload, _)",
        ),
        (
            Some(RustTypeShape::Option(Box::new(RustTypeShape::Str))),
            "Some(payload)",
        ),
        (
            Some(RustTypeShape::Result(
                Box::new(RustTypeShape::Str),
                Box::new(RustTypeShape::Int),
            )),
            "Ok(payload)",
        ),
    ];
    for (shape, pattern) in cases {
        let with_metadata = shape.is_some();
        for (reference, mutable) in [("&", false), ("&mut ", true), ("&&mut ", false), ("&mut &", false)] {
            let source = format!(
                r#"
pub def inspect(value: {reference}RawEnum) -> None:
    match value:
        RawEnum.Payload({pattern}) =>
            _ = payload
        _ => pass
"#
            );
            let metadata = shape.clone().map(|shape| RustItemMetadata {
                canonical_path: "fixture::Envelope".into(),
                definition_path: Some("fixture::Envelope".into()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    type_params: Vec::new(),
                    type_param_defaults: Vec::new(),
                    mutable_reference_type_params: Vec::new(),
                    expanded_derive_traits: Vec::new(),
                    has_const_params: false,
                    alias_target: None,
                    metadata_completeness: Default::default(),
                    fields: Vec::new(),
                    methods: Vec::new(),
                    implemented_traits: Vec::new(),
                    variants: vec![RustVariantInfo {
                        name: "Payload".into(),
                        fields: vec![shape],
                        field_carriers: Vec::new(),
                    }],
                }),
            });
            let mut checker = TypeChecker::new();
            checker.symbols.define(Symbol {
                name: "RawEnum".into(),
                kind: SymbolKind::RustItem(RustItemInfo {
                    crate_name: "fixture".into(),
                    path: "fixture::Envelope".into(),
                    binding: RustImportBindingKind::FromImport,
                    metadata,
                }),
                span: Span::default(),
                scope: 0,
            });
            let ast = parser::parse(&lexer::lex(&source).map_err(|error| format!("lex: {error:?}"))?)
                .map_err(|error| format!("parse: {error:?}"))?;
            checker
                .check_program(&ast)
                .map_err(|error| format!("check: {error:?}"))?;
            let start = source.rfind("payload").ok_or("payload use missing")?;
            let actual = checker
                .type_info()
                .expr_type(Span::new(start, start + "payload".len()))
                .ok_or("payload use has no checked type")?;
            let inner = if with_metadata {
                ResolvedType::Str
            } else {
                ResolvedType::RustPath("fixture::Envelope::Payload".into())
            };
            let expected = if mutable {
                ResolvedType::RefMut(Box::new(inner))
            } else {
                ResolvedType::Ref(Box::new(inner))
            };
            assert_eq!(actual, &expected, "metadata={with_metadata}, mutable={mutable}");
        }
    }
    Ok(())
}

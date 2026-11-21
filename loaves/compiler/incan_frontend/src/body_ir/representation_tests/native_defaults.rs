//! A native record construction that leaves out published defaults is complete in Body IR.

use super::*;
use crate::loaf_index::LoafIndex;
use incan_lang::interop::{
    RustFieldInfo, RustFunctionSig, RustItemKind, RustItemMetadata, RustMethodSig, RustTypeInfo, RustTypeShape,
    RustVisibility,
};
use std::collections::BTreeMap;
use std::sync::Arc;

/// `demo::Request` with a required `name` and a `limit` whose default the crate publishes.
fn request_facts() -> BTreeMap<String, RustItemMetadata> {
    let field = |name: &str| RustFieldInfo {
        name: name.into(),
        type_display: "i64".into(),
        type_shape: RustTypeShape::Int,
    };
    let published = RustMethodSig {
        name: "__incan_default_limit".into(),
        signature: RustFunctionSig {
            receiver_contract: None,
            type_params: Vec::new(),
            bounds: Vec::new(),
            params: Vec::new(),
            return_type: "i64".into(),
            is_async: false,
            is_unsafe: false,
        },
        impl_type_params: Vec::new(),
        impl_self_type: Some("demo::Request".into()),
    };
    BTreeMap::from([(
        "demo::Request".to_owned(),
        RustItemMetadata {
            canonical_path: "demo::Request".into(),
            definition_path: None,
            visibility: RustVisibility::Public,
            kind: RustItemKind::Type(RustTypeInfo {
                fields: vec![field("name"), field("limit")],
                methods: vec![published],
                ..RustTypeInfo::default()
            }),
        },
    )])
}

/// Body IR calls the published default for the field the source leaves out, declares that native callable, and
/// builds the record with every slot bound.
#[test]
fn native_construction_calls_published_defaults() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from rust::demo import Request\n\ndef make() -> Request:\n    return Request(name=1)\n";
    let mut loaves = LoafIndex::default();
    loaves.add_rust_facts(&Default::default(), request_facts());
    let mut checker = TypeChecker::new();
    checker.set_loaves(Arc::new(loaves));
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let facts = checker.into_type_info();
    let binding = facts
        .calls
        .body_ir_native_constructor_bindings
        .values()
        .map(|(_, binding)| binding)
        .next()
        .ok_or("missing retained native construction")?;
    assert_eq!(binding.defaulted_slots, vec![1]);
    let providers = ProviderOperationCatalog::new();
    let context = BodyIrContext {
        modules: crate::body_ir::ContextModules::default(),
        provider_operations: &providers,
        registry_package: None,
        library_package: None,
        include_inline_tests: false,
    };
    let module = super::build_module_seed(&program, &[], &facts, &context);
    let function = "demo::Request::__incan_default_limit";
    assert!(module.loaf_callables.iter().any(|callable| callable.item == function));
    let rendered = format!("{:?}", module.bodies);
    assert!(rendered.contains(function), "{rendered}");
    assert!(!rendered.contains("unsupported"), "{rendered}");
    assert!(!rendered.contains("defaulted_slots: [1]"), "{rendered}");
    Ok(())
}

/// A Rust `&'static str` constant used as Incan text reads the native item at its borrowed native type.
#[test]
fn static_text_constant_reads_its_native_item() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "from rust::demo import GREETING\n\ndef shout() -> str:\n    text: str = GREETING\n    return text + \"!\"\n";
    let module = greeting_module(source)?;
    let rendered = format!("{:?}", module.bodies);
    assert!(rendered.contains("RustItem(\"demo::GREETING\""), "{rendered}");
    assert!(!rendered.contains("unsupported"), "{rendered}");
    Ok(())
}

/// An unannotated `mut` local first bound to a Rust `&'static str` constant owns its text, as an annotated `str` local
/// does, so a later Incan text constant fits its storage.
#[test]
fn rebound_static_text_constant_local_owns_its_text() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from rust::demo import GREETING\n\nconst LOCAL: str = \"local\"\n\ndef pick(flag: bool) -> str:\n    mut text = GREETING\n    if flag:\n        text = LOCAL\n    return text\n";
    let module = greeting_module(source)?;
    let pick = module
        .bodies
        .iter()
        .find(|body| body.name == "pick")
        .ok_or("missing pick body")?;
    let text = pick
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("text"))
        .ok_or("missing text local")?;
    assert_eq!(
        text.ty,
        super::IncanType::Primitive(super::IncanPrimitiveType::Str),
        "{pick:?}"
    );
    let rendered = format!("{:?}", module.bodies);
    assert!(rendered.contains("RustItem(\"demo::GREETING\""), "{rendered}");
    assert!(!rendered.contains("unsupported"), "{rendered}");
    Ok(())
}

/// The Body IR module of `source`, checked against a crate whose `demo::GREETING` is a `&'static str` constant.
fn greeting_module(source: &str) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let mut loaves = LoafIndex::default();
    loaves.add_rust_facts(
        &Default::default(),
        BTreeMap::from([(
            "demo::GREETING".to_owned(),
            RustItemMetadata {
                canonical_path: "demo::GREETING".into(),
                definition_path: None,
                visibility: RustVisibility::Public,
                kind: RustItemKind::Constant {
                    type_display: "&'static str".into(),
                },
            },
        )]),
    );
    let mut checker = TypeChecker::new();
    checker.set_loaves(Arc::new(loaves));
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let facts = checker.into_type_info();
    let providers = ProviderOperationCatalog::new();
    let context = BodyIrContext {
        modules: crate::body_ir::ContextModules::default(),
        provider_operations: &providers,
        registry_package: None,
        library_package: None,
        include_inline_tests: false,
    };
    Ok(super::build_module_seed(&program, &[], &facts, &context))
}

/// A non-generic Rust function's byte-buffer result is Incan bytes: it compares with bytes and returns as bytes.
#[test]
fn byte_buffer_result_is_incan_bytes_in_body_ir() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from rust::demo import encode\n\ndef run() -> Result[bytes, str]:\n    value = encode()?\n    if value == b\"x\":\n        return Ok(b\"\")\n    return Ok(value)\n";
    let mut loaves = LoafIndex::default();
    loaves.add_rust_facts(
        &Default::default(),
        BTreeMap::from([(
            "demo::encode".to_owned(),
            RustItemMetadata {
                canonical_path: "demo::encode".into(),
                definition_path: None,
                visibility: RustVisibility::Public,
                kind: RustItemKind::Function(RustFunctionSig {
                    receiver_contract: None,
                    type_params: Vec::new(),
                    bounds: Vec::new(),
                    params: Vec::new(),
                    return_type: "std::result::Result<std::vec::Vec<u8>, std::string::String>".into(),
                    is_async: false,
                    is_unsafe: false,
                }),
            },
        )]),
    );
    let mut checker = TypeChecker::new();
    checker.set_loaves(Arc::new(loaves));
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let facts = checker.into_type_info();
    let providers = ProviderOperationCatalog::new();
    let context = BodyIrContext {
        modules: crate::body_ir::ContextModules::default(),
        provider_operations: &providers,
        registry_package: None,
        library_package: None,
        include_inline_tests: false,
    };
    let module = super::build_module_seed(&program, &[], &facts, &context);
    let rendered = format!("{:?}", module.bodies);
    assert!(rendered.contains("demo::encode"), "{rendered}");
    assert!(!rendered.contains("unsupported"), "{rendered}");
    Ok(())
}

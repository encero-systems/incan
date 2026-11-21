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
    let module = super::build_module_seed(&program, &[], &facts, &context);
    let rendered = format!("{:?}", module.bodies);
    assert!(rendered.contains("RustItem(\"demo::GREETING\""), "{rendered}");
    assert!(!rendered.contains("unsupported"), "{rendered}");
    Ok(())
}

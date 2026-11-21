//! A Rust record field whose type holds another Rust record retains that record's layout, so lowering can carry the
//! payload even where the source never reads the record's own fields.

use super::*;
use crate::loaf_index::LoafIndex;
use incan_lang::interop::{RustFieldInfo, RustItemKind, RustItemMetadata, RustTypeInfo, RustTypeShape, RustVisibility};
use std::collections::BTreeMap;
use std::sync::Arc;

/// `demo::Admitted { placement: Option<demo::Placement> }` and `demo::Placement { key: String }`.
fn placement_facts() -> BTreeMap<String, RustItemMetadata> {
    let record = |path: &str, field: RustFieldInfo| {
        (
            path.to_owned(),
            RustItemMetadata {
                canonical_path: path.into(),
                definition_path: None,
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    fields: vec![field],
                    ..RustTypeInfo::default()
                }),
            },
        )
    };
    BTreeMap::from([
        record(
            "demo::Admitted",
            RustFieldInfo {
                name: "placement".into(),
                type_display: "core::option::Option<demo::Placement>".into(),
                type_shape: RustTypeShape::Option(Box::new(RustTypeShape::RustPath {
                    path: "demo::Placement".into(),
                    args: Vec::new(),
                })),
            },
        ),
        record(
            "demo::Placement",
            RustFieldInfo {
                name: "key".into(),
                type_display: "alloc::string::String".into(),
                type_shape: RustTypeShape::Str,
            },
        ),
    ])
}

/// Testing an `Option` of a Rust record for `None` retains the record's layout beside its owner's.
#[test]
fn optional_rust_record_field_retains_its_payload_layout() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from rust::demo import Admitted\n\ndef placed(admitted: Admitted) -> bool:\n    return admitted.placement is None\n";
    let mut loaves = LoafIndex::default();
    loaves.add_rust_facts(&Default::default(), placement_facts());
    let mut checker = TypeChecker::new();
    checker.set_loaves(Arc::new(loaves));
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let facts = checker.into_type_info();
    let layouts: Vec<&String> = facts.rust.checked_layouts.keys().collect();
    assert!(facts.rust.checked_layouts.contains_key("demo::Admitted"), "{layouts:?}");
    assert!(
        facts.rust.checked_layouts.contains_key("demo::Placement"),
        "{layouts:?}"
    );
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
    assert!(!rendered.contains("unsupported"), "{rendered}");
    Ok(())
}

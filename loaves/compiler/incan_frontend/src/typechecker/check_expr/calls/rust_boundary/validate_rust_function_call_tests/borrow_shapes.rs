//! Native identity prewarming, generic boundaries, and Rust reference and trait-object borrow shapes.

use super::*;

#[test]
fn rust_identity_prewarm_skips_owner_relative_standard_wrappers() {
    assert!(
        !TypeChecker::rust_identity_metadata_base_should_probe("datafusion_expr::expr::Box"),
        "owner-relative Box displays are generic wrappers, not dependency metadata items"
    );
    assert!(
        !TypeChecker::rust_identity_metadata_base_should_probe("datafusion_expr::expr::Arc"),
        "owner-relative Arc displays are generic wrappers, not dependency metadata items"
    );
    assert!(
        !TypeChecker::rust_identity_metadata_base_should_probe("std::option::Option"),
        "std wrapper displays should not trigger identity metadata prewarm"
    );
    assert!(
        TypeChecker::rust_identity_metadata_base_should_probe("datafusion_expr::expr::WindowFunction"),
        "nominal dependency item displays should still reuse cache-only identity metadata"
    );
}

#[test]
fn exact_generic_rust_boundary_preserves_nested_borrowed_return() {
    let mut checker = TypeChecker::new();
    let span = Span::new(10, 20);
    let argument = Spanned::new(Expr::Ident("borrowed".to_string()), span);
    checker.type_info.rust.return_coercions.insert(
        span,
        RustArgCoercionInfo {
            rust_target_type: "String".to_string(),
            target_type: ResolvedType::Str,
            kind: RustArgCoercionKind::Builtin(CoercionPolicy::Exact),
        },
    );

    checker.validate_rust_boundary_value("std::string::String::from", "T", &argument, &ResolvedType::Str, false);

    assert!(
        checker.type_info.rust.return_coercions.is_empty(),
        "a direct generic Rust consumer must receive the original borrowed Rust result"
    );
    checker.record_rust_return_coercion_from_display("&str", &ResolvedType::Str, span);
    assert!(
        checker.type_info.rust.return_coercions.is_empty(),
        "later return inspection must preserve the generic consumer's native Rust value"
    );
    assert_eq!(
        checker.type_info.rust.native_return_consumers,
        std::collections::HashSet::from([span])
    );
}

#[test]
fn unresolved_rust_call_preserves_direct_native_return() {
    let mut checker = TypeChecker::new();
    let span = Span::new(30, 45);
    let argument = Spanned::new(Expr::Ident("borrowed".to_string()), span);
    checker.type_info.rust.return_coercions.insert(
        span,
        RustArgCoercionInfo {
            rust_target_type: "String".to_string(),
            target_type: ResolvedType::Str,
            kind: RustArgCoercionKind::Builtin(CoercionPolicy::Exact),
        },
    );

    checker.preserve_unresolved_rust_call_argument_returns(&[CallArg::Positional(argument)]);

    assert!(checker.type_info.rust.return_coercions.is_empty());
    assert!(checker.type_info.rust.native_return_consumers.contains(&span));
}

#[test]
fn rust_concrete_reference_parameters_record_the_required_borrow_shape() {
    let checker = TypeChecker::new();
    let header = ResolvedType::RustPath("demo::Header".to_string());

    assert_eq!(
        checker.rust_arg_boundary_match(&header, "&demo::Header"),
        RustArgBoundaryMatch::Coercion(RustArgCoercionKind::Borrow { mutable: false })
    );
    assert_eq!(
        checker.rust_arg_boundary_match(&header, "&mut demo::Header"),
        RustArgBoundaryMatch::Coercion(RustArgCoercionKind::Borrow { mutable: true })
    );
    assert_eq!(
        checker.rust_arg_boundary_match(&ResolvedType::Ref(Box::new(header.clone())), "&demo::Header"),
        RustArgBoundaryMatch::Exact
    );
    assert_eq!(
        checker.rust_arg_boundary_match(&ResolvedType::RefMut(Box::new(header)), "&mut demo::Header"),
        RustArgBoundaryMatch::Exact
    );
}

/// Native collection namespaces keep structural identity, public nominal owners remain import-independent, and
/// metadata aliases preserve unsized text only at the native boundary without changing source string values.
#[test]
fn canonical_rust_collection_namespaces_keep_structural_boundary_identity() {
    let mut loaves = crate::loaf_index::LoafIndex::default();
    loaves.add_rust_facts(
        &Default::default(),
        [("std::primitive::str", "str"), ("native::Text", "str")]
            .into_iter()
            .map(|(path, target)| {
                (
                    path.to_owned(),
                    RustItemMetadata {
                        canonical_path: path.to_owned(),
                        definition_path: None,
                        visibility: RustVisibility::Public,
                        kind: RustItemKind::Type(RustTypeInfo {
                            alias_target: Some(target.to_owned()),
                            ..RustTypeInfo::default()
                        }),
                    },
                )
            })
            .collect(),
    );
    let mut checker = TypeChecker::new();
    checker.set_loaves(std::sync::Arc::new(loaves));
    let strings = ResolvedType::Generic("List".to_string(), vec![ResolvedType::Str]);

    assert_eq!(
        checker.rust_arg_boundary_match(&strings, "alloc::vec::Vec<alloc::string::String>"),
        RustArgBoundaryMatch::Exact
    );
    assert_eq!(
        checker.rust_arg_boundary_match(&ResolvedType::Bytes, "alloc::vec::Vec<u8>"),
        RustArgBoundaryMatch::Exact
    );
    for display in ["str", "std::primitive::str", "native::Text"] {
        assert_eq!(
            checker.native_rust_signature_type(display, ""),
            ResolvedType::RustPath("str".into()),
            "{display} retains the metadata-proven unsized native referent"
        );
        assert_eq!(checker.resolved_type_from_rust_display(display), ResolvedType::Str);
    }
    assert_eq!(
        checker.native_rust_signature_type("&std::primitive::str", ""),
        ResolvedType::Ref(Box::new(ResolvedType::RustPath("str".into())))
    );
    for display in ["String", "std::string::String"] {
        assert_eq!(checker.native_rust_signature_type(display, ""), ResolvedType::Str);
    }
    verify_native_owner_is_import_independent();
}

/// Import aliases cannot replace a producer-authenticated public owner. Private definitions and another bound
/// extern remain behind their original public projections, even when their underlying metadata is available.
fn verify_native_owner_is_import_independent() {
    let owner = "native::types::Token";
    let first = "native::facade::Token";
    let second = "native::other::Token";
    let hidden = "native::private::Secret";
    let public_secret = "native::Secret";
    let other_extern = "renamed::Token";
    let definitions = [
        (owner, owner, RustVisibility::Public),
        (first, owner, RustVisibility::Public),
        (second, owner, RustVisibility::Public),
        (hidden, hidden, RustVisibility::Restricted),
        (public_secret, hidden, RustVisibility::Public),
        (other_extern, owner, RustVisibility::Public),
    ];
    for imports in [&[first][..], &[second][..], &[first, second][..]] {
        let mut loaves = crate::loaf_index::LoafIndex::default();
        loaves.add_rust_facts(
            &Default::default(),
            definitions
                .iter()
                .map(|(path, definition, visibility)| {
                    (
                        (*path).to_owned(),
                        RustItemMetadata {
                            canonical_path: (*path).to_owned(),
                            definition_path: (*path != *definition).then(|| (*definition).to_owned()),
                            visibility: *visibility,
                            kind: RustItemKind::Type(RustTypeInfo::default()),
                        },
                    )
                })
                .collect(),
        );
        let mut checker = TypeChecker::new();
        checker.set_loaves(std::sync::Arc::new(loaves));
        for (index, path) in imports.iter().copied().chain([public_secret, other_extern]).enumerate() {
            checker.symbols.define(Symbol {
                name: format!("Imported{index}"),
                kind: SymbolKind::RustItem(RustItemInfo {
                    crate_name: path.split("::").next().unwrap_or("").to_owned(),
                    path: path.to_owned(),
                    binding: RustImportBindingKind::FromImport,
                    metadata: checker.rust_item_metadata_for_path(path),
                }),
                span: Span::new(index, index + 1),
                scope: 0,
            });
        }
        let paths = checker.checked_rust_item_public_paths();
        for imported in imports {
            assert_eq!(paths.get(*imported).map(String::as_str), Some(owner));
        }
        assert_eq!(paths.get(public_secret).map(String::as_str), Some(public_secret));
        assert_eq!(paths.get(other_extern).map(String::as_str), Some(other_extern));
    }
}

#[test]
fn by_value_trait_object_inside_a_generic_matches_itself_on_every_route() {
    // #1229: the argument side reaches `resolved_type_from_rust_display` with the raw display while the
    // parameter side is compacted first. Both must land on one normal form, or `Arc<dyn Trait>` fails to match
    // itself because one route kept the space after `dyn` and the other erased it.
    let checker = TypeChecker::new();
    let arg = checker.resolved_type_from_rust_display("alloc::sync::Arc<dyn demo::Adapter>");

    assert_eq!(
        checker.rust_arg_boundary_match(&arg, "alloc::sync::Arc<dyn demo::Adapter>"),
        RustArgBoundaryMatch::Exact
    );
    assert_eq!(
        checker.rust_arg_boundary_match(&arg, "alloc::sync::Arc< dyn demo::Adapter >"),
        RustArgBoundaryMatch::Exact
    );
    assert_eq!(
        TypeChecker::compact_rust_display("alloc::sync::Arc< dyn demo::Adapter >"),
        "alloc::sync::Arc<dyn demo::Adapter>"
    );
}

#[test]
fn anonymous_impl_trait_parameters_keep_by_value_shape_in_both_spellings() {
    for display in ["impl demo::Buf", "implBuf", "impl Buf"] {
        assert!(
            TypeChecker::is_rust_generic_type_param_display(display),
            "{display} must be a by-value generic boundary"
        );
        assert_eq!(TypeChecker::rust_display_type_var_name(display), Some(display));
    }
    assert_eq!(TypeChecker::rust_display_type_var_name("dyn demo::Adapter"), None);
    assert_eq!(TypeChecker::rust_display_type_var_name("T"), Some("T"));
}

#[test]
fn rust_trait_object_parameters_record_the_required_borrow_shape() {
    let checker = TypeChecker::new();
    let adapter = checker.resolved_type_from_rust_display("dyn demo::Adapter");
    let adapter_mut = checker.resolved_type_from_rust_display("dyn demo::AdapterMut");

    assert_eq!(
        checker.rust_arg_boundary_match(&adapter, "&dyn demo::Adapter"),
        RustArgBoundaryMatch::Coercion(RustArgCoercionKind::TraitObjectBorrow { mutable: false })
    );
    assert_eq!(
        checker.rust_arg_boundary_match(&adapter_mut, "&mut dyn demo::AdapterMut"),
        RustArgBoundaryMatch::Coercion(RustArgCoercionKind::TraitObjectBorrow { mutable: true })
    );
}

#[test]
fn mutable_rust_reference_rejects_an_immutable_binding() {
    let span = Span::new(10, 16);
    let header = ResolvedType::RustPath("demo::Header".to_string());
    let argument = Spanned::new(Expr::Ident("header".to_string()), span);

    let mut immutable_checker = TypeChecker::new();
    immutable_checker.validate_rust_boundary_value("demo::Writer", "&mut demo::Header", &argument, &header, false);
    assert!(
        immutable_checker.errors.iter().any(|error| error
            .message
            .contains("Rust parameter requires a mutable borrow of 'header'")),
        "expected an immutable-binding diagnostic, got {:?}",
        immutable_checker.errors
    );

    let mut mutable_checker = TypeChecker::new();
    mutable_checker.mutable_bindings.insert("header".to_string());
    mutable_checker.validate_rust_boundary_value("demo::Writer", "&mut demo::Header", &argument, &header, false);
    assert!(
        mutable_checker.errors.is_empty(),
        "mutable binding should satisfy a Rust mutable reference, got {:?}",
        mutable_checker.errors
    );
    assert_eq!(
        mutable_checker
            .type_info
            .rust
            .arg_coercions
            .get(&span)
            .map(|coercion| coercion.kind),
        Some(RustArgCoercionKind::Borrow { mutable: true })
    );
}

/// #1561: a `mut` parameter or local belongs to the body that declares it. Once a module whose function declares a
/// `mut header` parameter and a `mut` local is checked, neither name is mutable anywhere else, so an immutable
/// `header` passed to a Rust `&mut` parameter is still refused.
#[test]
fn mutable_bindings_of_one_body_do_not_reach_another_issue1561() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def first(mut header: list[int]) -> None:\n    mut local: list[int] = [1]\n    local.append(1)\n    header.append(1)\n\nclass Writer:\n    lines: list[int]\n\n    def write(mut self, mut header: list[int]) -> None:\n        header.append(1)\n        self.lines.append(1)\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("the module must check: {errors:?}"))?;
    assert!(
        checker.mutable_bindings.is_empty(),
        "each body's `mut` bindings end with the body, got {:?}",
        checker.mutable_bindings
    );

    let span = Span::new(10, 16);
    let header = ResolvedType::RustPath("demo::Header".to_string());
    let argument = Spanned::new(Expr::Ident("header".to_string()), span);
    checker.validate_rust_boundary_value("demo::Writer", "&mut demo::Header", &argument, &header, false);
    assert!(
        checker.errors.iter().any(|error| error
            .message
            .contains("Rust parameter requires a mutable borrow of 'header'")),
        "another body's `mut header` does not make this `header` mutable, got {:?}",
        checker.errors
    );
    Ok(())
}

/// A method's mutable-reference parameter rejects an immutable binding without recording a coercion.
#[test]
fn rust_method_mutable_reference_rejects_an_immutable_binding() {
    let span = Span::new(10, 16);
    let argument = Spanned::new(Expr::Ident("header".to_string()), span);
    let args = [CallArg::Positional(argument)];
    let sig = RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: Vec::new(),
        params: vec![
            RustParam {
                name: Some("self".to_string()),
                type_display: "&mut demo::Writer".to_string(),
            },
            RustParam {
                name: Some("header".to_string()),
                type_display: "&mut demo::Header".to_string(),
            },
        ],
        return_type: "()".to_string(),
        is_async: false,
        is_unsafe: false,
    };
    let mut checker = TypeChecker::new();
    let _ = checker.validate_rust_method_call(
        "rust::demo::Writer.mutate",
        &sig,
        &args,
        &[ResolvedType::RustPath("demo::Header".to_string())],
        false,
        Span::new(0, 20),
    );
    assert!(checker.errors.iter().any(|error| {
        error
            .message
            .contains("Rust parameter requires a mutable borrow of 'header'")
    }));
    assert!(checker.type_info.rust.arg_coercions.is_empty());
}

#[test]
fn mutable_rust_trait_object_borrow_rejects_immutable_binding() {
    let mut checker = TypeChecker::new();
    let span = Span::new(10, 16);
    let arg = Spanned::new(Expr::Ident("output".to_string()), span);
    let adapter_mut = checker.resolved_type_from_rust_display("dyn demo::AdapterMut");

    checker.validate_rust_boundary_value(
        "demo::Processor",
        "&mut dyn demo::AdapterMut",
        &arg,
        &adapter_mut,
        false,
    );

    assert!(
        checker.errors.iter().any(|error| error
            .message
            .contains("Rust parameter requires a mutable borrow of 'output'")),
        "expected a mutable trait-object borrow diagnostic, got {:?}",
        checker.errors
    );
}

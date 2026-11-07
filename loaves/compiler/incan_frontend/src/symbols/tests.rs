use super::*;
use crate::ast::{Span, Spanned, Type};

/// Builtins that are checker-recognized without a physical root symbol still receive the one registry identity.
#[test]
fn every_builtin_function_registry_entry_has_a_canonical_identity() -> Result<(), String> {
    let table = SymbolTable::new();
    for entry in builtins::BUILTIN_FUNCTIONS {
        let identity = table
            .builtin_function_identity(entry.id)
            .ok_or_else(|| format!("missing canonical identity for builtin `{}`", entry.canonical))?;
        assert_eq!(identity.namespace, SymbolNamespace::OrdinaryLexical);
        assert_eq!(identity.origin, SymbolOrigin::Builtin);
        assert_eq!(identity.declaration_name, entry.canonical);
        assert_eq!(identity.kind, SemanticSourceTargetKind::Builtin);
        assert_eq!(identity.scope_discriminant, None);
        assert_eq!(identity.declaration_span, HirSourceSpan::new(0, 0));
    }
    Ok(())
}

#[test]
fn compiled_package_declarations_are_minted_in_the_package_origin() -> Result<(), String> {
    let mut table = SymbolTable::new();
    table.set_package_identity(Some("incan_stdlib_system".to_string()));
    table.set_module_path(vec!["fs".to_string(), "path".to_string()]);
    let id = table.define_with_target_kind(
        Symbol {
            name: "helper".to_string(),
            kind: SymbolKind::Variable(VariableInfo {
                ty: ResolvedType::Int,
                is_mutable: false,
                is_used: false,
            }),
            span: Span::new(10, 20),
            scope: 0,
        },
        SemanticSourceTargetKind::Function,
    );
    let identity = table.identity_of(id).ok_or("missing package-owned identity")?;

    assert_eq!(
        identity.origin,
        SymbolOrigin::Package {
            library: "incan_stdlib_system".to_string(),
            module_path: vec!["fs".to_string(), "path".to_string()],
        }
    );
    assert_eq!(identity.declaration_name, "helper");
    assert_eq!(identity.kind, SemanticSourceTargetKind::Function);
    Ok(())
}

#[test]
fn shared_binding_registration_preserves_the_first_site_and_reports_the_collision() {
    let mut bindings = HashMap::new();
    let first = Span::new(4, 8);
    let second = Span::new(20, 24);

    assert_eq!(
        register_binding(&mut bindings, "name".to_string(), first),
        BindingRegistration::Registered
    );
    assert_eq!(
        register_binding(&mut bindings, "name".to_string(), second),
        BindingRegistration::Collision { existing: first }
    );
    assert_eq!(bindings.get("name"), Some(&first));
}

#[test]
fn dependency_interface_bindings_are_temporary_and_transactional() -> Result<(), String> {
    let mut table = SymbolTable::new();
    let builtin_len = table.lookup("len").ok_or("missing builtin len")?;
    let builtin_len_identity = table
        .builtin_function_identity(BuiltinFnId::Len)
        .ok_or("missing canonical builtin len identity")?;
    table.begin_dependency_interface_bindings();
    let external = table.define_import_binding(
        Symbol {
            name: "External".to_string(),
            kind: SymbolKind::Type(TypeInfo::TypeAlias),
            span: Span::new(4, 12),
            scope: 0,
        },
        None,
    );
    assert_eq!(table.lookup("External"), Some(external));

    let interface_len = table.define_import_binding(
        Symbol {
            name: "len".to_string(),
            kind: SymbolKind::Module(ModuleInfo {
                path: vec!["dependency".to_string()],
                is_python: false,
            }),
            span: Span::new(20, 23),
            scope: 0,
        },
        None,
    );
    assert_eq!(
        table.lookup("len"),
        Some(interface_len),
        "the active dependency interface must resolve before root builtins"
    );
    assert_eq!(
        table.builtin_function_identity(BuiltinFnId::Len),
        Some(builtin_len_identity),
        "explicit builtin resolution must bypass the active dependency view"
    );

    table.begin_current_scope_binding_transaction();
    let replacement = table.define_import_binding(
        Symbol {
            name: "External".to_string(),
            kind: SymbolKind::Type(TypeInfo::Builtin),
            span: Span::new(30, 38),
            scope: 0,
        },
        None,
    );
    assert_eq!(table.lookup("External"), Some(replacement));
    let previous = table.finish_current_scope_binding_transaction();
    table.restore_current_scope_binding("External", previous.get("External").copied().flatten());
    assert_eq!(
        table.lookup("External"),
        Some(external),
        "the dependency-local import transaction must restore the interface view"
    );

    table.finish_dependency_interface_bindings();
    assert_eq!(table.lookup("External"), None);
    assert_eq!(table.lookup("len"), Some(builtin_len));
    assert!(
        table.get(external).is_some(),
        "historical interface metadata remains inspectable"
    );
    Ok(())
}

/// Both construction APIs retain builtin lookup and ordinary root/child lexical scope behavior.
#[test]
fn test_scope_lookup() {
    for mut table in [SymbolTable::new(), SymbolTable::default()] {
        assert_eq!(table.current_scope_index(), 0);
        assert!(table.lookup("int").is_some());
        assert!(table.lookup("len").is_some());

        // Define in global scope.
        table.define(Symbol {
            name: "x".to_string(),
            kind: SymbolKind::Variable(VariableInfo {
                ty: ResolvedType::Int,
                is_mutable: false,
                is_used: false,
            }),
            span: Span::default(),
            scope: 0,
        });

        // A child scope can still read the root binding.
        table.enter_scope(ScopeKind::Function);
        assert!(table.lookup("x").is_some());
        table.define(Symbol {
            name: "y".to_string(),
            kind: SymbolKind::Variable(VariableInfo {
                ty: ResolvedType::Int,
                is_mutable: false,
                is_used: false,
            }),
            span: Span::default(),
            scope: 0,
        });
        assert!(table.lookup("y").is_some());

        // Leaving the child retains the root and hides its local binding.
        table.exit_scope();
        assert!(table.lookup("x").is_some());
        assert!(table.lookup("y").is_none());
    }
}

#[test]
fn test_result_type_helpers() {
    let result_type = ResolvedType::Generic(
        "Result".to_string(),
        vec![ResolvedType::Int, ResolvedType::Named("AppError".to_string())],
    );

    assert!(result_type.is_result());
    assert_eq!(result_type.result_ok_type(), Some(&ResolvedType::Int));
    assert_eq!(
        result_type.result_err_type(),
        Some(&ResolvedType::Named("AppError".to_string()))
    );
}

#[test]
fn test_function_type_resolution() {
    let symbols = SymbolTable::new();

    // The parser desugars Callable[(), int] → Type::Function([], int).
    // Verify that resolve_type handles the desugared form correctly.

    // () -> int (zero params)
    let fn_zero = Type::Function(
        vec![],
        Box::new(Spanned::new(Type::Simple("int".to_string()), Span::default())),
    );
    let ty = resolve_type(&fn_zero, &symbols);
    assert_eq!(ty, ResolvedType::Function(vec![], Box::new(ResolvedType::Int)));

    // (int) -> int (single param)
    let fn_single = Type::Function(
        vec![Spanned::new(Type::Simple("int".to_string()), Span::default())],
        Box::new(Spanned::new(Type::Simple("int".to_string()), Span::default())),
    );
    let ty = resolve_type(&fn_single, &symbols);
    assert_eq!(
        ty,
        ResolvedType::Function(
            vec![CallableParam::positional(ResolvedType::Int)],
            Box::new(ResolvedType::Int),
        )
    );

    // (int, str) -> bool (multi param)
    let fn_multi = Type::Function(
        vec![
            Spanned::new(Type::Simple("int".to_string()), Span::default()),
            Spanned::new(Type::Simple("str".to_string()), Span::default()),
        ],
        Box::new(Spanned::new(Type::Simple("bool".to_string()), Span::default())),
    );
    let ty = resolve_type(&fn_multi, &symbols);
    assert_eq!(
        ty,
        ResolvedType::Function(
            vec![
                CallableParam::positional(ResolvedType::Int),
                CallableParam::positional(ResolvedType::Str),
            ],
            Box::new(ResolvedType::Bool),
        )
    );
}

#[test]
fn resolve_type_preserves_existing_int_float_bool_names() {
    let symbols = SymbolTable::new();

    assert_eq!(
        resolve_type(&Type::Simple("int".to_string()), &symbols),
        ResolvedType::Int
    );
    assert_eq!(
        resolve_type(&Type::Simple("float".to_string()), &symbols),
        ResolvedType::Float
    );
    assert_eq!(
        resolve_type(&Type::Simple("bool".to_string()), &symbols),
        ResolvedType::Bool
    );
}

#[test]
fn resolve_type_maps_exact_width_and_alias_numeric_names() {
    let symbols = SymbolTable::new();

    assert_eq!(
        resolve_type(&Type::Simple("integer".to_string()), &symbols),
        ResolvedType::Numeric(NumericTypeId::I32)
    );
    assert_eq!(
        resolve_type(&Type::Simple("byte".to_string()), &symbols),
        ResolvedType::Numeric(NumericTypeId::U8)
    );
    assert_eq!(
        resolve_type(&Type::Simple("real".to_string()), &symbols),
        ResolvedType::Numeric(NumericTypeId::F32)
    );
}

/// RFC 009: `int` is an alias of `i64`, so every `i64` spelling resolves to the one `int` type.
#[test]
fn resolve_type_maps_every_i64_spelling_to_int() {
    let symbols = SymbolTable::new();

    for spelling in ["i64", "int", "long", "bigint"] {
        assert_eq!(
            resolve_type(&Type::Simple(spelling.to_string()), &symbols),
            ResolvedType::Int,
            "{spelling}"
        );
    }
}

/// RFC 009: `float` is an alias of `f64`, so every `f64` spelling resolves to the one `float` type.
#[test]
fn resolve_type_maps_every_f64_spelling_to_float() {
    let symbols = SymbolTable::new();

    for spelling in ["f64", "float", "double", "fp64"] {
        assert_eq!(
            resolve_type(&Type::Simple(spelling.to_string()), &symbols),
            ResolvedType::Float,
            "{spelling}"
        );
    }
}

#[test]
fn resolve_type_qualified_rust_module_item() {
    let mut table = SymbolTable::new();
    table.define(Symbol {
        name: "proto_type".to_string(),
        kind: SymbolKind::RustItem(RustItemInfo {
            crate_name: "substrait".to_string(),
            path: "substrait::proto::type".to_string(),
            binding: RustImportBindingKind::FromImport,
            metadata: None,
        }),
        span: Span::default(),
        scope: 0,
    });
    let ty = Type::Qualified(vec!["proto_type".to_string(), "Binary".to_string()]);
    let r = resolve_type(&ty, &table);
    assert_eq!(r, ResolvedType::RustPath("substrait::proto::type::Binary".to_string()));
}

#[test]
fn resolve_type_canonicalizes_generic_rust_import_and_nested_collection() {
    let mut table = SymbolTable::new();
    for (name, path) in [
        ("RustEnvelope", "rust_shadow::Envelope"),
        ("RustToken", "rust_shadow::Token"),
    ] {
        table.define(Symbol {
            name: name.to_string(),
            kind: SymbolKind::RustItem(RustItemInfo {
                crate_name: "rust_shadow".to_string(),
                path: path.to_string(),
                binding: RustImportBindingKind::FromImport,
                metadata: None,
            }),
            span: Span::default(),
            scope: 0,
        });
    }
    let ty = Type::Generic(
        "RustEnvelope".to_string(),
        vec![Spanned::new(
            Type::Generic(
                "list".to_string(),
                vec![Spanned::new(Type::Simple("RustToken".to_string()), Span::default())],
            ),
            Span::default(),
        )],
    );

    assert_eq!(
        resolve_type(&ty, &table),
        ResolvedType::RustPath("rust_shadow::Envelope<Vec<rust_shadow::Token>>".to_string())
    );
}

#[test]
fn resolve_type_retains_non_nominal_standard_generic_path() {
    let mut table = SymbolTable::new();
    table.define(Symbol {
        name: "PhantomData".to_string(),
        kind: SymbolKind::RustItem(RustItemInfo {
            crate_name: "std".to_string(),
            path: "std::marker::PhantomData".to_string(),
            binding: RustImportBindingKind::FromImport,
            metadata: None,
        }),
        span: Span::default(),
        scope: 0,
    });
    table.define(Symbol {
        name: "Payload".to_string(),
        kind: SymbolKind::Type(TypeInfo::TypeAlias),
        span: Span::default(),
        scope: 0,
    });
    let ty = Type::Generic(
        "PhantomData".to_string(),
        vec![Spanned::new(Type::Simple("Payload".to_string()), Span::default())],
    );

    assert_eq!(
        resolve_type(&ty, &table),
        ResolvedType::RustPath("std::marker::PhantomData<Payload>".to_string())
    );
}

/// Source reflection normalizes aliases recursively while diagnostic descriptors retain their existing spelling.
#[test]
fn field_surface_type_name_uses_canonical_incan_collection_spelling() {
    let source = Type::Generic(
        "Dict".to_string(),
        vec![
            Spanned::new(Type::Simple("str".to_string()), Span::default()),
            Spanned::new(Type::Simple("TelemetryValue".to_string()), Span::default()),
        ],
    );
    let resolved = ResolvedType::Generic(
        "dict".to_string(),
        vec![ResolvedType::Str, ResolvedType::Named("TelemetryValue".to_string())],
    );

    assert_eq!(field_surface_type_name(&source, &resolved), "dict[str, TelemetryValue]");

    let source = Type::Generic(
        "List".to_string(),
        vec![Spanned::new(Type::Simple("int".to_string()), Span::default())],
    );
    let resolved = ResolvedType::Generic("List".to_string(), vec![ResolvedType::Int]);

    assert_eq!(field_surface_type_name(&source, &resolved), "list[int]");

    let source = Type::Generic(
        "Option".to_string(),
        vec![Spanned::new(
            Type::Dotted(vec!["errors".to_string(), "TomlError".to_string()]),
            Span::default(),
        )],
    );
    let resolved = ResolvedType::Generic(
        "Option".to_string(),
        vec![ResolvedType::Named(
            "__incan_carrier_model_6_6572726f7273_9_546f6d6c4572726f72_0_44".to_string(),
        )],
    );

    assert_eq!(field_surface_type_name(&source, &resolved), "Option[TomlError]");

    let controls = [
        (
            ResolvedType::Generic(
                "Option".to_string(),
                vec![ResolvedType::Generic(
                    "Dict".to_string(),
                    vec![
                        ResolvedType::Str,
                        ResolvedType::Generic("List".to_string(), vec![ResolvedType::Float]),
                    ],
                )],
            ),
            "Option[dict[str, list[float]]]",
            "Option[Dict[Str, List[Float]]]",
        ),
        (
            ResolvedType::Function(
                vec![
                    CallableParam::positional(ResolvedType::Generic("List".to_string(), vec![ResolvedType::Int]))
                        .with_mut(true),
                    CallableParam::positional(ResolvedType::Ref(Box::new(ResolvedType::Str))),
                    CallableParam::named("items", ResolvedType::Bytes, ParamKind::RestPositional),
                    CallableParam::named("named", ResolvedType::Bool, ParamKind::RestKeyword),
                ],
                Box::new(ResolvedType::RefMut(Box::new(ResolvedType::Generic(
                    "Set".to_string(),
                    vec![ResolvedType::TypeVar("T".to_string())],
                )))),
            ),
            "(mut list[int], &str, *bytes, **bool) -> &mut set[T]",
            "(mut List[Int], &Str, *Bytes, **Bool) -> &mut Set[T]",
        ),
        (
            ResolvedType::FrozenDict(
                Box::new(ResolvedType::FrozenStr),
                Box::new(ResolvedType::Tuple(vec![
                    ResolvedType::FrozenList(Box::new(ResolvedType::Numeric(NumericTypeId::U8))),
                    ResolvedType::FrozenSet(Box::new(ResolvedType::FrozenBytes)),
                    ResolvedType::TypeToken(Box::new(ResolvedType::Named("ListItem".to_string()))),
                ])),
            ),
            "FrozenDict[frozenstr, (FrozenList[u8], FrozenSet[frozenbytes], Type[ListItem])]",
            "FrozenDict[FrozenStr, (FrozenList[u8], FrozenSet[FrozenBytes], Type[ListItem])]",
        ),
    ];
    for (resolved, source_name, descriptor) in controls {
        assert_eq!(field_surface_type_name(&Type::Infer, &resolved), source_name);
        assert_eq!(resolved.to_string(), descriptor);
    }
}

#[test]
fn field_surface_type_name_preserves_authored_spelling_for_nested_rust_types() {
    let source = Type::Generic(
        "Box".to_string(),
        vec![Spanned::new(
            Type::Generic(
                "RustEnvelope".to_string(),
                vec![Spanned::new(
                    Type::Generic(
                        "list".to_string(),
                        vec![Spanned::new(Type::Simple("RustToken".to_string()), Span::default())],
                    ),
                    Span::default(),
                )],
            ),
            Span::default(),
        )],
    );
    let resolved = ResolvedType::Generic(
        "Box".to_string(),
        vec![ResolvedType::RustPath(
            "rust_shadow::Envelope<Vec<rust_shadow::Token>>".to_string(),
        )],
    );

    assert_eq!(
        field_surface_type_name(&source, &resolved),
        "Box[RustEnvelope[list[RustToken]]]"
    );
}

#[test]
fn resolved_iterator_item_type_uses_canonical_trait_identity_issue950_953() {
    let qualified = ResolvedType::Generic(
        "stdlib_core::__incan_std::derives::collection::Iterator".to_string(),
        vec![ResolvedType::Tuple(vec![ResolvedType::Int, ResolvedType::Str])],
    );
    let similarly_named = ResolvedType::Generic("RecordIterator".to_string(), vec![ResolvedType::Int]);

    assert_eq!(
        qualified.iterator_item_type(),
        Some(&ResolvedType::Tuple(vec![ResolvedType::Int, ResolvedType::Str]))
    );
    assert_eq!(similarly_named.iterator_item_type(), None);
}

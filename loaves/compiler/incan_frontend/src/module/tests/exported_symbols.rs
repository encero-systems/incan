//! Exported symbols and exported type-like documentation.

use super::*;

#[test]
fn test_exported_type_like_docs_reads_body_docstrings_from_parse() -> Result<(), Vec<String>> {
    let tokens = lexer::lex(BLOCK_DOCSTRING_PUBLIC_TYPE_LIKE)
        .map_err(|e| e.iter().map(|x| x.message.clone()).collect::<Vec<_>>())?;
    let ast = parser::parse(&tokens).map_err(|e| e.iter().map(|x| x.message.clone()).collect::<Vec<_>>())?;
    let docs = exported_type_like_docs(&ast);
    assert_eq!(docs.len(), 5, "expected one entry per public type-like decl");

    let mut by_name: std::collections::HashMap<&str, &ExportedTypeLikeDoc> = std::collections::HashMap::new();
    for d in &docs {
        by_name.insert(d.name.as_str(), d);
    }

    let m = by_name
        .get("CliModelProbe")
        .ok_or_else(|| vec!["missing CliModelProbe entry".to_string()])?;
    assert_eq!(m.kind, ExportedTypeLikeKind::Model);
    assert_docstring_has_marker_lines(m.docstring.as_deref(), "CliModelProbe")?;

    let c = by_name
        .get("CliClassProbe")
        .ok_or_else(|| vec!["missing CliClassProbe entry".to_string()])?;
    assert_eq!(c.kind, ExportedTypeLikeKind::Class);
    assert_docstring_has_marker_lines(c.docstring.as_deref(), "CliClassProbe")?;

    let e = by_name
        .get("CliEnumProbe")
        .ok_or_else(|| vec!["missing CliEnumProbe entry".to_string()])?;
    assert_eq!(e.kind, ExportedTypeLikeKind::Enum);
    assert_docstring_has_marker_lines(e.docstring.as_deref(), "CliEnumProbe")?;

    let t = by_name
        .get("CliTraitProbe")
        .ok_or_else(|| vec!["missing CliTraitProbe entry".to_string()])?;
    assert_eq!(t.kind, ExportedTypeLikeKind::Trait);
    assert_docstring_has_marker_lines(t.docstring.as_deref(), "CliTraitProbe")?;

    let n = by_name
        .get("CliNewtypeProbe")
        .ok_or_else(|| vec!["missing CliNewtypeProbe entry".to_string()])?;
    assert_eq!(n.kind, ExportedTypeLikeKind::Newtype);
    assert_docstring_has_marker_lines(n.docstring.as_deref(), "CliNewtypeProbe")?;

    Ok(())
}

#[test]
fn test_exported_type_like_docs_skips_private_even_with_docstring() -> Result<(), Vec<String>> {
    let source = r#"class Secret:
    """
    Line A documents the class API.
    Line B keeps interior newlines after trim().
    """
    x: int
"#;
    let tokens = lexer::lex(source).map_err(|e| e.iter().map(|x| x.message.clone()).collect::<Vec<_>>())?;
    let ast = parser::parse(&tokens).map_err(|e| e.iter().map(|x| x.message.clone()).collect::<Vec<_>>())?;
    assert!(exported_type_like_docs(&ast).is_empty());
    Ok(())
}

// ========================================
// exported_symbols tests
// ========================================

#[test]
fn test_exported_symbols_empty_program() {
    let program = Program {
        declarations: vec![],
        rust_module_path: None,
        source_path: None,
        warnings: vec![],
    };
    let exports = exported_symbols(&program);
    assert!(exports.is_empty());
}

#[test]
fn test_exported_symbols_model() {
    let model = ModelDecl {
        visibility: Visibility::Public,
        decorators: vec![],
        name: "User".to_string(),
        type_params: vec![],
        traits: vec![],
        docstring: None,
        fields: vec![],
        method_aliases: vec![],
        method_partials: vec![],
        properties: vec![],
        methods: vec![],
    };
    let program = Program {
        declarations: vec![make_spanned(Declaration::Model(model))],
        rust_module_path: None,
        source_path: None,
        warnings: vec![],
    };
    let exports = exported_symbols(&program);
    assert_eq!(exports.len(), 1);
    match &exports[0] {
        ExportedSymbol::Type(name) => assert_eq!(name, "User"),
        _ => panic!("Expected Type export"),
    }
}

#[test]
fn test_exported_symbols_class() {
    let class = ClassDecl {
        visibility: Visibility::Public,
        decorators: vec![],
        name: "MyClass".to_string(),
        type_params: vec![],
        extends: None,
        traits: vec![],
        docstring: None,
        fields: vec![],
        method_aliases: vec![],
        method_partials: vec![],
        properties: vec![],
        methods: vec![],
        declarative_members: vec![],
    };
    let program = Program {
        declarations: vec![make_spanned(Declaration::Class(class))],
        rust_module_path: None,
        source_path: None,
        warnings: vec![],
    };
    let exports = exported_symbols(&program);
    assert_eq!(exports.len(), 1);
    match &exports[0] {
        ExportedSymbol::Type(name) => assert_eq!(name, "MyClass"),
        _ => panic!("Expected Type export"),
    }
}

#[test]
fn test_exported_symbols_enum_with_variants() {
    let enum_decl = EnumDecl {
        visibility: Visibility::Public,
        decorators: vec![],
        name: "Color".to_string(),
        type_params: vec![],
        value_type: None,
        traits: vec![],
        docstring: None,
        variants: vec![
            make_spanned(VariantDecl {
                name: "Red".to_string(),
                fields: vec![],
                value: None,
            }),
            make_spanned(VariantDecl {
                name: "Green".to_string(),
                fields: vec![],
                value: None,
            }),
            make_spanned(VariantDecl {
                name: "Blue".to_string(),
                fields: vec![],
                value: None,
            }),
        ],
        variant_aliases: vec![],
        methods: vec![],
    };
    let program = Program {
        declarations: vec![make_spanned(Declaration::Enum(enum_decl))],
        rust_module_path: None,
        source_path: None,
        warnings: vec![],
    };
    let exports = exported_symbols(&program);
    // 1 type + 3 variants = 4 exports
    assert_eq!(exports.len(), 4);

    // First should be the type
    match &exports[0] {
        ExportedSymbol::Type(name) => assert_eq!(name, "Color"),
        _ => panic!("Expected Type export"),
    }

    // Rest are variants
    match &exports[1] {
        ExportedSymbol::Variant {
            enum_name,
            variant_name,
        } => {
            assert_eq!(enum_name, "Color");
            assert_eq!(variant_name, "Red");
        }
        _ => panic!("Expected Variant export"),
    }
}

#[test]
fn test_exported_symbols_newtype() {
    let newtype = NewtypeDecl {
        visibility: Visibility::Public,
        decorators: vec![],
        name: "UserId".to_string(),
        type_params: vec![],
        is_rusttype: false,
        underlying: make_spanned(Type::Simple("i64".to_string())),
        traits: vec![],
        docstring: None,
        rebindings: vec![],
        method_aliases: vec![],
        method_partials: vec![],
        associated_types: vec![],
        interop_edges: vec![],
        methods: vec![],
    };
    let program = Program {
        declarations: vec![make_spanned(Declaration::Newtype(newtype))],
        rust_module_path: None,
        source_path: None,
        warnings: vec![],
    };
    let exports = exported_symbols(&program);
    assert_eq!(exports.len(), 1);
    match &exports[0] {
        ExportedSymbol::Type(name) => assert_eq!(name, "UserId"),
        _ => panic!("Expected Type export"),
    }
}

#[test]
fn test_exported_symbols_trait() {
    let trait_decl = TraitDecl {
        visibility: Visibility::Public,
        decorators: vec![],
        name: "Printable".to_string(),
        type_params: vec![],
        traits: vec![],
        docstring: None,
        method_aliases: vec![],
        method_partials: vec![],
        properties: vec![],
        methods: vec![],
    };
    let program = Program {
        declarations: vec![make_spanned(Declaration::Trait(trait_decl))],
        rust_module_path: None,
        source_path: None,
        warnings: vec![],
    };
    let exports = exported_symbols(&program);
    assert_eq!(exports.len(), 1);
    match &exports[0] {
        ExportedSymbol::Trait(name) => assert_eq!(name, "Printable"),
        _ => panic!("Expected Trait export"),
    }
}

#[test]
fn test_exported_symbols_function() {
    let func = FunctionDecl {
        visibility: Visibility::Public,
        decorators: vec![],
        surface_modifiers: vec![],
        name: "calculate".to_string(),
        type_params: vec![],
        params: vec![],
        return_type: make_spanned(Type::Unit),
        result_source: None,
        body: vec![],
    };
    let program = Program {
        declarations: vec![make_spanned(Declaration::Function(func))],
        rust_module_path: None,
        source_path: None,
        warnings: vec![],
    };
    let exports = exported_symbols(&program);
    assert_eq!(exports.len(), 1);
    match &exports[0] {
        ExportedSymbol::Function(name) => assert_eq!(name, "calculate"),
        _ => panic!("Expected Function export"),
    }
}

#[test]
fn test_exported_symbols_same_name_overload_functions() -> Result<(), Vec<String>> {
    let source = r#"
pub model ColumnExpr:
    pub name: str

pub model IntColumnExpr:
    pub source: str

pub model FloatColumnExpr:
    pub source: str

pub model StringColumnExpr:
    pub source: str

pub def cast(expr: ColumnExpr, target: Type[int]) -> IntColumnExpr:
    return IntColumnExpr(source=expr.name)

pub def cast(expr: ColumnExpr, target: Type[float]) -> FloatColumnExpr:
    return FloatColumnExpr(source=expr.name)

pub def cast(expr: ColumnExpr, target: Type[str]) -> StringColumnExpr:
    return StringColumnExpr(source=expr.name)

pub def cast(expr: ColumnExpr, target: str) -> ColumnExpr:
    return ColumnExpr(name=f"{expr.name}:{target}")
"#;
    let tokens = lexer::lex(source).map_err(|e| e.iter().map(|x| x.message.clone()).collect::<Vec<_>>())?;
    let ast = parser::parse(&tokens).map_err(|e| e.iter().map(|x| x.message.clone()).collect::<Vec<_>>())?;
    let exports = exported_symbols(&ast);
    assert!(
        exports
            .iter()
            .any(|export| matches!(export, ExportedSymbol::Function(name) if name == "cast")),
        "expected overloaded public function export, got {exports:?}"
    );
    Ok(())
}

#[test]
fn test_exported_symbols_partial() -> Result<(), Vec<String>> {
    let source = r#"
pub def route(method: str, path: str) -> str:
  return path

pub get = partial route(method="GET")
"#;
    let tokens = lexer::lex(source).map_err(|e| e.iter().map(|x| x.message.clone()).collect::<Vec<_>>())?;
    let ast = parser::parse(&tokens).map_err(|e| e.iter().map(|x| x.message.clone()).collect::<Vec<_>>())?;
    let exports = exported_symbols(&ast);
    assert!(
        exports
            .iter()
            .any(|export| matches!(export, ExportedSymbol::Function(name) if name == "get")),
        "expected public partial callable export, got {exports:?}"
    );
    Ok(())
}

#[test]
fn test_exported_symbols_ignores_module_imports() {
    let import = ImportDecl {
        visibility: Visibility::Private,
        kind: ImportKind::Module(ImportPath {
            segments: vec!["std".to_string()],
            is_absolute: false,
            parent_levels: 0,
        }),
        alias: None,
    };
    let program = Program {
        declarations: vec![make_spanned(Declaration::Import(import))],
        rust_module_path: None,
        source_path: None,
        warnings: vec![],
    };
    let exports = exported_symbols(&program);
    assert!(exports.is_empty());
}

#[test]
fn test_exported_symbols_reexports_from_import_items() {
    let import = ImportDecl {
        visibility: Visibility::Private,
        kind: ImportKind::From {
            module: ImportPath {
                segments: vec!["std".to_string(), "web".to_string(), "routing".to_string()],
                is_absolute: false,
                parent_levels: 0,
            },
            items: vec![
                ImportItem {
                    name: "route".to_string(),
                    alias: None,
                },
                ImportItem {
                    name: "GET".to_string(),
                    alias: Some("METHOD_GET".to_string()),
                },
            ],
        },
        alias: None,
    };
    let program = Program {
        declarations: vec![make_spanned(Declaration::Import(import))],
        rust_module_path: None,
        source_path: None,
        warnings: vec![],
    };
    let exports = exported_symbols(&program);
    assert_eq!(exports.len(), 2);
    match &exports[0] {
        ExportedSymbol::Reexported(name) => assert_eq!(name, "route"),
        _ => panic!("Expected Reexported export"),
    }
    match &exports[1] {
        ExportedSymbol::Reexported(name) => assert_eq!(name, "METHOD_GET"),
        _ => panic!("Expected Reexported export"),
    }
}

#[test]
fn test_exported_symbols_ignores_docstrings() {
    let program = Program {
        declarations: vec![make_spanned(Declaration::Docstring("Module documentation".to_string()))],
        rust_module_path: None,
        source_path: None,
        warnings: vec![],
    };
    let exports = exported_symbols(&program);
    assert!(exports.is_empty());
}

#[test]
fn test_exported_symbols_const() {
    let konst = ConstDecl {
        visibility: crate::ast::Visibility::Public,
        name: "X".to_string(),
        ty: Some(make_spanned(Type::Simple("int".to_string()))),
        value: make_spanned(Expr::Literal(Literal::Int(IntLiteral::synthetic(1)))),
    };
    let program = Program {
        declarations: vec![make_spanned(Declaration::Const(konst))],
        rust_module_path: None,
        source_path: None,
        warnings: vec![],
    };
    let exports = exported_symbols(&program);
    assert_eq!(exports.len(), 1);
    match &exports[0] {
        ExportedSymbol::Const(name) => assert_eq!(name, "X"),
        _ => panic!("Expected Const export"),
    }
}

#[test]
fn test_exported_symbols_static() {
    let static_decl = StaticDecl {
        visibility: crate::ast::Visibility::Public,
        name: "COUNTER".to_string(),
        ty: make_spanned(Type::Simple("int".to_string())),
        value: make_spanned(Expr::Literal(Literal::Int(IntLiteral::synthetic(0)))),
    };
    let program = Program {
        declarations: vec![make_spanned(Declaration::Static(static_decl))],
        rust_module_path: None,
        source_path: None,
        warnings: vec![],
    };
    let exports = exported_symbols(&program);
    assert_eq!(exports.len(), 1);
    match &exports[0] {
        ExportedSymbol::Static(name) => assert_eq!(name, "COUNTER"),
        _ => panic!("Expected Static export"),
    }
}

#[test]
fn test_exported_symbols_multiple_declarations() {
    let model = ModelDecl {
        visibility: Visibility::Public,
        decorators: vec![],
        name: "User".to_string(),
        type_params: vec![],
        traits: vec![],
        docstring: None,
        fields: vec![],
        method_aliases: vec![],
        method_partials: vec![],
        properties: vec![],
        methods: vec![],
    };
    let func = FunctionDecl {
        visibility: Visibility::Public,
        decorators: vec![],
        surface_modifiers: vec![],
        name: "create_user".to_string(),
        type_params: vec![],
        params: vec![],
        return_type: make_spanned(Type::Unit),
        result_source: None,
        body: vec![],
    };
    let trait_decl = TraitDecl {
        visibility: Visibility::Public,
        decorators: vec![],
        name: "Serializable".to_string(),
        type_params: vec![],
        traits: vec![],
        docstring: None,
        method_aliases: vec![],
        method_partials: vec![],
        properties: vec![],
        methods: vec![],
    };
    let program = Program {
        declarations: vec![
            make_spanned(Declaration::Model(model)),
            make_spanned(Declaration::Function(func)),
            make_spanned(Declaration::Trait(trait_decl)),
        ],
        rust_module_path: None,
        source_path: None,
        warnings: vec![],
    };
    let exports = exported_symbols(&program);
    assert_eq!(exports.len(), 3);
}

// ========================================
// ExportedSymbol tests
// ========================================

#[test]
fn test_exported_symbol_type_clone() {
    let sym = ExportedSymbol::Type("MyType".to_string());
    let cloned = sym.clone();
    match cloned {
        ExportedSymbol::Type(name) => assert_eq!(name, "MyType"),
        _ => panic!("Clone changed variant"),
    }
}

#[test]
fn test_exported_symbol_variant_clone() {
    let sym = ExportedSymbol::Variant {
        enum_name: "Status".to_string(),
        variant_name: "Active".to_string(),
    };
    let cloned = sym.clone();
    match cloned {
        ExportedSymbol::Variant {
            enum_name,
            variant_name,
        } => {
            assert_eq!(enum_name, "Status");
            assert_eq!(variant_name, "Active");
        }
        _ => panic!("Clone changed variant"),
    }
}

#[test]
fn test_exported_symbol_debug() {
    let sym = ExportedSymbol::Function("test".to_string());
    let debug_str = format!("{:?}", sym);
    assert!(debug_str.contains("Function"));
    assert!(debug_str.contains("test"));
}

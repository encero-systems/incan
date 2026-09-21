//! Helpers that more than one typechecker test module uses: source checking with and without a library index,
//! diagnostic filters, and the artifact-root and name fixtures that library-index builders share. A fixture builder
//! only one module needs stays private in that module.

use super::*;

pub(super) fn check_str(source: &str) -> Result<(), Vec<CompileError>> {
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    check(&ast)
}

pub(super) fn parse_program(source: &str, context: &str) -> crate::ast::Program {
    let tokens = lexer::lex(source).unwrap_or_else(|errs| panic!("{context} lex failed: {errs:?}"));
    parser::parse(&tokens).unwrap_or_else(|errs| panic!("{context} parse failed: {errs:?}"))
}

pub(super) fn check_str_err(source: &str, context: &str) -> Vec<CompileError> {
    match check_str(source) {
        Err(errs) => errs,
        Ok(()) => panic!("{context}"),
    }
}

pub(super) fn check_str_with_library_index_err(
    source: &str,
    library_index: LibraryManifestIndex,
    context: &str,
) -> Result<Vec<CompileError>, String> {
    match check_str_with_library_index(source, library_index) {
        Err(errs) => Ok(errs),
        Ok(()) => Err(context.to_string()),
    }
}

pub(super) fn check_str_warnings(source: &str, context: &str) -> Vec<CompileError> {
    let tokens = match lexer::lex(source) {
        Ok(tokens) => tokens,
        Err(errs) => panic!("{context} lex failed: {errs:?}"),
    };
    let ast = match parser::parse(&tokens) {
        Ok(ast) => ast,
        Err(errs) => panic!("{context} parse failed: {errs:?}"),
    };
    let mut checker = TypeChecker::new();
    if let Err(errs) = checker.check_program(&ast) {
        panic!("{context} typecheck failed: {errs:?}");
    }
    checker.warnings
}

pub(super) fn has_unknown_symbol_error(errors: &[CompileError], symbol: &str) -> bool {
    let needle = format!("Unknown symbol '{symbol}'");
    errors.iter().any(|err| err.message.contains(&needle))
}

pub(super) fn check_str_with_library_index(
    source: &str,
    library_index: LibraryManifestIndex,
) -> Result<(), Vec<CompileError>> {
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.set_library_manifest_index(library_index);
    checker.check_program(&ast)
}

pub(super) fn synthetic_artifact_root(name: &str) -> PathBuf {
    let mut root = std::env::temp_dir();
    root.push(format!("incan_test_{name}_artifacts"));
    root.push("target");
    root.push("lib");
    root
}

pub(super) fn shadowed_trait_name() -> String {
    builtin_traits::as_str(TraitId::Clone).to_string()
}

pub(super) fn assert_check_ok(source: &str) {
    if let Err(errs) = check_str(source) {
        for e in &errs {
            eprintln!("typecheck error: {} @ {:?}", e.message, e.span);
        }
        panic!("expected Ok, got errors (see stderr)");
    }
}

pub(super) fn has_private_field_error(errors: &[CompileError], type_name: &str, field: &str) -> bool {
    let needle = format!("Field '{field}' on '{type_name}' is private");
    errors.iter().any(|err| err.message.contains(&needle))
}

pub(super) fn library_index_with_mylib_exports() -> LibraryManifestIndex {
    let mut manifest = LibraryManifest {
        name: "mylib".to_string(),
        version: "0.1.0".to_string(),
        incan_version: incan_lang::version::INCAN_VERSION.to_string(),
        manifest_format: crate::library_manifest::LIBRARY_MANIFEST_FORMAT,
        exports: LibraryExports {
            aliases: Vec::new(),
            partials: vec![PartialExport {
                name: "make_default_widget".to_string(),
                target_path: vec!["make_widget".to_string()],
                target_kind: PartialTargetKindExport::Function,
                presets: vec![PartialPresetExport {
                    name: "name".to_string(),
                    ty: TypeRef::Named {
                        origin: None,
                        name: "str".to_string(),
                    },
                    value: PresetValueExport::String("default".to_string()),
                }],
                type_params: Vec::new(),
                params: vec![ParamExport {
                    name: "name".to_string(),
                    ty: TypeRef::Named {
                        origin: None,
                        name: "str".to_string(),
                    },
                    kind: ParamKindExport::Normal,
                    has_default: true,
                    default: None,
                }],
                return_type: TypeRef::Named {
                    origin: None,
                    name: "Widget".to_string(),
                },
                is_async: false,
            }],
            models: vec![ModelExport {
                name: "Widget".to_string(),
                type_params: Vec::new(),
                traits: Vec::new(),
                trait_adoptions: Vec::new(),
                derives: Vec::new(),
                fields: Vec::new(),
                properties: Vec::new(),
                methods: Vec::new(),
            }],
            classes: Vec::new(),
            functions: vec![
                FunctionExport {
                    name: "make_widget".to_string(),
                    emitted_name: None,
                    type_params: Vec::new(),
                    params: vec![ParamExport {
                        name: "name".to_string(),
                        ty: TypeRef::Named {
                            origin: None,
                            name: "str".to_string(),
                        },
                        kind: ParamKindExport::Normal,
                        has_default: false,
                        default: None,
                    }],
                    return_type: TypeRef::Named {
                        origin: None,
                        name: "Widget".to_string(),
                    },
                    is_async: false,
                },
                FunctionExport {
                    name: "collect_widgets".to_string(),
                    emitted_name: None,
                    type_params: Vec::new(),
                    params: Vec::new(),
                    return_type: TypeRef::Applied {
                        origin: None,
                        name: "list".to_string(),
                        args: vec![TypeRef::Named {
                            origin: None,
                            name: "Widget".to_string(),
                        }],
                    },
                    is_async: false,
                },
            ],
            traits: vec![TraitExport {
                name: "Labeled".to_string(),
                source_name: None,
                type_params: Vec::new(),
                supertraits: Vec::new(),
                requires: Vec::new(),
                methods: vec![MethodExport {
                    alias_of: None,
                    name: "label".to_string(),
                    canonical: None,
                    type_params: Vec::new(),
                    receiver: Some(ReceiverExport::Immutable),
                    params: Vec::new(),
                    return_type: TypeRef::Named {
                        origin: None,
                        name: "str".to_string(),
                    },
                    is_async: false,
                    has_body: false,
                }],
            }],
            enums: vec![EnumExport {
                name: "Status".to_string(),
                type_params: Vec::new(),
                traits: vec!["Labeled".to_string()],
                trait_adoptions: Vec::new(),
                value_type: Some(EnumValueTypeExport::Str),
                ordinal_type_identity: Some("mylib.Status".to_string()),
                variants: vec![
                    EnumVariantExport {
                        name: "Active".to_string(),
                        canonical: None,
                        fields: Vec::new(),
                        value: Some(EnumValueExport::Str("active".to_string())),
                    },
                    EnumVariantExport {
                        name: "Disabled".to_string(),
                        canonical: None,
                        fields: Vec::new(),
                        value: Some(EnumValueExport::Str("disabled".to_string())),
                    },
                ],
                variant_aliases: Vec::new(),
                methods: vec![MethodExport {
                    alias_of: None,
                    name: "label".to_string(),
                    canonical: None,
                    type_params: Vec::new(),
                    receiver: Some(ReceiverExport::Immutable),
                    params: Vec::new(),
                    return_type: TypeRef::Named {
                        origin: None,
                        name: "str".to_string(),
                    },
                    is_async: false,
                    has_body: true,
                }],
                derives: Vec::new(),
            }],
            type_aliases: vec![TypeAliasExport {
                name: "WidgetAlias".to_string(),
                type_params: Vec::new(),
                target: TypeRef::Named {
                    origin: None,
                    name: "Widget".to_string(),
                },
            }],
            newtypes: Vec::new(),
            consts: vec![ConstExport {
                name: "DEFAULT_NAME".to_string(),
                ty: TypeRef::Named {
                    origin: None,
                    name: "str".to_string(),
                },
            }],
            statics: vec![StaticExport {
                name: "SHARED_ITEMS".to_string(),
                ty: TypeRef::Applied {
                    origin: None,
                    name: "list".to_string(),
                    args: vec![TypeRef::Named {
                        origin: None,
                        name: "int".to_string(),
                    }],
                },
            }],
        },
        vocab: None,
        soft_keywords: Default::default(),
        contract_metadata: LibraryContractMetadata::default(),
        rust_abi: None,
    };
    manifest.contract_metadata.identity_graph = LibraryIdentityGraph {
        schema_version: LEGACY_LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION,
        exports: vec![ExportIdentity {
            public_name: "Widget".to_string(),
            public_path: vec!["mylib".to_string(), "Widget".to_string()],
            source_path: vec!["widgets".to_string(), "Widget".to_string()],
            kind: ExportIdentityKind::Model,
            projection: ExportIdentityProjection::Direct,
            canonical: None,
        }],
    };

    LibraryManifestIndex::from_entries(HashMap::from([(
        "mylib".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root("mylib", "mylib", synthetic_artifact_root("mylib")),
        },
    )]))
}

pub(super) fn typecheck_info_for_module(
    source: &str,
    module_path: Vec<String>,
    context: &str,
) -> Result<TypeCheckInfo, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path));
    checker
        .check_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("{context}: {errs:?}")))?;
    Ok(checker.type_info().clone())
}

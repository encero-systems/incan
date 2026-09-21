//! RFC 031 `pub::` imports of manifest symbols (aliases, partials, enums, module aliases, error cases) and provider
//! identity across split and aliased imports (#892), dependency bindings that must not leak (#898), and legacy nominal
//! identity hashing.

use super::*;

/// Build two compiled-provider manifests that export distinct models and constructors under the same short names.
fn library_index_with_colliding_pub_type_identities() -> LibraryManifestIndex {
    let provider_manifest = |library: &str| {
        let mut manifest = LibraryManifest::new(library, "0.1.0");
        manifest.exports.models.push(ModelExport {
            name: "Widget".to_string(),
            type_params: Vec::new(),
            traits: Vec::new(),
            trait_adoptions: Vec::new(),
            derives: Vec::new(),
            fields: Vec::new(),
            properties: Vec::new(),
            methods: Vec::new(),
        });
        manifest.exports.models.push(ModelExport {
            name: "Factory".to_string(),
            type_params: Vec::new(),
            traits: Vec::new(),
            trait_adoptions: Vec::new(),
            derives: Vec::new(),
            fields: vec![FieldExport {
                name: "widget".to_string(),
                canonical: None,
                ty: TypeRef::Named {
                    origin: None,
                    name: "Widget".to_string(),
                },
                surface_type_name: None,
                visibility: FieldVisibilityExport::Public,
                has_default: false,
                default: None,
                alias: None,
                description: None,
            }],
            properties: Vec::new(),
            methods: Vec::new(),
        });
        manifest.exports.functions.push(FunctionExport {
            name: "make_widget".to_string(),
            emitted_name: None,
            type_params: Vec::new(),
            params: Vec::new(),
            return_type: TypeRef::Named {
                origin: None,
                name: "Widget".to_string(),
            },
            is_async: false,
        });
        manifest.exports.functions.push(FunctionExport {
            name: "make_factory".to_string(),
            emitted_name: None,
            type_params: Vec::new(),
            params: Vec::new(),
            return_type: TypeRef::Named {
                origin: None,
                name: "Factory".to_string(),
            },
            is_async: false,
        });
        manifest.exports.enums.push(EnumExport {
            name: "Envelope".to_string(),
            type_params: Vec::new(),
            traits: Vec::new(),
            trait_adoptions: Vec::new(),
            value_type: None,
            ordinal_type_identity: None,
            variants: vec![EnumVariantExport {
                name: "WithWidget".to_string(),
                canonical: None,
                fields: vec![TypeRef::Named {
                    origin: None,
                    name: "Widget".to_string(),
                }],
                value: None,
            }],
            variant_aliases: Vec::new(),
            methods: Vec::new(),
            derives: Vec::new(),
        });
        manifest.contract_metadata.identity_graph = LibraryIdentityGraph {
            schema_version: LEGACY_LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION,
            exports: vec![
                ExportIdentity {
                    public_name: "Widget".to_string(),
                    public_path: vec![library.to_string(), "Widget".to_string()],
                    source_path: vec!["widgets".to_string(), "Widget".to_string()],
                    kind: ExportIdentityKind::Model,
                    projection: ExportIdentityProjection::Direct,
                    canonical: None,
                },
                ExportIdentity {
                    public_name: "Envelope".to_string(),
                    public_path: vec![library.to_string(), "Envelope".to_string()],
                    source_path: vec!["widgets".to_string(), "Envelope".to_string()],
                    kind: ExportIdentityKind::Enum,
                    projection: ExportIdentityProjection::Direct,
                    canonical: None,
                },
                ExportIdentity {
                    public_name: "Factory".to_string(),
                    public_path: vec![library.to_string(), "Factory".to_string()],
                    source_path: vec!["widgets".to_string(), "Factory".to_string()],
                    kind: ExportIdentityKind::Model,
                    projection: ExportIdentityProjection::Direct,
                    canonical: None,
                },
            ],
        };
        manifest
    };

    LibraryManifestIndex::from_entries(HashMap::from([
        (
            "alpha".to_string(),
            LibraryManifestIndexEntry::Loaded {
                manifest: Box::new(provider_manifest("alpha")),
                metadata: LibraryArtifactMetadata::from_crate_root(
                    "alpha",
                    "alpha",
                    synthetic_artifact_root("pub_identity_alpha"),
                ),
            },
        ),
        (
            "beta".to_string(),
            LibraryManifestIndexEntry::Loaded {
                manifest: Box::new(provider_manifest("beta")),
                metadata: LibraryArtifactMetadata::from_crate_root(
                    "beta",
                    "beta",
                    synthetic_artifact_root("pub_identity_beta"),
                ),
            },
        ),
    ]))
}

fn library_index_with_callable_alias_export() -> LibraryManifestIndex {
    let manifest = LibraryManifest {
        name: "mylib".to_string(),
        version: "0.1.0".to_string(),
        incan_version: incan_lang::version::INCAN_VERSION.to_string(),
        manifest_format: crate::library_manifest::LIBRARY_MANIFEST_FORMAT,
        exports: LibraryExports {
            aliases: vec![AliasExport {
                name: "public_target".to_string(),
                target_path: vec!["target_impl".to_string()],
                projected_type: None,
                projected_function: Some(FunctionExport {
                    name: "public_target".to_string(),
                    emitted_name: None,
                    type_params: Vec::new(),
                    params: vec![ParamExport {
                        name: "value".to_string(),
                        ty: TypeRef::Named {
                            origin: None,
                            name: "int".to_string(),
                        },
                        kind: ParamKindExport::Normal,
                        has_default: false,
                        default: None,
                    }],
                    return_type: TypeRef::Named {
                        origin: None,
                        name: "int".to_string(),
                    },
                    is_async: false,
                }),
            }],
            partials: Vec::new(),
            models: Vec::new(),
            classes: Vec::new(),
            functions: Vec::new(),
            traits: Vec::new(),
            enums: Vec::new(),
            type_aliases: Vec::new(),
            newtypes: Vec::new(),
            consts: Vec::new(),
            statics: Vec::new(),
        },
        vocab: None,
        soft_keywords: Default::default(),
        contract_metadata: LibraryContractMetadata::default(),
        rust_abi: None,
    };

    LibraryManifestIndex::from_entries(HashMap::from([(
        "mylib".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root("mylib", "mylib", synthetic_artifact_root("mylib")),
        },
    )]))
}

fn library_index_with_identity_graph_alias_collision() -> LibraryManifestIndex {
    let root_cast = FunctionExport {
        name: "cast".to_string(),
        emitted_name: None,
        type_params: Vec::new(),
        params: vec![ParamExport {
            name: "value".to_string(),
            ty: TypeRef::Named {
                origin: None,
                name: "int".to_string(),
            },
            kind: ParamKindExport::Normal,
            has_default: false,
            default: None,
        }],
        return_type: TypeRef::Named {
            origin: None,
            name: "int".to_string(),
        },
        is_async: false,
    };
    let helper_cast = ApiFunction {
        name: "cast".to_string(),
        anchor: SourceAnchor {
            id: "helpers.cast".to_string(),
            span: SourceSpan { start: 0, end: 0 },
        },
        docstring: None,
        docstring_sections: None,
        decorators: Vec::new(),
        type_params: Vec::new(),
        params: vec![ParamExport {
            name: "value".to_string(),
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
            name: "str".to_string(),
        },
        is_async: false,
    };
    let manifest = LibraryManifest {
        name: "mylib".to_string(),
        version: "0.1.0".to_string(),
        incan_version: incan_lang::version::INCAN_VERSION.to_string(),
        manifest_format: crate::library_manifest::LIBRARY_MANIFEST_FORMAT,
        exports: LibraryExports {
            aliases: vec![AliasExport {
                name: "safe_cast".to_string(),
                target_path: vec!["helpers".to_string(), "cast".to_string()],
                projected_type: None,
                projected_function: None,
            }],
            partials: Vec::new(),
            models: Vec::new(),
            classes: Vec::new(),
            functions: vec![root_cast],
            traits: Vec::new(),
            enums: Vec::new(),
            type_aliases: Vec::new(),
            newtypes: Vec::new(),
            consts: Vec::new(),
            statics: Vec::new(),
        },
        vocab: None,
        soft_keywords: Default::default(),
        contract_metadata: LibraryContractMetadata {
            native_unions: Vec::new(),
            executable_representation: None,
            models: Default::default(),
            api: Some(CheckedApiMetadataPackage {
                schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
                package: None,
                modules: vec![CheckedApiMetadata {
                    schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
                    derivable_traits: Vec::new(),
                    module_path: vec!["helpers".to_string()],
                    declarations: vec![ApiDeclaration::Function(helper_cast)],
                }],
                public_namespaces: Vec::new(),
            }),
            registry: None,
            identity_graph: LibraryIdentityGraph {
                schema_version: LEGACY_LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION,
                exports: vec![ExportIdentity {
                    public_name: "safe_cast".to_string(),
                    public_path: vec!["mylib".to_string(), "safe_cast".to_string()],
                    source_path: vec!["facade".to_string(), "safe_cast".to_string()],
                    kind: ExportIdentityKind::Alias,
                    projection: ExportIdentityProjection::Alias {
                        target_path: vec!["helpers".to_string(), "cast".to_string()],
                    },
                    canonical: None,
                }],
            },
            provider: Default::default(),
        },
        rust_abi: None,
    };

    LibraryManifestIndex::from_entries(HashMap::from([(
        "mylib".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root("mylib", "mylib", synthetic_artifact_root("mylib")),
        },
    )]))
}

fn library_index_with_trait_export() -> LibraryManifestIndex {
    let manifest = LibraryManifest {
        name: "mylib".to_string(),
        version: "0.1.0".to_string(),
        incan_version: incan_lang::version::INCAN_VERSION.to_string(),
        manifest_format: crate::library_manifest::LIBRARY_MANIFEST_FORMAT,
        exports: LibraryExports {
            aliases: Vec::new(),
            partials: Vec::new(),
            models: Vec::new(),
            classes: Vec::new(),
            functions: Vec::new(),
            traits: vec![TraitExport {
                name: "ExternBox".to_string(),
                source_name: None,
                type_params: vec![TypeParamExport {
                    name: "T".to_string(),
                    bounds: Vec::new(),
                }],
                supertraits: Vec::new(),
                requires: Vec::new(),
                methods: Vec::new(),
            }],
            enums: Vec::new(),
            type_aliases: Vec::new(),
            newtypes: Vec::new(),
            consts: Vec::new(),
            statics: Vec::new(),
        },
        vocab: None,
        soft_keywords: Default::default(),
        contract_metadata: LibraryContractMetadata::default(),
        rust_abi: None,
    };

    LibraryManifestIndex::from_entries(HashMap::from([(
        "mylib".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(
                "mylib",
                "mylib",
                synthetic_artifact_root("mylib_trait_export"),
            ),
        },
    )]))
}

#[test]
fn test_pub_from_import_manifest_symbols_typecheck() {
    let source = r#"
from pub::mylib import Widget, make_widget, DEFAULT_NAME

def build() -> Widget:
  return make_widget(DEFAULT_NAME)
"#;
    let result = check_str_with_library_index(source, library_index_with_mylib_exports());
    assert!(result.is_ok(), "expected pub import to typecheck, got: {result:?}");
}

#[test]
fn test_pub_from_import_type_alias_is_transparent() {
    let source = r#"
from pub::mylib import WidgetAlias, make_widget

def keep(widget: WidgetAlias) -> WidgetAlias:
  return widget

def build() -> WidgetAlias:
  return keep(make_widget("ok"))
"#;
    let result = check_str_with_library_index(source, library_index_with_mylib_exports());
    assert!(
        result.is_ok(),
        "expected pub-imported type alias to behave transparently, got: {result:?}"
    );
}

#[test]
fn test_pub_from_import_manifest_partial_callable_typechecks() {
    let source = r#"
from pub::mylib import Widget, make_default_widget

def build() -> Widget:
  first = make_default_widget()
  return make_default_widget(name="override")
"#;
    let result = check_str_with_library_index(source, library_index_with_mylib_exports());
    assert!(
        result.is_ok(),
        "expected pub-imported manifest partial callable to typecheck, got: {result:?}"
    );
}

#[test]
fn test_pub_from_import_manifest_callable_alias_typechecks() {
    let source = r#"
from pub::mylib import public_target

def build() -> int:
  return public_target(1)
"#;
    let result = check_str_with_library_index(source, library_index_with_callable_alias_export());
    assert!(
        result.is_ok(),
        "expected pub-imported callable alias to typecheck, got: {result:?}"
    );
}

#[test]
fn test_pub_from_import_alias_uses_identity_graph_before_short_target_lookup() {
    let source = r#"
from pub::mylib import safe_cast

def build() -> str:
  return safe_cast("ok")
"#;
    let result = check_str_with_library_index(source, library_index_with_identity_graph_alias_collision());
    assert!(
        result.is_ok(),
        "expected identity graph to resolve alias to helpers.cast instead of root cast, got: {result:?}"
    );
}

#[test]
fn test_pub_imported_enum_methods_and_trait_adoption_typecheck() {
    let source = r#"
from pub::mylib import Status, Labeled

def label_status(status: Status) -> str:
  return status.label()

def keep_labeled[T with Labeled](value: T) -> T:
  return value

def keep_status(status: Status) -> Status:
  return keep_labeled(status)
"#;
    let result = check_str_with_library_index(source, library_index_with_mylib_exports());
    assert!(
        result.is_ok(),
        "expected imported enum methods and traits to typecheck, got: {result:?}"
    );
}

#[test]
fn test_pub_from_import_manifest_symbols_are_in_symbol_table() -> Result<(), Box<dyn std::error::Error>> {
    // This test simulates what the LSP needs for completion and hover tooltips. It verifies that `pub::` symbols are
    // properly resolved and available in `checker.symbols` so that the LSP can extract their types and signatures.
    let source = "from pub::mylib import Widget, make_widget, DEFAULT_NAME\n";
    let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_library_manifest_index(library_index_with_mylib_exports());
    let _ = checker.check_program(&ast);

    // Verify Widget
    let widget_id = checker
        .symbols
        .lookup("Widget")
        .ok_or_else(|| "Widget should be in symbols".to_string())?;
    let widget_sym = checker
        .symbols
        .get(widget_id)
        .ok_or_else(|| "Widget symbol id should resolve".to_string())?;
    assert!(matches!(
        widget_sym.kind,
        crate::symbols::SymbolKind::Type(crate::symbols::TypeInfo::Model(_))
    ));

    // Verify make_widget
    let fn_id = checker
        .symbols
        .lookup("make_widget")
        .ok_or_else(|| "make_widget should be in symbols".to_string())?;
    let fn_sym = checker
        .symbols
        .get(fn_id)
        .ok_or_else(|| "make_widget symbol id should resolve".to_string())?;
    assert!(matches!(fn_sym.kind, crate::symbols::SymbolKind::Function(_)));

    // Verify DEFAULT_NAME
    let const_id = checker
        .symbols
        .lookup("DEFAULT_NAME")
        .ok_or_else(|| "DEFAULT_NAME should be in symbols".to_string())?;
    let const_sym = checker
        .symbols
        .get(const_id)
        .ok_or_else(|| "DEFAULT_NAME symbol id should resolve".to_string())?;
    assert!(matches!(const_sym.kind, crate::symbols::SymbolKind::Variable(_)));
    Ok(())
}

#[test]
fn test_type_info_records_imported_trait_metadata_for_lowering() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from pub::mylib import ExternBox

model Cell[T] with ExternBox:
  value: T
"#;
    let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_library_manifest_index(library_index_with_trait_export());
    checker
        .check_program(&ast)
        .map_err(|errs| format!("typecheck failed: {errs:?}"))?;

    let type_info = checker.type_info();
    assert_eq!(
        type_info.traits.type_params.get("ExternBox"),
        Some(&vec!["T".to_string()]),
        "Imported trait type params should be available to lowering metadata"
    );
    assert_eq!(
        type_info.traits.direct_supertraits.get("ExternBox"),
        Some(&Vec::new()),
        "Imported trait supertraits should be recorded even when empty"
    );
    Ok(())
}

#[test]
fn test_pub_from_import_unknown_library_is_error() {
    let source = "from pub::missinglib import Widget\n";
    let result = check_str_with_library_index(source, library_index_with_mylib_exports());
    let Err(errs) = result else {
        panic!("expected unknown pub library error");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("Unknown `pub::` library")),
        "Expected unknown-library diagnostic; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_pub_from_import_missing_export_is_error() {
    let source = "from pub::mylib import MissingSymbol\n";
    let result = check_str_with_library_index(source, library_index_with_mylib_exports());
    let Err(errs) = result else {
        panic!("expected missing export error");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("is not exported by `pub::mylib`")),
        "Expected missing-export diagnostic; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_pub_from_import_collision_with_local_symbol_is_error() {
    let source = r#"
def Widget() -> None:
  pass

from pub::mylib import Widget
"#;
    let result = check_str_with_library_index(source, library_index_with_mylib_exports());
    let Err(errs) = result else {
        panic!("expected collision diagnostic");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("already in scope")),
        "Expected collision diagnostic; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_pub_from_import_alias_recovers_from_collision() {
    let source = r#"
def Widget() -> None:
  pass

from pub::mylib import Widget as LibWidget, make_widget

def build() -> LibWidget:
  return make_widget("ok")
"#;
    let result = check_str_with_library_index(source, library_index_with_mylib_exports());
    assert!(result.is_ok(), "expected alias recovery to typecheck, got: {result:?}");
}

/// #1249: a public import cannot introduce either spelling of the protected print builtin.
#[test]
fn test_pub_import_alias_cannot_replace_protected_print_builtin_issue1249() -> Result<(), String> {
    for alias in ["print", "println"] {
        let source = format!("from pub::mylib import make_widget as {alias}\n");
        let errors = check_str_with_library_index_err(
            &source,
            library_index_with_mylib_exports(),
            "public import aliases must not replace protected print bindings",
        )?;
        assert_eq!(
            errors.len(),
            1,
            "protected public import alias `{alias}` should report only its primary diagnostic, got {errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("protected builtin binding")),
            "expected protected-binding diagnostic for `{alias}`, got {errors:?}"
        );
    }
    Ok(())
}

/// Regression for #892: callable signatures retain provider identity across separate import statements.
#[test]
fn test_pub_from_split_import_alias_preserves_provider_identity_issue892() {
    let source = r#"
from pub::mylib import Widget as LibWidget
from pub::mylib import make_widget

def build() -> LibWidget:
  return make_widget("ok")
"#;
    let result = check_str_with_library_index(source, library_index_with_mylib_exports());
    assert!(
        result.is_ok(),
        "expected split pub import alias to preserve identity, got: {result:?}"
    );
}

/// Regression for #892: whole-program identity collection is independent of split-import declaration order.
#[test]
fn test_pub_from_reversed_split_import_alias_preserves_provider_identity_issue892() {
    let source = r#"
from pub::mylib import make_widget
from pub::mylib import Widget as LibWidget

def build() -> LibWidget:
  return make_widget("ok")
"#;
    let result = check_str_with_library_index(source, library_index_with_mylib_exports());
    assert!(
        result.is_ok(),
        "expected reversed split pub import alias to preserve identity, got: {result:?}"
    );
}

/// Regression for #892: every local alias of one provider-owned type compares as the same nominal declaration.
#[test]
fn test_pub_from_multiple_aliases_share_provider_identity_issue892() {
    let source = r#"
from pub::mylib import Widget as FirstWidget
from pub::mylib import Widget as SecondWidget
from pub::mylib import make_widget

def make_first() -> FirstWidget:
  return make_widget("ok")

def keep_identity(value: SecondWidget) -> FirstWidget:
  return value
"#;
    let result = check_str_with_library_index(source, library_index_with_mylib_exports());
    assert!(
        result.is_ok(),
        "expected aliases of one provider type to share identity, got: {result:?}"
    );
}

/// Regression for #892: provider identity is compared recursively through generic type arguments.
#[test]
fn test_pub_from_split_alias_preserves_nested_generic_identity_issue892() {
    let source = r#"
from pub::mylib import Widget as LibWidget
from pub::mylib import collect_widgets

def collect() -> list[LibWidget]:
  return collect_widgets()
"#;
    let result = check_str_with_library_index(source, library_index_with_mylib_exports());
    assert!(
        result.is_ok(),
        "expected nested pub type identity to survive split imports, got: {result:?}"
    );
}

/// Regression for #892: matching short names from separate dependencies remain nominally distinct.
#[test]
fn test_pub_from_same_short_name_different_providers_stay_distinct_issue892() -> Result<(), String> {
    let source = r#"
from pub::alpha import Widget as AlphaWidget, make_widget as make_alpha_widget
from pub::beta import Widget as BetaWidget, make_widget as make_beta_widget

def wrong_provider() -> BetaWidget:
  return make_alpha_widget()
"#;
    let errors = check_str_with_library_index_err(
        source,
        library_index_with_colliding_pub_type_identities(),
        "different provider identities must not unify",
    )?;
    if !errors.iter().any(|error| {
        error
            .message
            .contains("Return type mismatch: expected 'BetaWidget', found 'AlphaWidget'")
    }) {
        return Err(format!("expected provider-qualified nominal mismatch, got: {errors:?}"));
    }
    Ok(())
}

/// Regression for #892: callable-only provider signatures remain qualified against an unaliased colliding type.
#[test]
fn test_pub_callable_only_same_short_name_different_providers_stays_distinct_issue892() -> Result<(), String> {
    let source = r#"
from pub::alpha import make_widget as make_alpha_widget
from pub::beta import Widget

def wrong_provider() -> Widget:
  return make_alpha_widget()
"#;
    let errors = check_str_with_library_index_err(
        source,
        library_index_with_colliding_pub_type_identities(),
        "callable-only provider identity must not unify with another provider's same-named type",
    )?;
    if !errors.iter().any(|error| {
        error
            .message
            .starts_with("Return type mismatch: expected 'Widget', found '")
    }) {
        return Err(format!(
            "expected callable-only provider-qualified mismatch, got: {errors:?}"
        ));
    }
    Ok(())
}

/// Regression for #892: qualified module calls preserve provider identity instead of reverting to a short name.
#[test]
fn test_pub_qualified_callable_same_short_name_different_providers_stays_distinct_issue892() -> Result<(), String> {
    let source = r#"
import pub::alpha as alpha
from pub::beta import Widget

def wrong_provider() -> Widget:
  return alpha.make_widget()
"#;
    let errors = check_str_with_library_index_err(
        source,
        library_index_with_colliding_pub_type_identities(),
        "qualified provider callable identity must not unify with another provider's same-named type",
    )?;
    if !errors.iter().any(|error| {
        error
            .message
            .starts_with("Return type mismatch: expected 'Widget', found '")
    }) {
        return Err(format!("expected qualified-call provider mismatch, got: {errors:?}"));
    }
    Ok(())
}

/// Regression for #892: enum variant payload metadata retains the provider identity of its manifest carrier types.
#[test]
fn test_pub_enum_variant_payload_same_short_name_different_providers_stays_distinct_issue892() -> Result<(), String> {
    let source = r#"
from pub::alpha import WithWidget
from pub::beta import Widget
"#;
    let tokens = lexer::lex(source).map_err(|errors| format!("enum payload repro lex failed: {errors:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errors| format!("enum payload repro parse failed: {errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_library_manifest_index(library_index_with_colliding_pub_type_identities());
    checker
        .check_program(&ast)
        .map_err(|errors| format!("enum payload imports should typecheck: {errors:?}"))?;
    let payload_ty = match checker.lookup_symbol("WithWidget").map(|symbol| &symbol.kind) {
        Some(SymbolKind::Variant(info)) => info
            .fields
            .first()
            .cloned()
            .ok_or("expected WithWidget payload metadata")?,
        other => return Err(format!("expected imported WithWidget variant metadata, got: {other:?}")),
    };
    if checker.types_compatible(&payload_ty, &ResolvedType::Named("Widget".to_string())) {
        return Err(format!(
            "alpha variant payload must stay distinct from beta Widget, got payload {payload_ty}"
        ));
    }
    Ok(())
}

/// Regression for #892: split enum aliases and variant imports retain one provider-owned enum identity.
#[test]
fn test_pub_split_enum_alias_and_variant_share_provider_identity_issue892() {
    let source = r#"
from pub::alpha import Envelope as ProviderEnvelope
from pub::alpha import WithWidget
from pub::alpha import make_widget

def make_envelope() -> ProviderEnvelope:
  return WithWidget(make_widget())
"#;
    let result = check_str_with_library_index(source, library_index_with_colliding_pub_type_identities());
    assert!(
        result.is_ok(),
        "expected split enum alias and variant imports to share provider identity, got: {result:?}"
    );
}

/// Regression for #892: transitive fields reached through qualified callables retain their provider identity.
#[test]
fn test_pub_transitive_field_same_short_name_different_providers_stays_distinct_issue892() -> Result<(), String> {
    let source = r#"
import pub::alpha as alpha
from pub::beta import Widget

def wrong_provider() -> Widget:
  return alpha.make_factory().widget
"#;
    let errors = check_str_with_library_index_err(
        source,
        library_index_with_colliding_pub_type_identities(),
        "transitive provider field identity must not unify with another provider's same-named type",
    )?;
    if !errors.iter().any(|error| {
        error
            .message
            .starts_with("Return type mismatch: expected 'Widget', found '")
    }) {
        return Err(format!("expected transitive provider field mismatch, got: {errors:?}"));
    }
    Ok(())
}

#[test]
fn test_dependency_import_does_not_leak_pub_bindings_into_consumer_issue898() {
    let dependency = parse_program(
        r#"
from pub::mylib import Widget, make_widget

pub def bridge() -> Widget:
  return make_widget("ok")
"#,
        "#898 dependency",
    );
    let consumer = parse_program(
        r#"
from pub::mylib import Widget, make_widget
from bridge import bridge

def build() -> Widget:
  _ = make_widget("direct")
  return bridge()
"#,
        "#898 consumer",
    );
    let mut public_checker = TypeChecker::new();
    public_checker.set_library_manifest_index(library_index_with_mylib_exports());
    public_checker.import_module(&dependency, "bridge");

    let public_result = public_checker.check_program(&consumer);

    assert!(
        public_result.is_ok(),
        "public dependency-local bindings must not collide with explicit consumer imports, got: {:?}",
        public_result.err()
    );

    let mut private_checker = TypeChecker::new();
    private_checker.set_library_manifest_index(library_index_with_mylib_exports());
    let private_result = private_checker.check_with_imports_allow_private(&consumer, &[("bridge", &dependency)]);

    assert!(
        private_result.is_ok(),
        "private dependency-local bindings must not collide with explicit consumer imports, got: {:?}",
        private_result.err()
    );
}

#[test]
fn test_dependency_import_does_not_leak_pub_type_alias_into_consumer_issue898() {
    let dependency = parse_program(
        r#"
from pub::mylib import WidgetAlias, make_widget

pub def bridge() -> WidgetAlias:
  return make_widget("ok")
"#,
        "#898 alias dependency",
    );
    let consumer = parse_program(
        r#"
from pub::mylib import WidgetAlias
from bridge import bridge

def build() -> WidgetAlias:
  return bridge()
"#,
        "#898 alias consumer",
    );
    let mut checker = TypeChecker::new();
    checker.set_library_manifest_index(library_index_with_mylib_exports());
    checker.import_module(&dependency, "bridge");

    assert!(
        !checker.type_aliases.contains_key("WidgetAlias"),
        "the dependency's imported type alias must be removed before checking the consumer"
    );
    assert!(
        checker.symbols.lookup("WidgetAlias").is_none(),
        "the dependency's imported type-alias symbol must be removed before checking the consumer"
    );

    assert!(
        checker.check_program(&consumer).is_ok(),
        "the consumer must be able to import the same alias explicitly after the dependency transaction"
    );
}

#[test]
fn test_pub_import_module_alias_resolves_manifest_exports() {
    let source = r#"
import pub::mylib as lib
from pub::mylib import Widget

def build() -> Widget:
  return lib.make_widget("ok")
"#;
    let result = check_str_with_library_index(source, library_index_with_mylib_exports());
    assert!(
        result.is_ok(),
        "expected module alias pub import to typecheck, got: {result:?}"
    );
}

#[test]
fn test_pub_from_import_enum_variant_parity() {
    let source = r#"
from pub::mylib import Status, Active

def current() -> Status:
  return Active
"#;
    let result = check_str_with_library_index(source, library_index_with_mylib_exports());
    assert!(
        result.is_ok(),
        "expected enum variant pub import to typecheck, got: {result:?}"
    );
}

#[test]
fn test_pub_import_value_enum_generated_surface_typechecks() {
    let source = r#"
from pub::mylib import Status

def current_raw(status: Status) -> str:
  return status.value()

def parse() -> Option[Status]:
  return Status.from_value("active")
"#;
    let result = check_str_with_library_index(source, library_index_with_mylib_exports());
    assert!(
        result.is_ok(),
        "expected imported value enum generated helpers to typecheck, got: {result:?}"
    );
}

#[test]
fn test_pub_import_manifest_load_failure_is_error() {
    let broken_index = LibraryManifestIndex::from_entries(HashMap::from([(
        "brokenlib".to_string(),
        LibraryManifestIndexEntry::Failed(LibraryManifestLoadFailure {
            path: synthetic_artifact_root("brokenlib").join("brokenlib.incnlib"),
            kind: LibraryManifestFailureKind::ManifestInvalid,
            message: "invalid library manifest: unsupported manifest_format 999 (expected 1)".to_string(),
        }),
    )]));

    let source = "from pub::brokenlib import Widget\n";
    let result = check_str_with_library_index(source, broken_index);
    let Err(errs) = result else {
        panic!("expected manifest-load failure diagnostic");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("Failed to load manifest")),
        "Expected manifest-load diagnostic; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn admitted_legacy_nominals_keep_distinct_source_paths_and_consistent_hashes() {
    use std::hash::{Hash, Hasher};
    let provider = crate::provider::ProviderIdentity {
        name: "legacy".into(),
        version: "1.0.0".into(),
        digest: "selected-digest".into(),
        feature_projection: Default::default(),
    };
    let make = |dependency: &str, name: &str| {
        let mut identity = super::PublicLibraryTypeIdentity::new(dependency, &["lib".into(), name.into()]);
        identity.selected_provider = Some(provider.clone());
        identity
    };
    let product = make("direct", "Product");
    let product_alias = make("facade", "Product");
    let order = make("direct", "Order");
    assert_eq!(product, product_alias);
    assert_ne!(product, order);
    let hash = |identity: &super::PublicLibraryTypeIdentity| {
        let mut state = std::collections::hash_map::DefaultHasher::new();
        identity.hash(&mut state);
        state.finish()
    };
    assert_eq!(hash(&product), hash(&product_alias));
    assert_eq!(
        std::collections::HashSet::from([product, product_alias, order]).len(),
        2
    );
}

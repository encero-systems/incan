//! RFC 031 `pub::` imports of nested module namespaces (#948) and of private fields (#883, #885): checked-API
//! namespaces, nominal identity across modules, sibling ambiguity, parent-field defaults, and provider identity of Rust
//! generic arguments.

use super::*;

fn check_type_info_with_library_index(
    source: &str,
    library_index: LibraryManifestIndex,
) -> Result<TypeCheckInfo, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_library_manifest_index(library_index);
    checker
        .check_program(&ast)
        .map_err(|errors| format!("typecheck failed: {errors:?}"))?;
    Ok(checker.type_info().clone())
}

fn library_index_with_public_module_api_issue948() -> Result<LibraryManifestIndex, String> {
    let source = r#"
pub model HyperquantIndex:
  pub size: int

pub class IndexBuilder:
  pub size: int = 3

  def build(self) -> HyperquantIndex:
    return HyperquantIndex(size=self.size)

pub enum IndexMode:
  Dense
  Sparse

pub type IndexSize = newtype int

pub const DEFAULT_SIZE: int = 4

pub def build_index(size: int = DEFAULT_SIZE) -> HyperquantIndex:
  return HyperquantIndex(size=size)

pub default_index = partial build_index(size=DEFAULT_SIZE)

def pack_bits(size: int) -> int:
  return size
"#;
    let ast = parse_program(source, "#948 public module provider");
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["hyperquant".to_string(), "index".to_string()]));
    checker
        .check_program(&ast)
        .map_err(|errors| format!("#948 public module provider should typecheck: {errors:?}"))?;
    let api_module = collect_checked_api_metadata(&ast, &checker, vec!["hyperquant".to_string(), "index".to_string()]);
    let facade_source = "pub from hyperquant.index import HyperquantIndex as PublicIndex, build_index as make_index\n";
    let facade_ast = parse_program(facade_source, "#948 public module facade");
    let facade_api =
        collect_checked_api_alias_metadata(&facade_ast, vec!["hyperquant".to_string(), "facade".to_string()]);
    let mut api_modules = vec![api_module, facade_api];
    materialize_api_alias_projections(&mut api_modules);
    let mut package = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: api_modules,
        public_namespaces: Vec::new(),
    };
    materialize_checked_api_public_namespaces(&mut package).map_err(|error| error.to_string())?;
    let mut manifest = LibraryManifest::new("modulelib", "0.1.0");
    manifest.contract_metadata.api = Some(package);
    Ok(LibraryManifestIndex::from_entries(HashMap::from([(
        "modulelib".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(
                "modulelib",
                "modulelib",
                synthetic_artifact_root("modulelib"),
            ),
        },
    )])))
}

fn library_index_with_ambiguous_public_module_api_issue948() -> Result<LibraryManifestIndex, String> {
    let mut modules = Vec::new();
    for module_path in [
        vec!["hyperquant".to_string(), "index".to_string()],
        vec!["hyperquant".to_string(), "search".to_string()],
    ] {
        let ast = parse_program(
            "pub def find(value: int) -> int:\n  return value\n",
            "#948 ambiguous public module provider",
        );
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        checker
            .check_program(&ast)
            .map_err(|errors| format!("#948 ambiguous public module provider should typecheck: {errors:?}"))?;
        modules.push(collect_checked_api_metadata(&ast, &checker, module_path));
    }
    let mut package = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules,
        public_namespaces: Vec::new(),
    };
    materialize_checked_api_public_namespaces(&mut package).map_err(|error| error.to_string())?;
    let mut manifest = LibraryManifest::new("modulelib", "0.1.0");
    manifest.contract_metadata.api = Some(package);
    Ok(LibraryManifestIndex::from_entries(HashMap::from([(
        "modulelib".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(
                "modulelib",
                "modulelib",
                synthetic_artifact_root("modulelib"),
            ),
        },
    )])))
}

fn library_index_with_root_export_namespace_collision_issue948() -> Result<LibraryManifestIndex, String> {
    let module_path = vec!["hyperquant".to_string(), "index".to_string()];
    let ast = parse_program(
        "pub def build(value: int) -> int:\n  return value\n",
        "#948 root export and namespace collision",
    );
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&ast)
        .map_err(|errors| format!("#948 collision provider should typecheck: {errors:?}"))?;
    let mut package = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![collect_checked_api_metadata(&ast, &checker, module_path)],
        public_namespaces: Vec::new(),
    };
    materialize_checked_api_public_namespaces(&mut package).map_err(|error| error.to_string())?;
    let mut manifest = LibraryManifest::new("modulelib", "0.1.0");
    manifest.exports.functions.push(FunctionExport {
        name: "hyperquant".to_string(),
        emitted_name: None,
        type_params: Vec::new(),
        params: Vec::new(),
        return_type: TypeRef::Named {
            origin: None,
            name: "int".to_string(),
        },
        is_async: false,
    });
    manifest.contract_metadata.api = Some(package);
    Ok(LibraryManifestIndex::from_entries(HashMap::from([(
        "modulelib".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(
                "modulelib",
                "modulelib",
                synthetic_artifact_root("modulelib"),
            ),
        },
    )])))
}

fn library_index_with_declaration_namespace_collision_issue948() -> Result<LibraryManifestIndex, String> {
    let mut modules = Vec::new();
    for (module_path, source) in [
        (vec!["hyperquant".to_string()], "pub def index() -> int:\n  return 1\n"),
        (
            vec!["hyperquant".to_string(), "index".to_string()],
            "pub def build() -> int:\n  return 2\n",
        ),
    ] {
        let ast = parse_program(source, "#948 declaration and child namespace collision");
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        checker
            .check_program(&ast)
            .map_err(|errors| format!("#948 declaration collision provider should typecheck: {errors:?}"))?;
        modules.push(collect_checked_api_metadata(&ast, &checker, module_path));
    }
    let mut package = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules,
        public_namespaces: Vec::new(),
    };
    materialize_checked_api_public_namespaces(&mut package).map_err(|error| error.to_string())?;
    let mut manifest = LibraryManifest::new("modulelib", "0.1.0");
    manifest.contract_metadata.api = Some(package);
    Ok(LibraryManifestIndex::from_entries(HashMap::from([(
        "modulelib".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(
                "modulelib",
                "modulelib",
                synthetic_artifact_root("modulelib"),
            ),
        },
    )])))
}

fn library_index_with_private_class_field_issue883() -> LibraryManifestIndex {
    let mut manifest = LibraryManifest::new("sealed_class_lib", "0.1.0");
    manifest.exports.classes.push(ClassExport {
        name: "Vault".to_string(),
        type_params: Vec::new(),
        extends: None,
        traits: Vec::new(),
        trait_adoptions: Vec::new(),
        derives: Vec::new(),
        fields: vec![
            FieldExport {
                name: "secret".to_string(),
                canonical: None,
                ty: TypeRef::Named {
                    origin: None,
                    name: "str".to_string(),
                },
                surface_type_name: None,
                visibility: FieldVisibilityExport::Private,
                has_default: false,
                default: None,
                alias: None,
                description: None,
            },
            FieldExport {
                name: "label".to_string(),
                canonical: None,
                ty: TypeRef::Named {
                    origin: None,
                    name: "str".to_string(),
                },
                surface_type_name: None,
                visibility: FieldVisibilityExport::Public,
                has_default: true,
                default: Some(ParamDefaultExport::Call {
                    path: vec!["defaults".to_string(), "make_label".to_string()],
                    args: vec![ParamDefaultCallArgExport {
                        name: None,
                        value: ParamDefaultExport::ConstRef(vec!["defaults".to_string(), "FALLBACK".to_string()]),
                    }],
                    signature: Some(Box::new(ParamDefaultCallSignatureExport {
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
                    })),
                }),
                alias: None,
                description: None,
            },
            FieldExport {
                name: "computed_secret".to_string(),
                canonical: None,
                ty: TypeRef::Named {
                    origin: None,
                    name: "int".to_string(),
                },
                surface_type_name: None,
                visibility: FieldVisibilityExport::Private,
                has_default: true,
                default: None,
                alias: None,
                description: None,
            },
        ],
        properties: Vec::new(),
        methods: Vec::new(),
    });
    LibraryManifestIndex::from_entries(HashMap::from([(
        "sealed_class_lib".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(
                "sealed_class_lib",
                "sealed_class_lib",
                synthetic_artifact_root("private_class_field_issue883"),
            ),
        },
    )]))
}

#[test]
fn test_pub_from_import_derives_public_module_namespace_from_checked_api_issue948() -> Result<(), String> {
    let source = r#"
from pub::modulelib import hyperquant

def build() -> int:
  return hyperquant.build_index().size
"#;
    let result = check_str_with_library_index(source, library_index_with_public_module_api_issue948()?);
    assert!(
        result.is_ok(),
        "expected a checked source directory to form a public module namespace, got: {result:?}"
    );
    Ok(())
}

#[test]
fn test_pub_nested_module_import_resolves_checked_api_declaration_issue948() -> Result<(), String> {
    let source = r#"
from pub::modulelib.hyperquant import build_index

def build() -> int:
  return build_index().size
"#;
    let result = check_str_with_library_index(source, library_index_with_public_module_api_issue948()?);
    assert!(
        result.is_ok(),
        "expected direct imports from a public module namespace to typecheck, got: {result:?}"
    );
    Ok(())
}

#[test]
fn test_pub_nested_module_direct_import_resolves_functions_and_constants_issue948() -> Result<(), String> {
    let source = r#"
import pub::modulelib.hyperquant as h

def build() -> int:
  return h.build_index(h.DEFAULT_SIZE).size
"#;
    let result = check_str_with_library_index(source, library_index_with_public_module_api_issue948()?);
    assert!(
        result.is_ok(),
        "expected an imported public module namespace to resolve checked functions and constants, got: {result:?}"
    );
    Ok(())
}

#[test]
fn test_pub_nested_module_direct_import_resolves_nominal_types_issue948() -> Result<(), String> {
    let source = r#"
import pub::modulelib.hyperquant as h

def build() -> int:
  index = h.HyperquantIndex(size=h.DEFAULT_SIZE)
  builder = h.IndexBuilder()
  size = h.IndexSize(index.size)
  mode = h.IndexMode.Dense
  if mode == h.IndexMode.Dense:
    return index.size
  return 0
"#;
    let type_info = check_type_info_with_library_index(source, library_index_with_public_module_api_issue948()?)?;
    let constant_receiver_start = source
        .find("h.DEFAULT_SIZE")
        .ok_or_else(|| "missing qualified constant receiver in fixture".to_string())?;
    assert_eq!(
        type_info.ident_kind(Span::new(constant_receiver_start, constant_receiver_start + 1)),
        Some(IdentKind::Module),
        "qualified constant receivers must retain their module classification for Rust path emission"
    );
    Ok(())
}

#[test]
fn test_pub_nested_module_import_preserves_nominal_type_identity_issue948() -> Result<(), String> {
    let source = r#"
from pub::modulelib.hyperquant import HyperquantIndex as Index

def build() -> int:
  value = Index(size=5)
  return value.size
"#;
    let result = check_str_with_library_index(source, library_index_with_public_module_api_issue948()?);
    assert!(
        result.is_ok(),
        "expected a nested public type import to retain its checked fields and constructor, got: {result:?}"
    );
    Ok(())
}

#[test]
fn test_pub_nested_module_constructor_diagnostic_uses_source_spelling_issue948() -> Result<(), String> {
    let source = "import pub::modulelib.hyperquant as h\n\ndef build() -> int:\n  return h.HyperquantIndex().size\n";
    let errors = check_str_with_library_index_err(
        source,
        library_index_with_public_module_api_issue948()?,
        "public module constructor diagnostics must not expose internal nominal identities",
    )?;
    assert!(
        errors.iter().any(|error| {
            error.message.contains("h.HyperquantIndex")
                && error.message.contains("size")
                && !error.message.contains("pub::modulelib")
        }),
        "expected the source-level constructor spelling in the missing-field diagnostic, got: {errors:?}"
    );
    Ok(())
}

#[test]
fn test_pub_nested_module_import_preserves_public_facade_aliases_issue948() -> Result<(), String> {
    let source = r#"
from pub::modulelib.hyperquant.facade import PublicIndex, make_index

def build() -> int:
  value: PublicIndex = make_index()
  return value.size
"#;
    let result = check_str_with_library_index(source, library_index_with_public_module_api_issue948()?);
    assert!(
        result.is_ok(),
        "expected nested public aliases to preserve callable and nominal target identity, got: {result:?}"
    );
    Ok(())
}

#[test]
fn test_nested_pub_module_binding_supports_aliases_and_partials_issue948() -> Result<(), String> {
    let source = r#"
import pub::modulelib.hyperquant as h

build = h.build_index
make_small = partial h.build_index(size=2)

def use() -> int:
  first = build(size=1)
  second = make_small()
  third = h.default_index()
  return first.size + second.size + third.size
"#;
    let type_info = check_type_info_with_library_index(source, library_index_with_public_module_api_issue948()?)?;
    let projection = type_info
        .partial_projection("h.default_index")
        .ok_or_else(|| "expected qualified provider partial projection metadata".to_string())?;
    assert_eq!(projection.presets.len(), 1);
    assert_eq!(projection.presets[0].name, "size");
    Ok(())
}

#[test]
fn test_parent_pub_import_records_qualified_partial_projection_issue948() -> Result<(), String> {
    let source = r#"
from pub::modulelib import hyperquant

def use() -> int:
  return hyperquant.default_index().size
"#;
    let type_info = check_type_info_with_library_index(source, library_index_with_public_module_api_issue948()?)?;
    let projection = type_info
        .partial_projection("hyperquant.default_index")
        .ok_or_else(|| "expected parent namespace import to record its qualified partial projection".to_string())?;
    assert_eq!(projection.presets.len(), 1);
    assert_eq!(projection.presets[0].name, "size");
    Ok(())
}

#[test]
fn test_pub_nested_module_direct_import_rejects_unknown_namespace_issue948() -> Result<(), String> {
    let source = "import pub::modulelib.missing as missing\n";
    let errors = check_str_with_library_index_err(
        source,
        library_index_with_public_module_api_issue948()?,
        "unknown public module namespaces must not produce permissive placeholders",
    )?;
    assert!(
        errors.iter().any(|error| {
            error.message.contains("pub::modulelib.missing")
                && error.message.contains("does not exist")
                && error.hints.iter().any(|hint| hint.contains("hyperquant"))
        }),
        "expected an actionable missing-module diagnostic, got: {errors:?}"
    );
    Ok(())
}

#[test]
fn test_pub_nested_module_import_reports_sibling_name_ambiguity_issue948() -> Result<(), String> {
    let source = "from pub::modulelib.hyperquant import find\n";
    let errors = check_str_with_library_index_err(
        source,
        library_index_with_ambiguous_public_module_api_issue948()?,
        "sibling public declarations must require a child-module-qualified import",
    )?;
    assert!(
        errors.iter().any(|error| {
            error.message.contains("find")
                && error.message.contains("pub::modulelib.hyperquant")
                && error
                    .hints
                    .iter()
                    .any(|hint| hint.contains("hyperquant.index") && hint.contains("hyperquant.search"))
        }),
        "expected deterministic child-module guidance for an ambiguous parent member, got: {errors:?}"
    );
    Ok(())
}

#[test]
fn test_pub_nested_module_qualified_call_reports_sibling_name_ambiguity_issue948() -> Result<(), String> {
    let source = "import pub::modulelib.hyperquant as h\n\ndef use() -> int:\n  return h.find(1)\n";
    let errors = check_str_with_library_index_err(
        source,
        library_index_with_ambiguous_public_module_api_issue948()?,
        "qualified sibling collisions must retain their exact source-module diagnostic",
    )?;
    assert!(
        errors.iter().any(|error| {
            error.message.contains("find")
                && error.message.contains("pub::modulelib.hyperquant")
                && error
                    .hints
                    .iter()
                    .any(|hint| hint.contains("from pub::modulelib.hyperquant.index import find"))
        }),
        "expected exact-child import guidance for a qualified ambiguous call, got: {errors:?}"
    );
    Ok(())
}

#[test]
fn test_pub_package_root_rejects_flat_export_namespace_collision_issue948() -> Result<(), String> {
    let errors = check_str_with_library_index_err(
        "from pub::modulelib import hyperquant\n",
        library_index_with_root_export_namespace_collision_issue948()?,
        "a package-root facade export and automatic child namespace must not resolve by precedence",
    )?;
    assert!(
        errors.iter().any(|error| {
            error.message.contains("hyperquant")
                && error.message.contains("ambiguous")
                && error.hints.iter().any(|hint| hint.contains("package-root export"))
                && error.hints.iter().any(|hint| hint.contains("module `hyperquant`"))
        }),
        "expected a typed root export/namespace ambiguity, got: {errors:?}"
    );
    Ok(())
}

#[test]
fn test_pub_nested_declaration_wins_over_child_namespace_issue948() -> Result<(), String> {
    let source = r#"
from pub::modulelib.hyperquant import index
from pub::modulelib.hyperquant.index import build

def use() -> int:
  return index() + build()
"#;
    let result = check_str_with_library_index(source, library_index_with_declaration_namespace_collision_issue948()?);
    assert!(
        result.is_ok(),
        "expected the directory declaration to win while the exact child namespace remains reachable, got: {result:?}"
    );
    Ok(())
}

#[test]
fn test_pub_module_namespace_does_not_expose_private_declarations_issue948() -> Result<(), String> {
    let source = "from pub::modulelib.hyperquant import pack_bits\n";
    let errors = check_str_with_library_index_err(
        source,
        library_index_with_public_module_api_issue948()?,
        "private declarations must not cross the public module boundary",
    )?;
    assert!(
        errors.iter().any(|error| {
            error.message.contains("pack_bits")
                && error.message.contains("not exported")
                && error.message.contains("modulelib.hyperquant")
        }),
        "expected a module-scoped not-exported diagnostic, got: {errors:?}"
    );
    Ok(())
}

#[test]
fn test_pub_imported_private_class_field_access_is_rejected_issue883() -> Result<(), String> {
    let source = r#"
from pub::sealed_class_lib import Vault

def leak(value: Vault) -> str:
  return value.secret
"#;
    let errors = check_str_with_library_index_err(
        source,
        library_index_with_private_class_field_issue883(),
        "compiled-library private class field access should fail typechecking",
    )?;
    assert!(
        has_private_field_error(&errors, "Vault", "secret"),
        "expected private field error, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn test_pub_imported_private_parent_field_access_is_rejected_in_child_issue883() -> Result<(), String> {
    let source = r#"
from pub::sealed_class_lib import Vault

class Child extends Vault:
  def leak(self) -> str:
    return self.secret
"#;
    let errors = check_str_with_library_index_err(
        source,
        library_index_with_private_class_field_issue883(),
        "compiled-library private parent field access should fail in a child method",
    )?;
    assert!(
        has_private_field_error(&errors, "Child", "secret"),
        "expected imported parent private field error, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn test_pub_imported_class_preserves_public_access_and_named_construction_issue883() {
    let source = r#"
from pub::sealed_class_lib import Vault

def build() -> Vault:
  return Vault(secret="authority", label="sealed")

def label(value: Vault) -> str:
  return value.label
"#;
    let result = check_str_with_library_index(source, library_index_with_private_class_field_issue883());
    assert!(
        result.is_ok(),
        "expected public access and existing named construction to typecheck, got: {result:?}"
    );
}

#[test]
fn test_imported_parent_field_default_keeps_provider_path_and_signature_issue883()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from pub::sealed_class_lib import Vault

pub class Child extends Vault:
  computed_secret: int = 0
  pub marker: int = 1
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["child".to_string()]));
    checker.set_library_manifest_index(library_index_with_private_class_field_issue883());
    checker
        .check_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

    let exports = collect_checked_public_exports(&ast, &checker);
    let manifest = LibraryManifest::from_checked_exports("child_lib".to_string(), "0.1.0".to_string(), &exports);
    let child = manifest
        .exports
        .classes
        .iter()
        .find(|class| class.name == "Child")
        .ok_or("missing Child export")?;
    let label = child
        .fields
        .iter()
        .find(|field| field.name == "label")
        .ok_or("missing inherited label field")?;

    assert_eq!(
        label.default,
        Some(ParamDefaultExport::Call {
            path: vec![
                "__incan_provider_rust".to_string(),
                "sealed_class_lib".to_string(),
                "defaults".to_string(),
                "make_label".to_string(),
            ],
            args: vec![ParamDefaultCallArgExport {
                name: None,
                value: ParamDefaultExport::ConstRef(vec![
                    "__incan_provider_rust".to_string(),
                    "sealed_class_lib".to_string(),
                    "defaults".to_string(),
                    "FALLBACK".to_string(),
                ]),
            }],
            signature: Some(Box::new(ParamDefaultCallSignatureExport {
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
            })),
        }),
        "compiled parent defaults must retain their original provider path and checked call signature"
    );
    let computed_secret = child
        .fields
        .iter()
        .find(|field| field.name == "computed_secret")
        .ok_or("missing inherited computed_secret field")?;
    assert!(computed_secret.has_default);
    assert_eq!(computed_secret.default, Some(ParamDefaultExport::Int(0)));
    Ok(())
}

#[test]
fn test_imported_parent_unmaterializable_default_requires_local_override_issue885() -> Result<(), String> {
    let source = r#"
from pub::sealed_class_lib import Vault

class Child extends Vault:
  pub marker: int = 1
"#;
    let errors = check_str_with_library_index_err(
        source,
        library_index_with_private_class_field_issue883(),
        "compiled-parent subclass should reject an inherited default the provider artifact cannot materialize",
    )?;
    assert!(
        errors.iter().any(|error| {
            error.message.contains("cannot inherit compiled default")
                && error.message.contains("computed_secret")
                && error.hints.iter().any(|hint| hint.contains("local default"))
        }),
        "expected a materialization-boundary diagnostic, got: {errors:?}"
    );
    Ok(())
}

#[test]
fn test_rust_generic_argument_retains_imported_public_provider_identity_issue885() {
    let mut checker = TypeChecker::new();
    checker.public_library_type_identities.insert(
        "Payload".to_string(),
        PublicLibraryTypeIdentity::new("compiled_parent", &["classes".to_string(), "Payload".to_string()]),
    );
    checker.symbols.define_import_binding_at_path(
        Symbol {
            name: "Payload".to_string(),
            kind: SymbolKind::Type(TypeInfo::TypeAlias),
            span: Span::default(),
            scope: 0,
        },
        None,
        vec!["pub".to_string(), "compiled_parent".to_string(), "Payload".to_string()],
    );
    checker.symbols.define(Symbol {
        name: "RustEnvelope".to_string(),
        kind: SymbolKind::RustItem(RustItemInfo {
            crate_name: "rust_shadow".to_string(),
            path: "rust_shadow::Envelope".to_string(),
            binding: RustImportBindingKind::FromImport,
            metadata: None,
        }),
        span: Span::default(),
        scope: 0,
    });
    checker.symbols.define(Symbol {
        name: "Box".to_string(),
        kind: SymbolKind::RustItem(RustItemInfo {
            crate_name: "std".to_string(),
            path: "std::boxed::Box".to_string(),
            binding: RustImportBindingKind::FromImport,
            metadata: None,
        }),
        span: Span::default(),
        scope: 0,
    });
    let ty = Type::Generic(
        "RustEnvelope".to_string(),
        vec![Spanned::new(Type::Simple("Payload".to_string()), Span::default())],
    );

    assert_eq!(
        resolve_type_with_rust_arg_renderer(
            &ty,
            &checker.symbols,
            &|arg| checker.render_provider_aware_rust_arg(arg),
            &|arg| checker.canonicalize_public_library_nominals(arg),
            &|_| None,
        ),
        ResolvedType::RustPath("rust_shadow::Envelope<compiled_parent::Payload>".to_string())
    );

    let shared_ty = Type::Generic(
        "Box".to_string(),
        vec![Spanned::new(Type::Simple("Payload".to_string()), Span::default())],
    );
    assert_eq!(
        resolve_type_with_rust_arg_renderer(
            &shared_ty,
            &checker.symbols,
            &|arg| checker.render_provider_aware_rust_arg(arg),
            &|arg| checker.canonicalize_public_library_nominals(arg),
            &|_| None,
        ),
        ResolvedType::Generic(
            "Box".to_string(),
            vec![ResolvedType::Named("pub::compiled_parent::Payload".to_string())]
        )
    );
}

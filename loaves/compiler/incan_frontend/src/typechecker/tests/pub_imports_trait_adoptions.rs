//! RFC 025 trait adoptions and boundary type fidelity through `pub::` imports: multi-instantiation adoptions, checked
//! public exports, and transitive method and trait lookups.

use super::*;

fn none_constructor_name() -> String {
    surface_constructors::as_str(ConstructorId::None).to_string()
}

fn library_index_with_rfc025_trait_adoptions() -> LibraryManifestIndex {
    let convert_int = TypeBoundExport {
        name: "Convert".to_string(),
        source_name: None,
        module_path: None,
        type_args: vec![TypeRef::Named {
            origin: None,
            name: "int".to_string(),
        }],
        implementation_type_params: Vec::new(),
        inferred: false,
    };
    let convert_float = TypeBoundExport {
        name: "Convert".to_string(),
        source_name: None,
        module_path: None,
        type_args: vec![TypeRef::Named {
            origin: None,
            name: "float".to_string(),
        }],
        implementation_type_params: Vec::new(),
        inferred: false,
    };
    let manifest = LibraryManifest {
        name: "mylib".to_string(),
        version: "0.1.0".to_string(),
        incan_version: incan_lang::version::INCAN_VERSION.to_string(),
        manifest_format: crate::library_manifest::LIBRARY_MANIFEST_FORMAT,
        exports: LibraryExports {
            aliases: Vec::new(),
            partials: Vec::new(),
            models: vec![ModelExport {
                name: "ImportedReading".to_string(),
                type_params: Vec::new(),
                traits: vec!["Convert".to_string(), "Convert".to_string()],
                trait_adoptions: vec![convert_int.clone(), convert_float.clone()],
                derives: Vec::new(),
                fields: Vec::new(),
                properties: Vec::new(),
                methods: vec![
                    MethodExport {
                        alias_of: None,
                        name: "convert".to_string(),
                        canonical: None,
                        type_params: Vec::new(),
                        receiver: Some(ReceiverExport::Immutable),
                        params: Vec::new(),
                        return_type: TypeRef::Named {
                            origin: None,
                            name: "int".to_string(),
                        },
                        is_async: false,
                        has_body: true,
                    },
                    MethodExport {
                        alias_of: None,
                        name: "convert".to_string(),
                        canonical: None,
                        type_params: Vec::new(),
                        receiver: Some(ReceiverExport::Immutable),
                        params: Vec::new(),
                        return_type: TypeRef::Named {
                            origin: None,
                            name: "float".to_string(),
                        },
                        is_async: false,
                        has_body: true,
                    },
                ],
            }],
            classes: Vec::new(),
            functions: Vec::new(),
            traits: vec![TraitExport {
                name: "Convert".to_string(),
                source_name: None,
                type_params: vec![TypeParamExport {
                    name: "T".to_string(),
                    bounds: Vec::new(),
                }],
                supertraits: Vec::new(),
                requires: Vec::new(),
                methods: vec![MethodExport {
                    alias_of: None,
                    name: "convert".to_string(),
                    canonical: None,
                    type_params: Vec::new(),
                    receiver: Some(ReceiverExport::Immutable),
                    params: Vec::new(),
                    return_type: TypeRef::TypeParam { name: "T".to_string() },
                    is_async: false,
                    has_body: false,
                }],
            }],
            enums: vec![EnumExport {
                name: "ImportedToken".to_string(),
                type_params: Vec::new(),
                traits: vec!["Convert".to_string(), "Convert".to_string()],
                trait_adoptions: vec![convert_int.clone(), convert_float.clone()],
                value_type: None,
                ordinal_type_identity: None,
                variants: vec![EnumVariantExport {
                    name: "Number".to_string(),
                    canonical: None,
                    fields: Vec::new(),
                    value: None,
                }],
                variant_aliases: Vec::new(),
                methods: vec![
                    MethodExport {
                        alias_of: None,
                        name: "convert".to_string(),
                        canonical: None,
                        type_params: Vec::new(),
                        receiver: Some(ReceiverExport::Immutable),
                        params: Vec::new(),
                        return_type: TypeRef::Named {
                            origin: None,
                            name: "int".to_string(),
                        },
                        is_async: false,
                        has_body: true,
                    },
                    MethodExport {
                        alias_of: None,
                        name: "convert".to_string(),
                        canonical: None,
                        type_params: Vec::new(),
                        receiver: Some(ReceiverExport::Immutable),
                        params: Vec::new(),
                        return_type: TypeRef::Named {
                            origin: None,
                            name: "float".to_string(),
                        },
                        is_async: false,
                        has_body: true,
                    },
                ],
                derives: Vec::new(),
            }],
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
                synthetic_artifact_root("mylib_rfc025"),
            ),
        },
    )]))
}

fn library_index_with_pub_boundary_type_fidelity_exports() -> LibraryManifestIndex {
    let type_param_t = TypeParamExport {
        name: "T".to_string(),
        bounds: Vec::new(),
    };
    let manifest = LibraryManifest {
        name: "pubdemo".to_string(),
        version: "0.1.0".to_string(),
        incan_version: incan_lang::version::INCAN_VERSION.to_string(),
        manifest_format: crate::library_manifest::LIBRARY_MANIFEST_FORMAT,
        exports: LibraryExports {
            aliases: Vec::new(),
            partials: Vec::new(),
            models: vec![ModelExport {
                name: "SessionError".to_string(),
                type_params: Vec::new(),
                traits: Vec::new(),
                trait_adoptions: Vec::new(),
                derives: Vec::new(),
                fields: Vec::new(),
                properties: Vec::new(),
                methods: Vec::new(),
            }],
            classes: vec![
                ClassExport {
                    name: "Session".to_string(),
                    type_params: Vec::new(),
                    extends: None,
                    traits: Vec::new(),
                    trait_adoptions: Vec::new(),
                    derives: Vec::new(),
                    fields: Vec::new(),
                    properties: Vec::new(),
                    methods: vec![
                        MethodExport {
                            alias_of: None,
                            name: "default".to_string(),
                            canonical: None,
                            type_params: Vec::new(),
                            receiver: None,
                            params: Vec::new(),
                            return_type: TypeRef::Named {
                                origin: None,
                                name: "Session".to_string(),
                            },
                            is_async: false,
                            has_body: true,
                        },
                        MethodExport {
                            alias_of: None,
                            name: "read_csv".to_string(),
                            canonical: None,
                            type_params: vec![type_param_t.clone()],
                            receiver: Some(ReceiverExport::Mutable),
                            params: vec![
                                ParamExport {
                                    name: "logical_name".to_string(),
                                    ty: TypeRef::Named {
                                        origin: None,
                                        name: "str".to_string(),
                                    },
                                    kind: ParamKindExport::Normal,
                                    has_default: false,
                                    default: None,
                                },
                                ParamExport {
                                    name: "uri".to_string(),
                                    ty: TypeRef::Named {
                                        origin: None,
                                        name: "str".to_string(),
                                    },
                                    kind: ParamKindExport::Normal,
                                    has_default: false,
                                    default: None,
                                },
                            ],
                            return_type: TypeRef::Applied {
                                origin: None,
                                name: "Result".to_string(),
                                args: vec![
                                    TypeRef::Applied {
                                        origin: None,
                                        name: "LazyFrame".to_string(),
                                        args: vec![TypeRef::TypeParam { name: "T".to_string() }],
                                    },
                                    TypeRef::Named {
                                        origin: None,
                                        name: "SessionError".to_string(),
                                    },
                                ],
                            },
                            is_async: false,
                            has_body: true,
                        },
                        MethodExport {
                            alias_of: None,
                            name: "collect".to_string(),
                            canonical: None,
                            type_params: vec![type_param_t.clone()],
                            receiver: Some(ReceiverExport::Immutable),
                            params: vec![ParamExport {
                                name: "data".to_string(),
                                ty: TypeRef::Applied {
                                    origin: None,
                                    name: "LazyFrame".to_string(),
                                    args: vec![TypeRef::TypeParam { name: "T".to_string() }],
                                },
                                kind: ParamKindExport::Normal,
                                has_default: false,
                                default: None,
                            }],
                            return_type: TypeRef::Applied {
                                origin: None,
                                name: "Result".to_string(),
                                args: vec![
                                    TypeRef::Applied {
                                        origin: None,
                                        name: "DataFrame".to_string(),
                                        args: vec![TypeRef::TypeParam { name: "T".to_string() }],
                                    },
                                    TypeRef::Named {
                                        origin: None,
                                        name: "SessionError".to_string(),
                                    },
                                ],
                            },
                            is_async: false,
                            has_body: true,
                        },
                    ],
                },
                ClassExport {
                    name: "DataFrame".to_string(),
                    type_params: vec![type_param_t.clone()],
                    extends: None,
                    traits: vec!["BoundedDataSet".to_string()],
                    trait_adoptions: vec![TypeBoundExport {
                        name: "BoundedDataSet".to_string(),
                        source_name: None,
                        module_path: None,
                        type_args: Vec::new(),
                        implementation_type_params: Vec::new(),
                        inferred: false,
                    }],
                    derives: vec![shadowed_trait_name()],
                    fields: Vec::new(),
                    properties: Vec::new(),
                    methods: Vec::new(),
                },
                ClassExport {
                    name: "LazyFrame".to_string(),
                    type_params: vec![type_param_t.clone()],
                    extends: None,
                    traits: vec!["BoundedDataSet".to_string()],
                    trait_adoptions: vec![TypeBoundExport {
                        name: "BoundedDataSet".to_string(),
                        source_name: None,
                        module_path: None,
                        type_args: Vec::new(),
                        implementation_type_params: Vec::new(),
                        inferred: false,
                    }],
                    derives: vec![shadowed_trait_name()],
                    fields: Vec::new(),
                    properties: Vec::new(),
                    methods: vec![MethodExport {
                        alias_of: None,
                        name: "collect".to_string(),
                        canonical: None,
                        type_params: Vec::new(),
                        receiver: Some(ReceiverExport::Immutable),
                        params: Vec::new(),
                        return_type: TypeRef::Applied {
                            origin: None,
                            name: "Result".to_string(),
                            args: vec![
                                TypeRef::Applied {
                                    origin: None,
                                    name: "DataFrame".to_string(),
                                    args: vec![TypeRef::TypeParam { name: "T".to_string() }],
                                },
                                TypeRef::Named {
                                    origin: None,
                                    name: "SessionError".to_string(),
                                },
                            ],
                        },
                        is_async: false,
                        has_body: true,
                    }],
                },
            ],
            functions: vec![FunctionExport {
                name: "display".to_string(),
                emitted_name: None,
                type_params: vec![type_param_t.clone()],
                params: vec![ParamExport {
                    name: "data".to_string(),
                    ty: TypeRef::Applied {
                        origin: None,
                        name: "DataSet".to_string(),
                        args: vec![TypeRef::TypeParam { name: "T".to_string() }],
                    },
                    kind: ParamKindExport::Normal,
                    has_default: false,
                    default: None,
                }],
                return_type: TypeRef::Named {
                    origin: None,
                    name: none_constructor_name(),
                },
                is_async: false,
            }],
            traits: vec![
                TraitExport {
                    name: "DataSet".to_string(),
                    source_name: None,
                    type_params: vec![type_param_t.clone()],
                    supertraits: Vec::new(),
                    requires: Vec::new(),
                    methods: Vec::new(),
                },
                TraitExport {
                    name: "BoundedDataSet".to_string(),
                    source_name: None,
                    type_params: vec![type_param_t],
                    supertraits: vec![TypeBoundExport {
                        name: "DataSet".to_string(),
                        source_name: None,
                        module_path: None,
                        type_args: vec![TypeRef::TypeParam { name: "T".to_string() }],
                        implementation_type_params: Vec::new(),
                        inferred: false,
                    }],
                    requires: Vec::new(),
                    methods: Vec::new(),
                },
            ],
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
        "pubdemo".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(
                "pubdemo",
                "pubdemo",
                synthetic_artifact_root("pub_boundary_type_fidelity"),
            ),
        },
    )]))
}

#[test]
fn test_pub_import_multi_instantiation_trait_adoptions_typecheck() -> Result<(), Vec<CompileError>> {
    let source = r#"
from pub::mylib import Convert, ImportedReading

def read_float[T with Convert[float]](value: T) -> float:
  precise: float = value.convert()
  return precise

def direct(reading: ImportedReading) -> float:
  precise: float = reading.convert()
  return precise

def main(reading: ImportedReading) -> float:
  return read_float[ImportedReading](reading)
"#;

    check_str_with_library_index(source, library_index_with_rfc025_trait_adoptions())
}

#[test]
fn test_pub_import_enum_multi_instantiation_trait_adoptions_typecheck() -> Result<(), Vec<CompileError>> {
    let source = r#"
from pub::mylib import Convert, ImportedToken

def read_float[T with Convert[float]](value: T) -> float:
  precise: float = value.convert()
  return precise

def direct(token: ImportedToken) -> float:
  precise: float = token.convert()
  return precise

def main(token: ImportedToken) -> float:
  return read_float[ImportedToken](token)
"#;

    check_str_with_library_index(source, library_index_with_rfc025_trait_adoptions())
}

#[test]
fn test_checked_public_exports_preserve_same_name_trait_methods() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
pub trait Convert[T]:
  def convert(self) -> T: ...

pub model Reading with Convert[int], Convert[float]:
  pub value: int

  def convert(self) -> int:
    return self.value

  def convert(self) -> float:
    return 1.0
"#;

    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

    let exports = collect_checked_public_exports(&ast, &checker);
    let manifest = LibraryManifest::from_checked_exports("mylib".to_string(), "0.1.0".to_string(), &exports);
    let Some(reading) = manifest.exports.models.iter().find(|model| model.name == "Reading") else {
        return Err("missing Reading export".into());
    };
    let convert_returns = reading
        .methods
        .iter()
        .filter(|method| method.name == "convert")
        .map(|method| method.return_type.clone())
        .collect::<Vec<_>>();

    assert_eq!(convert_returns.len(), 2, "expected both convert overloads in manifest");
    assert!(
        convert_returns
            .iter()
            .any(|ty| matches!(ty, TypeRef::Named { name, .. } if name == "int")),
        "missing int convert overload: {convert_returns:?}"
    );
    assert!(
        convert_returns
            .iter()
            .any(|ty| matches!(ty, TypeRef::Named { name, .. } if name == "float")),
        "missing float convert overload: {convert_returns:?}"
    );
    Ok(())
}

#[test]
fn test_checked_public_exports_qualify_default_expression_provider_paths() -> Result<(), Box<dyn std::error::Error>> {
    let defaults_source = r#"
pub const FALLBACK: str = "fallback"

pub def make_label(value: str) -> str:
  return value
"#;
    let source = r#"
from defaults import FALLBACK, make_label

pub const LOCAL_SENTINEL: str = "local"

pub def imported_default(label: str = make_label(FALLBACK)) -> str:
  return label

pub def local_default(label: str = LOCAL_SENTINEL) -> str:
  return label
"#;

    let defaults_tokens = lexer::lex(defaults_source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let defaults_ast = parser::parse(&defaults_tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["helpers".to_string()]));
    checker
        .check_with_imports(&ast, &[("defaults", &defaults_ast)])
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

    let exports = collect_checked_public_exports(&ast, &checker);
    let manifest = LibraryManifest::from_checked_exports("querykit".to_string(), "0.1.0".to_string(), &exports);
    let imported = manifest
        .exports
        .functions
        .iter()
        .find(|function| function.name == "imported_default")
        .ok_or("missing imported_default export")?;
    let local = manifest
        .exports
        .functions
        .iter()
        .find(|function| function.name == "local_default")
        .ok_or("missing local_default export")?;

    assert_eq!(
        imported.params[0].default,
        Some(ParamDefaultExport::Call {
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
        })
    );
    assert_eq!(
        local.params[0].default,
        Some(ParamDefaultExport::ConstRef(vec![
            "helpers".to_string(),
            "LOCAL_SENTINEL".to_string(),
        ]))
    );
    Ok(())
}

#[test]
fn test_pub_import_multi_instantiation_trait_adoptions_check_type_args() {
    let source = r#"
from pub::mylib import Convert, ImportedReading

def read_str[T with Convert[str]](value: T) -> str:
  precise: str = value.convert()
  return precise

def main(reading: ImportedReading) -> str:
  return read_str[ImportedReading](reading)
"#;

    let Err(errs) = check_str_with_library_index(source, library_index_with_rfc025_trait_adoptions()) else {
        panic!("expected imported generic bound type-argument diagnostic");
    };
    assert!(
        errs.iter().any(|err| {
            err.message
                .contains("type parameter 'T' requires 'Convert[str]' but got 'ImportedReading'")
        }),
        "expected imported generic bound type-argument diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_pub_import_transitive_method_return_type_supports_follow_up_method_lookup() {
    let source = r#"
from pub::pubdemo import Session, SessionError

model Row:
  value: int

def main() -> Result[None, SessionError]:
  mut session = Session.default()
  lines = session.read_csv[Row]("orders", "orders.csv")?
  df = lines.collect()?
  print(df)
  return Ok(None)
"#;

    let result = check_str_with_library_index(source, library_index_with_pub_boundary_type_fidelity_exports());
    assert!(
        result.is_ok(),
        "expected transitive pub-returned carrier methods to resolve, got: {:?}",
        result.err()
    );
}

#[test]
fn test_pub_import_transitive_derived_method_chain_supports_follow_up_method_lookup() {
    let source = r#"
from pub::pubdemo import Session, SessionError

model Row:
  value: int

def main() -> Result[None, SessionError]:
  mut session = Session.default()
  lines = session.read_csv[Row]("orders", "orders.csv")?
  df = lines.clone().collect()?
  print(df)
  return Ok(None)
"#;

    let result = check_str_with_library_index(source, library_index_with_pub_boundary_type_fidelity_exports());
    assert!(
        result.is_ok(),
        "expected transitive pub derived-method chains to resolve, got: {:?}",
        result.err()
    );
}

#[test]
fn test_pub_import_transitive_trait_conformance_accepts_concrete_carrier() {
    let source = r#"
from pub::pubdemo import Session, SessionError, display

model Row:
  value: int

def main() -> Result[None, SessionError]:
  mut session = Session.default()
  lines = session.read_csv[Row]("orders", "orders.csv")?
  df = session.collect(lines)?
  display(df)
  return Ok(None)
"#;

    let result = check_str_with_library_index(source, library_index_with_pub_boundary_type_fidelity_exports());
    assert!(
        result.is_ok(),
        "expected transitive pub trait conformance to resolve, got: {:?}",
        result.err()
    );
}

use super::*;
use crate::{lexer, parser, typechecker};

fn metadata_for(source: &str) -> Result<CheckedApiMetadata, Vec<crate::diagnostics::CompileError>> {
    let tokens = lexer::lex(source)?;
    let program = parser::parse(&tokens)?;
    let mut checker = typechecker::TypeChecker::new();
    checker.check_program(&program)?;
    Ok(collect_checked_api_metadata(
        &program,
        &checker,
        vec!["demo".to_string()],
    ))
}

fn metadata_for_src_lib(source: &str) -> Result<CheckedApiMetadata, Vec<crate::diagnostics::CompileError>> {
    let tokens = lexer::lex(source)?;
    let program = parser::parse_with_module_path(&tokens, Some("project/src/lib.incn"))?;
    let mut checker = typechecker::TypeChecker::new();
    checker.check_program(&program)?;
    Ok(collect_checked_api_metadata(
        &program,
        &checker,
        vec!["lib".to_string()],
    ))
}

/// Every public overload must retain its own authored anchor so nested aliases can prove canonical backing.
#[test]
fn checked_api_preserves_each_overloaded_alias_target() -> Result<(), String> {
    let source = r#"
pub def select(value: int) -> int:
    return value

pub def select(value: str) -> str:
    return value

pub pick = alias select
"#;
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = typechecker::TypeChecker::new();
    checker.set_current_package_identity(Some("alias_repro".to_string()));
    checker.set_current_module_path(Some(vec!["entries".to_string()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let exports = collect_checked_public_exports(&program, &checker);
    let metadata = collect_checked_api_metadata(&program, &checker, vec!["entries".to_string()]);
    let functions = metadata
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            ApiDeclaration::Function(function) => Some(function),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(functions.len(), 2, "checked exports: {exports:#?}; API: {metadata:#?}");
    for export in exports.iter().filter(|export| export.name == "select") {
        let identity = export
            .identity
            .canonical
            .as_ref()
            .ok_or("missing canonical overload identity")?;
        assert!(
            functions
                .iter()
                .any(|function| function.name == identity.declaration_name
                    && function.anchor.span == identity.declaration_span),
            "unbacked overload: {identity:#?}; API: {metadata:#?}"
        );
    }
    assert_ne!(functions[0].return_type, functions[1].return_type);
    Ok(())
}

#[test]
fn checked_api_preserves_package_owned_method_identity() -> Result<(), String> {
    let source = r#"
pub class Buffer:
    def getvalue(self) -> int:
        return 1
"#;
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = typechecker::TypeChecker::new();
    checker.set_current_package_identity(Some("incan_stdlib_system".to_string()));
    checker.set_current_module_path(Some(vec!["io".to_string()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let metadata = collect_checked_api_metadata(&program, &checker, vec!["io".to_string()]);
    let method = metadata
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            ApiDeclaration::Class(class) if class.name == "Buffer" => {
                class.methods.iter().find(|method| method.name == "getvalue")
            }
            _ => None,
        })
        .ok_or("missing checked Buffer.getvalue metadata")?;
    let canonical = method.canonical.as_ref().ok_or("missing canonical method identity")?;

    assert_eq!(canonical.declaration_name, "getvalue");
    assert_eq!(canonical.kind, "method");
    assert_eq!(
        canonical.origin,
        crate::library_manifest::CanonicalIdentityOriginExport::Package {
            library: "incan_stdlib_system".to_string(),
            module_path: vec!["io".to_string()],
        }
    );
    assert_eq!(method_export_from_api(method).canonical.as_ref(), Some(canonical));
    Ok(())
}

#[test]
fn checked_api_metadata_extracts_function_decorator_and_docstring() -> Result<(), String> {
    let source = r#"
@rust.allow("dead_code")
pub def avg(values: List[float]) -> float:
    """
    Return the arithmetic mean.

    Args:
        values: Input values.

    Returns:
        float: Mean value.

    Decorators:
        rust.allow: Allows generated Rust lint suppression.
    """
    return 0.0
"#;
    let metadata = metadata_for(source).map_err(|errs| format!("{errs:?}"))?;
    let function = metadata
        .declarations
        .iter()
        .find_map(|decl| match decl {
            ApiDeclaration::Function(function) => Some(function),
            _ => None,
        })
        .ok_or_else(|| "expected function metadata".to_string())?;

    assert_eq!(function.name, "avg");
    assert_eq!(function.anchor.id, "demo::avg");
    assert_eq!(
        function.docstring.as_deref().map(str::trim),
        Some(
            "Return the arithmetic mean.\n\n    Args:\n        values: Input values.\n\n    Returns:\n        float: Mean value.\n\n    Decorators:\n        rust.allow: Allows generated Rust lint suppression."
        )
    );
    let docstring = function
        .docstring_sections
        .as_ref()
        .ok_or_else(|| "expected parsed docstring sections".to_string())?;
    assert_eq!(docstring.summary.as_deref(), Some("Return the arithmetic mean."));
    assert_eq!(
        docstring.params,
        vec![ApiDocstringEntry {
            name: "values".to_string(),
            description: "Input values.".to_string(),
        }]
    );
    assert_eq!(
        docstring.returns,
        Some(ApiDocstringReturn {
            ty: Some("float".to_string()),
            description: "Mean value.".to_string(),
        })
    );
    assert_eq!(
        docstring.decorators,
        vec![ApiDocstringEntry {
            name: "rust.allow".to_string(),
            description: "Allows generated Rust lint suppression.".to_string(),
        }]
    );
    assert_eq!(function.params.len(), 1);
    assert_eq!(function.decorators.len(), 1);
    assert_eq!(
        function.decorators[0].path,
        vec!["rust".to_string(), "allow".to_string()]
    );
    assert_eq!(
        function.decorators[0].args,
        vec![DecoratorArgMetadata::Positional {
            value: DecoratorValue::Literal {
                value: SafeMetadataValue::String("dead_code".to_string()),
            },
        }]
    );
    Ok(())
}

#[test]
fn checked_api_docstring_validation_reports_signature_drift() -> Result<(), String> {
    let source = r#"
pub def avg(values: List[float]) -> float:
    """
    Return the arithmetic mean.

    Args:
        missing: Stale argument.

    Returns:
        str: Wrong return type.

    Decorators:
        rust.allow: Stale decorator.
    """
    return 0.0
"#;
    let metadata = metadata_for(source).map_err(|errs| format!("{errs:?}"))?;
    let diagnostics = validate_checked_api_docstrings(&[metadata]);
    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.error.message.as_str())
        .collect::<Vec<_>>();

    assert!(
        messages
            .iter()
            .any(|message| message.contains("documented parameter `missing` does not exist")),
        "expected unknown parameter diagnostic, got {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("checked parameter `values` is missing")),
        "expected missing checked parameter diagnostic, got {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("documented return type `str` does not match checked return type `float`")),
        "expected return type drift diagnostic, got {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("documented decorator `rust.allow` does not exist")),
        "expected decorator drift diagnostic, got {messages:?}"
    );
    Ok(())
}

#[test]
fn checked_api_metadata_preserves_decorated_function_source_signature() -> Result<(), String> {
    let source = r#"
def keep(func: (int) -> int) -> (int) -> int:
    return func

@keep
pub def decorated(value: int) -> int:
    """Return the input value.

    Args:
        value: Input value.
    """
    return value
"#;
    let metadata = metadata_for(source).map_err(|errs| format!("{errs:?}"))?;
    let function = metadata
        .declarations
        .iter()
        .find_map(|decl| match decl {
            ApiDeclaration::Function(function) if function.name == "decorated" => Some(function),
            _ => None,
        })
        .ok_or_else(|| "expected decorated function metadata".to_string())?;

    assert_eq!(function.params.len(), 1);
    assert_eq!(function.params[0].name, "value");
    assert_eq!(
        function.params[0].ty,
        TypeRef::Named {
            origin: None,
            name: "int".to_string(),
        }
    );

    let diagnostics = validate_checked_api_docstrings(&[metadata]);
    assert!(
        diagnostics.is_empty(),
        "expected decorated source signature to satisfy docstring validation, got {diagnostics:?}"
    );
    Ok(())
}

#[test]
fn checked_api_metadata_preserves_generic_decorator_factory_source_signature() -> Result<(), String> {
    let source = r#"
model ColumnExpr:
    name: str

def registered[F](name: str) -> ((F) -> F):
    return (func) => func

@registered("incql.functions.col")
pub def col(name: str) -> ColumnExpr:
    """Build a column expression.

    Args:
        name: Column name.
    """
    return ColumnExpr(name=name)
"#;
    let metadata = metadata_for(source).map_err(|errs| format!("{errs:?}"))?;
    let function = metadata
        .declarations
        .iter()
        .find_map(|decl| match decl {
            ApiDeclaration::Function(function) if function.name == "col" => Some(function),
            _ => None,
        })
        .ok_or_else(|| "expected decorated function metadata".to_string())?;

    assert_eq!(function.params.len(), 1);
    assert_eq!(function.params[0].name, "name");
    assert_eq!(
        function.params[0].ty,
        TypeRef::Named {
            origin: None,
            name: "str".to_string(),
        }
    );
    assert_eq!(
        function.return_type,
        TypeRef::Named {
            origin: None,
            name: "ColumnExpr".to_string(),
        }
    );

    let diagnostics = validate_checked_api_docstrings(&[metadata]);
    assert!(
        diagnostics.is_empty(),
        "expected generic decorator factory source signature to satisfy docstring validation, got {diagnostics:?}"
    );
    Ok(())
}

/// Require the decorator context to retain the callable's complete named parameter and return signature.
fn assert_decorated_callable_signature(callable: &ApiCallableMetadata) {
    assert_eq!(callable.name, "eq");
    assert_eq!(
        callable
            .params
            .iter()
            .map(|param| (param.name.as_str(), &param.ty))
            .collect::<Vec<_>>(),
        vec![
            (
                "left",
                &TypeRef::Named {
                    origin: None,
                    name: "ColumnExpr".to_string()
                }
            ),
            (
                "right",
                &TypeRef::Named {
                    origin: None,
                    name: "ColumnExpr".to_string()
                }
            ),
        ]
    );
    assert_eq!(
        callable.return_type,
        TypeRef::Named {
            origin: None,
            name: "ColumnExpr".to_string()
        }
    );
}

/// Preserve decorated callable context and nested structural arguments without losing checked constant facts.
#[test]
fn checked_api_metadata_projects_decorated_callable_context_issue694() -> Result<(), String> {
    let source = r#"
const EQUAL_FUNCTION_ANCHOR = "substrait.equal"

model ColumnExpr:
    name: str

model FunctionLifecycle:
    since: str
    changed: List[str]
    deprecated: Option[str]

def extension_mapping(name: str, anchor: str) -> str:
    return name

def deterministic_spec(kind: str, lifecycle: FunctionLifecycle, mapping: str) -> str:
    return kind

def registered[F](spec: str) -> ((F) -> F):
    return (func) => func

@registered(deterministic_spec("scalar", FunctionLifecycle(since="v0.3", changed=[], deprecated=None), extension_mapping("equal", EQUAL_FUNCTION_ANCHOR)))
pub def eq(left: ColumnExpr, right: ColumnExpr) -> ColumnExpr:
    return left
"#;
    let metadata = metadata_for(source).map_err(|errs| format!("{errs:?}"))?;
    let function = metadata
        .declarations
        .iter()
        .find_map(|decl| match decl {
            ApiDeclaration::Function(function) if function.name == "eq" => Some(function),
            _ => None,
        })
        .ok_or_else(|| "expected decorated function metadata".to_string())?;
    let decorator = function
        .decorators
        .first()
        .ok_or_else(|| "expected decorator metadata".to_string())?;
    let callable = decorator
        .decorated_callable
        .as_ref()
        .ok_or_else(|| "expected decorated callable context".to_string())?;

    assert_decorated_callable_signature(callable);

    let [
        DecoratorArgMetadata::Positional {
            value: DecoratorValue::Call { callee, args, .. },
        },
    ] = decorator.args.as_slice()
    else {
        return Err(format!(
            "expected structured decorator call metadata, got {decorator:?}"
        ));
    };
    assert_eq!(callee, &vec!["deterministic_spec".to_string()]);
    let lifecycle_args = args
        .iter()
        .find_map(|arg| match arg {
            DecoratorCallArgMetadata::Positional {
                value: DecoratorValue::Call { callee, args, .. },
            } if callee == &vec!["FunctionLifecycle".to_string()] => Some(args),
            _ => None,
        })
        .ok_or_else(|| format!("expected nested lifecycle constructor call metadata, got {args:?}"))?;
    assert!(
        lifecycle_args.iter().any(|arg| matches!(
            arg,
            DecoratorCallArgMetadata::Named {
                name,
                value: DecoratorValue::List { items },
            } if name == "changed" && items.is_empty()
        )),
        "expected lifecycle `changed=[]` metadata, got {lifecycle_args:?}"
    );
    assert!(
        lifecycle_args.iter().any(|arg| matches!(
            arg,
            DecoratorCallArgMetadata::Named {
                name,
                value: DecoratorValue::Literal {
                    value: SafeMetadataValue::None,
                },
            } if name == "deprecated"
        )),
        "expected lifecycle `deprecated=None` metadata, got {lifecycle_args:?}"
    );
    assert!(
        args.iter().any(|arg| matches!(
            arg,
            DecoratorCallArgMetadata::Positional {
                value: DecoratorValue::Call { callee, args, .. },
            } if callee == &vec!["extension_mapping".to_string()]
                && args.iter().any(|arg| matches!(
                    arg,
                    DecoratorCallArgMetadata::Positional {
                        value: DecoratorValue::ConstRef {
                            name,
                            value: Some(SafeMetadataValue::String(value)),
                        },
                    } if name == "EQUAL_FUNCTION_ANCHOR" && value == "substrait.equal"
                ))
        )),
        "expected nested extension mapping call metadata with checked const ref, got {args:?}"
    );
    Ok(())
}

#[test]
fn checked_api_metadata_rejects_non_symbolic_decorator_field_metadata() -> Result<(), String> {
    let source = r#"
model Holder:
    value: str

def holder() -> Holder:
    return Holder(value="equal")

def registered[F](name: str) -> ((F) -> F):
    return (func) => func

@registered(holder().value)
pub def eq(left: int, right: int) -> int:
    return left
"#;
    let metadata = metadata_for(source).map_err(|errs| format!("{errs:?}"))?;
    let function = metadata
        .declarations
        .iter()
        .find_map(|decl| match decl {
            ApiDeclaration::Function(function) if function.name == "eq" => Some(function),
            _ => None,
        })
        .ok_or_else(|| "expected decorated function metadata".to_string())?;
    let [
        DecoratorArgMetadata::Positional {
            value: DecoratorValue::Unsupported { reason },
        },
    ] = function.decorators[0].args.as_slice()
    else {
        return Err(format!(
            "expected non-symbolic field decorator argument to stay unsupported, got {:?}",
            function.decorators[0].args
        ));
    };

    assert_eq!(reason, "decorator field expression is not a symbolic path");
    Ok(())
}

#[test]
fn checked_api_docstring_validation_matches_overloaded_method_by_params() -> Result<(), String> {
    let source = r#"
pub class Writer:
    def write(self, data: bytes) -> Result[int, str]:
        """
        Write raw bytes.

        Args:
            data: Bytes to write.
        """
        return Ok(len(data))

    def write(self, value: u8, _endian: str) -> Result[None, str]:
        return Ok(None)
"#;
    let tokens = lexer::lex(source).map_err(|errs| format!("{errs:?}"))?;
    let program = parser::parse(&tokens).map_err(|errs| format!("{errs:?}"))?;
    let class = program
        .declarations
        .iter()
        .find_map(|decl| match &decl.node {
            Declaration::Class(class) => Some(class),
            _ => None,
        })
        .ok_or_else(|| "expected class declaration".to_string())?;
    let checked_methods = vec![
        CheckedMethod {
            ownership: None,
            name: "write".to_string(),
            canonical: None,
            visibility: crate::ast::Visibility::Public,
            alias_of: None,
            type_params: Vec::new(),
            receiver: Some(crate::ast::Receiver::Immutable),
            params: vec![
                crate::symbols::CallableParam::named(
                    "value",
                    crate::symbols::ResolvedType::Int,
                    crate::ast::ParamKind::Normal,
                ),
                crate::symbols::CallableParam::named(
                    "_endian",
                    crate::symbols::ResolvedType::Str,
                    crate::ast::ParamKind::Normal,
                ),
            ],
            param_defaults: vec![None, None],
            return_type: crate::symbols::ResolvedType::Unit,
            is_async: false,
            has_body: true,
        },
        CheckedMethod {
            ownership: None,
            name: "write".to_string(),
            canonical: None,
            visibility: crate::ast::Visibility::Public,
            alias_of: None,
            type_params: Vec::new(),
            receiver: Some(crate::ast::Receiver::Immutable),
            params: vec![crate::symbols::CallableParam::named(
                "data",
                crate::symbols::ResolvedType::Bytes,
                crate::ast::ParamKind::Normal,
            )],
            param_defaults: vec![None],
            return_type: crate::symbols::ResolvedType::Int,
            is_async: false,
            has_body: true,
        },
    ];
    let checker = typechecker::TypeChecker::new();
    let api_methods = methods(
        &class.methods,
        &class.method_aliases,
        &checked_methods,
        &checker,
        &["demo".to_string()],
        "Writer",
    );
    assert_eq!(api_methods.len(), 2);
    assert_eq!(
        api_methods[0]
            .params
            .iter()
            .map(|param| param.name.as_str())
            .collect::<Vec<_>>(),
        vec!["data"]
    );
    assert_eq!(
        api_methods[1]
            .params
            .iter()
            .map(|param| param.name.as_str())
            .collect::<Vec<_>>(),
        vec!["value", "_endian"]
    );
    Ok(())
}

#[test]
fn checked_api_docstring_validation_matches_overloaded_method_by_type_shape() -> Result<(), String> {
    let source = r#"
pub class Parser:
    def parse(self, value: str) -> str:
        """
        Parse text.
        """
        return value

    def parse(self, value: bytes) -> bytes:
        """
        Parse bytes.
        """
        return value
"#;
    let tokens = lexer::lex(source).map_err(|errs| format!("{errs:?}"))?;
    let program = parser::parse(&tokens).map_err(|errs| format!("{errs:?}"))?;
    let class = program
        .declarations
        .iter()
        .find_map(|decl| match &decl.node {
            Declaration::Class(class) => Some(class),
            _ => None,
        })
        .ok_or_else(|| "expected class declaration".to_string())?;
    let checked_methods = vec![
        CheckedMethod {
            ownership: None,
            name: "parse".to_string(),
            canonical: None,
            visibility: crate::ast::Visibility::Public,
            alias_of: None,
            type_params: Vec::new(),
            receiver: Some(crate::ast::Receiver::Immutable),
            params: vec![crate::symbols::CallableParam::named(
                "value",
                crate::symbols::ResolvedType::Bytes,
                crate::ast::ParamKind::Normal,
            )],
            param_defaults: vec![None],
            return_type: crate::symbols::ResolvedType::Bytes,
            is_async: false,
            has_body: true,
        },
        CheckedMethod {
            ownership: None,
            name: "parse".to_string(),
            canonical: None,
            visibility: crate::ast::Visibility::Public,
            alias_of: None,
            type_params: Vec::new(),
            receiver: Some(crate::ast::Receiver::Immutable),
            params: vec![crate::symbols::CallableParam::named(
                "value",
                crate::symbols::ResolvedType::Str,
                crate::ast::ParamKind::Normal,
            )],
            param_defaults: vec![None],
            return_type: crate::symbols::ResolvedType::Str,
            is_async: false,
            has_body: true,
        },
    ];
    let checker = typechecker::TypeChecker::new();
    let api_methods = methods(
        &class.methods,
        &class.method_aliases,
        &checked_methods,
        &checker,
        &["demo".to_string()],
        "Parser",
    );
    assert_eq!(api_methods.len(), 2);
    assert_eq!(
        api_methods[0].return_type,
        TypeRef::Named {
            origin: None,
            name: "str".to_string()
        }
    );
    assert_eq!(
        api_methods[0].docstring.as_deref().unwrap_or_default().trim(),
        "Parse text."
    );
    assert_eq!(
        api_methods[1].return_type,
        TypeRef::Named {
            origin: None,
            name: "bytes".to_string()
        }
    );
    assert_eq!(
        api_methods[1].docstring.as_deref().unwrap_or_default().trim(),
        "Parse bytes."
    );
    Ok(())
}

#[test]
fn checked_api_docstring_validation_reports_field_drift() -> Result<(), String> {
    let source = r#"
pub model Order:
    """
    Order contract.

    Fields:
        missing: Stale field documentation.
    """
    pub id: int
"#;
    let metadata = metadata_for(source).map_err(|errs| format!("{errs:?}"))?;
    let diagnostics = validate_checked_api_docstrings(&[metadata]);
    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.error.message.as_str())
        .collect::<Vec<_>>();

    assert!(
        messages
            .iter()
            .any(|message| message.contains("documented field `missing` does not exist")),
        "expected unknown field diagnostic, got {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("checked field `id` is missing")),
        "expected missing checked field diagnostic, got {messages:?}"
    );
    Ok(())
}

#[test]
fn checked_api_metadata_extracts_model_fields_methods_and_const_values() -> Result<(), String> {
    let source = r#"
pub const DEFAULT_LABEL = "none"

pub trait Labeled:
    def label(self) -> str: ...


@derive(Clone)
pub model Order with Labeled:
    """
    Order contract.
    """
    pub id [description="Stable id"] as "orderId": int
    pub fallback_label: str = DEFAULT_LABEL

    def label(self) -> str:
        """
        Return the display label.
        """
        return DEFAULT_LABEL
"#;
    let metadata = metadata_for(source).map_err(|errs| format!("{errs:?}"))?;
    let konst = metadata
        .declarations
        .iter()
        .find_map(|decl| match decl {
            ApiDeclaration::Const(konst) => Some(konst),
            _ => None,
        })
        .ok_or_else(|| "expected const metadata".to_string())?;
    assert_eq!(konst.value, Some(SafeMetadataValue::String("none".to_string())));

    let model = metadata
        .declarations
        .iter()
        .find_map(|decl| match decl {
            ApiDeclaration::Model(model) => Some(model),
            _ => None,
        })
        .ok_or_else(|| "expected model metadata".to_string())?;
    assert_eq!(model.docstring.as_deref().map(str::trim), Some("Order contract."));
    assert_eq!(model.traits, vec!["Labeled".to_string()]);
    assert_eq!(model.trait_adoptions.len(), 1);
    assert_eq!(model.trait_adoptions[0].name, "Labeled");
    assert_eq!(
        model.fields.iter().map(|field| field.name.as_str()).collect::<Vec<_>>(),
        vec!["id", "fallback_label"]
    );
    assert_eq!(model.fields[0].alias.as_deref(), Some("orderId"));
    assert_eq!(model.fields[0].description.as_deref(), Some("Stable id"));
    assert_eq!(
        model.methods[0].docstring.as_deref().map(str::trim),
        Some("Return the display label.")
    );
    Ok(())
}

#[test]
fn checked_api_metadata_preserves_same_type_method_aliases() -> Result<(), String> {
    let source = r#"
pub model Measurements:
    pub value: int

    def average(self) -> int:
        return self.value

    mean = alias average
"#;
    let metadata = metadata_for(source).map_err(|errs| format!("{errs:?}"))?;
    let model = metadata
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            ApiDeclaration::Model(model) => Some(model),
            _ => None,
        })
        .ok_or_else(|| "expected model metadata".to_string())?;
    let mean = model
        .methods
        .iter()
        .find(|method| method.name == "mean")
        .ok_or_else(|| "expected method alias metadata".to_string())?;
    assert_eq!(mean.alias_of.as_deref(), Some("average"));
    assert_eq!(mean.anchor.id, "demo::Measurements.mean");
    assert_eq!(mean.params.len(), 0);
    assert_eq!(
        mean.return_type,
        TypeRef::Named {
            origin: None,
            name: "int".to_string()
        }
    );
    assert_eq!(
        model_export_from_api(model)
            .methods
            .iter()
            .find(|method| method.name == "mean")
            .and_then(|method| method.alias_of.as_deref()),
        Some("average")
    );
    Ok(())
}

#[test]
fn checked_api_metadata_extracts_public_import_alias_targets() -> Result<(), String> {
    let source = r#"
pub from crate.widgets import Widget as PublicWidget
"#;
    let metadata = metadata_for_src_lib(source).map_err(|errs| format!("{errs:?}"))?;
    let alias = metadata
        .declarations
        .iter()
        .find_map(|decl| match decl {
            ApiDeclaration::Alias(alias) => Some(alias),
            _ => None,
        })
        .ok_or_else(|| "expected alias metadata".to_string())?;

    assert_eq!(alias.name, "PublicWidget");
    assert_eq!(alias.anchor.id, "lib::PublicWidget");
    assert!(alias.is_public);
    assert_eq!(
        alias.target_path,
        vec!["crate".to_string(), "widgets".to_string(), "Widget".to_string()]
    );
    Ok(())
}

#[test]
fn checked_api_materializes_public_namespace_members_and_children_issue948() -> Result<(), String> {
    let mut index = metadata_for("pub def build() -> int:\n  return 1\n").map_err(|errors| format!("{errors:?}"))?;
    index.module_path = vec!["hyperquant".to_string(), "index".to_string()];
    let mut package = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![index],
        public_namespaces: Vec::new(),
    };

    materialize_checked_api_public_namespaces(&mut package).map_err(|error| error.to_string())?;

    let parent = checked_api_public_namespace(&package, &["hyperquant".to_string()])
        .ok_or_else(|| "missing parent namespace".to_string())?;
    assert_eq!(parent.child_modules, vec!["index".to_string()]);
    assert_eq!(parent.members.len(), 1);
    assert_eq!(parent.members[0].name, "build");
    assert_eq!(
        parent.members[0].source_path,
        vec!["hyperquant".to_string(), "index".to_string(), "build".to_string()]
    );
    Ok(())
}

#[test]
fn checked_api_rejects_duplicate_logical_source_modules_issue948() -> Result<(), String> {
    let mut first = metadata_for("pub def left() -> int:\n  return 1\n").map_err(|errors| format!("{errors:?}"))?;
    first.module_path = vec!["hyperquant".to_string()];
    let mut second = metadata_for("pub def right() -> int:\n  return 2\n").map_err(|errors| format!("{errors:?}"))?;
    second.module_path = vec!["hyperquant".to_string()];
    let mut package = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![first, second],
        public_namespaces: Vec::new(),
    };

    let Err(error) = materialize_checked_api_public_namespaces(&mut package) else {
        return Err("duplicate logical modules unexpectedly produced a public namespace graph".to_string());
    };

    assert!(error.to_string().contains("multiple source files"));
    assert!(error.to_string().contains("hyperquant"));
    Ok(())
}

#[test]
fn checked_api_metadata_marks_implementation_imports_private() -> Result<(), String> {
    let source = "from crate.widgets import Widget\n";
    let metadata = metadata_for_src_lib(source).map_err(|errs| format!("{errs:?}"))?;
    let alias = metadata
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            ApiDeclaration::Alias(alias) => Some(alias),
            _ => None,
        })
        .ok_or_else(|| "expected internal alias metadata for registry projection".to_string())?;

    assert_eq!(alias.name, "Widget");
    assert!(!alias.is_public);
    Ok(())
}

#[test]
fn checked_api_metadata_extracts_public_partial_callable_preset() -> Result<(), String> {
    let source = r#"
pub def route(method: str, path: str = "/") -> str:
    return path

pub get = partial route(method="GET")
"#;
    let metadata = metadata_for(source).map_err(|errs| format!("{errs:?}"))?;
    let partial = metadata
        .declarations
        .iter()
        .find_map(|decl| match decl {
            ApiDeclaration::Partial(partial) => Some(partial),
            _ => None,
        })
        .ok_or_else(|| "expected partial metadata".to_string())?;

    assert_eq!(partial.name, "get");
    assert_eq!(partial.anchor.id, "demo::get");
    assert_eq!(partial.target_path, vec!["route".to_string()]);
    assert_eq!(partial.target_kind, PartialTargetKindExport::Function);
    assert_eq!(partial.presets.len(), 1);
    assert_eq!(partial.presets[0].name, "method");
    assert_eq!(
        partial
            .params
            .iter()
            .map(|param| param.name.as_str())
            .collect::<Vec<_>>(),
        vec!["method", "path"]
    );
    assert!(
        partial.params[0].has_default,
        "partial-projected callable params should preserve ordinary default display metadata"
    );
    assert!(
        partial.params[1].has_default,
        "ordinary target defaults should remain visually distinct on partial metadata"
    );
    Ok(())
}

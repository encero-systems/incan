//! Decorator metadata, safe metadata values, and function docstrings.

use super::*;

/// Convert source decorators into checked API metadata entries.
pub(super) fn decorators_metadata(
    decorators: &[Spanned<Decorator>],
    checker: &TypeChecker,
    decorated_callable: Option<&ApiCallableMetadata>,
) -> Vec<DecoratorMetadata> {
    decorators
        .iter()
        .map(|decorator| {
            let resolved = decorator_resolution::resolve_decorator_path(&decorator.node, &checker.symbols);
            DecoratorMetadata {
                path: resolved,
                source_name: decorator.node.path.segments.join("."),
                anchor: source_span(decorator.span),
                type_args: decorator
                    .node
                    .type_args
                    .iter()
                    .map(|type_arg| {
                        type_ref_from_resolved(&crate::symbols::resolve_type(&type_arg.node, &checker.symbols))
                    })
                    .collect(),
                args: decorator
                    .node
                    .args
                    .iter()
                    .map(|arg| decorator_arg_metadata(arg, checker))
                    .collect(),
                decorated_callable: decorated_callable.cloned(),
            }
        })
        .collect()
}

/// Convert a decorator argument into API metadata.
pub(super) fn decorator_arg_metadata(arg: &DecoratorArg, checker: &TypeChecker) -> DecoratorArgMetadata {
    match arg {
        DecoratorArg::Positional(expr) => DecoratorArgMetadata::Positional {
            value: decorator_expr_value(expr, checker),
        },
        DecoratorArg::Named(name, DecoratorArgValue::Expr(expr)) => DecoratorArgMetadata::Named {
            name: name.clone(),
            value: decorator_expr_value(expr, checker),
        },
        DecoratorArg::Named(name, DecoratorArgValue::Type(ty)) => DecoratorArgMetadata::Named {
            name: name.clone(),
            value: DecoratorValue::Type {
                ty: type_ref_from_resolved(&crate::symbols::resolve_type(&ty.node, &checker.symbols)),
            },
        },
    }
}

/// Convert a decorator expression into a safe metadata value.
pub(super) fn decorator_expr_value(expr: &Spanned<Expr>, checker: &TypeChecker) -> DecoratorValue {
    match &expr.node {
        Expr::Literal(literal) => DecoratorValue::Literal {
            value: safe_value_from_literal(literal),
        },
        Expr::Ident(name) => DecoratorValue::ConstRef {
            name: name.clone(),
            value: checker.type_info().const_value(name).map(safe_value_from_const),
        },
        Expr::Field(base, field) => {
            let mut path = decorator_expr_path(&base.node);
            if path.is_empty() {
                DecoratorValue::Unsupported {
                    reason: "decorator field expression is not a symbolic path".to_string(),
                }
            } else {
                path.push(field.clone());
                DecoratorValue::SymbolRef { path }
            }
        }
        Expr::List(entries) => DecoratorValue::List {
            items: entries
                .iter()
                .map(|entry| match entry {
                    ListEntry::Element(value) => decorator_expr_value(value, checker),
                    ListEntry::Spread(value) => DecoratorValue::Unsupported {
                        reason: format!(
                            "decorator list spread `{}` is not declaration-safe metadata",
                            decorator_expr_label(&value.node)
                        ),
                    },
                })
                .collect(),
        },
        Expr::Dict(entries) => {
            let mut metadata_entries = Vec::new();
            for entry in entries {
                match entry {
                    DictEntry::Pair(key, value) => metadata_entries.push(DecoratorDictEntry {
                        key: decorator_expr_value(key, checker),
                        value: decorator_expr_value(value, checker),
                    }),
                    DictEntry::Spread(value) => metadata_entries.push(DecoratorDictEntry {
                        key: DecoratorValue::Unsupported {
                            reason: "decorator dict spread has no declaration-safe key".to_string(),
                        },
                        value: decorator_expr_value(value, checker),
                    }),
                }
            }
            DecoratorValue::Dict {
                entries: metadata_entries,
            }
        }
        Expr::Call(callee, type_args, args) => {
            let path = decorator_expr_path(&callee.node);
            if path.is_empty() {
                return DecoratorValue::Unsupported {
                    reason: "decorator call callee is not a symbolic path".to_string(),
                };
            }
            DecoratorValue::Call {
                callee: path,
                type_args: type_args
                    .iter()
                    .map(|type_arg| {
                        type_ref_from_resolved(&crate::symbols::resolve_type(&type_arg.node, &checker.symbols))
                    })
                    .collect(),
                args: args
                    .iter()
                    .map(|arg| decorator_call_arg_metadata(arg, checker))
                    .collect(),
            }
        }
        Expr::Constructor(name, args) => DecoratorValue::Call {
            callee: vec![name.clone()],
            type_args: Vec::new(),
            args: args
                .iter()
                .map(|arg| decorator_call_arg_metadata(arg, checker))
                .collect(),
        },
        _ => DecoratorValue::Unsupported {
            reason: "decorator argument is not a literal, const reference, or type".to_string(),
        },
    }
}

/// Convert a decorator call argument into API metadata.
pub(super) fn decorator_call_arg_metadata(arg: &CallArg, checker: &TypeChecker) -> DecoratorCallArgMetadata {
    match arg {
        CallArg::Positional(value) => DecoratorCallArgMetadata::Positional {
            value: decorator_expr_value(value, checker),
        },
        CallArg::Named(name, value) => DecoratorCallArgMetadata::Named {
            name: name.node.clone(),
            value: decorator_expr_value(value, checker),
        },
        CallArg::PositionalUnpack(value) => DecoratorCallArgMetadata::PositionalUnpack {
            value: decorator_expr_value(value, checker),
        },
        CallArg::KeywordUnpack(value) => DecoratorCallArgMetadata::KeywordUnpack {
            value: decorator_expr_value(value, checker),
        },
    }
}

/// Return the source path represented by a decorator expression.
pub(super) fn decorator_expr_path(expr: &Expr) -> Vec<String> {
    match expr {
        Expr::Ident(name) => vec![name.clone()],
        Expr::Field(base, field) => {
            let mut path = decorator_expr_path(&base.node);
            if path.is_empty() {
                return Vec::new();
            }
            path.push(field.clone());
            path
        }
        _ => Vec::new(),
    }
}

/// Return a stable label for a decorator expression shape.
pub(super) fn decorator_expr_label(expr: &Expr) -> &'static str {
    match expr {
        Expr::Ident(_) => "identifier",
        Expr::Literal(_) => "literal",
        Expr::Call(_, _, _) | Expr::Constructor(_, _) => "call",
        Expr::List(_) => "list",
        Expr::Dict(_) => "dict",
        Expr::Field(_, _) => "field",
        _ => "expression",
    }
}

/// Convert a literal into the safe metadata subset used by checked API output.
pub(super) fn safe_value_from_literal(literal: &crate::ast::Literal) -> SafeMetadataValue {
    match literal {
        crate::ast::Literal::Int(value) => SafeMetadataValue::Int(value.value),
        crate::ast::Literal::Float(value) => SafeMetadataValue::Float(value.value),
        crate::ast::Literal::Decimal(value) => SafeMetadataValue::String(value.repr.clone()),
        crate::ast::Literal::String(value) => SafeMetadataValue::String(value.clone()),
        crate::ast::Literal::Bytes(value) => SafeMetadataValue::Bytes(value.clone()),
        crate::ast::Literal::Bool(value) => SafeMetadataValue::Bool(*value),
        crate::ast::Literal::None => SafeMetadataValue::None,
    }
}

/// Convert a constant value into a safe metadata value.
pub(super) fn safe_value_from_const(value: &ConstValue) -> SafeMetadataValue {
    match value {
        ConstValue::Int(value) => SafeMetadataValue::Int(*value),
        ConstValue::Float(value) => SafeMetadataValue::Float(*value),
        ConstValue::Bool(value) => SafeMetadataValue::Bool(*value),
        ConstValue::FrozenStr(value) => SafeMetadataValue::String(value.clone()),
        ConstValue::FrozenBytes(value) => SafeMetadataValue::Bytes(value.clone()),
    }
}

/// Extract the leading function docstring expression, when present.
pub(super) fn function_docstring(body: &[Spanned<Statement>]) -> Option<String> {
    let first = body.first()?;
    let Statement::Expr(expr) = &first.node else {
        return None;
    };
    let Expr::Literal(crate::ast::Literal::String(docstring)) = &expr.node else {
        return None;
    };
    Some(docstring.clone())
}

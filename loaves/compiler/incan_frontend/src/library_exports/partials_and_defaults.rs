//! Checked partial exports, preset values, and parameter defaults.

use super::*;

/// Build checked export metadata for a public partial callable preset.
pub(super) fn checked_partial_export(partial: &PartialDecl, checker: &TypeChecker) -> Option<CheckedPartialExport> {
    let symbol = checker.lookup_symbol(partial.name.as_str())?;
    let SymbolKind::Function(info) = &symbol.kind else {
        return None;
    };
    let projection = checker.type_info.partial_projection(&partial.name);
    let default_context = DefaultPathContext::for_checker(checker);
    let presets = partial
        .args
        .iter()
        .map(|arg| CheckedPartialPreset {
            name: arg.name.clone(),
            ty: info
                .params
                .iter()
                .find(|param| param.name() == Some(arg.name.as_str()))
                .map(|param| param.ty.clone())
                .unwrap_or(ResolvedType::Unknown),
            value: checked_preset_value(&arg.value.node, default_context),
        })
        .collect();
    Some(CheckedPartialExport {
        name: partial.name.clone(),
        target_path: projection.map_or_else(
            || partial.target.segments.clone(),
            |projection| projection.target_path.clone(),
        ),
        target_kind: projection.map_or_else(
            || checked_partial_target_kind(partial, checker),
            |projection| checked_partial_target_kind_from_projection(projection.target_kind),
        ),
        presets,
        type_params: info
            .type_params
            .iter()
            .map(|name| CheckedTypeParam {
                name: name.clone(),
                bounds: info
                    .type_param_bound_details
                    .get(name)
                    .map_or_else(Vec::new, |bounds| map_type_bound_infos(bounds)),
            })
            .collect(),
        params: info.params.clone(),
        return_type: info.return_type.clone(),
        is_async: info.is_async,
    })
}

/// Convert collection-time partial projection metadata into public export metadata.
pub(super) fn checked_partial_target_kind_from_projection(
    kind: PartialProjectionTargetKind,
) -> CheckedPartialTargetKind {
    match kind {
        PartialProjectionTargetKind::Function => CheckedPartialTargetKind::Function,
        PartialProjectionTargetKind::ModelConstructor => CheckedPartialTargetKind::ModelConstructor,
        PartialProjectionTargetKind::ClassConstructor => CheckedPartialTargetKind::ClassConstructor,
        PartialProjectionTargetKind::NewtypeConstructor => CheckedPartialTargetKind::NewtypeConstructor,
        PartialProjectionTargetKind::Unknown => CheckedPartialTargetKind::Unknown,
    }
}

/// Classify the direct target kind for manifest/API partial provenance.
pub(super) fn checked_partial_target_kind(partial: &PartialDecl, checker: &TypeChecker) -> CheckedPartialTargetKind {
    let [target] = partial.target.segments.as_slice() else {
        return CheckedPartialTargetKind::Unknown;
    };
    match checker.lookup_symbol(target).map(|symbol| &symbol.kind) {
        Some(SymbolKind::Function(_) | SymbolKind::FunctionOverloads(_)) => CheckedPartialTargetKind::Function,
        Some(SymbolKind::Type(TypeInfo::Model(_))) => CheckedPartialTargetKind::ModelConstructor,
        Some(SymbolKind::Type(TypeInfo::Class(_))) => CheckedPartialTargetKind::ClassConstructor,
        Some(SymbolKind::Type(TypeInfo::Newtype(_))) => CheckedPartialTargetKind::NewtypeConstructor,
        Some(SymbolKind::Type(_) | SymbolKind::Trait(_) | SymbolKind::Variable(_) | SymbolKind::Static(_))
        | Some(SymbolKind::Field(_))
        | Some(SymbolKind::Property(_))
        | Some(SymbolKind::Module(_))
        | Some(SymbolKind::Variant(_))
        | Some(SymbolKind::RustItem(_))
        | Some(SymbolKind::Capability(_))
        | None => CheckedPartialTargetKind::Unknown,
    }
}

/// Convert a preset expression into the metadata-safe subset used by public partial provenance.
pub(super) fn checked_preset_value(expr: &Expr, context: DefaultPathContext<'_>) -> CheckedPresetValue {
    match expr {
        Expr::Literal(literal) => checked_preset_literal(literal),
        Expr::Unary(UnaryOp::Neg, value) => match &value.node {
            Expr::Literal(Literal::Int(value)) => value
                .value
                .checked_neg()
                .map(CheckedPresetValue::Int)
                .unwrap_or(CheckedPresetValue::Unsupported),
            Expr::Literal(Literal::Float(value)) => CheckedPresetValue::Float(-value.value),
            _ => CheckedPresetValue::Unsupported,
        },
        Expr::Paren(inner) => checked_preset_value(&inner.node, context),
        Expr::Set(items) => CheckedPresetValue::Set(
            items
                .iter()
                .map(|item| checked_preset_value(&item.node, context))
                .collect(),
        ),
        Expr::Tuple(items) => CheckedPresetValue::Tuple(
            items
                .iter()
                .map(|item| checked_preset_value(&item.node, context))
                .collect(),
        ),
        Expr::Ident(name) => CheckedPresetValue::ConstRef(context.canonical_value_path(vec![name.clone()])),
        Expr::Field(base, field) => {
            let mut path = checked_preset_path(&base.node);
            if path.is_empty() {
                CheckedPresetValue::Unsupported
            } else {
                path.push(field.clone());
                CheckedPresetValue::ConstRef(context.canonical_value_path(path))
            }
        }
        Expr::List(entries) => CheckedPresetValue::List(
            entries
                .iter()
                .map(|entry| match entry {
                    ListEntry::Element(value) => checked_preset_value(&value.node, context),
                    ListEntry::Spread(_) => CheckedPresetValue::Unsupported,
                })
                .collect(),
        ),
        Expr::Dict(entries) => {
            let pairs = entries
                .iter()
                .map(|entry| match entry {
                    DictEntry::Pair(key, value) => (
                        checked_preset_value(&key.node, context),
                        checked_preset_value(&value.node, context),
                    ),
                    DictEntry::Spread(_) => (CheckedPresetValue::Unsupported, CheckedPresetValue::Unsupported),
                })
                .collect();
            CheckedPresetValue::Dict(pairs)
        }
        Expr::Call(callee, _type_args, args) => {
            let path = checked_preset_path(&callee.node);
            let [name] = path.as_slice() else {
                return CheckedPresetValue::Unsupported;
            };
            let mut fields = Vec::new();
            for arg in args {
                let crate::ast::CallArg::Named(field, value) = arg else {
                    return CheckedPresetValue::Unsupported;
                };
                fields.push((field.node.clone(), checked_preset_value(&value.node, context)));
            }
            CheckedPresetValue::ModelLiteral {
                name: name.clone(),
                fields,
            }
        }
        Expr::Constructor(name, args) => {
            let mut fields = Vec::new();
            for arg in args {
                let crate::ast::CallArg::Named(field, value) = arg else {
                    return CheckedPresetValue::Unsupported;
                };
                fields.push((field.node.clone(), checked_preset_value(&value.node, context)));
            }
            CheckedPresetValue::ModelLiteral {
                name: name.clone(),
                fields,
            }
        }
        _ => CheckedPresetValue::Unsupported,
    }
}

/// Convert a source parameter default into the metadata-safe subset that public library consumers can materialize.
pub(super) fn checked_param_default(expr: &Spanned<Expr>, context: DefaultPathContext<'_>) -> CheckedParamDefault {
    match &expr.node {
        Expr::Literal(literal) => checked_param_default_literal(literal),
        Expr::Unary(UnaryOp::Neg, value) => match &value.node {
            Expr::Literal(Literal::Int(value)) => value
                .value
                .checked_neg()
                .map(CheckedParamDefault::Int)
                .unwrap_or(CheckedParamDefault::Unsupported),
            Expr::Literal(Literal::Float(value)) => CheckedParamDefault::Float(-value.value),
            _ => CheckedParamDefault::Unsupported,
        },
        Expr::Ident(name) => CheckedParamDefault::ConstRef(context.canonical_value_path(vec![name.clone()])),
        Expr::Field(base, field) => {
            let mut path = checked_preset_path(&base.node);
            if path.is_empty() {
                CheckedParamDefault::Unsupported
            } else {
                path.push(field.clone());
                CheckedParamDefault::ConstRef(context.canonical_value_path(path))
            }
        }
        Expr::List(entries) => CheckedParamDefault::List(
            entries
                .iter()
                .map(|entry| match entry {
                    ListEntry::Element(value) => checked_param_default(value, context),
                    ListEntry::Spread(_) => CheckedParamDefault::Unsupported,
                })
                .collect(),
        ),
        Expr::Dict(entries) => CheckedParamDefault::Dict(
            entries
                .iter()
                .map(|entry| match entry {
                    DictEntry::Pair(key, value) => (
                        checked_param_default(key, context),
                        checked_param_default(value, context),
                    ),
                    DictEntry::Spread(_) => (CheckedParamDefault::Unsupported, CheckedParamDefault::Unsupported),
                })
                .collect(),
        ),
        Expr::Call(callee, _type_args, args) => {
            // A consumer constructs a model or class, or calls a function, through the package's public path to it, so
            // a default that constructs or calls anything without such a path is not carried; the parameter stays
            // required for such callers.
            let carried = if context.names_model_or_class(&callee.node) {
                context.carries_constructions && context.names_exported_type(&callee.node)
            } else {
                context.call_is_carried(callee)
            };
            if !carried {
                return CheckedParamDefault::Unsupported;
            }
            let path = context.canonical_value_path(checked_preset_path(&callee.node));
            if path.is_empty() {
                return CheckedParamDefault::Unsupported;
            }
            let args = args
                .iter()
                .map(|arg| match arg {
                    crate::ast::CallArg::Positional(value) => CheckedParamDefaultArg {
                        name: None,
                        value: checked_param_default(value, context),
                    },
                    crate::ast::CallArg::Named(name, value) => CheckedParamDefaultArg {
                        name: Some(name.node.clone()),
                        value: checked_param_default(value, context),
                    },
                    crate::ast::CallArg::PositionalUnpack(_) | crate::ast::CallArg::KeywordUnpack(_) => {
                        CheckedParamDefaultArg {
                            name: None,
                            value: CheckedParamDefault::Unsupported,
                        }
                    }
                })
                .collect();
            CheckedParamDefault::Call {
                path,
                args,
                signature: checked_param_default_call_signature(callee, context.checker),
            }
        }
        _ => CheckedParamDefault::Unsupported,
    }
}

/// Capture the checked callable surface for a default-expression helper call.
pub(super) fn checked_param_default_call_signature(
    callee: &Spanned<Expr>,
    checker: &TypeChecker,
) -> Option<CheckedParamDefaultCallSignature> {
    let callee_ty = checker
        .type_info()
        .expr_type(callee.span)
        .cloned()
        .or_else(|| checked_param_default_callee_symbol_type(&callee.node, checker))?;
    let ResolvedType::Function(params, return_type) = callee_ty else {
        return None;
    };
    Some(CheckedParamDefaultCallSignature {
        params,
        return_type: return_type.as_ref().clone(),
    })
}

/// Fallback callable-surface lookup for default calls whose callee span has no recorded type.
pub(super) fn checked_param_default_callee_symbol_type(callee: &Expr, checker: &TypeChecker) -> Option<ResolvedType> {
    let path = checked_preset_path(callee);
    if path.is_empty() {
        return None;
    }
    let symbol_names = [(path.len() > 1).then(|| path.join(".")), path.last().cloned()];
    for name in symbol_names.into_iter().flatten() {
        let Some(symbol) = checker.lookup_symbol(&name) else {
            continue;
        };
        match &symbol.kind {
            SymbolKind::Function(info) => {
                return Some(ResolvedType::Function(
                    info.params.clone(),
                    Box::new(info.return_type.clone()),
                ));
            }
            SymbolKind::Variable(VariableInfo {
                ty: ResolvedType::Function(params, return_type),
                ..
            }) => return Some(ResolvedType::Function(params.clone(), return_type.clone())),
            _ => {}
        }
    }
    None
}

/// Convert a literal parameter default into checked public metadata.
pub(super) fn checked_param_default_literal(literal: &Literal) -> CheckedParamDefault {
    match literal {
        Literal::Int(value) => CheckedParamDefault::Int(value.value),
        Literal::Float(value) => CheckedParamDefault::Float(value.value),
        Literal::Decimal(value) => CheckedParamDefault::String(value.repr.clone()),
        Literal::String(value) => CheckedParamDefault::String(value.clone()),
        Literal::Bytes(value) => CheckedParamDefault::Bytes(value.clone()),
        Literal::Bool(value) => CheckedParamDefault::Bool(*value),
        Literal::None => CheckedParamDefault::None,
    }
}

/// Convert a literal preset expression into checked partial export metadata.
pub(super) fn checked_preset_literal(literal: &Literal) -> CheckedPresetValue {
    match literal {
        Literal::Int(value) => CheckedPresetValue::Int(value.value),
        Literal::Float(value) => CheckedPresetValue::Float(value.value),
        Literal::Decimal(value) => CheckedPresetValue::String(value.repr.clone()),
        Literal::String(value) => CheckedPresetValue::String(value.clone()),
        Literal::Bytes(value) => CheckedPresetValue::Bytes(value.clone()),
        Literal::Bool(value) => CheckedPresetValue::Bool(*value),
        Literal::None => CheckedPresetValue::None,
    }
}

/// Extract a dotted constant/model path from a metadata-safe preset expression.
pub(super) fn checked_preset_path(expr: &Expr) -> Vec<String> {
    match expr {
        Expr::Ident(name) => vec![name.clone()],
        Expr::Field(base, field) => {
            let mut path = checked_preset_path(&base.node);
            if path.is_empty() {
                return Vec::new();
            }
            path.push(field.clone());
            path
        }
        _ => Vec::new(),
    }
}

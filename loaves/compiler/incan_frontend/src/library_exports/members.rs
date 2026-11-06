//! Checked fields, properties, methods and method defaults, and manifest-qualified defaults.

use super::*;

/// Map checked field metadata into manifest order, preserving source declaration order when it is available.
///
/// Dependency manifests use this order to reconstruct constructor and partial surfaces. Any fields missing from the
/// recorded source order are appended lexically so generated metadata remains deterministic.
pub(super) fn map_fields(
    fields: &HashMap<String, FieldInfo>,
    field_order: &[String],
    default_exprs: &HashMap<String, Spanned<Expr>>,
    preserved_defaults: Option<&HashMap<String, CheckedParamDefault>>,
    provider_libraries: Option<&HashMap<String, String>>,
    type_params: &[String],
    checker: &TypeChecker,
) -> Option<Vec<CheckedField>> {
    let mut defaults = default_exprs
        .iter()
        .map(|(name, default)| {
            (
                name.as_str(),
                checked_param_default(default, DefaultPathContext::for_checker(checker)),
            )
        })
        .collect::<HashMap<_, _>>();
    if let Some(preserved_defaults) = preserved_defaults {
        for (name, default) in preserved_defaults {
            let default = provider_libraries
                .and_then(|providers| providers.get(name))
                .map_or_else(
                    || Ok(default.clone()),
                    |provider| qualify_checked_default_for_manifest(default, provider, type_params),
                )
                .ok()?;
            defaults.insert(name.as_str(), default);
        }
    }
    let mut used = std::collections::HashSet::new();
    let mut entries = Vec::with_capacity(fields.len());
    let map_type = |name: &str, info: &FieldInfo| {
        let mut ty = info.ty.clone();
        checker.canonicalize_public_library_nominals(&mut ty);
        crate::rust_type_display::qualify_for_manifest(
            &ty,
            provider_libraries
                .and_then(|providers| providers.get(name))
                .map(String::as_str),
            type_params,
        )
        .ok()
    };
    for name in field_order {
        if let Some(info) = fields.get(name) {
            used.insert(name.as_str());
            entries.push(CheckedField {
                name: name.clone(),
                canonical: info.identity.clone(),
                ty: map_type(name, info)?,
                surface_type_name: info.surface_type_name.clone(),
                visibility: info.visibility,
                has_default: defaults.contains_key(name.as_str()),
                default: defaults.get(name.as_str()).cloned(),
                alias: info.alias.clone(),
                description: info.description.clone(),
            });
        }
    }
    let mut remaining = fields
        .iter()
        .filter(|(name, _)| !used.contains(name.as_str()))
        .collect::<Vec<_>>();
    remaining.sort_by_key(|(name, _)| *name);
    entries.extend(
        remaining
            .into_iter()
            .map(|(name, info)| {
                Some(CheckedField {
                    name: name.clone(),
                    canonical: info.identity.clone(),
                    ty: map_type(name, info)?,
                    surface_type_name: info.surface_type_name.clone(),
                    visibility: info.visibility,
                    has_default: defaults.contains_key(name.as_str()),
                    default: defaults.get(name.as_str()).cloned(),
                    alias: info.alias.clone(),
                    description: info.description.clone(),
                })
            })
            .collect::<Option<Vec<_>>>()?,
    );
    Some(entries)
}

/// Project public computed properties into a deterministic compiled-library contract.
pub(super) fn map_public_properties(
    properties: &HashMap<String, PropertyInfo>,
    checker: &TypeChecker,
) -> Vec<CheckedProperty> {
    let mut entries = properties
        .iter()
        .filter(|(_, property)| property.visibility == Visibility::Public)
        .map(|(name, property)| {
            let mut return_type = property.return_type.clone();
            checker.canonicalize_public_library_nominals(&mut return_type);
            CheckedProperty {
                name: name.clone(),
                canonical: property.identity.clone(),
                return_type,
            }
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    entries
}

/// Rebase one inherited compiled default through the current provider's compiler-owned dependency bridge.
pub fn qualify_checked_default_for_manifest(
    default: &CheckedParamDefault,
    provider: &str,
    type_params: &[String],
) -> Result<CheckedParamDefault, String> {
    let qualify_path = |path: &[String]| {
        let mut qualified = vec![
            crate::rust_type_display::PROVIDER_RUST_BRIDGE_MODULE.to_string(),
            provider.to_string(),
        ];
        qualified.extend(path.iter().cloned());
        qualified
    };
    match default {
        CheckedParamDefault::List(values) => Ok(CheckedParamDefault::List(
            values
                .iter()
                .map(|value| qualify_checked_default_for_manifest(value, provider, type_params))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        CheckedParamDefault::Dict(entries) => Ok(CheckedParamDefault::Dict(
            entries
                .iter()
                .map(|(key, value)| {
                    Ok((
                        qualify_checked_default_for_manifest(key, provider, type_params)?,
                        qualify_checked_default_for_manifest(value, provider, type_params)?,
                    ))
                })
                .collect::<Result<Vec<_>, String>>()?,
        )),
        CheckedParamDefault::ConstRef(path) => Ok(CheckedParamDefault::ConstRef(qualify_path(path))),
        CheckedParamDefault::Call { path, args, signature } => Ok(CheckedParamDefault::Call {
            path: qualify_path(path),
            args: args
                .iter()
                .map(|arg| {
                    Ok(CheckedParamDefaultArg {
                        name: arg.name.clone(),
                        value: qualify_checked_default_for_manifest(&arg.value, provider, type_params)?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
            signature: signature
                .as_ref()
                .map(|signature| -> Result<CheckedParamDefaultCallSignature, String> {
                    let mut params = signature.params.clone();
                    for param in &mut params {
                        param.ty =
                            crate::rust_type_display::qualify_for_manifest(&param.ty, Some(provider), type_params)?;
                    }
                    Ok(CheckedParamDefaultCallSignature {
                        params,
                        return_type: crate::rust_type_display::qualify_for_manifest(
                            &signature.return_type,
                            Some(provider),
                            type_params,
                        )?,
                    })
                })
                .transpose()?,
        }),
        CheckedParamDefault::MethodCall {
            receiver_path,
            method,
            args,
        } => Ok(CheckedParamDefault::MethodCall {
            receiver_path: qualify_path(receiver_path),
            method: method.clone(),
            args: args
                .iter()
                .map(|arg| {
                    Ok(CheckedParamDefaultArg {
                        name: arg.name.clone(),
                        value: qualify_checked_default_for_manifest(&arg.value, provider, type_params)?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
        }),
        CheckedParamDefault::Int(_)
        | CheckedParamDefault::Float(_)
        | CheckedParamDefault::Bool(_)
        | CheckedParamDefault::String(_)
        | CheckedParamDefault::Bytes(_)
        | CheckedParamDefault::None
        | CheckedParamDefault::Unsupported => Ok(default.clone()),
    }
}

/// Convert legacy one-method-per-name metadata into checked export methods.
pub(super) fn map_methods_with_defaults(
    methods: &HashMap<String, MethodInfo>,
    ast_methods: &[Spanned<crate::ast::MethodDecl>],
    checker: &TypeChecker,
) -> Vec<CheckedMethod> {
    let mut entries: Vec<_> = methods
        .iter()
        .filter(|(name, info)| is_exported_method_name(name) || is_registry_description_method(info))
        .map(|(name, info)| checked_method_from_info(name, info))
        .collect();
    attach_method_defaults(&mut entries, ast_methods, checker);
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    entries
}

/// Flatten same-name overload groups into manifest-ready checked methods without losing RFC 025 trait implementations.
pub(super) fn map_method_overloads_with_defaults(
    method_overloads: &HashMap<String, Vec<MethodInfo>>,
    ast_methods: &[Spanned<crate::ast::MethodDecl>],
    checker: &TypeChecker,
) -> Vec<CheckedMethod> {
    let mut entries: Vec<_> = method_overloads
        .iter()
        .flat_map(|(name, overloads)| {
            overloads
                .iter()
                .filter(|info| is_exported_method_name(name) || is_registry_description_method(info))
                .map(|info| checked_method_from_info(name, info))
        })
        .collect();
    attach_method_defaults(&mut entries, ast_methods, checker);
    entries.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| method_signature_sort_key(left).cmp(&method_signature_sort_key(right)))
    });
    entries
}

/// Return whether a method belongs in public API metadata.
pub(super) fn is_exported_method_name(name: &str) -> bool {
    !name.starts_with('_') || name.starts_with("__") || matches!(name, "_checked_current_unit" | "_checked_package")
}

/// Publish the standard registry's reserved registration signatures for compiler-generated descriptions.
/// Underscore-prefixed methods elsewhere retain ordinary private metadata; the checked provider identity grants this
/// exception.
pub(super) fn is_registry_description_method(info: &MethodInfo) -> bool {
    let Some(identity) = &info.identity else {
        return false;
    };
    if identity.kind != SemanticSourceTargetKind::Method
        || !matches!(
            identity.declaration_name.as_str(),
            "_describe" | "_describe_function" | "_describe_method"
        )
    {
        return false;
    }
    match &identity.origin {
        SymbolOrigin::Package { library, module_path } => {
            matches!(library.as_str(), "incan_stdlib_core" | "incan_stdlib_primitives")
                && module_path.iter().map(String::as_str).eq(["registry"])
        }
        SymbolOrigin::Module(path) => path.iter().map(String::as_str).eq(["std", "registry"]),
        _ => false,
    }
}

/// Convert one semantic method entry into the checked export shape.
pub(super) fn checked_method_from_info(name: &str, info: &MethodInfo) -> CheckedMethod {
    CheckedMethod {
        ownership: None,
        name: name.to_string(),
        canonical: info.identity.clone(),
        visibility: info.scope.published_visibility(),
        alias_of: info.alias_of.clone(),
        type_params: info
            .type_params
            .iter()
            .map(|type_param| CheckedTypeParam {
                name: type_param.clone(),
                bounds: info
                    .type_param_bound_details
                    .get(type_param)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|bound| CheckedTypeBound {
                        name: bound.name,
                        source_name: bound.source_name,
                        type_args: bound.type_args,
                        module_path: bound.module_path,
                        implementation_type_params: bound.implementation_type_params,
                        inferred: bound.inferred,
                    })
                    .collect(),
            })
            .collect(),
        receiver: info.receiver,
        params: info.params.clone(),
        param_defaults: vec![None; info.params.len()],
        return_type: info.return_type.clone(),
        is_async: info.is_async,
        has_body: info.has_body,
    }
}

/// Attach source-level method defaults to checked semantic method entries.
pub(super) fn attach_method_defaults(
    entries: &mut [CheckedMethod],
    ast_methods: &[Spanned<crate::ast::MethodDecl>],
    checker: &TypeChecker,
) {
    let mut used = vec![false; ast_methods.len()];
    let default_context = DefaultPathContext::for_callable_parameters(checker);
    for entry in entries {
        let Some(idx) = matching_ast_method_index(entry, ast_methods, &used, checker) else {
            continue;
        };
        used[idx] = true;
        entry.param_defaults = ast_methods[idx]
            .node
            .params
            .iter()
            .map(|param| {
                param
                    .node
                    .default
                    .as_ref()
                    .map(|default| checked_param_default(default, default_context))
            })
            .collect();
    }
}

/// Find the source method that corresponds to one checked method entry.
pub(super) fn matching_ast_method_index(
    checked: &CheckedMethod,
    ast_methods: &[Spanned<crate::ast::MethodDecl>],
    used: &[bool],
    checker: &TypeChecker,
) -> Option<usize> {
    ast_methods
        .iter()
        .enumerate()
        .find(|(idx, method)| {
            !used.get(*idx).copied().unwrap_or(false) && ast_method_shape_matches_checked(method, checked, checker)
        })
        .map(|(idx, _)| idx)
        .or_else(|| {
            ast_methods
                .iter()
                .enumerate()
                .find(|(idx, method)| {
                    !used.get(*idx).copied().unwrap_or(false) && ast_method_param_names_match_checked(method, checked)
                })
                .map(|(idx, _)| idx)
        })
}

/// Return whether an AST method and checked method have the same parameter names.
pub(super) fn ast_method_param_names_match_checked(
    ast_method: &Spanned<crate::ast::MethodDecl>,
    checked: &CheckedMethod,
) -> bool {
    ast_method.node.name == checked.name
        && ast_method.node.params.len() == checked.params.len()
        && ast_method
            .node
            .params
            .iter()
            .zip(checked.params.iter())
            .all(|(ast_param, checked_param)| checked_param.name() == Some(ast_param.node.name.as_str()))
}

/// Return whether an AST method and checked method have the same callable shape.
pub(super) fn ast_method_shape_matches_checked(
    ast_method: &Spanned<crate::ast::MethodDecl>,
    checked: &CheckedMethod,
    checker: &TypeChecker,
) -> bool {
    let ast_type_params = ast_method
        .node
        .type_params
        .iter()
        .map(|type_param| type_param.name.as_str())
        .collect::<Vec<_>>();
    ast_method.node.name == checked.name
        && ast_method.node.params.len() == checked.params.len()
        && ast_method.node.receiver == checked.receiver
        && ast_method.node.is_async() == checked.is_async
        && ast_type_params
            == checked
                .type_params
                .iter()
                .map(|param| param.name.as_str())
                .collect::<Vec<_>>()
        && ast_method
            .node
            .params
            .iter()
            .zip(checked.params.iter())
            .all(|(ast_param, checked_param)| {
                checked_param.name() == Some(ast_param.node.name.as_str())
                    && checked_param.kind == ast_param.node.kind
                    && checked_param.has_default == ast_param.node.default.is_some()
                    && checked_param.ty == resolve_type(&ast_param.node.ty.node, &checker.symbols)
            })
}

/// Render a deterministic method-signature sort key for overload groups.
pub(super) fn method_signature_sort_key(method: &CheckedMethod) -> String {
    let params = method
        .params
        .iter()
        .map(|param| {
            let name = param.name().unwrap_or("_");
            format!("{name}:{}", param.ty)
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("({params})->{}", method.return_type)
}

pub(super) fn sorted_vec(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values
}

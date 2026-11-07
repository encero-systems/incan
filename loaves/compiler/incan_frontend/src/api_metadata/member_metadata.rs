//! Checked API records for methods, type parameters, bounds, fields and properties.

use super::*;

/// Pair AST method declarations with checked method metadata for documentation output.
pub(super) fn methods(
    ast_methods: &[Spanned<MethodDecl>],
    ast_aliases: &[Spanned<MethodAliasDecl>],
    checked_methods: &[CheckedMethod],
    checker: &TypeChecker,
    module_path: &[String],
    owner: &str,
) -> Vec<ApiMethod> {
    let mut checked_by_name: HashMap<&str, VecDeque<&CheckedMethod>> = HashMap::new();
    for method in checked_methods {
        checked_by_name
            .entry(method.name.as_str())
            .or_default()
            .push_back(method);
    }
    let mut out = Vec::new();
    for method in ast_methods {
        let Some(candidates) = checked_by_name.get_mut(method.node.name.as_str()) else {
            continue;
        };
        let Some(checked) = take_checked_method_for_ast(&method.node, candidates, checker) else {
            continue;
        };
        let docstring = method.node.body.as_ref().and_then(|body| function_docstring(body));
        let mut callable = ApiCallableMetadata {
            ownership: checked_api_ownership(checked.ownership.as_ref(), checker),
            name: checked.name.clone(),
            anchor: anchor(module_path, &format!("{owner}.{}", checked.name), method.span),
            type_params: type_params(&checked.type_params),
            receiver: checked.receiver.map(|receiver| match receiver {
                crate::ast::Receiver::Immutable => ReceiverExport::Immutable,
                crate::ast::Receiver::Mutable => ReceiverExport::Mutable,
            }),
            params: params_from_checked(&checked.params, &checked.param_defaults),
            return_type: type_ref_from_resolved(&checked.return_type),
            is_async: checked.is_async,
        };
        crate::library_manifest::ownership::mark_types(
            callable.ownership.as_ref(),
            &mut callable.params,
            &mut callable.return_type,
        );
        out.push(ApiMethod {
            ownership: callable.ownership.clone(),
            name: callable.name.clone(),
            canonical: canonical_identity_export(checked.canonical.as_ref()),
            visibility: checked.visibility.into(),
            alias_of: checked.alias_of.clone(),
            anchor: callable.anchor.clone(),
            docstring_sections: parse_docstring(docstring.as_deref()),
            docstring,
            decorators: decorators_metadata(&method.node.decorators, checker, Some(&callable)),
            type_params: callable.type_params,
            receiver: callable.receiver,
            params: callable.params,
            return_type: callable.return_type,
            is_async: callable.is_async,
            has_body: checked.has_body,
            made_private: checked.made_private.as_ref().map(Into::into),
        });
    }
    for alias in ast_aliases {
        let Some(candidates) = checked_by_name.get_mut(alias.node.name.as_str()) else {
            continue;
        };
        while let Some(checked) = candidates.pop_front() {
            if checked.alias_of.as_deref() != Some(alias.node.target.as_str()) {
                continue;
            }
            let ownership = checked_api_ownership(checked.ownership.as_ref(), checker);
            let mut params = params_from_checked(&checked.params, &checked.param_defaults);
            let mut return_type = type_ref_from_resolved(&checked.return_type);
            crate::library_manifest::ownership::mark_types(ownership.as_ref(), &mut params, &mut return_type);
            out.push(ApiMethod {
                ownership,
                name: checked.name.clone(),
                canonical: canonical_identity_export(checked.canonical.as_ref()),
                visibility: checked.visibility.into(),
                alias_of: checked.alias_of.clone(),
                anchor: anchor(module_path, &format!("{owner}.{}", checked.name), alias.span),
                docstring: None,
                docstring_sections: None,
                decorators: Vec::new(),
                type_params: type_params(&checked.type_params),
                receiver: checked.receiver.map(|receiver| match receiver {
                    crate::ast::Receiver::Immutable => ReceiverExport::Immutable,
                    crate::ast::Receiver::Mutable => ReceiverExport::Mutable,
                }),
                params,
                return_type,
                is_async: checked.is_async,
                has_body: checked.has_body,
                made_private: checked.made_private.as_ref().map(Into::into),
            });
        }
    }
    out
}

/// Remove the checked method that best matches one AST method declaration.
pub(super) fn take_checked_method_for_ast<'a>(
    ast_method: &MethodDecl,
    candidates: &mut VecDeque<&'a CheckedMethod>,
    checker: &TypeChecker,
) -> Option<&'a CheckedMethod> {
    if let Some(index) = candidates
        .iter()
        .position(|checked| checked_method_shape_matches(ast_method, checked, checker))
    {
        return candidates.remove(index);
    }
    if let Some(index) = candidates
        .iter()
        .position(|checked| checked_method_param_names_match(ast_method, checked))
    {
        return candidates.remove(index);
    }
    candidates.pop_front()
}

/// Return whether an AST method and checked method have the same parameter names.
pub(super) fn checked_method_param_names_match(ast_method: &MethodDecl, checked: &CheckedMethod) -> bool {
    ast_method.params.len() == checked.params.len()
        && ast_method
            .params
            .iter()
            .zip(checked.params.iter())
            .all(|(ast_param, checked_param)| checked_param.name() == Some(ast_param.node.name.as_str()))
}

/// Return whether an AST method and checked method have the same callable shape.
pub(super) fn checked_method_shape_matches(
    ast_method: &MethodDecl,
    checked: &CheckedMethod,
    checker: &TypeChecker,
) -> bool {
    let ast_type_params: Vec<&str> = ast_method
        .type_params
        .iter()
        .map(|type_param| type_param.name.as_str())
        .collect();
    ast_method.params.len() == checked.params.len()
        && ast_method.receiver == checked.receiver
        && ast_method.is_async() == checked.is_async
        && ast_type_params
            == checked
                .type_params
                .iter()
                .map(|param| param.name.as_str())
                .collect::<Vec<_>>()
        && ast_method
            .params
            .iter()
            .zip(checked.params.iter())
            .all(|(ast_param, checked_param)| {
                checked_param.name() == Some(ast_param.node.name.as_str())
                    && checked_param.kind == ast_param.node.kind
                    && checked_param.has_default == ast_param.node.default.is_some()
                    && type_ref_from_resolved(&checked_param.ty)
                        == type_ref_from_resolved(&crate::symbols::resolve_type(
                            &ast_param.node.ty.node,
                            &checker.symbols,
                        ))
            })
        && type_ref_from_resolved(&checked.return_type)
            == type_ref_from_resolved(&crate::symbols::resolve_type(
                &ast_method.return_type.node,
                &checker.symbols,
            ))
}

/// Convert checked type parameters into API metadata exports.
pub(super) fn type_params(type_params: &[CheckedTypeParam]) -> Vec<TypeParamExport> {
    type_params
        .iter()
        .map(|type_param| TypeParamExport {
            name: type_param.name.clone(),
            bounds: type_param.bounds.iter().map(type_bound).collect(),
        })
        .collect()
}

/// Convert a checked trait bound into the exported API metadata representation.
pub(super) fn type_bound(bound: &CheckedTypeBound) -> TypeBoundExport {
    TypeBoundExport {
        name: bound.name.clone(),
        source_name: bound.source_name.clone(),
        module_path: bound.module_path.clone(),
        type_args: bound.type_args.iter().map(type_ref_from_resolved).collect(),
        implementation_type_params: bound
            .implementation_type_params
            .iter()
            .map(implementation_type_param)
            .collect(),
        inferred: bound.inferred,
    }
}

/// Convert one checked implementation parameter into API manifest metadata.
pub(super) fn implementation_type_param(type_param: &ImplementationTypeParamInfo) -> ImplementationTypeParamExport {
    ImplementationTypeParamExport {
        name: type_param.name.clone(),
        bounds: type_param.bounds.iter().map(implementation_trait_bound).collect(),
    }
}

/// Convert one checked implementation requirement into API manifest metadata.
pub(super) fn implementation_trait_bound(bound: &ImplementationTraitBoundInfo) -> ImplementationTraitBoundExport {
    ImplementationTraitBoundExport {
        trait_path: bound.trait_path.clone(),
        type_args: bound.type_args.iter().map(type_ref_from_resolved).collect(),
        associated_types: bound
            .associated_types
            .iter()
            .map(|(name, ty)| ImplementationAssociatedTypeExport {
                name: name.clone(),
                ty: type_ref_from_resolved(ty),
            })
            .collect(),
        origin: match bound.origin {
            ImplementationTraitBoundOriginInfo::Standard => ImplementationTraitBoundOriginExport::Standard,
            ImplementationTraitBoundOriginInfo::RustCapability => ImplementationTraitBoundOriginExport::RustCapability,
            ImplementationTraitBoundOriginInfo::SourceCallable => ImplementationTraitBoundOriginExport::SourceCallable,
        },
    }
}

/// Convert a checked field into API metadata.
pub(super) fn field(field: &crate::library_exports::CheckedField) -> FieldExport {
    let default = field.default.as_ref().and_then(param_default_from_checked);
    FieldExport {
        name: field.name.clone(),
        canonical: canonical_identity_export(field.canonical.as_ref()),
        record_identity: hex::encode(&field.record_identity),
        record_canonical: field.record_canonical,
        renamed_from: field.renamed_from.clone(),
        ty: type_ref_from_resolved(&field.ty),
        surface_type_name: field.surface_type_name.clone(),
        visibility: match field.visibility {
            Visibility::Private => crate::library_manifest::MemberVisibilityExport::Private,
            Visibility::Public => crate::library_manifest::MemberVisibilityExport::Public,
        },
        has_default: field.has_default,
        default,
        alias: field.alias.clone(),
        description: field.description.clone(),
        made_private: field.made_private.as_ref().map(Into::into),
    }
}

/// Convert checked computed properties into API metadata.
pub(super) fn properties(properties: &[CheckedProperty]) -> Vec<ApiProperty> {
    properties
        .iter()
        .map(|property| ApiProperty {
            name: property.name.clone(),
            canonical: canonical_identity_export(property.canonical.as_ref()),
            return_type: type_ref_from_resolved(&property.return_type),
        })
        .collect()
}

/// Convert a package-owned checked identity into its stable manifest representation.
///
/// API metadata is also produced for editor-only module checks that have no package owner. Those retain the legacy
/// absence rather than inventing a package name; compiled-library producers assign package origins before collection.
pub(super) fn canonical_identity_export(identity: Option<&CanonicalSymbolId>) -> Option<CanonicalIdentityExport> {
    let identity = identity?;
    let SymbolOrigin::Package { library, .. } = &identity.origin else {
        return None;
    };
    CanonicalIdentityExport::from_canonical(library, identity)
}

/// Return checked fields ordered to match the source declaration.
pub(super) fn fields_in_source_order(
    ast_fields: &[Spanned<FieldDecl>],
    checked_fields: &[CheckedField],
) -> Vec<FieldExport> {
    let checked_by_name: HashMap<&str, &CheckedField> = checked_fields
        .iter()
        .map(|field| (field.name.as_str(), field))
        .collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();

    for ast_field in ast_fields {
        if let Some(checked) = checked_by_name.get(ast_field.node.name.as_str()) {
            seen.insert(checked.name.as_str());
            out.push(field(checked));
        }
    }

    for checked in checked_fields {
        if seen.insert(checked.name.as_str()) {
            out.push(field(checked));
        }
    }

    out
}

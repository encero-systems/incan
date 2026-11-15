//! Checked function, type alias, model, class, trait, enum, newtype, const and static exports, and bounds.

use super::*;

/// Build checked export metadata for a function or callable-valued decorated function binding.
///
/// A type parameter exports its declared bounds as spelled in source, then the `Eq` and `Hash` bounds the checker
/// inferred for it because the body hashes it (#1758).
pub(crate) fn checked_function_export(
    function: &FunctionDecl,
    checker: &TypeChecker,
    span: Span,
) -> Option<CheckedFunctionExport> {
    let symbol = checker.lookup_symbol(function.name.as_str())?;
    let (params, return_type, is_async, emitted_name) = match &symbol.kind {
        SymbolKind::Function(FunctionInfo {
            params,
            return_type,
            is_async,
            emitted_name,
            ..
        }) => (params.clone(), return_type.clone(), *is_async, emitted_name.clone()),
        SymbolKind::FunctionOverloads(overloads) => {
            let overload = overloads.iter().find(|overload| overload.span == span)?;
            (
                overload.info.params.clone(),
                overload.info.return_type.clone(),
                overload.info.is_async,
                overload.info.emitted_name.clone(),
            )
        }
        SymbolKind::Variable(VariableInfo {
            ty: ResolvedType::Function(params, return_type),
            ..
        }) => {
            // Callable values do not yet carry asyncness, so preserve the source declaration marker until decorator
            // typing records async callable metadata explicitly.
            (params.clone(), return_type.as_ref().clone(), function.is_async(), None)
        }
        _ => return None,
    };

    let default_context = DefaultPathContext::for_callable_parameters(checker);
    Some(CheckedFunctionExport {
        ownership: None,
        name: function.name.clone(),
        emitted_name,
        type_params: checked_type_params(&function.type_params, checker)
            .into_iter()
            .map(|mut type_param| {
                type_param.bounds.extend(map_type_bound_infos(
                    checker.inferred_type_param_bounds(&function.name, &type_param.name),
                ));
                type_param
            })
            .collect(),
        param_defaults: function
            .params
            .iter()
            .map(|param| {
                param
                    .node
                    .default
                    .as_ref()
                    .map(|default| checked_param_default(default, default_context))
            })
            .collect(),
        params,
        return_type,
        is_async,
    })
}

/// Convert checked function metadata type parameters into export metadata type parameters.
pub(super) fn checked_function_type_params(info: &FunctionInfo) -> Vec<CheckedTypeParam> {
    info.type_params
        .iter()
        .map(|name| CheckedTypeParam {
            name: name.clone(),
            bounds: info
                .type_param_bound_details
                .get(name)
                .map_or_else(Vec::new, |bounds| map_type_bound_infos(bounds)),
        })
        .collect()
}

pub(super) fn checked_type_alias_export(alias: &TypeAliasDecl, checker: &TypeChecker) -> CheckedTypeAliasExport {
    let target = resolve_type(&alias.target.node, &checker.symbols);
    CheckedTypeAliasExport {
        name: alias.name.clone(),
        type_params: checked_type_params(&alias.type_params, checker),
        target,
    }
}

/// Extract a checked public model export from collected model symbol metadata.
pub(super) fn checked_model_export(model: &ModelDecl, checker: &TypeChecker) -> Option<CheckedModelExport> {
    let symbol = checker.lookup_symbol(model.name.as_str())?;
    let SymbolKind::Type(TypeInfo::Model(ModelInfo {
        type_params: owner_type_params,
        traits,
        trait_adoptions,
        derives,
        fields,
        field_order,
        properties,
        method_overloads,
        ..
    })) = &symbol.kind
    else {
        return None;
    };

    let defaults = model
        .fields
        .iter()
        .filter_map(|field| {
            field
                .node
                .default
                .as_ref()
                .map(|default| (field.node.name.clone(), default.clone()))
        })
        .collect();
    Some(CheckedModelExport {
        name: model.name.clone(),
        type_params: checked_type_params(&model.type_params, checker),
        traits: sorted_vec(traits.to_vec()),
        trait_adoptions: checked_trait_adoption_closure(trait_adoptions, owner_type_params, checker),
        derives: sorted_vec(derives.to_vec()),
        fields: map_fields(fields, field_order, &defaults, None, None, &[], checker)?,
        properties: map_public_properties(properties, checker),
        methods: map_method_overloads_with_defaults(method_overloads, &model.methods, checker),
    })
}

/// Extract a checked public class export from collected class symbol metadata.
pub(super) fn checked_class_export(class: &ClassDecl, checker: &TypeChecker) -> Option<CheckedClassExport> {
    let symbol = checker.lookup_symbol(class.name.as_str())?;
    let SymbolKind::Type(TypeInfo::Class(ClassInfo {
        type_params: owner_type_params,
        extends,
        traits,
        trait_adoptions,
        derives,
        fields,
        field_defaults,
        field_default_metadata,
        field_provider_libraries,
        field_order,
        properties,
        method_overloads,
        ..
    })) = &symbol.kind
    else {
        return None;
    };

    let type_params = class
        .type_params
        .iter()
        .map(|param| param.name.clone())
        .collect::<Vec<_>>();
    Some(CheckedClassExport {
        name: class.name.clone(),
        type_params: checked_type_params(&class.type_params, checker),
        extends: extends.clone(),
        traits: sorted_vec(traits.to_vec()),
        trait_adoptions: checked_trait_adoption_closure(trait_adoptions, owner_type_params, checker),
        derives: sorted_vec(derives.to_vec()),
        fields: map_fields(
            fields,
            field_order,
            field_defaults,
            Some(field_default_metadata),
            Some(field_provider_libraries),
            &type_params,
            checker,
        )?,
        properties: map_public_properties(properties, checker),
        methods: map_method_overloads_with_defaults(method_overloads, &class.methods, checker),
    })
}

/// Extract the checked public trait contract, preserving resolved supertrait identities for package consumers.
pub(super) fn checked_trait_export(trait_decl: &TraitDecl, checker: &TypeChecker) -> Option<CheckedTraitExport> {
    let symbol = checker.lookup_symbol(trait_decl.name.as_str())?;
    let SymbolKind::Trait(TraitInfo { requires, methods, .. }) = &symbol.kind else {
        return None;
    };

    let mut sorted_requires = requires.clone();
    sorted_requires.sort_by(|(left, _), (right, _)| left.cmp(right));

    Some(CheckedTraitExport {
        name: trait_decl.name.clone(),
        source_name: trait_decl.name.clone(),
        type_params: checked_type_params(&trait_decl.type_params, checker),
        supertraits: sorted_type_bounds(checked_spanned_trait_bounds(&trait_decl.traits, checker)),
        requires: sorted_requires,
        methods: map_methods_with_defaults(methods, &trait_decl.methods, checker),
    })
}

/// Extract the checked public enum contract, including value-enum metadata when present.
///
/// Each variant publishes the payload its own enum declares, as the enum's checked metadata records it, whatever other
/// enum in scope shares the variant's name.
pub(super) fn checked_enum_export(enum_decl: &EnumDecl, checker: &TypeChecker) -> Option<CheckedEnumExport> {
    let symbol = checker.lookup_symbol(enum_decl.name.as_str())?;
    let SymbolKind::Type(TypeInfo::Enum(enum_info)) = &symbol.kind else {
        return None;
    };

    let mut variants = Vec::new();
    for variant in &enum_decl.variants {
        // Not the bare variant name's symbol: that binds whichever enum declared the name first, and it keeps a payload
        // type declared after the enum as a placeholder.
        let fields = enum_info
            .variant_fields
            .get(&variant.node.name)
            .cloned()
            .unwrap_or_else(|| {
                variant
                    .node
                    .fields
                    .iter()
                    .map(|field| resolve_type(&field.node, &checker.symbols))
                    .collect()
            });

        variants.push(CheckedEnumVariant {
            name: variant.node.name.clone(),
            canonical: Some(checker.symbols.member_declaration_identity(
                &variant.node.name,
                SemanticSourceTargetKind::Variant,
                variant.span,
            )),
            fields,
            value: enum_info
                .value_enum
                .as_ref()
                .and_then(|value_enum| value_enum.values.get(&variant.node.name).cloned()),
        });
    }

    Some(CheckedEnumExport {
        name: enum_decl.name.clone(),
        type_params: checked_type_params(&enum_decl.type_params, checker),
        traits: sorted_vec(enum_info.traits.clone()),
        trait_adoptions: checked_trait_adoption_closure(&enum_info.trait_adoptions, &enum_info.type_params, checker),
        value_type: enum_info.value_enum.as_ref().map(|value_enum| value_enum.value_type),
        variants,
        variant_aliases: enum_decl
            .variant_aliases
            .iter()
            .map(|alias| CheckedEnumVariantAlias {
                name: alias.node.name.clone(),
                target: alias.node.target.clone(),
            })
            .collect(),
        methods: map_method_overloads_with_defaults(&enum_info.method_overloads, &enum_decl.methods, checker),
        derives: sorted_vec(enum_info.derives.clone()),
    })
}

/// Build a manifest-ready newtype export from the checked symbol metadata.
pub(super) fn checked_newtype_export(
    newtype_decl: &NewtypeDecl,
    checker: &TypeChecker,
) -> Option<CheckedNewtypeExport> {
    let symbol = checker.lookup_symbol(newtype_decl.name.as_str())?;
    let SymbolKind::Type(TypeInfo::Newtype(info)) = &symbol.kind else {
        return None;
    };
    let NewtypeInfo {
        is_rusttype,
        underlying,
        constraints,
        implicit_coercion_enabled,
        traits,
        trait_adoptions,
        derives,
        method_overloads,
        ..
    } = info;

    Some(CheckedNewtypeExport {
        name: newtype_decl.name.clone(),
        type_params: checked_type_params(&newtype_decl.type_params, checker),
        traits: sorted_vec(traits.clone()),
        trait_adoptions: checked_trait_adoption_closure(trait_adoptions, &info.type_params, checker),
        derives: sorted_vec(derives.clone()),
        is_rusttype: *is_rusttype,
        underlying: underlying.clone(),
        checked_constructor: checker.validated_newtype_ctor_name(&newtype_decl.name, info),
        constraints: constraints.clone(),
        implicit_coercion_enabled: *implicit_coercion_enabled,
        methods: map_method_overloads_with_defaults(method_overloads, &newtype_decl.methods, checker),
    })
}

pub(super) fn checked_const_export(const_name: &str, checker: &TypeChecker) -> Option<CheckedConstExport> {
    let symbol = checker.lookup_symbol(const_name)?;
    let SymbolKind::Variable(variable_info) = &symbol.kind else {
        return None;
    };
    Some(CheckedConstExport {
        name: const_name.to_string(),
        ty: variable_info.ty.clone(),
    })
}

pub(super) fn checked_static_export(static_name: &str, checker: &TypeChecker) -> Option<CheckedStaticExport> {
    let symbol = checker.lookup_symbol(static_name)?;
    let SymbolKind::Static(static_info) = &symbol.kind else {
        return None;
    };
    Some(CheckedStaticExport {
        name: static_name.to_string(),
        ty: static_info.ty.clone(),
    })
}

pub(super) fn checked_type_params(type_params: &[TypeParam], checker: &TypeChecker) -> Vec<CheckedTypeParam> {
    type_params
        .iter()
        .map(|type_param| CheckedTypeParam {
            name: type_param.name.clone(),
            bounds: checked_trait_bounds(&type_param.bounds, checker),
        })
        .collect()
}

/// Resolve source trait bounds into checked export metadata.
pub(super) fn checked_trait_bounds(bounds: &[TraitBound], checker: &TypeChecker) -> Vec<CheckedTypeBound> {
    bounds.iter().map(|bound| checked_trait_bound(bound, checker)).collect()
}

/// Resolve source trait bounds that still carry their parse spans.
pub(super) fn checked_spanned_trait_bounds(
    bounds: &[Spanned<TraitBound>],
    checker: &TypeChecker,
) -> Vec<CheckedTypeBound> {
    bounds
        .iter()
        .map(|bound| checked_trait_bound(&bound.node, checker))
        .collect()
}

/// Resolve one source trait bound into checked export metadata.
pub(super) fn checked_trait_bound(bound: &TraitBound, checker: &TypeChecker) -> CheckedTypeBound {
    CheckedTypeBound {
        name: checker
            .imported_generic_rust_bound_path(&bound.name)
            .unwrap_or_else(|| bound.name.clone()),
        source_name: checker.trait_bound_source_name(&bound.name),
        type_args: bound
            .type_args
            .iter()
            .map(|type_arg| resolve_type(&type_arg.node, &checker.symbols))
            .collect(),
        module_path: checker.trait_bound_module_path(&bound.name),
        implementation_type_params: Vec::new(),
        inferred: false,
    }
}

/// Convert collected trait adoption metadata into the manifest-ready checked bound shape.
pub(super) fn map_type_bound_infos(bounds: &[TypeBoundInfo]) -> Vec<CheckedTypeBound> {
    bounds
        .iter()
        .map(|bound| CheckedTypeBound {
            name: bound.name.clone(),
            source_name: bound.source_name.clone(),
            type_args: bound.type_args.clone(),
            module_path: bound.module_path.clone(),
            implementation_type_params: bound.implementation_type_params.clone(),
            inferred: bound.inferred,
        })
        .collect()
}

/// Publish direct trait adoptions together with the transitive supertraits they semantically adopt.
///
/// A bare generic adoption uses the owner's type parameters positionally, matching typechecking and lowering. The
/// manifest must carry that instantiated shape because implementation-bound inference may attach requirements to an
/// implied supertrait rather than to the directly written subtrait.
pub(super) fn checked_trait_adoption_closure(
    adoptions: &[TypeBoundInfo],
    owner_type_params: &[String],
    checker: &TypeChecker,
) -> Vec<CheckedTypeBound> {
    let owner_args = owner_type_params
        .iter()
        .cloned()
        .map(ResolvedType::TypeVar)
        .collect::<Vec<_>>();
    let mut checked = Vec::new();

    for adoption in adoptions {
        let mut direct = map_type_bound_infos(std::slice::from_ref(adoption));
        if let Some(bound) = direct.first_mut() {
            bound.module_path = bound
                .module_path
                .take()
                .map(|path| canonicalize_library_declaration_module_path(&path));
        }
        let Some(trait_info) = checker.lookup_trait_adoption_info(adoption) else {
            checked.extend(direct);
            continue;
        };
        let direct_args = if adoption.type_args.is_empty() {
            owner_args.iter().take(trait_info.type_params.len()).cloned().collect()
        } else {
            adoption.type_args.clone()
        };
        if let Some(bound) = direct.first_mut() {
            bound.type_args = direct_args.clone();
        }
        checked.extend(direct);

        if direct_args.len() != trait_info.type_params.len() {
            continue;
        }
        let substitutions = type_param_subst_map(&trait_info.type_params, &direct_args);
        for (name, type_args) in checker.semantic_supertrait_closure(&adoption.name) {
            let source_name = checker.trait_bound_source_name(&name);
            let implied = CheckedTypeBound {
                module_path: checker
                    .trait_bound_module_path(&name)
                    .map(|path| canonicalize_library_declaration_module_path(&path)),
                source_name,
                type_args: type_args
                    .iter()
                    .map(|arg| substitute_resolved_type(arg, &substitutions))
                    .collect(),
                name,
                implementation_type_params: Vec::new(),
                inferred: false,
            };
            if !checked.iter().any(|bound| {
                bound.name == implied.name
                    && bound.module_path == implied.module_path
                    && bound.type_args == implied.type_args
            }) {
                checked.push(implied);
            }
        }
    }

    sorted_type_bounds(checked)
}

/// Sort generic trait adoptions deterministically for stable library manifests.
pub(super) fn sorted_type_bounds(mut bounds: Vec<CheckedTypeBound>) -> Vec<CheckedTypeBound> {
    bounds.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| type_args_sort_key(&left.type_args).cmp(&type_args_sort_key(&right.type_args)))
    });
    bounds
}

/// Render type arguments into a deterministic key for sorting trait adoptions.
pub(super) fn type_args_sort_key(args: &[ResolvedType]) -> String {
    args.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")
}

//! Checked API records for partials, functions, models, classes, traits, enums, newtypes and aliases.

use super::*;

/// Convert checked partial export metadata into the checked API package shape.
pub(super) fn api_partial(export: &CheckedPartialExport, span: Span, module_path: &[String]) -> ApiPartial {
    ApiPartial {
        name: export.name.clone(),
        anchor: anchor(module_path, &export.name, span),
        target_path: export.target_path.clone(),
        target_kind: api_partial_target_kind(export.target_kind),
        presets: export
            .presets
            .iter()
            .map(|preset| PartialPresetExport {
                name: preset.name.clone(),
                ty: type_ref_from_resolved(&preset.ty),
                value: api_preset_value(&preset.value),
            })
            .collect(),
        type_params: type_params(&export.type_params),
        params: params_from_checked(&export.params, &[]),
        return_type: type_ref_from_resolved(&export.return_type),
        is_async: export.is_async,
    }
}

/// Convert frontend partial target kind metadata into manifest/API vocabulary.
pub(super) fn api_partial_target_kind(kind: CheckedPartialTargetKind) -> PartialTargetKindExport {
    match kind {
        CheckedPartialTargetKind::Function => PartialTargetKindExport::Function,
        CheckedPartialTargetKind::ModelConstructor => PartialTargetKindExport::ModelConstructor,
        CheckedPartialTargetKind::ClassConstructor => PartialTargetKindExport::ClassConstructor,
        CheckedPartialTargetKind::NewtypeConstructor => PartialTargetKindExport::NewtypeConstructor,
        CheckedPartialTargetKind::Partial => PartialTargetKindExport::Partial,
        CheckedPartialTargetKind::Unknown => PartialTargetKindExport::Unknown,
    }
}

/// Convert checked preset values into the serialized API metadata representation.
pub(super) fn api_preset_value(value: &CheckedPresetValue) -> PresetValueExport {
    match value {
        CheckedPresetValue::Int(value) => PresetValueExport::Int(*value),
        CheckedPresetValue::Float(value) => PresetValueExport::Float(value.to_string()),
        CheckedPresetValue::Bool(value) => PresetValueExport::Bool(*value),
        CheckedPresetValue::String(value) => PresetValueExport::String(value.clone()),
        CheckedPresetValue::Bytes(value) => PresetValueExport::Bytes(value.clone()),
        CheckedPresetValue::None => PresetValueExport::None,
        CheckedPresetValue::List(values) => PresetValueExport::List(values.iter().map(api_preset_value).collect()),
        CheckedPresetValue::Set(values) => PresetValueExport::Set(values.iter().map(api_preset_value).collect()),
        CheckedPresetValue::Tuple(values) => PresetValueExport::Tuple(values.iter().map(api_preset_value).collect()),
        CheckedPresetValue::Dict(entries) => PresetValueExport::Dict(
            entries
                .iter()
                .map(|(key, value)| PresetDictEntryExport {
                    key: api_preset_value(key),
                    value: api_preset_value(value),
                })
                .collect(),
        ),
        CheckedPresetValue::ConstRef(path) => PresetValueExport::ConstRef(path.clone()),
        CheckedPresetValue::ModelLiteral { name, fields } => PresetValueExport::ModelLiteral {
            name: name.clone(),
            fields: fields
                .iter()
                .map(|(field, value)| PresetModelFieldExport {
                    name: field.clone(),
                    value: api_preset_value(value),
                })
                .collect(),
        },
        CheckedPresetValue::Unsupported => PresetValueExport::Unsupported,
    }
}

/// Convert a source function declaration into API metadata.
pub(super) fn api_function(
    function: &FunctionDecl,
    span: Span,
    export: &CheckedFunctionExport,
    checker: &TypeChecker,
    module_path: &[String],
) -> ApiFunction {
    let docstring = function_docstring(&function.body);
    let callable = api_callable_for_function(function, span, export, checker, module_path);
    ApiFunction {
        ownership: callable.ownership.clone(),
        name: callable.name.clone(),
        anchor: callable.anchor.clone(),
        docstring_sections: parse_docstring(docstring.as_deref()),
        docstring,
        decorators: decorators_metadata(&function.decorators, checker, Some(&callable)),
        type_params: callable.type_params,
        params: callable.params,
        return_type: callable.return_type,
        is_async: callable.is_async,
    }
}

/// Convert a source function declaration into callable API metadata.
pub(super) fn api_callable_for_function(
    function: &FunctionDecl,
    span: Span,
    export: &CheckedFunctionExport,
    checker: &TypeChecker,
    module_path: &[String],
) -> ApiCallableMetadata {
    let mut callable = ApiCallableMetadata {
        ownership: checked_api_ownership(export.ownership.as_ref(), checker),
        name: export.name.clone(),
        anchor: anchor(module_path, &export.name, span),
        type_params: type_params(&export.type_params),
        receiver: None,
        params: params_from_checked(&source_function_params(function, checker), &export.param_defaults),
        return_type: source_function_return_type(function, checker),
        is_async: function.is_async(),
    };
    crate::library_manifest::ownership::mark_types(
        callable.ownership.as_ref(),
        &mut callable.params,
        &mut callable.return_type,
    );
    callable
}

/// Resolve the source return type used by function API metadata.
pub(super) fn source_function_return_type(function: &FunctionDecl, checker: &TypeChecker) -> TypeRef {
    type_ref_from_resolved(&crate::symbols::resolve_type(
        &function.return_type.node,
        &checker.symbols,
    ))
}

/// Resolve source-declared callable parameters, including caller-visible mutation, for checked API metadata.
pub(super) fn source_function_params(
    function: &FunctionDecl,
    checker: &TypeChecker,
) -> Vec<crate::symbols::CallableParam> {
    function
        .params
        .iter()
        .map(|param| {
            crate::symbols::CallableParam::named_with_default(
                param.node.name.clone(),
                crate::symbols::resolve_type(&param.node.ty.node, &checker.symbols),
                param.node.kind,
                param.node.default.is_some(),
            )
            .with_mut(param.node.is_mut)
        })
        .collect()
}

/// Convert a source model declaration into API metadata.
pub(super) fn api_model(
    model: &ModelDecl,
    span: Span,
    export: &CheckedModelExport,
    checker: &TypeChecker,
    module_path: &[String],
) -> ApiModel {
    let docstring = model.docstring.clone();
    ApiModel {
        name: export.name.clone(),
        anchor: anchor(module_path, &export.name, span),
        docstring_sections: parse_docstring(docstring.as_deref()),
        docstring,
        decorators: decorators_metadata(&model.decorators, checker, None),
        type_params: type_params(&export.type_params),
        traits: export.traits.clone(),
        trait_adoptions: export.trait_adoptions.iter().map(type_bound).collect(),
        derives: export.derives.clone(),
        fields: fields_in_source_order(&model.fields, &export.fields),
        properties: properties(&export.properties),
        methods: methods(
            &model.methods,
            &model.method_aliases,
            &export.methods,
            checker,
            module_path,
            &export.name,
        ),
    }
}

/// Convert a source class declaration into API metadata.
pub(super) fn api_class(
    class: &ClassDecl,
    span: Span,
    export: &CheckedClassExport,
    checker: &TypeChecker,
    module_path: &[String],
) -> ApiClass {
    let docstring = class.docstring.clone();
    ApiClass {
        name: export.name.clone(),
        anchor: anchor(module_path, &export.name, span),
        docstring_sections: parse_docstring(docstring.as_deref()),
        docstring,
        decorators: decorators_metadata(&class.decorators, checker, None),
        type_params: type_params(&export.type_params),
        extends: export.extends.clone(),
        traits: export.traits.clone(),
        trait_adoptions: export.trait_adoptions.iter().map(type_bound).collect(),
        derives: export.derives.clone(),
        // Class export order is the provider's constructor ABI: inherited fields first, then the class's own fields.
        // Reordering the checked sequence to the child's AST declaration order would make manifest-backed consumers
        // pass positional bridge arguments in a different order than the generated Rust constructor.
        fields: export.fields.iter().map(field).collect(),
        properties: properties(&export.properties),
        methods: methods(
            &class.methods,
            &class.method_aliases,
            &export.methods,
            checker,
            module_path,
            &export.name,
        ),
    }
}

/// Convert a checked trait export into API metadata.
pub(super) fn api_trait(
    trait_decl: &TraitDecl,
    span: Span,
    export: &CheckedTraitExport,
    checker: &TypeChecker,
    module_path: &[String],
) -> ApiTrait {
    let docstring = trait_decl.docstring.clone();
    ApiTrait {
        name: export.name.clone(),
        anchor: anchor(module_path, &export.name, span),
        docstring_sections: parse_docstring(docstring.as_deref()),
        docstring,
        decorators: decorators_metadata(&trait_decl.decorators, checker, None),
        type_params: type_params(&export.type_params),
        supertraits: export.supertraits.iter().map(type_bound).collect(),
        requires: export
            .requires
            .iter()
            .map(|(name, ty)| FieldExport {
                name: name.clone(),
                canonical: None,
                record_identity: String::new(),
                record_canonical: true,
                renamed_from: Vec::new(),
                ty: type_ref_from_resolved(ty),
                surface_type_name: Some(ty.to_string()),
                visibility: crate::library_manifest::MemberVisibilityExport::Public,
                has_default: false,
                default: None,
                alias: None,
                description: None,
            })
            .collect(),
        methods: methods(
            &trait_decl.methods,
            &trait_decl.method_aliases,
            &export.methods,
            checker,
            module_path,
            &export.name,
        ),
    }
}

/// Convert a checked enum export into API metadata, preserving canonical variants and public aliases.
pub(super) fn api_enum(
    enum_decl: &EnumDecl,
    span: Span,
    export: &CheckedEnumExport,
    checker: &TypeChecker,
    module_path: &[String],
) -> ApiEnum {
    let docstring = enum_decl.docstring.clone();
    ApiEnum {
        name: export.name.clone(),
        anchor: anchor(module_path, &export.name, span),
        docstring_sections: parse_docstring(docstring.as_deref()),
        docstring,
        decorators: decorators_metadata(&enum_decl.decorators, checker, None),
        type_params: type_params(&export.type_params),
        traits: export.traits.clone(),
        trait_adoptions: export.trait_adoptions.iter().map(type_bound).collect(),
        value_type: export.value_type.map(|value_type| match value_type {
            crate::symbols::ValueEnumBacking::Str => EnumValueTypeExport::Str,
            crate::symbols::ValueEnumBacking::Int => EnumValueTypeExport::Int,
        }),
        variants: export
            .variants
            .iter()
            .map(|variant| ApiEnumVariant {
                name: variant.name.clone(),
                canonical: canonical_identity_export(variant.canonical.as_ref()),
                fields: variant.fields.iter().map(type_ref_from_resolved).collect(),
                value: variant.value.as_ref().map(|value| match value {
                    crate::symbols::ValueEnumValue::Str(value) => EnumValueExport::Str(value.clone()),
                    crate::symbols::ValueEnumValue::Int(value) => EnumValueExport::Int(*value),
                }),
            })
            .collect(),
        variant_aliases: export
            .variant_aliases
            .iter()
            .map(|alias| ApiEnumVariantAlias {
                name: alias.name.clone(),
                target: alias.target.clone(),
            })
            .collect(),
        methods: methods(
            &enum_decl.methods,
            &[],
            &export.methods,
            checker,
            module_path,
            &export.name,
        ),
        derives: export.derives.clone(),
    }
}

/// Convert a source newtype declaration into API metadata.
pub(super) fn api_newtype(
    newtype: &NewtypeDecl,
    span: Span,
    export: &CheckedNewtypeExport,
    checker: &TypeChecker,
    module_path: &[String],
) -> ApiNewtype {
    let docstring = newtype.docstring.clone();
    ApiNewtype {
        name: export.name.clone(),
        anchor: anchor(module_path, &export.name, span),
        docstring_sections: parse_docstring(docstring.as_deref()),
        docstring,
        decorators: decorators_metadata(&newtype.decorators, checker, None),
        type_params: type_params(&export.type_params),
        traits: export.traits.clone(),
        trait_adoptions: export.trait_adoptions.iter().map(type_bound).collect(),
        derives: export.derives.clone(),
        is_rusttype: export.is_rusttype,
        underlying: type_ref_from_resolved(&export.underlying),
        checked_constructor: export.checked_constructor.clone(),
        constraints: export
            .constraints
            .iter()
            .map(NewtypeConstraintExport::from_checked)
            .collect(),
        implicit_coercion_enabled: export.implicit_coercion_enabled,
        methods: methods(
            &newtype.methods,
            &newtype.method_aliases,
            &export.methods,
            checker,
            module_path,
            &export.name,
        ),
    }
}

/// Convert a source type alias declaration into API metadata.
pub(super) fn api_type_alias(
    alias: &TypeAliasDecl,
    span: Span,
    export: &CheckedTypeAliasExport,
    module_path: &[String],
) -> ApiTypeAlias {
    ApiTypeAlias {
        name: alias.name.clone(),
        anchor: anchor(module_path, &alias.name, span),
        type_alias: TypeAliasExport {
            name: export.name.clone(),
            type_params: type_params(&export.type_params),
            target: type_ref_from_resolved(&export.target),
        },
    }
}

/// Convert a checked constant declaration into API metadata.
pub(super) fn api_const(
    name: &str,
    span: Span,
    export: &CheckedConstExport,
    checker: &TypeChecker,
    module_path: &[String],
) -> ApiConst {
    ApiConst {
        name: export.name.clone(),
        anchor: anchor(module_path, name, span),
        ty: type_ref_from_resolved(&export.ty),
        value: checker.type_info().const_value(name).map(safe_value_from_const),
    }
}

/// Convert an import declaration into API alias metadata.
pub(super) fn api_aliases(import: &ImportDecl, span: Span, module_path: &[String]) -> Vec<ApiAlias> {
    let is_public = public(import.visibility);
    match &import.kind {
        ImportKind::From { module, items } => {
            let base_path =
                canonicalize_source_module_segments(&decorator_resolution::path_segments_with_prefix(module));
            aliases_from_items(items, base_path, span, module_path, is_public)
        }
        ImportKind::RustFrom {
            crate_name,
            path,
            items,
            ..
        } => {
            let mut base_path = vec!["rust".to_string(), crate_name.clone()];
            base_path.extend(path.iter().cloned());
            aliases_from_items(items, base_path, span, module_path, is_public)
        }
        ImportKind::PubFrom { library, path, items } => {
            let mut base_path = vec!["pub".to_string(), library.clone()];
            base_path.extend(path.iter().cloned());
            aliases_from_items(items, base_path, span, module_path, is_public)
        }
        _ => Vec::new(),
    }
}

/// Convert one checked declaration into the public aliases it contributes.
pub(super) fn checked_api_aliases_for_declaration(
    declaration: &Declaration,
    span: Span,
    module_path: &[String],
) -> Vec<ApiAlias> {
    match declaration {
        Declaration::Alias(alias) if public(alias.visibility) => vec![ApiAlias {
            name: alias.name.clone(),
            anchor: anchor(module_path, &alias.name, span),
            target_path: alias.target.segments.clone(),
            is_public: true,
            projected_type: None,
            projected_function: None,
        }],
        // Module-level `from ... import ...` bindings are part of a module's public surface, including facade modules
        // such as `std.web` whose `prelude.incn` consists entirely of reexports. Rust imports remain implementation
        // details unless explicitly declared public.
        Declaration::Import(import) if matches!(import.kind, ImportKind::From { .. }) || public(import.visibility) => {
            api_aliases(import, span, module_path)
        }
        _ => Vec::new(),
    }
}

/// Convert import items into API alias metadata rooted at a base path.
pub(super) fn aliases_from_items(
    items: &[ImportItem],
    base_path: Vec<String>,
    span: Span,
    module_path: &[String],
    is_public: bool,
) -> Vec<ApiAlias> {
    items
        .iter()
        .map(|item| {
            let name = item.alias.as_ref().unwrap_or(&item.name).clone();
            let mut target_path = base_path.clone();
            target_path.push(item.name.clone());
            ApiAlias {
                anchor: anchor(module_path, &name, span),
                name,
                target_path,
                is_public,
                projected_type: None,
                projected_function: None,
            }
        })
        .collect()
}

/// Project ownership into API metadata only when the current package can name its source field declarations.
pub(super) fn checked_api_ownership(
    contract: Option<&crate::ownership_metadata::CheckedOwnershipContract>,
    checker: &TypeChecker,
) -> Option<crate::library_manifest::ownership::OwnershipExport> {
    let contract = contract?;
    if checker.symbols.package_identity().is_none()
        && contract.result_source.as_ref().is_some_and(|source| {
            source
                .fields
                .iter()
                .any(|field| matches!(field.origin, SymbolOrigin::Module(_)))
        })
    {
        return Some(crate::library_manifest::ownership::OwnershipExport::Unsupported);
    }
    Some(crate::library_manifest::ownership::project(
        checker.symbols.package_identity().unwrap_or_default(),
        contract,
    ))
}

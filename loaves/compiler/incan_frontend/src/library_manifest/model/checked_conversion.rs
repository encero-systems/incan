//! Conversion of checked exports into manifest exports.

use super::*;
use crate::library_exports::CheckedParamDefaultArg;

/// Rewrite source-owned type references to the selected public export names.
pub(super) fn rewrite_type_ref_names(
    ty: &mut TypeRef,
    owner_module_path: &[String],
    public_names: &std::collections::HashMap<Vec<String>, String>,
    source_paths_by_leaf: &std::collections::HashMap<String, Vec<Vec<String>>>,
) {
    let public_name_for = |name: &str| {
        let mut local_source_path = owner_module_path.to_vec();
        local_source_path.push(name.to_string());
        public_names.get(&local_source_path).or_else(|| {
            let candidates = source_paths_by_leaf.get(name)?;
            (candidates.len() == 1)
                .then(|| public_names.get(&candidates[0]))
                .flatten()
        })
    };
    match ty {
        TypeRef::Named { name, .. } => {
            if let Some(public_name) = public_name_for(name) {
                *name = public_name.clone();
            }
        }
        TypeRef::Applied { name, args, .. } => {
            if let Some(public_name) = public_name_for(name) {
                *name = public_name.clone();
            }
            for arg in args {
                rewrite_type_ref_names(arg, owner_module_path, public_names, source_paths_by_leaf);
            }
        }
        TypeRef::Function { params, return_type } => {
            for param in params {
                rewrite_type_ref_names(param, owner_module_path, public_names, source_paths_by_leaf);
            }
            rewrite_type_ref_names(return_type, owner_module_path, public_names, source_paths_by_leaf);
        }
        TypeRef::Ownership { inner, .. }
        | TypeRef::TypeToken { inner }
        | TypeRef::Ref { inner }
        | TypeRef::MutParam { inner } => {
            rewrite_type_ref_names(inner, owner_module_path, public_names, source_paths_by_leaf);
        }
        TypeRef::Tuple { elements } => {
            for element in elements {
                rewrite_type_ref_names(element, owner_module_path, public_names, source_paths_by_leaf);
            }
        }
        TypeRef::NativeUnion(native) => {
            for member in &mut native.members {
                rewrite_type_ref_names(member, owner_module_path, public_names, source_paths_by_leaf);
            }
        }
        TypeRef::Trait { .. }
        | TypeRef::TypeParam { .. }
        | TypeRef::SelfType
        | TypeRef::RustPath { .. }
        | TypeRef::Unknown => {}
    }
}

/// Build one manifest identity-graph entry from checked export metadata.
pub(super) fn export_identity_from_checked(package_name: &str, export: &CheckedNamedExport) -> ExportIdentity {
    ExportIdentity {
        public_name: export.name.clone(),
        public_path: vec![package_name.to_string(), export.name.clone()],
        source_path: export.identity.source_path.clone(),
        kind: export_identity_kind_from_checked(&export.kind),
        projection: export_identity_projection_from_checked(&export.identity),
        canonical: export
            .identity
            .canonical
            .as_ref()
            .and_then(|identity| CanonicalIdentityExport::from_canonical(package_name, identity)),
    }
}

/// Classify a checked public export for the stable manifest identity graph.
pub(super) fn export_identity_kind_from_checked(kind: &CheckedExportKind) -> ExportIdentityKind {
    match kind {
        CheckedExportKind::Function(_) => ExportIdentityKind::Function,
        CheckedExportKind::Partial(_) => ExportIdentityKind::Partial,
        CheckedExportKind::Alias(_) => ExportIdentityKind::Alias,
        CheckedExportKind::TypeAlias(_) => ExportIdentityKind::TypeAlias,
        CheckedExportKind::Model(_) => ExportIdentityKind::Model,
        CheckedExportKind::Class(_) => ExportIdentityKind::Class,
        CheckedExportKind::Trait(_) => ExportIdentityKind::Trait,
        CheckedExportKind::Enum(_) => ExportIdentityKind::Enum,
        CheckedExportKind::Newtype(_) => ExportIdentityKind::Newtype,
        CheckedExportKind::Const(_) => ExportIdentityKind::Const,
        CheckedExportKind::Static(_) => ExportIdentityKind::Static,
    }
}

/// Convert checked identity projection metadata into the serializable manifest representation.
pub(super) fn export_identity_projection_from_checked(identity: &CheckedExportIdentity) -> ExportIdentityProjection {
    match &identity.projection {
        CheckedExportProjection::Direct => ExportIdentityProjection::Direct,
        CheckedExportProjection::Alias { target_path } => ExportIdentityProjection::Alias {
            target_path: target_path.clone(),
        },
        CheckedExportProjection::Reexport { target_path } => ExportIdentityProjection::Reexport {
            target_path: target_path.clone(),
        },
        CheckedExportProjection::Partial {
            target_path,
            target_kind,
        } => ExportIdentityProjection::Partial {
            target_path: target_path.clone(),
            target_kind: partial_target_kind_from_checked(*target_kind),
        },
    }
}

/// Convert checked alias metadata into manifest alias metadata.
pub(super) fn alias_export_from_checked(package_name: &str, export: &CheckedAliasExport) -> AliasExport {
    AliasExport {
        name: export.name.clone(),
        target_path: export.target_path.clone(),
        projected_type: export.projected_type.as_ref().map(type_ref_from_resolved),
        projected_function: export
            .projected_function
            .as_ref()
            .map(|function| function_export_from_checked(package_name, function)),
    }
}

/// Convert checked partial metadata into manifest partial export metadata.
pub(super) fn partial_export_from_checked(export: &CheckedPartialExport) -> PartialExport {
    PartialExport {
        name: export.name.clone(),
        target_path: export.target_path.clone(),
        target_kind: partial_target_kind_from_checked(export.target_kind),
        presets: export
            .presets
            .iter()
            .map(|preset| PartialPresetExport {
                name: preset.name.clone(),
                ty: type_ref_from_resolved(&preset.ty),
                value: preset_value_from_checked(&preset.value),
            })
            .collect(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        params: params_from_checked(&export.params, &[]),
        return_type: type_ref_from_resolved(&export.return_type),
        is_async: export.is_async,
    }
}

/// Convert checked partial target kinds into the manifest vocabulary.
pub(super) fn partial_target_kind_from_checked(kind: CheckedPartialTargetKind) -> PartialTargetKindExport {
    match kind {
        CheckedPartialTargetKind::Function => PartialTargetKindExport::Function,
        CheckedPartialTargetKind::ModelConstructor => PartialTargetKindExport::ModelConstructor,
        CheckedPartialTargetKind::ClassConstructor => PartialTargetKindExport::ClassConstructor,
        CheckedPartialTargetKind::NewtypeConstructor => PartialTargetKindExport::NewtypeConstructor,
        CheckedPartialTargetKind::Partial => PartialTargetKindExport::Partial,
        CheckedPartialTargetKind::Unknown => PartialTargetKindExport::Unknown,
    }
}

/// Convert checked preset values into the manifest value vocabulary.
pub(super) fn preset_value_from_checked(value: &CheckedPresetValue) -> PresetValueExport {
    match value {
        CheckedPresetValue::Int(value) => PresetValueExport::Int(*value),
        CheckedPresetValue::Float(value) => PresetValueExport::Float(value.to_string()),
        CheckedPresetValue::Bool(value) => PresetValueExport::Bool(*value),
        CheckedPresetValue::String(value) => PresetValueExport::String(value.clone()),
        CheckedPresetValue::Bytes(value) => PresetValueExport::Bytes(value.clone()),
        CheckedPresetValue::None => PresetValueExport::None,
        CheckedPresetValue::List(values) => {
            PresetValueExport::List(values.iter().map(preset_value_from_checked).collect())
        }
        CheckedPresetValue::Set(values) => {
            PresetValueExport::Set(values.iter().map(preset_value_from_checked).collect())
        }
        CheckedPresetValue::Tuple(values) => {
            PresetValueExport::Tuple(values.iter().map(preset_value_from_checked).collect())
        }
        CheckedPresetValue::Dict(entries) => PresetValueExport::Dict(
            entries
                .iter()
                .map(|(key, value)| PresetDictEntryExport {
                    key: preset_value_from_checked(key),
                    value: preset_value_from_checked(value),
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
                    value: preset_value_from_checked(value),
                })
                .collect(),
        },
        CheckedPresetValue::Unsupported => PresetValueExport::Unsupported,
    }
}

/// Convert checked parameter defaults into the manifest default-expression vocabulary when consumers can materialize
/// them.
pub fn param_default_from_checked(value: &CheckedParamDefault) -> Option<ParamDefaultExport> {
    match value {
        CheckedParamDefault::Int(value) => Some(ParamDefaultExport::Int(*value)),
        CheckedParamDefault::Float(value) => Some(ParamDefaultExport::Float(value.to_string())),
        CheckedParamDefault::Bool(value) => Some(ParamDefaultExport::Bool(*value)),
        CheckedParamDefault::String(value) => Some(ParamDefaultExport::String(value.clone())),
        CheckedParamDefault::Bytes(value) => Some(ParamDefaultExport::Bytes(value.clone())),
        CheckedParamDefault::None => Some(ParamDefaultExport::None),
        CheckedParamDefault::List(values) => values
            .iter()
            .map(param_default_from_checked)
            .collect::<Option<Vec<_>>>()
            .map(ParamDefaultExport::List),
        CheckedParamDefault::Dict(entries) => entries
            .iter()
            .map(|(key, value)| {
                Some(ParamDefaultDictEntryExport {
                    key: param_default_from_checked(key)?,
                    value: param_default_from_checked(value)?,
                })
            })
            .collect::<Option<Vec<_>>>()
            .map(ParamDefaultExport::Dict),
        CheckedParamDefault::ConstRef(path) => Some(ParamDefaultExport::ConstRef(path.clone())),
        CheckedParamDefault::Call { path, args, signature } => args
            .iter()
            .map(|arg| {
                Some(ParamDefaultCallArgExport {
                    name: arg.name.clone(),
                    value: param_default_from_checked(&arg.value)?,
                })
            })
            .collect::<Option<Vec<_>>>()
            .map(|args| ParamDefaultExport::Call {
                path: path.clone(),
                args,
                signature: signature
                    .as_ref()
                    .map(param_default_call_signature_from_checked)
                    .map(Box::new),
            }),
        CheckedParamDefault::MethodCall {
            receiver_path,
            method,
            args,
        } => method_call_default_from_checked(receiver_path, method, args),
        CheckedParamDefault::Unsupported => None,
    }
}

/// Convert a checked static method call into replayable manifest metadata.
fn method_call_default_from_checked(
    receiver_path: &[String],
    method: &str,
    args: &[CheckedParamDefaultArg],
) -> Option<ParamDefaultExport> {
    args.iter()
        .map(|arg| {
            Some(ParamDefaultCallArgExport {
                name: arg.name.clone(),
                value: param_default_from_checked(&arg.value)?,
            })
        })
        .collect::<Option<Vec<_>>>()
        .map(|args| ParamDefaultExport::MethodCall {
            receiver_path: receiver_path.to_vec(),
            method: method.to_owned(),
            args,
        })
}

/// Convert a checked default-helper callable surface into manifest metadata.
pub(super) fn param_default_call_signature_from_checked(
    signature: &CheckedParamDefaultCallSignature,
) -> ParamDefaultCallSignatureExport {
    ParamDefaultCallSignatureExport {
        params: params_from_checked(&signature.params, &[]),
        return_type: type_ref_from_resolved(&signature.return_type),
    }
}

pub(super) fn type_param_from_checked(type_param: &CheckedTypeParam) -> TypeParamExport {
    TypeParamExport {
        name: type_param.name.clone(),
        bounds: type_param.bounds.iter().map(type_bound_from_checked).collect(),
    }
}

/// Convert checked trait-bound metadata into the serialized manifest shape.
pub(super) fn type_bound_from_checked(bound: &CheckedTypeBound) -> TypeBoundExport {
    TypeBoundExport {
        name: bound.name.clone(),
        source_name: bound.source_name.clone(),
        module_path: bound.module_path.clone(),
        type_args: bound.type_args.iter().map(type_ref_from_resolved).collect(),
        implementation_type_params: bound
            .implementation_type_params
            .iter()
            .map(implementation_type_param_from_info)
            .collect(),
        inferred: bound.inferred,
    }
}

/// Convert one checked implementation parameter into the shared manifest model.
pub(super) fn implementation_type_param_from_info(
    type_param: &ImplementationTypeParamInfo,
) -> ImplementationTypeParamExport {
    ImplementationTypeParamExport {
        name: type_param.name.clone(),
        bounds: type_param
            .bounds
            .iter()
            .map(implementation_trait_bound_from_info)
            .collect(),
    }
}

/// Convert one checked implementation requirement into the shared manifest model.
pub(super) fn implementation_trait_bound_from_info(
    bound: &ImplementationTraitBoundInfo,
) -> ImplementationTraitBoundExport {
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

/// Convert checked callable parameters into library-manifest parameter records.
pub fn params_from_checked(params: &[CallableParam], defaults: &[Option<CheckedParamDefault>]) -> Vec<ParamExport> {
    params
        .iter()
        .enumerate()
        .filter_map(|param| {
            let (idx, param) = param;
            let default = defaults
                .get(idx)
                .and_then(|default| default.as_ref())
                .and_then(param_default_from_checked);
            let has_default = if defaults.is_empty() {
                param.has_default
            } else {
                default.is_some()
            };
            Some(ParamExport {
                name: param.name.clone()?,
                ty: type_ref_from_resolved(&param.ty),
                surface_type_name: param.surface_type_name.clone(),
                kind: param_kind_from_ast(param.kind),
                has_default,
                is_mut: param.is_mut,
                default,
            })
        })
        .collect()
}

/// Convert an AST parameter kind into a library-manifest parameter kind.
pub(super) fn param_kind_from_ast(kind: crate::ast::ParamKind) -> ParamKindExport {
    match kind {
        crate::ast::ParamKind::Normal => ParamKindExport::Normal,
        crate::ast::ParamKind::RestPositional => ParamKindExport::RestPositional,
        crate::ast::ParamKind::RestKeyword => ParamKindExport::RestKeyword,
    }
}

/// Map a checked method receiver onto its manifest export form.
pub(super) fn receiver_from_checked(receiver: Option<crate::ast::Receiver>) -> Option<ReceiverExport> {
    receiver.map(|value| match value {
        crate::ast::Receiver::Immutable => ReceiverExport::Immutable,
        crate::ast::Receiver::Mutable => ReceiverExport::Mutable,
    })
}

/// Convert checked method metadata into manifest method metadata.
pub(super) fn method_from_checked(package_name: &str, method: &crate::library_exports::CheckedMethod) -> MethodExport {
    let mut projected = MethodExport {
        ownership: method
            .ownership
            .as_ref()
            .map(|contract| super::ownership::project(package_name, contract)),
        name: method.name.clone(),
        canonical: method
            .canonical
            .as_ref()
            .and_then(|identity| CanonicalIdentityExport::from_canonical(package_name, identity)),
        visibility: method.visibility.into(),
        alias_of: method.alias_of.clone(),
        type_params: method.type_params.iter().map(type_param_from_checked).collect(),
        receiver: receiver_from_checked(method.receiver),
        params: params_from_checked(&method.params, &method.param_defaults),
        return_type: type_ref_from_resolved(&method.return_type),
        is_async: method.is_async,
        has_body: method.has_body,
        made_private: method.made_private.as_ref().map(Into::into),
    };
    super::ownership::mark_types(
        projected.ownership.as_ref(),
        &mut projected.params,
        &mut projected.return_type,
    );
    projected
}

/// Convert checked field metadata into artifact metadata, including a materializable default when available.
pub(super) fn field_from_checked(package_name: &str, field: &crate::library_exports::CheckedField) -> FieldExport {
    let default = field.default.as_ref().and_then(param_default_from_checked);
    FieldExport {
        name: field.name.clone(),
        canonical: field
            .canonical
            .as_ref()
            .and_then(|identity| CanonicalIdentityExport::from_canonical(package_name, identity)),
        record_identity: hex::encode(&field.record_identity),
        record_canonical: field.record_canonical,
        renamed_from: field.renamed_from.clone(),
        renamed_from_lifecycle: field.renamed_from_lifecycle.clone(),
        default_is_empty: field.default_is_empty,
        ty: type_ref_from_resolved(&field.ty),
        surface_type_name: field.surface_type_name.clone(),
        visibility: match field.visibility {
            crate::ast::Visibility::Private => MemberVisibilityExport::Private,
            crate::ast::Visibility::Public => MemberVisibilityExport::Public,
        },
        has_default: field.has_default,
        default,
        alias: field.alias.clone(),
        description: field.description.clone(),
        made_private: field.made_private.as_ref().map(Into::into),
    }
}

/// Convert checked computed-property metadata into artifact metadata.
pub(super) fn property_from_checked(package_name: &str, property: &CheckedProperty) -> PropertyExport {
    PropertyExport {
        name: property.name.clone(),
        canonical: property
            .canonical
            .as_ref()
            .and_then(|identity| CanonicalIdentityExport::from_canonical(package_name, identity)),
        return_type: type_ref_from_resolved(&property.return_type),
    }
}

/// Convert a checked source function export into manifest metadata, including the materializable default subset.
pub(in crate::library_manifest) fn function_export_from_checked(
    package_name: &str,
    export: &CheckedFunctionExport,
) -> FunctionExport {
    let mut projected = FunctionExport {
        ownership: export
            .ownership
            .as_ref()
            .map(|contract| super::ownership::project(package_name, contract)),
        name: export.name.clone(),
        emitted_name: export.emitted_name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        params: params_from_checked(&export.params, &export.param_defaults),
        return_type: type_ref_from_resolved(&export.return_type),
        is_async: export.is_async,
    };
    super::ownership::mark_types(
        projected.ownership.as_ref(),
        &mut projected.params,
        &mut projected.return_type,
    );
    projected
}

pub(super) fn type_alias_export_from_checked(export: &CheckedTypeAliasExport) -> TypeAliasExport {
    TypeAliasExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        target: type_ref_from_resolved(&export.target),
    }
}

/// Convert a checked model export into the serialized manifest model shape.
pub(super) fn model_export_from_checked(package_name: &str, export: &CheckedModelExport) -> ModelExport {
    ModelExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        traits: export.traits.clone(),
        trait_adoptions: export.trait_adoptions.iter().map(type_bound_from_checked).collect(),
        derives: export.derives.clone(),
        fields: export
            .fields
            .iter()
            .map(|field| field_from_checked(package_name, field))
            .collect(),
        properties: export
            .properties
            .iter()
            .map(|property| property_from_checked(package_name, property))
            .collect(),
        methods: export
            .methods
            .iter()
            .map(|method| method_from_checked(package_name, method))
            .collect(),
    }
}

/// Convert a checked class export into the serialized manifest class shape.
pub(super) fn class_export_from_checked(package_name: &str, export: &CheckedClassExport) -> ClassExport {
    ClassExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        extends: export.extends.clone(),
        traits: export.traits.clone(),
        trait_adoptions: export.trait_adoptions.iter().map(type_bound_from_checked).collect(),
        derives: export.derives.clone(),
        fields: export
            .fields
            .iter()
            .map(|field| field_from_checked(package_name, field))
            .collect(),
        properties: export
            .properties
            .iter()
            .map(|property| property_from_checked(package_name, property))
            .collect(),
        methods: export
            .methods
            .iter()
            .map(|method| method_from_checked(package_name, method))
            .collect(),
    }
}

/// Convert a checked trait export into the serialized manifest trait shape.
pub(super) fn trait_export_from_checked(package_name: &str, export: &CheckedTraitExport) -> TraitExport {
    TraitExport {
        name: export.name.clone(),
        source_name: (export.source_name != export.name).then(|| export.source_name.clone()),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        supertraits: export.supertraits.iter().map(type_bound_from_checked).collect(),
        requires: export
            .requires
            .iter()
            .map(|(name, ty)| FieldRequirementExport {
                name: name.clone(),
                ty: type_ref_from_resolved(ty),
            })
            .collect(),
        methods: export
            .methods
            .iter()
            .map(|method| method_from_checked(package_name, method))
            .collect(),
    }
}

/// Convert a checked enum export into the manifest enum contract.
pub(super) fn enum_export_from_checked(package_name: &str, export: &CheckedEnumExport) -> EnumExport {
    EnumExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        traits: export.traits.clone(),
        trait_adoptions: export.trait_adoptions.iter().map(type_bound_from_checked).collect(),
        value_type: export.value_type.map(value_enum_type_from_checked),
        ordinal_type_identity: None,
        variants: export
            .variants
            .iter()
            .map(|variant| EnumVariantExport {
                name: variant.name.clone(),
                canonical: variant
                    .canonical
                    .as_ref()
                    .and_then(|identity| CanonicalIdentityExport::from_canonical(package_name, identity)),
                fields: variant.fields.iter().map(type_ref_from_resolved).collect(),
                value: variant.value.as_ref().map(value_enum_value_from_checked),
            })
            .collect(),
        variant_aliases: export
            .variant_aliases
            .iter()
            .map(|alias| EnumVariantAliasExport {
                name: alias.name.clone(),
                target: alias.target.clone(),
            })
            .collect(),
        methods: export
            .methods
            .iter()
            .map(|method| method_from_checked(package_name, method))
            .collect(),
        derives: export.derives.clone(),
    }
}

/// Convert checked value-enum backing metadata into the manifest representation.
pub(super) fn value_enum_type_from_checked(value_type: ValueEnumBacking) -> EnumValueTypeExport {
    match value_type {
        ValueEnumBacking::Str => EnumValueTypeExport::Str,
        ValueEnumBacking::Int => EnumValueTypeExport::Int,
    }
}

/// Convert one checked value-enum raw value into the manifest representation.
pub(super) fn value_enum_value_from_checked(value: &ValueEnumValue) -> EnumValueExport {
    match value {
        ValueEnumValue::Str(value) => EnumValueExport::Str(value.clone()),
        ValueEnumValue::Int(value) => EnumValueExport::Int(*value),
    }
}

/// Convert a checked newtype export into the serialized manifest shape.
pub(super) fn newtype_export_from_checked(package_name: &str, export: &CheckedNewtypeExport) -> NewtypeExport {
    NewtypeExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        traits: export.traits.clone(),
        trait_adoptions: export.trait_adoptions.iter().map(type_bound_from_checked).collect(),
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
        methods: export
            .methods
            .iter()
            .map(|method| method_from_checked(package_name, method))
            .collect(),
    }
}

pub(super) fn const_export_from_checked(export: &CheckedConstExport) -> ConstExport {
    ConstExport {
        name: export.name.clone(),
        ty: type_ref_from_resolved(&export.ty),
    }
}

pub(super) fn static_export_from_checked(export: &CheckedStaticExport) -> StaticExport {
    StaticExport {
        name: export.name.clone(),
        ty: type_ref_from_resolved(&export.ty),
    }
}

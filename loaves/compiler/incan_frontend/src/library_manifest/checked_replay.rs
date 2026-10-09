//! Reconstruct checked export consumers from validated ordinary package contracts, without source or symbols.
//!
//! Each replay record preserves one explicit identity-to-shape binding. An overload set cannot be reconstructed by
//! zipping sorted manifest shapes with sorted canonical identities: those are independent orders.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::*;
use crate::library_exports::*;
use crate::symbols::{
    CallableParam, ImplementationTraitBoundInfo, ImplementationTraitBoundOriginInfo, ImplementationTypeParamInfo,
};

/// Versioned checked export replay, binding each exact public shape to its original checked identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckedExportReplay {
    schema_version: u32,
    #[serde(with = "manifest_contract")]
    projection: LibraryManifest,
}

impl CheckedExportReplay {
    /// Project one already checked export through the canonical manifest encoder, retaining overload association.
    pub fn from_checked(
        package: &str,
        version: &str,
        export: &CheckedNamedExport,
    ) -> Result<Self, LibraryManifestError> {
        validate_replayable_shape(&export.kind)?;
        let projection = LibraryManifest::from_checked_exports(package, version, std::slice::from_ref(export));
        projection.to_json_string()?;
        let replay = Self {
            schema_version: 1,
            projection,
        };
        replay.to_checked()?;
        Ok(replay)
    }

    /// Recover one complete export only after validating its version, manifest and unique identity-to-shape binding.
    pub fn to_checked(&self) -> Result<CheckedNamedExport, LibraryManifestError> {
        if self.schema_version != 1 {
            return Err(invalid("unsupported checked export replay version"));
        }
        self.projection.to_json_string()?;
        let manifest = &self.projection;
        let roots: Vec<_> = manifest
            .contract_metadata
            .identity_graph
            .exports
            .iter()
            .filter(|identity| identity.public_path.len() == 2)
            .collect();
        let [identity] = roots.as_slice() else {
            return Err(invalid("checked export replay requires one exact root identity"));
        };
        let canonical = identity
            .canonical
            .as_ref()
            .and_then(CanonicalIdentityExport::hydrate)
            .ok_or_else(|| invalid("checked export replay has no canonical authority"))?;
        let mut kinds = Vec::new();
        let exports = &manifest.exports;
        for value in &exports.functions {
            kinds.push((
                ExportIdentityKind::Function,
                CheckedExportKind::Function(function(value)?),
            ));
        }
        for value in &exports.aliases {
            kinds.push((
                ExportIdentityKind::Alias,
                CheckedExportKind::Alias(CheckedAliasExport {
                    name: value.name.clone(),
                    target_path: value.target_path.clone(),
                    projected_function: value.projected_function.as_ref().map(function).transpose()?,
                    projected_type: value.projected_type.as_ref().map(resolved_type_from_manifest_type_ref),
                }),
            ));
        }
        for value in &exports.type_aliases {
            kinds.push((
                ExportIdentityKind::TypeAlias,
                CheckedExportKind::TypeAlias(CheckedTypeAliasExport {
                    name: value.name.clone(),
                    type_params: type_params(&value.type_params),
                    target: resolved_type_from_manifest_type_ref(&value.target),
                }),
            ));
        }
        for value in &exports.models {
            kinds.push((
                ExportIdentityKind::Model,
                CheckedExportKind::Model(CheckedModelExport {
                    name: value.name.clone(),
                    type_params: type_params(&value.type_params),
                    traits: value.traits.clone(),
                    trait_adoptions: bounds(&value.trait_adoptions),
                    derives: value.derives.clone(),
                    fields: fields(&value.fields)?,
                    properties: properties(&value.properties),
                    methods: methods(&value.methods)?,
                }),
            ));
        }
        for value in &exports.classes {
            kinds.push((
                ExportIdentityKind::Class,
                CheckedExportKind::Class(CheckedClassExport {
                    name: value.name.clone(),
                    type_params: type_params(&value.type_params),
                    extends: value.extends.clone(),
                    traits: value.traits.clone(),
                    trait_adoptions: bounds(&value.trait_adoptions),
                    derives: value.derives.clone(),
                    fields: fields(&value.fields)?,
                    properties: properties(&value.properties),
                    methods: methods(&value.methods)?,
                }),
            ));
        }
        for value in &exports.traits {
            kinds.push((
                ExportIdentityKind::Trait,
                CheckedExportKind::Trait(CheckedTraitExport {
                    name: value.name.clone(),
                    source_name: value.source_name.clone().unwrap_or_else(|| value.name.clone()),
                    type_params: type_params(&value.type_params),
                    supertraits: bounds(&value.supertraits),
                    requires: value
                        .requires
                        .iter()
                        .map(|field| (field.name.clone(), resolved_type_from_manifest_type_ref(&field.ty)))
                        .collect(),
                    methods: methods(&value.methods)?,
                }),
            ));
        }
        for value in &exports.enums {
            kinds.push((
                ExportIdentityKind::Enum,
                CheckedExportKind::Enum(CheckedEnumExport {
                    name: value.name.clone(),
                    type_params: type_params(&value.type_params),
                    traits: value.traits.clone(),
                    trait_adoptions: bounds(&value.trait_adoptions),
                    value_type: value.value_type.map(|backing| match backing {
                        EnumValueTypeExport::Str => crate::symbols::ValueEnumBacking::Str,
                        EnumValueTypeExport::Int => crate::symbols::ValueEnumBacking::Int,
                    }),
                    variants: value
                        .variants
                        .iter()
                        .map(|variant| CheckedEnumVariant {
                            name: variant.name.clone(),
                            canonical: variant.canonical.as_ref().and_then(CanonicalIdentityExport::hydrate),
                            fields: variant
                                .fields
                                .iter()
                                .map(resolved_type_from_manifest_type_ref)
                                .collect(),
                            value: variant.value.as_ref().map(|value| match value {
                                EnumValueExport::Str(value) => crate::symbols::ValueEnumValue::Str(value.clone()),
                                EnumValueExport::Int(value) => crate::symbols::ValueEnumValue::Int(*value),
                            }),
                        })
                        .collect(),
                    variant_aliases: value
                        .variant_aliases
                        .iter()
                        .map(|alias| CheckedEnumVariantAlias {
                            name: alias.name.clone(),
                            target: alias.target.clone(),
                        })
                        .collect(),
                    methods: methods(&value.methods)?,
                    derives: value.derives.clone(),
                }),
            ));
        }
        for value in &exports.newtypes {
            kinds.push((
                ExportIdentityKind::Newtype,
                CheckedExportKind::Newtype(CheckedNewtypeExport {
                    name: value.name.clone(),
                    type_params: type_params(&value.type_params),
                    traits: value.traits.clone(),
                    trait_adoptions: bounds(&value.trait_adoptions),
                    derives: value.derives.clone(),
                    is_rusttype: value.is_rusttype,
                    underlying: resolved_type_from_manifest_type_ref(&value.underlying),
                    checked_constructor: value.checked_constructor.clone(),
                    constraints: value
                        .constraints
                        .iter()
                        .map(NewtypeConstraintExport::to_checked)
                        .collect(),
                    implicit_coercion_enabled: value.implicit_coercion_enabled,
                    methods: methods(&value.methods)?,
                }),
            ));
        }
        for value in &exports.consts {
            kinds.push((
                ExportIdentityKind::Const,
                CheckedExportKind::Const(CheckedConstExport {
                    name: value.name.clone(),
                    ty: resolved_type_from_manifest_type_ref(&value.ty),
                }),
            ));
        }
        for value in &exports.statics {
            kinds.push((
                ExportIdentityKind::Static,
                CheckedExportKind::Static(CheckedStaticExport {
                    name: value.name.clone(),
                    ty: resolved_type_from_manifest_type_ref(&value.ty),
                }),
            ));
        }
        for value in &exports.partials {
            kinds.push((
                ExportIdentityKind::Partial,
                CheckedExportKind::Partial(CheckedPartialExport {
                    name: value.name.clone(),
                    target_path: value.target_path.clone(),
                    target_kind: partial_kind(value.target_kind),
                    presets: value
                        .presets
                        .iter()
                        .map(|preset| {
                            Ok(CheckedPartialPreset {
                                name: preset.name.clone(),
                                ty: resolved_type_from_manifest_type_ref(&preset.ty),
                                value: preset_value(&preset.value)?,
                            })
                        })
                        .collect::<Result<_, LibraryManifestError>>()?,
                    type_params: type_params(&value.type_params),
                    params: partial_params(value),
                    return_type: resolved_type_from_manifest_type_ref(&value.return_type),
                    is_async: value.is_async,
                }),
            ));
        }
        let [(kind, shape)] = kinds.as_slice() else {
            return Err(invalid("checked export replay requires one complete shape"));
        };
        if *kind != identity.kind {
            return Err(invalid("checked export replay shape and identity kinds disagree"));
        }
        let mut origins = BTreeMap::new();
        let mut projected = manifest.exports.clone();
        let mut competing = false;
        projected.visit_type_refs(&mut |ty| {
            if let TypeRef::Named {
                origin: Some(origin), ..
            }
            | TypeRef::Applied {
                origin: Some(origin), ..
            } = ty
            {
                let key = origin.binding_key();
                if origins
                    .insert(key, origin.clone())
                    .is_some_and(|previous| previous != *origin)
                {
                    competing = true;
                }
            }
        });
        if competing {
            return Err(invalid("checked export replay has competing nominal owners"));
        }
        let projection = match &identity.projection {
            ExportIdentityProjection::Direct => CheckedExportProjection::Direct,
            ExportIdentityProjection::Alias { target_path } => CheckedExportProjection::Alias {
                target_path: target_path.clone(),
            },
            ExportIdentityProjection::Reexport { target_path } => CheckedExportProjection::Reexport {
                target_path: target_path.clone(),
            },
            ExportIdentityProjection::Partial {
                target_path,
                target_kind,
            } => CheckedExportProjection::Partial {
                target_path: target_path.clone(),
                target_kind: partial_kind(*target_kind),
            },
        };
        Ok(CheckedNamedExport {
            name: identity.public_name.clone(),
            identity: CheckedExportIdentity {
                source_path: identity.source_path.clone(),
                projection,
                canonical: Some(canonical),
                type_origins: origins,
            },
            kind: shape.clone(),
        })
    }
}

/// Refuse projections that would silently discard a checked callable fact. Source preparation remains available
/// for these shapes until the ordinary contract can encode them without loss.
fn validate_replayable_shape(kind: &CheckedExportKind) -> Result<(), LibraryManifestError> {
    let validate = |parameters: &[CallableParam], defaults: &[Option<CheckedParamDefault>]| {
        let projected = params_from_checked(parameters, defaults);
        if projected.len() != parameters.len()
            || projected
                .iter()
                .zip(parameters)
                .any(|(published, checked)| published.has_default != checked.has_default || checked.is_partial_preset)
        {
            return Err(invalid(
                "checked callable facts are not fully represented by the published contract",
            ));
        }
        Ok(())
    };
    match kind {
        CheckedExportKind::Function(value) => validate(&value.params, &value.param_defaults)?,
        CheckedExportKind::Alias(value) => {
            if let Some(function) = &value.projected_function {
                validate(&function.params, &function.param_defaults)?;
            }
        }
        CheckedExportKind::Partial(value) => {
            for parameter in &value.params {
                let name = parameter
                    .name
                    .as_ref()
                    .ok_or_else(|| invalid("unnamed checked partial parameter"))?;
                if parameter.is_partial_preset != value.presets.iter().any(|preset| preset.name == *name) {
                    return Err(invalid(
                        "checked partial preset facts disagree with the published contract",
                    ));
                }
            }
        }
        _ => {}
    }
    let methods = match kind {
        CheckedExportKind::Model(value) => &value.methods,
        CheckedExportKind::Class(value) => &value.methods,
        CheckedExportKind::Trait(value) => &value.methods,
        CheckedExportKind::Enum(value) => &value.methods,
        CheckedExportKind::Newtype(value) => &value.methods,
        _ => return Ok(()),
    };
    for method in methods {
        validate(&method.params, &method.param_defaults)?;
    }
    Ok(())
}

/// Recover partial preset flags from the explicit checked preset-to-parameter binding.
fn partial_params(value: &PartialExport) -> Vec<CallableParam> {
    let mut parameters = params(&value.params);
    for parameter in &mut parameters {
        parameter.is_partial_preset = parameter
            .name
            .as_ref()
            .is_some_and(|name| value.presets.iter().any(|preset| preset.name == *name));
    }
    parameters
}

/// Reconstruct exact type parameters and compiler-inferred/declared generic obligations.
fn type_params(values: &[TypeParamExport]) -> Vec<CheckedTypeParam> {
    values
        .iter()
        .map(|value| CheckedTypeParam {
            name: value.name.clone(),
            bounds: bounds(&value.bounds),
        })
        .collect()
}

/// Preserve bound origin, associated-type equalities and implementation-only generic headers.
fn bounds(values: &[TypeBoundExport]) -> Vec<CheckedTypeBound> {
    values
        .iter()
        .map(|value| CheckedTypeBound {
            name: value.name.clone(),
            source_name: value.source_name.clone(),
            type_args: value
                .type_args
                .iter()
                .map(resolved_type_from_manifest_type_ref)
                .collect(),
            module_path: value.module_path.clone(),
            inferred: value.inferred,
            implementation_type_params: value
                .implementation_type_params
                .iter()
                .map(|parameter| ImplementationTypeParamInfo {
                    name: parameter.name.clone(),
                    bounds: parameter
                        .bounds
                        .iter()
                        .map(|bound| ImplementationTraitBoundInfo {
                            trait_path: bound.trait_path.clone(),
                            type_args: bound
                                .type_args
                                .iter()
                                .map(resolved_type_from_manifest_type_ref)
                                .collect(),
                            associated_types: bound
                                .associated_types
                                .iter()
                                .map(|value| (value.name.clone(), resolved_type_from_manifest_type_ref(&value.ty)))
                                .collect(),
                            origin: match bound.origin {
                                ImplementationTraitBoundOriginExport::Standard => {
                                    ImplementationTraitBoundOriginInfo::Standard
                                }
                                ImplementationTraitBoundOriginExport::RustCapability => {
                                    ImplementationTraitBoundOriginInfo::RustCapability
                                }
                                ImplementationTraitBoundOriginExport::SourceCallable => {
                                    ImplementationTraitBoundOriginInfo::SourceCallable
                                }
                            },
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect()
}

/// Preserve parameter names, rest shape, mutability and default presence without resolving source spellings.
fn params(values: &[ParamExport]) -> Vec<CallableParam> {
    values
        .iter()
        .map(|value| CallableParam {
            name: Some(value.name.clone()),
            ty: resolved_type_from_manifest_type_ref(&value.ty),
            kind: match value.kind {
                ParamKindExport::Normal => crate::ast::ParamKind::Normal,
                ParamKindExport::RestPositional => crate::ast::ParamKind::RestPositional,
                ParamKindExport::RestKeyword => crate::ast::ParamKind::RestKeyword,
            },
            has_default: value.has_default,
            is_partial_preset: false,
            is_mut: value.is_mut,
        })
        .collect()
}

/// Recover the exact callable signature and separately materializable defaults.
fn function(value: &FunctionExport) -> Result<CheckedFunctionExport, LibraryManifestError> {
    Ok(CheckedFunctionExport {
        name: value.name.clone(),
        emitted_name: value.emitted_name.clone(),
        type_params: type_params(&value.type_params),
        params: params(&value.params),
        param_defaults: defaults(&value.params)?,
        return_type: resolved_type_from_manifest_type_ref(&value.return_type),
        is_async: value.is_async,
    })
}

/// Reconstruct declared defaults recursively, refusing malformed floating-point spellings.
fn defaults(values: &[ParamExport]) -> Result<Vec<Option<CheckedParamDefault>>, LibraryManifestError> {
    values
        .iter()
        .map(|value| value.default.as_ref().map(default_value).transpose())
        .collect()
}

/// Decode one metadata-safe default without source evaluation or caller-side type inference.
fn default_value(value: &ParamDefaultExport) -> Result<CheckedParamDefault, LibraryManifestError> {
    Ok(match value {
        ParamDefaultExport::Int(value) => CheckedParamDefault::Int(*value),
        ParamDefaultExport::Float(value) => {
            CheckedParamDefault::Float(value.parse().map_err(|_| invalid("invalid checked float default"))?)
        }
        ParamDefaultExport::Bool(value) => CheckedParamDefault::Bool(*value),
        ParamDefaultExport::String(value) => CheckedParamDefault::String(value.clone()),
        ParamDefaultExport::Bytes(value) => CheckedParamDefault::Bytes(value.clone()),
        ParamDefaultExport::None => CheckedParamDefault::None,
        ParamDefaultExport::List(values) => {
            CheckedParamDefault::List(values.iter().map(default_value).collect::<Result<_, _>>()?)
        }
        ParamDefaultExport::Dict(values) => CheckedParamDefault::Dict(
            values
                .iter()
                .map(|entry| Ok((default_value(&entry.key)?, default_value(&entry.value)?)))
                .collect::<Result<_, LibraryManifestError>>()?,
        ),
        ParamDefaultExport::ConstRef(path) => CheckedParamDefault::ConstRef(path.clone()),
        ParamDefaultExport::Call { path, args, signature } => CheckedParamDefault::Call {
            path: path.clone(),
            args: args
                .iter()
                .map(|arg| {
                    Ok(CheckedParamDefaultArg {
                        name: arg.name.clone(),
                        value: default_value(&arg.value)?,
                    })
                })
                .collect::<Result<_, LibraryManifestError>>()?,
            signature: signature.as_ref().map(|signature| CheckedParamDefaultCallSignature {
                params: params(&signature.params),
                return_type: resolved_type_from_manifest_type_ref(&signature.return_type),
            }),
        },
        ParamDefaultExport::Unsupported => CheckedParamDefault::Unsupported,
    })
}

/// Reconstruct fields while preserving private visibility, checked defaults and member authority.
fn fields(values: &[FieldExport]) -> Result<Vec<CheckedField>, LibraryManifestError> {
    values
        .iter()
        .map(|value| {
            Ok(CheckedField {
                name: value.name.clone(),
                canonical: value.canonical.as_ref().and_then(CanonicalIdentityExport::hydrate),
                ty: resolved_type_from_manifest_type_ref(&value.ty),
                surface_type_name: value.surface_type_name.clone(),
                visibility: match value.visibility {
                    FieldVisibilityExport::Public => crate::ast::Visibility::Public,
                    FieldVisibilityExport::Private => crate::ast::Visibility::Private,
                },
                has_default: value.has_default,
                default: value.default.as_ref().map(default_value).transpose()?,
                alias: value.alias.clone(),
                description: value.description.clone(),
            })
        })
        .collect()
}

/// Retain computed properties with exact return types and member canonical identities.
fn properties(values: &[PropertyExport]) -> Vec<CheckedProperty> {
    values
        .iter()
        .map(|value| CheckedProperty {
            name: value.name.clone(),
            canonical: value.canonical.as_ref().and_then(CanonicalIdentityExport::hydrate),
            return_type: resolved_type_from_manifest_type_ref(&value.return_type),
        })
        .collect()
}

/// Reconstruct method overloads, receiver ownership, defaults and exact implementation bounds.
fn methods(values: &[MethodExport]) -> Result<Vec<CheckedMethod>, LibraryManifestError> {
    values
        .iter()
        .map(|value| {
            Ok(CheckedMethod {
                name: value.name.clone(),
                canonical: value.canonical.as_ref().and_then(CanonicalIdentityExport::hydrate),
                alias_of: value.alias_of.clone(),
                type_params: type_params(&value.type_params),
                receiver: value.receiver.as_ref().map(|receiver| match receiver {
                    ReceiverExport::Immutable => crate::ast::Receiver::Immutable,
                    ReceiverExport::Mutable => crate::ast::Receiver::Mutable,
                }),
                params: params(&value.params),
                param_defaults: defaults(&value.params)?,
                return_type: resolved_type_from_manifest_type_ref(&value.return_type),
                is_async: value.is_async,
                has_body: value.has_body,
            })
        })
        .collect()
}

/// Preserve explicit partial target classification; unknown remains unproven rather than becoming a function.
fn partial_kind(value: PartialTargetKindExport) -> CheckedPartialTargetKind {
    match value {
        PartialTargetKindExport::Function => CheckedPartialTargetKind::Function,
        PartialTargetKindExport::ModelConstructor => CheckedPartialTargetKind::ModelConstructor,
        PartialTargetKindExport::ClassConstructor => CheckedPartialTargetKind::ClassConstructor,
        PartialTargetKindExport::NewtypeConstructor => CheckedPartialTargetKind::NewtypeConstructor,
        PartialTargetKindExport::Partial => CheckedPartialTargetKind::Partial,
        PartialTargetKindExport::Unknown => CheckedPartialTargetKind::Unknown,
    }
}

/// Decode structured partial presets without executing constructors or discarding capture ownership.
fn preset_value(value: &PresetValueExport) -> Result<CheckedPresetValue, LibraryManifestError> {
    Ok(match value {
        PresetValueExport::Int(value) => CheckedPresetValue::Int(*value),
        PresetValueExport::Float(value) => {
            CheckedPresetValue::Float(value.parse().map_err(|_| invalid("invalid checked float preset"))?)
        }
        PresetValueExport::Bool(value) => CheckedPresetValue::Bool(*value),
        PresetValueExport::String(value) => CheckedPresetValue::String(value.clone()),
        PresetValueExport::Bytes(value) => CheckedPresetValue::Bytes(value.clone()),
        PresetValueExport::None => CheckedPresetValue::None,
        PresetValueExport::List(values) => {
            CheckedPresetValue::List(values.iter().map(preset_value).collect::<Result<_, _>>()?)
        }
        PresetValueExport::Set(values) => {
            CheckedPresetValue::Set(values.iter().map(preset_value).collect::<Result<_, _>>()?)
        }
        PresetValueExport::Tuple(values) => {
            CheckedPresetValue::Tuple(values.iter().map(preset_value).collect::<Result<_, _>>()?)
        }
        PresetValueExport::Dict(values) => CheckedPresetValue::Dict(
            values
                .iter()
                .map(|entry| Ok((preset_value(&entry.key)?, preset_value(&entry.value)?)))
                .collect::<Result<_, LibraryManifestError>>()?,
        ),
        PresetValueExport::ConstRef(path) => CheckedPresetValue::ConstRef(path.clone()),
        PresetValueExport::ModelLiteral { name, fields } => CheckedPresetValue::ModelLiteral {
            name: name.clone(),
            fields: fields
                .iter()
                .map(|field| Ok((field.name.clone(), preset_value(&field.value)?)))
                .collect::<Result<_, LibraryManifestError>>()?,
        },
        PresetValueExport::Unsupported => CheckedPresetValue::Unsupported,
    })
}

/// Describe a replay boundary failure through the existing typed manifest diagnostic family.
fn invalid(message: &str) -> LibraryManifestError {
    LibraryManifestError::Invalid(message.to_string())
}

#[cfg(test)]
mod tests;

/// Use the existing validated transport contract; the semantic model deliberately does not implement serde.
mod manifest_contract {
    use super::LibraryManifest;
    use serde::{Deserialize, Serialize};

    /// Validate before writing a replay projection into its immutable payload.
    pub fn serialize<S: serde::Serializer>(value: &LibraryManifest, serializer: S) -> Result<S::Ok, S::Error> {
        value.to_json_string().map_err(serde::ser::Error::custom)?;
        super::super::wire::RawLibraryManifest::from_semantic(value).serialize(serializer)
    }

    /// Enter the same manifest version and semantic admission gates as an ordinary `.incnlib` consumer.
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<LibraryManifest, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        LibraryManifest::from_json_str(&value.to_string()).map_err(serde::de::Error::custom)
    }
}

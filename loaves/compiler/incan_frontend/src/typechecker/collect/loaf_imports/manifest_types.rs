//! Model, class, trait, enum and newtype infos, fields, properties and methods from manifests.

use super::*;

impl TypeChecker {
    /// Convert one manifest model export into semantic model metadata.
    pub(in crate::typechecker::collect) fn model_info_from_manifest(&self, export: &ModelExport) -> ModelInfo {
        let methods = self.methods_from_manifest(&export.methods, Some(&export.name));
        let method_overloads = self.method_overloads_from_manifest(&export.methods, Some(&export.name));
        ModelInfo {
            type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
            traits: export.traits.clone(),
            trait_adoptions: Self::trait_adoptions_from_manifest(&export.traits, &export.trait_adoptions),
            derives: export.derives.clone(),
            fields: self.fields_from_manifest(&export.name, &export.fields, false),
            field_order: export.fields.iter().map(|field| field.name.clone()).collect(),
            properties: self.properties_from_manifest(&export.name, &export.properties),
            method_overloads,
            methods,
            method_aliases: Self::method_aliases_from_manifest(&export.methods),
        }
    }

    /// Convert one manifest class export into semantic class metadata.
    pub(in crate::typechecker::collect) fn class_info_from_manifest(&self, export: &ClassExport) -> ClassInfo {
        let methods = self.methods_from_manifest(&export.methods, Some(&export.name));
        let method_overloads = self.method_overloads_from_manifest(&export.methods, Some(&export.name));
        ClassInfo {
            type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
            extends: export.extends.clone(),
            traits: export.traits.clone(),
            trait_adoptions: Self::trait_adoptions_from_manifest(&export.traits, &export.trait_adoptions),
            derives: export.derives.clone(),
            fields: self.fields_from_manifest(
                &export.name,
                &export.fields,
                export.fields.iter().any(|field| {
                    matches!(
                        field.visibility,
                        crate::library_manifest::MemberVisibilityExport::Private
                    )
                }),
            ),
            field_defaults: Box::new(
                export
                    .fields
                    .iter()
                    .filter_map(|field| {
                        field.default.as_ref().and_then(|default| {
                            Self::manifest_param_default_expr(default, Span::default())
                                .map(|expr| (field.name.clone(), expr))
                        })
                    })
                    .collect(),
            ),
            field_default_metadata: Box::new(
                export
                    .fields
                    .iter()
                    .filter_map(|field| {
                        field.default.as_ref().map_or_else(
                            || {
                                field
                                    .has_default
                                    .then(|| (field.name.clone(), CheckedParamDefault::Unsupported))
                            },
                            |default| Some((field.name.clone(), self.checked_param_default_from_manifest(default))),
                        )
                    })
                    .collect(),
            ),
            field_provider_libraries: Box::new(HashMap::new()),
            field_order: export.fields.iter().map(|field| field.name.clone()).collect(),
            properties: self.properties_from_manifest(&export.name, &export.properties),
            method_overloads,
            methods,
            method_aliases: Self::method_aliases_from_manifest(&export.methods),
        }
    }

    /// Preserve a compiled-library field default in the checked export vocabulary without reparsing or rebasing it.
    pub(in crate::typechecker::collect) fn checked_param_default_from_manifest(
        &self,
        default: &ParamDefaultExport,
    ) -> CheckedParamDefault {
        match default {
            ParamDefaultExport::Int(value) => CheckedParamDefault::Int(*value),
            ParamDefaultExport::Float(value) => value
                .parse::<f64>()
                .map(CheckedParamDefault::Float)
                .unwrap_or(CheckedParamDefault::Unsupported),
            ParamDefaultExport::Bool(value) => CheckedParamDefault::Bool(*value),
            ParamDefaultExport::String(value) => CheckedParamDefault::String(value.clone()),
            ParamDefaultExport::Bytes(value) => CheckedParamDefault::Bytes(value.clone()),
            ParamDefaultExport::None => CheckedParamDefault::None,
            ParamDefaultExport::List(values) => CheckedParamDefault::List(
                values
                    .iter()
                    .map(|value| self.checked_param_default_from_manifest(value))
                    .collect(),
            ),
            ParamDefaultExport::Dict(entries) => CheckedParamDefault::Dict(
                entries
                    .iter()
                    .map(|entry| {
                        (
                            self.checked_param_default_from_manifest(&entry.key),
                            self.checked_param_default_from_manifest(&entry.value),
                        )
                    })
                    .collect(),
            ),
            ParamDefaultExport::ConstRef(path) => CheckedParamDefault::ConstRef(path.clone()),
            ParamDefaultExport::Call { path, args, signature } => CheckedParamDefault::Call {
                path: path.clone(),
                args: args
                    .iter()
                    .map(|arg| CheckedParamDefaultArg {
                        name: arg.name.clone(),
                        value: self.checked_param_default_from_manifest(&arg.value),
                    })
                    .collect(),
                signature: signature.as_ref().map(|signature| CheckedParamDefaultCallSignature {
                    params: self.params_from_manifest(&signature.params),
                    return_type: resolved_type_from_manifest_type_ref(&signature.return_type),
                }),
            },
            ParamDefaultExport::MethodCall {
                receiver_path,
                method,
                args,
            } => CheckedParamDefault::MethodCall {
                receiver_path: receiver_path.clone(),
                method: method.clone(),
                args: args
                    .iter()
                    .map(|arg| CheckedParamDefaultArg {
                        name: arg.name.clone(),
                        value: self.checked_param_default_from_manifest(&arg.value),
                    })
                    .collect(),
            },
            ParamDefaultExport::Unsupported => CheckedParamDefault::Unsupported,
        }
    }

    /// Rebuild a manifest-safe field default as synthetic source metadata for inherited constructor exports.
    pub(in crate::typechecker::collect) fn manifest_param_default_expr(
        default: &ParamDefaultExport,
        span: Span,
    ) -> Option<Spanned<Expr>> {
        let expr = match default {
            ParamDefaultExport::Int(value) => Expr::Literal(Literal::Int(IntLiteral::synthetic(*value))),
            ParamDefaultExport::Float(value) => Expr::Literal(Literal::Float(FloatLiteral {
                value: value.parse().ok()?,
                repr: value.clone(),
                suffix: None,
            })),
            ParamDefaultExport::Bool(value) => Expr::Literal(Literal::Bool(*value)),
            ParamDefaultExport::String(value) => Expr::Literal(Literal::String(value.clone())),
            ParamDefaultExport::Bytes(value) => Expr::Literal(Literal::Bytes(value.clone())),
            ParamDefaultExport::None => Expr::Literal(Literal::None),
            ParamDefaultExport::List(values) => Expr::List(
                values
                    .iter()
                    .map(|value| Self::manifest_param_default_expr(value, span).map(ListEntry::Element))
                    .collect::<Option<Vec<_>>>()?,
            ),
            ParamDefaultExport::Dict(entries) => Expr::Dict(
                entries
                    .iter()
                    .map(|entry| {
                        Some(DictEntry::Pair(
                            Self::manifest_param_default_expr(&entry.key, span)?,
                            Self::manifest_param_default_expr(&entry.value, span)?,
                        ))
                    })
                    .collect::<Option<Vec<_>>>()?,
            ),
            ParamDefaultExport::ConstRef(path) => Self::manifest_const_ref_expr(path, span)?.node,
            ParamDefaultExport::Call { path, args, .. } => Expr::Call(
                Box::new(Self::manifest_const_ref_expr(path, span)?),
                Vec::new(),
                args.iter()
                    .map(|arg| {
                        let value = Self::manifest_param_default_expr(&arg.value, span)?;
                        Some(match &arg.name {
                            Some(name) => CallArg::Named(Spanned::new(name.clone(), span), value),
                            None => CallArg::Positional(value),
                        })
                    })
                    .collect::<Option<Vec<_>>>()?,
            ),
            ParamDefaultExport::MethodCall {
                receiver_path,
                method,
                args,
            } => Expr::MethodCall(
                Box::new(Self::manifest_const_ref_expr(receiver_path, span)?),
                method.clone(),
                Vec::new(),
                args.iter()
                    .map(|arg| {
                        let value = Self::manifest_param_default_expr(&arg.value, span)?;
                        Some(match &arg.name {
                            Some(name) => CallArg::Named(Spanned::new(name.clone(), span), value),
                            None => CallArg::Positional(value),
                        })
                    })
                    .collect::<Option<Vec<_>>>()?,
            ),
            ParamDefaultExport::Unsupported => return None,
        };
        Some(Spanned::new(expr, span))
    }

    /// Convert one manifest trait export into semantic trait metadata.
    pub(in crate::typechecker::collect) fn trait_info_from_manifest(&self, export: &TraitExport) -> TraitInfo {
        TraitInfo {
            type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
            supertraits: export
                .supertraits
                .iter()
                .map(|bound| {
                    (
                        bound.name.clone(),
                        bound
                            .type_args
                            .iter()
                            .map(resolved_type_from_manifest_type_ref)
                            .collect(),
                    )
                })
                .collect(),
            methods: self.methods_from_manifest(&export.methods, None),
            method_aliases: Self::method_aliases_from_manifest(&export.methods),
            properties: std::collections::HashMap::new(),
            requires: export
                .requires
                .iter()
                .map(|required| {
                    (
                        required.name.clone(),
                        resolved_type_from_manifest_type_ref(&required.ty),
                    )
                })
                .collect(),
        }
    }

    /// Convert manifest trait adoption metadata, falling back to legacy trait-name-only manifests.
    pub(in crate::typechecker::collect) fn trait_adoptions_from_manifest(
        trait_names: &[String],
        trait_adoptions: &[TypeBoundExport],
    ) -> Vec<TypeBoundInfo> {
        if trait_adoptions.is_empty() {
            return trait_names
                .iter()
                .map(|name| TypeBoundInfo {
                    name: name.clone(),
                    source_name: None,
                    type_args: Vec::new(),
                    module_path: None,
                    implementation_type_params: Vec::new(),
                    inferred: false,
                })
                .collect();
        }

        trait_adoptions
            .iter()
            .map(|bound| TypeBoundInfo {
                name: bound.name.clone(),
                source_name: bound.source_name.clone(),
                type_args: bound
                    .type_args
                    .iter()
                    .map(resolved_type_from_manifest_type_ref)
                    .collect(),
                module_path: bound.module_path.clone(),
                implementation_type_params: Self::implementation_type_params_from_manifest(
                    &bound.implementation_type_params,
                ),
                inferred: bound.inferred,
            })
            .collect()
    }

    /// Decode one manifest implementation header into frontend semantic types.
    pub(in crate::typechecker::collect) fn implementation_type_params_from_manifest(
        type_params: &[ImplementationTypeParamExport],
    ) -> Vec<ImplementationTypeParamInfo> {
        type_params
            .iter()
            .map(|type_param| ImplementationTypeParamInfo {
                name: type_param.name.clone(),
                bounds: type_param
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
                            .map(|associated| {
                                (
                                    associated.name.clone(),
                                    resolved_type_from_manifest_type_ref(&associated.ty),
                                )
                            })
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
            .collect()
    }

    /// Convert a manifest enum export into local enum symbol metadata.
    pub(in crate::typechecker::collect) fn enum_info_from_manifest(&self, export: &EnumExport) -> EnumInfo {
        let value_enum = export.value_type.map(|value_type| ValueEnumInfo {
            value_type: match value_type {
                EnumValueTypeExport::Str => ValueEnumBacking::Str,
                EnumValueTypeExport::Int => ValueEnumBacking::Int,
            },
            values: export
                .variants
                .iter()
                .filter_map(|variant| {
                    let value = match variant.value.as_ref()? {
                        EnumValueExport::Str(value) => ValueEnumValue::Str(value.clone()),
                        EnumValueExport::Int(value) => ValueEnumValue::Int(*value),
                    };
                    Some((variant.name.clone(), value))
                })
                .collect(),
        });

        EnumInfo {
            type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
            traits: export.traits.clone(),
            trait_adoptions: Self::trait_adoptions_from_manifest(&export.traits, &export.trait_adoptions),
            variants: export.variants.iter().map(|variant| variant.name.clone()).collect(),
            variant_identities: export
                .variants
                .iter()
                .filter_map(|variant| Some((variant.name.clone(), variant.canonical.as_ref()?.hydrate()?)))
                .chain(export.variant_aliases.iter().filter_map(|alias| {
                    let identity = export
                        .variants
                        .iter()
                        .find(|variant| variant.name == alias.target)?
                        .canonical
                        .as_ref()?
                        .hydrate()?;
                    Some((alias.name.clone(), identity))
                }))
                .collect(),
            variant_fields: export
                .variants
                .iter()
                .map(|variant| {
                    (
                        variant.name.clone(),
                        variant
                            .fields
                            .iter()
                            .map(resolved_type_from_manifest_type_ref)
                            .collect(),
                    )
                })
                .collect(),
            variant_aliases: export
                .variant_aliases
                .iter()
                .map(|alias| (alias.name.clone(), alias.target.clone()))
                .collect(),
            value_enum,
            derives: export.derives.clone(),
            method_overloads: self.method_overloads_from_manifest(&export.methods, Some(&export.name)),
            methods: self.methods_from_manifest(&export.methods, Some(&export.name)),
        }
    }

    /// Convert a manifest newtype export into local typechecker metadata.
    pub(in crate::typechecker::collect) fn newtype_info_from_manifest(&self, export: &NewtypeExport) -> NewtypeInfo {
        NewtypeInfo {
            type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
            is_rusttype: export.is_rusttype,
            has_interop: false,
            underlying: resolved_type_from_manifest_type_ref(&export.underlying),
            constraints: export
                .constraints
                .iter()
                .map(|constraint| constraint.to_checked())
                .collect(),
            implicit_coercion_enabled: export.implicit_coercion_enabled,
            method_rebindings: std::collections::HashMap::new(),
            traits: export.traits.clone(),
            trait_adoptions: Self::trait_adoptions_from_manifest(&export.traits, &export.trait_adoptions),
            derives: export.derives.clone(),
            method_aliases: Self::method_aliases_from_manifest(&export.methods),
            methods: self.methods_from_manifest(&export.methods, Some(&export.name)),
            method_overloads: self.method_overloads_from_manifest(&export.methods, Some(&export.name)),
        }
    }

    pub(in crate::typechecker::collect) fn type_param_bounds_from_manifest(
        &self,
        type_params: &[TypeParamExport],
    ) -> std::collections::HashMap<String, Vec<String>> {
        type_params
            .iter()
            .map(|param| {
                (
                    param.name.clone(),
                    param.bounds.iter().map(|bound| bound.name.clone()).collect(),
                )
            })
            .collect()
    }

    /// Convert manifest type-parameter bounds while preserving generic trait arguments.
    pub(in crate::typechecker::collect) fn type_param_bound_details_from_manifest(
        &self,
        type_params: &[TypeParamExport],
    ) -> std::collections::HashMap<String, Vec<TypeBoundInfo>> {
        type_params
            .iter()
            .map(|param| {
                (
                    param.name.clone(),
                    param
                        .bounds
                        .iter()
                        .map(|bound| TypeBoundInfo {
                            name: bound.name.clone(),
                            source_name: bound.source_name.clone(),
                            type_args: bound
                                .type_args
                                .iter()
                                .map(resolved_type_from_manifest_type_ref)
                                .collect(),
                            module_path: bound.module_path.clone(),
                            implementation_type_params: Self::implementation_type_params_from_manifest(
                                &bound.implementation_type_params,
                            ),
                            inferred: bound.inferred,
                        })
                        .collect(),
                )
            })
            .collect()
    }

    /// Convert exported manifest fields into semantic field metadata for imported-library typechecking.
    pub(in crate::typechecker::collect) fn fields_from_manifest(
        &self,
        owner: &str,
        fields: &[FieldExport],
        uses_provider_constructor_bridge: bool,
    ) -> std::collections::HashMap<String, FieldInfo> {
        fields
            .iter()
            .map(|field| {
                (
                    field.name.clone(),
                    FieldInfo {
                        identity: field.canonical.as_ref().and_then(|identity| identity.hydrate()),
                        record_identity: field.hydrated_record_identity(),
                        record_canonical: field.record_canonical,
                        renamed_from: field.renamed_from.clone(),
                        ty: resolved_type_from_manifest_type_ref(&field.ty),
                        surface_type_name: field.surface_type_name.clone(),
                        visibility: match field.visibility {
                            crate::library_manifest::MemberVisibilityExport::Private => crate::ast::Visibility::Private,
                            crate::library_manifest::MemberVisibilityExport::Public => crate::ast::Visibility::Public,
                        },
                        is_type_private: matches!(
                            field.visibility,
                            crate::library_manifest::MemberVisibilityExport::Private
                        ),
                        owner: Some(owner.to_string()),
                        has_default: if uses_provider_constructor_bridge {
                            field.has_default
                        } else {
                            field
                                .default
                                .as_ref()
                                .is_some_and(ParamDefaultExport::is_materializable)
                        },
                        alias: field.alias.clone(),
                        description: field.description.clone(),
                        made_private: field.made_private.as_ref().map(Into::into),
                    },
                )
            })
            .collect()
    }

    /// Convert manifest computed properties into public semantic member metadata.
    pub(in crate::typechecker::collect) fn properties_from_manifest(
        &self,
        owner: &str,
        properties: &[PropertyExport],
    ) -> std::collections::HashMap<String, PropertyInfo> {
        properties
            .iter()
            .map(|property| {
                (
                    property.name.clone(),
                    PropertyInfo {
                        identity: property.canonical.as_ref().and_then(|identity| identity.hydrate()),
                        return_type: resolved_type_from_manifest_type_ref(&property.return_type),
                        visibility: crate::ast::Visibility::Public,
                        owner: Some(owner.to_string()),
                        has_body: true,
                    },
                )
            })
            .collect()
    }

    /// Convert manifest methods into the legacy single-method-per-name lookup map.
    pub(in crate::typechecker::collect) fn methods_from_manifest(
        &self,
        methods: &[MethodExport],
        owner: Option<&str>,
    ) -> std::collections::HashMap<String, MethodInfo> {
        methods
            .iter()
            .map(|method| (method.name.clone(), self.method_info_from_manifest(method, owner)))
            .collect()
    }

    /// Preserve same-type aliases so lowering can emit the canonical Rust method from a compiled dependency.
    pub(in crate::typechecker::collect) fn method_aliases_from_manifest(
        methods: &[MethodExport],
    ) -> HashMap<String, String> {
        methods
            .iter()
            .filter_map(|method| {
                method
                    .alias_of
                    .as_ref()
                    .map(|target| (method.name.clone(), target.clone()))
            })
            .collect()
    }

    /// Group manifest methods by name without dropping same-name trait-backed overloads.
    pub(in crate::typechecker::collect) fn method_overloads_from_manifest(
        &self,
        methods: &[MethodExport],
        owner: Option<&str>,
    ) -> std::collections::HashMap<String, Vec<MethodInfo>> {
        let mut groups: std::collections::HashMap<String, Vec<MethodInfo>> = std::collections::HashMap::new();
        for method in methods {
            groups
                .entry(method.name.clone())
                .or_default()
                .push(self.method_info_from_manifest(method, owner));
        }
        groups
    }

    /// Convert one manifest method export into semantic method metadata, owned by the type named `owner`.
    pub(in crate::typechecker::collect) fn method_info_from_manifest(
        &self,
        method: &MethodExport,
        owner: Option<&str>,
    ) -> MethodInfo {
        MethodInfo {
            identity: method.canonical.as_ref().and_then(|identity| identity.hydrate()),
            scope: MethodScope::written(method.visibility.into(), true),
            owner: owner.map(str::to_string),
            type_params: method.type_params.iter().map(|tp| tp.name.clone()).collect(),
            type_param_bounds: method
                .type_params
                .iter()
                .map(|tp| {
                    (
                        tp.name.clone(),
                        tp.bounds.iter().map(|bound| bound.name.clone()).collect(),
                    )
                })
                .collect(),
            type_param_bound_details: method
                .type_params
                .iter()
                .map(|tp| {
                    (
                        tp.name.clone(),
                        tp.bounds
                            .iter()
                            .map(|bound| TypeBoundInfo {
                                name: bound.name.clone(),
                                source_name: bound.source_name.clone(),
                                type_args: bound
                                    .type_args
                                    .iter()
                                    .map(resolved_type_from_manifest_type_ref)
                                    .collect(),
                                module_path: bound.module_path.clone(),
                                implementation_type_params: Self::implementation_type_params_from_manifest(
                                    &bound.implementation_type_params,
                                ),
                                inferred: bound.inferred,
                            })
                            .collect(),
                    )
                })
                .collect(),
            trait_target: None,
            receiver: self.receiver_from_manifest(method.receiver.as_ref()),
            params: self.params_from_manifest(&method.params),
            return_type: resolved_type_from_manifest_type_ref(&method.return_type),
            is_async: method.is_async,
            has_body: method.has_body,
            alias_of: method.alias_of.clone(),
            made_private: method.made_private.as_ref().map(Into::into),
        }
    }

    /// Convert manifest parameters into checked callable parameters.
    pub(in crate::typechecker::collect) fn params_from_manifest(&self, params: &[ParamExport]) -> Vec<CallableParam> {
        params
            .iter()
            .map(|param| {
                CallableParam::named_with_default(
                    param.name.clone(),
                    resolved_type_from_manifest_type_ref(&param.ty),
                    param_kind_from_manifest(param.kind),
                    param
                        .default
                        .as_ref()
                        .map_or(param.has_default, ParamDefaultExport::is_materializable),
                )
                .with_mut(param.is_mut)
                .with_surface_type_name(param.surface_type_name.clone())
            })
            .collect()
    }

    /// Preserve an imported method's declared receiver access mode.
    pub(in crate::typechecker::collect) fn receiver_from_manifest(
        &self,
        receiver: Option<&ReceiverExport>,
    ) -> Option<Receiver> {
        match receiver {
            Some(ReceiverExport::Immutable) => Some(Receiver::Immutable),
            Some(ReceiverExport::Mutable) => Some(Receiver::Mutable),
            None => None,
        }
    }
}

//! Conversion of checked API declarations into library manifest exports.

use super::*;

/// Convert a checked API function into manifest-style callable metadata.
pub fn function_export_from_api(function: &ApiFunction) -> FunctionExport {
    FunctionExport {
        ownership: function.ownership.clone(),
        name: function.name.clone(),
        emitted_name: None,
        type_params: function.type_params.clone(),
        params: function.params.clone(),
        return_type: function.return_type.clone(),
        is_async: function.is_async,
    }
}

/// Convert a projected alias callable into manifest-style callable metadata while preserving source callable identity.
pub fn function_export_from_api_projected(function: &ApiProjectedFunction) -> FunctionExport {
    FunctionExport {
        ownership: function.callable.ownership.clone(),
        name: function.callable.name.clone(),
        emitted_name: None,
        type_params: function.callable.type_params.clone(),
        params: function.callable.params.clone(),
        return_type: function.callable.return_type.clone(),
        is_async: function.callable.is_async,
    }
}

/// Convert a checked API partial into manifest-style partial metadata.
pub fn partial_export_from_api(partial: &ApiPartial) -> PartialExport {
    PartialExport {
        name: partial.name.clone(),
        target_path: partial.target_path.clone(),
        target_kind: partial.target_kind,
        presets: partial.presets.clone(),
        type_params: partial.type_params.clone(),
        params: partial.params.clone(),
        return_type: partial.return_type.clone(),
        is_async: partial.is_async,
    }
}

/// Convert a checked API method into manifest-style method metadata.
pub fn method_export_from_api(method: &ApiMethod) -> MethodExport {
    MethodExport {
        ownership: method.ownership.clone(),
        name: method.name.clone(),
        canonical: method.canonical.clone(),
        visibility: method.visibility,
        alias_of: method.alias_of.clone(),
        type_params: method.type_params.clone(),
        receiver: method.receiver.clone(),
        params: method.params.clone(),
        return_type: method.return_type.clone(),
        is_async: method.is_async,
        has_body: method.has_body,
    }
}

/// Convert a checked API computed property into manifest-style metadata.
pub(super) fn property_export_from_api(property: &ApiProperty) -> PropertyExport {
    PropertyExport {
        name: property.name.clone(),
        canonical: property.canonical.clone(),
        return_type: property.return_type.clone(),
    }
}

/// Convert a checked API model into manifest-style model metadata for public boundary consumers.
pub fn model_export_from_api(model: &ApiModel) -> ModelExport {
    ModelExport {
        name: model.name.clone(),
        type_params: model.type_params.clone(),
        traits: model.traits.clone(),
        trait_adoptions: model.trait_adoptions.clone(),
        derives: model.derives.clone(),
        fields: model.fields.clone(),
        properties: model.properties.iter().map(property_export_from_api).collect(),
        methods: model.methods.iter().map(method_export_from_api).collect(),
    }
}

/// Convert a checked API class into manifest-style class metadata for public boundary consumers.
pub fn class_export_from_api(class: &ApiClass) -> ClassExport {
    ClassExport {
        name: class.name.clone(),
        type_params: class.type_params.clone(),
        extends: class.extends.clone(),
        traits: class.traits.clone(),
        trait_adoptions: class.trait_adoptions.clone(),
        derives: class.derives.clone(),
        fields: class.fields.clone(),
        properties: class.properties.iter().map(property_export_from_api).collect(),
        methods: class.methods.iter().map(method_export_from_api).collect(),
    }
}

/// Convert a checked API trait into manifest-style trait metadata for public boundary consumers.
pub fn trait_export_from_api(trait_decl: &ApiTrait) -> TraitExport {
    TraitExport {
        name: trait_decl.name.clone(),
        source_name: None,
        type_params: trait_decl.type_params.clone(),
        supertraits: trait_decl.supertraits.clone(),
        requires: trait_decl
            .requires
            .iter()
            .map(|field| FieldRequirementExport {
                name: field.name.clone(),
                ty: field.ty.clone(),
            })
            .collect(),
        methods: trait_decl.methods.iter().map(method_export_from_api).collect(),
    }
}

/// Convert a checked API enum into manifest-style enum metadata for public boundary consumers.
pub fn enum_export_from_api(enum_decl: &ApiEnum) -> EnumExport {
    EnumExport {
        name: enum_decl.name.clone(),
        type_params: enum_decl.type_params.clone(),
        traits: enum_decl.traits.clone(),
        trait_adoptions: enum_decl.trait_adoptions.clone(),
        value_type: enum_decl.value_type,
        ordinal_type_identity: None,
        variants: enum_decl
            .variants
            .iter()
            .map(|variant| EnumVariantExport {
                name: variant.name.clone(),
                canonical: variant.canonical.clone(),
                fields: variant.fields.clone(),
                value: variant.value.clone(),
            })
            .collect(),
        variant_aliases: enum_decl
            .variant_aliases
            .iter()
            .map(|alias| EnumVariantAliasExport {
                name: alias.name.clone(),
                target: alias.target.clone(),
            })
            .collect(),
        methods: enum_decl.methods.iter().map(method_export_from_api).collect(),
        derives: enum_decl.derives.clone(),
    }
}

/// Convert a checked API newtype into manifest-style newtype metadata for public boundary consumers.
pub fn newtype_export_from_api(newtype: &ApiNewtype) -> NewtypeExport {
    NewtypeExport {
        name: newtype.name.clone(),
        type_params: newtype.type_params.clone(),
        traits: newtype.traits.clone(),
        trait_adoptions: newtype.trait_adoptions.clone(),
        derives: newtype.derives.clone(),
        is_rusttype: newtype.is_rusttype,
        underlying: newtype.underlying.clone(),
        checked_constructor: newtype.checked_constructor.clone(),
        constraints: newtype.constraints.clone(),
        implicit_coercion_enabled: newtype.implicit_coercion_enabled,
        methods: newtype.methods.iter().map(method_export_from_api).collect(),
    }
}

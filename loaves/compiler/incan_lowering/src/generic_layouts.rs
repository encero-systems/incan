//! Generic native ADTs published by their declaring Loaf, without consumer-owned replacement structs.

use incan_lang::lang::types::collections::{CollectionTypeId, from_str as collection_type};
use incan_mir_plan_native::{GenericLayoutField, GenericLayoutType, GenericModelDeclaration, source_span as plan_span};
use incan_semantics_core::body_ir::{Body, BodyIrModule, NominalDeclaration, StaticInitializer};
use incan_semantics_core::{
    IncanCallableParam, IncanCallableParamKind, IncanInputAccess, IncanType, SemanticSourceTargetKind,
};

use crate::closures::{callable_component, packed_input};
use crate::decls::{library_enum_item, library_item};
use crate::layout_types::native_source_type;
use crate::native_type_identity::ConstructorIndex;
use crate::nominal_instances::NominalInstance;

/// Retain source model/class templates needed by consumers; linked Loaves own their existing ADTs.
///
/// 1. Inventory template references in checked frame and field types.
/// 2. Select public, instantiated or referenced source templates.
/// 3. Validate their fields and derives before emitting native declarations.
pub fn generic_models(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    referenced: &[String],
    instances: &[NominalInstance],
    file: &str,
) -> Result<Vec<GenericModelDeclaration>, String> {
    let mut models = Vec::new();
    // 2. Keep private templates that a retained source frame or field can instantiate later.
    for declaration in &module.nominal_declarations {
        if declaration.type_parameters.is_empty() || !library_item(declaration).is_empty() {
            continue;
        }
        let instantiated = instances
            .iter()
            .any(|instance| instance.declaration.canonical == declaration.canonical);
        if !declaration.public && !instantiated && !referenced.contains(&declaration.name) {
            continue;
        }
        if declaration.canonical.kind == SemanticSourceTargetKind::Newtype {
            if !instantiated {
                continue;
            }
            return Err("unsupported Body IR generic Loaf newtype layout".into());
        }
        // 3. Validate native layout and derive requirements before publishing this declaration.
        let mut fields = Vec::with_capacity(declaration.fields.len());
        for (index, name) in declaration.fields.iter().enumerate() {
            let ty = match generic_field_type(
                constructors,
                module,
                &declaration.field_types[index],
                &declaration.type_parameters,
            ) {
                Ok(ty) => ty,
                Err(error) => return Err(format!("{}.{}: {error}", declaration.name, name)),
            };
            fields.push(GenericLayoutField {
                name: name.clone(),
                ty,
                public: declaration.field_public[index],
            });
        }
        let derives = generic_derives(declaration)?;
        models.push(GenericModelDeclaration {
            name: declaration.name.clone(),
            source_name: declaration.canonical.declaration_name.clone(),
            parameters: declaration.type_parameters.clone(),
            fields,
            derives,
            span: plan_span(file, 1, 1),
        });
    }
    Ok(models)
}

/// The native derives a generic template keeps; semantic construction and reflection stay in checked Incan records.
fn generic_derives(declaration: &NominalDeclaration) -> Result<Vec<String>, String> {
    let mut derives = Vec::new();
    for derive in &declaration.derives {
        if ["Default", "Validate", "FieldInfo", "IncanClass"].contains(&derive.as_str()) {
            continue;
        }
        let native = [
            "Clone",
            "Debug",
            "Display",
            "PartialEq",
            "Eq",
            "PartialOrd",
            "Ord",
            "Hash",
            "serde::Serialize",
            "serde::Deserialize",
        ];
        if !native.contains(&derive.as_str()) {
            return Err(format!("unsupported Body IR generic Loaf derive {derive}"));
        }
        derives.push(derive.clone());
    }
    let serde = derives
        .iter()
        .any(|derive| derive == "serde::Serialize" || derive == "serde::Deserialize");
    if serde && declaration.field_aliases.iter().any(Option::is_some) {
        return Err(format!(
            "unsupported Body IR generic Loaf serde field alias on {}",
            declaration.name
        ));
    }
    Ok(derives)
}

/// Translate a checked field structurally, preserving binders and exact nominal owners before native instantiation.
pub fn generic_field_type(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    ty: &IncanType,
    parameters: &[String],
) -> Result<GenericLayoutType, String> {
    match ty {
        IncanType::TypeVar(name) => {
            if !parameters.contains(name) {
                return Err(format!("invalid Body IR generic layout parameter {name}"));
            }
            Ok(GenericLayoutType::Parameter(name.clone()))
        }
        IncanType::Generic { base, args } => {
            let path = generic_layout_path(module, base)?;
            let mut applied = Vec::with_capacity(args.len());
            for argument in args {
                applied.push(generic_field_type(constructors, module, argument, parameters)?);
            }
            Ok(GenericLayoutType::Applied(path, applied))
        }
        IncanType::Tuple(elements) => {
            let mut fields = Vec::with_capacity(elements.len());
            for element in elements {
                fields.push(generic_field_type(constructors, module, element, parameters)?);
            }
            Ok(GenericLayoutType::Tuple(fields))
        }
        IncanType::Function { params, return_type } => {
            let mut signature = Vec::with_capacity(params.len() + 1);
            for parameter in params {
                signature.push(generic_callable_input(constructors, module, parameter, parameters)?);
            }
            signature.push(generic_field_type(constructors, module, return_type, parameters)?);
            Ok(GenericLayoutType::FunctionPointer(signature))
        }
        _ => Ok(GenericLayoutType::Concrete(native_source_type(
            constructors,
            &module.nominal_declarations,
            &module.enum_declarations,
            &[],
            ty,
            &[],
            "",
        )?)),
    }
}

/// Retain an owned declaration binder; closed inputs reuse the ordinary callable ABI policy.
///
/// Mutable and rest inputs whose storage depends on an open binder remain refused, as do unclosed aggregate inputs.
fn generic_callable_input(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    parameter: &IncanCallableParam,
    parameters: &[String],
) -> Result<GenericLayoutType, String> {
    if let IncanType::TypeVar(_) = parameter.ty {
        if parameter.kind != IncanCallableParamKind::Normal
            || parameter.is_mut
            || parameter.input_access != IncanInputAccess::Inferred
        {
            return Err("unsupported Body IR generic callable field input ownership".into());
        }
        return generic_field_type(constructors, module, &parameter.ty, parameters);
    }
    let component = callable_component(&parameter.ty)?;
    let storage = packed_input(
        component,
        parameter.kind,
        parameter.is_mut,
        parameter.native_owned,
        parameter.input_access,
    )?;
    Ok(GenericLayoutType::Concrete(storage))
}

/// Use the checked declaration's native item or its local carrier; collection heads have fixed native ADTs.
fn generic_layout_path(module: &BodyIrModule, base: &str) -> Result<String, String> {
    match collection_type(base) {
        Some(CollectionTypeId::List) => return Ok("std::vec::Vec".into()),
        Some(CollectionTypeId::Option) => return Ok("std::option::Option".into()),
        Some(CollectionTypeId::Result) => return Ok("std::result::Result".into()),
        _ => {}
    }
    if let Some(declaration) = module
        .nominal_declarations
        .iter()
        .find(|declaration| declaration.name == base)
    {
        let item = library_item(declaration);
        if !item.is_empty() {
            return Ok(item.to_string());
        }
        return Ok(format!("crate::{base}"));
    }
    if let Some(declaration) = module
        .enum_declarations
        .iter()
        .find(|declaration| declaration.carrier_name() == base)
    {
        let item = library_enum_item(declaration);
        if !item.is_empty() {
            return Ok(item.to_string());
        }
        return Ok(format!("crate::{base}"));
    }
    Err(format!("unsupported Body IR generic Loaf field owner {base}"))
}

/// Inventory checked template heads without selecting implementations or inferring bindings from source names.
pub fn referenced_templates(module: &BodyIrModule) -> Vec<String> {
    let mut referenced = Vec::new();
    for body in &module.bodies {
        retain_frame_templates(body, &mut referenced);
    }
    for declaration in &module.nominal_declarations {
        for field in &declaration.field_types {
            retain_type_templates(field, &mut referenced);
        }
    }
    for declaration in &module.type_alias_declarations {
        retain_type_templates(&declaration.ty, &mut referenced);
    }
    for declaration in &module.enum_declarations {
        for variant in &declaration.variants {
            for field in &variant.fields {
                retain_type_templates(field, &mut referenced);
            }
        }
    }
    for declaration in &module.static_declarations {
        if let StaticInitializer::Computed(body) = &declaration.initial {
            retain_frame_templates(body, &mut referenced);
        }
    }
    referenced
}

/// Template requirements from checked locals, including parameters, and the frame's return type.
fn retain_frame_templates(body: &Body, referenced: &mut Vec<String>) {
    retain_type_templates(&body.return_type, referenced);
    for local in &body.locals {
        retain_type_templates(&local.ty, referenced);
    }
}

/// Exact checked generic heads through the structural types a retained frame may instantiate.
fn retain_type_templates(ty: &IncanType, referenced: &mut Vec<String>) {
    match ty {
        IncanType::Generic { base, args } => {
            if !referenced.contains(base) {
                referenced.push(base.clone());
            }
            for argument in args {
                retain_type_templates(argument, referenced);
            }
        }
        IncanType::Tuple(elements) => {
            for element in elements {
                retain_type_templates(element, referenced);
            }
        }
        IncanType::Ref(inner) | IncanType::RefMut(inner) => retain_type_templates(inner, referenced),
        IncanType::NativeArray { element, .. } => retain_type_templates(element, referenced),
        IncanType::Function { params, return_type } => {
            for parameter in params {
                retain_type_templates(&parameter.ty, referenced);
            }
            retain_type_templates(return_type, referenced);
        }
        _ => {}
    }
}

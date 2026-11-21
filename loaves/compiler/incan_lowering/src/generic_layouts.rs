//! Generic model templates a library publishes and the template heads its frames reference.

use incan_mir_plan_native::GenericModelDeclaration;
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::{Body, BodyIrModule, StaticInitializer};

use crate::decls::library_item;
use crate::native_type_identity::ConstructorIndex;
use crate::nominal_instances::NominalInstance;
use crate::pending::untranslated;

/// Retain source model and class templates needed by consumers; linked Loaves own their existing ADTs.
pub fn generic_models(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    referenced: &[String],
    instances: &[NominalInstance],
    file: &str,
) -> Result<Vec<GenericModelDeclaration>, String> {
    let _ = (constructors, file);
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
        return Err(untranslated("generic_layouts::generic_models"));
    }
    Ok(Vec::new())
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

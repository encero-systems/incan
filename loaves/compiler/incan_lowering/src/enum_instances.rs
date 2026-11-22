//! Concrete enum payload layouts selected from checked Body IR declaration binders and uses.

use std::collections::HashMap;

use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::{BodyIrModule, EnumDeclaration};

use crate::nominal_frames::declaration_storage;
use crate::nominal_instances::nominal_symbol;
use crate::type_parameters::substitute;

/// One enum declaration and its closed checked arguments, with a dependency lineage for recursion refusal.
struct EnumInstance {
    declaration: EnumDeclaration,
    arguments: Vec<IncanType>,
    ancestors: Vec<String>,
}

/// Close the enum payload layouts a module's checked types use; open templates never become physical declarations.
pub fn specialized_enum_layouts(module: &BodyIrModule) -> Result<Vec<EnumDeclaration>, String> {
    let generics = generic_carriers(module);
    let mut instances: Vec<EnumInstance> = Vec::new();
    for ty in declaration_storage(module) {
        collect_type(module, &generics, &ty, &mut instances, &[])?;
    }
    for body in &module.bodies {
        for local in &body.locals {
            collect_type(module, &generics, &local.ty, &mut instances, &[])?;
        }
        collect_type(module, &generics, &body.return_type, &mut instances, &[])?;
    }
    for declaration in &module.nominal_declarations {
        for field in &declaration.field_types {
            collect_type(module, &generics, field, &mut instances, &[])?;
        }
    }
    let mut layouts: Vec<EnumDeclaration> = Vec::new();
    for declaration in &module.enum_declarations {
        if declaration.type_parameters.is_empty() {
            layouts.push(declaration.clone());
            for variant in &declaration.variants {
                for field in &variant.fields {
                    collect_type(module, &generics, field, &mut instances, &[])?;
                }
            }
        }
    }
    let mut cursor = 0;
    while cursor < instances.len() {
        let declaration = close_layout(&instances[cursor])?;
        let mut lineage = instances[cursor].ancestors.clone();
        lineage.push(instances[cursor].declaration.canonical.render_compact());
        cursor += 1;
        if layouts.iter().any(|retained| retained.name == declaration.name) {
            return Err(format!(
                "unsupported Body IR enum instance symbol collision {}",
                declaration.name
            ));
        }
        for variant in &declaration.variants {
            for field in &variant.fields {
                collect_type(module, &generics, field, &mut instances, &lineage)?;
            }
        }
        layouts.push(declaration);
    }
    Ok(layouts)
}

/// Replace generic enum templates in place with every closed layout selected from the joined context.
pub fn specialize_enums(module: &mut BodyIrModule) -> Result<(), String> {
    module.enum_declarations = specialized_enum_layouts(module)?;
    Ok(())
}

/// The first generic enum declaration of each carrier name, by its position: the one a closed use of that name
/// instantiates. A nongeneric declaration is never instantiated, so it carries no name here.
fn generic_carriers(module: &BodyIrModule) -> HashMap<String, usize> {
    let mut generics = HashMap::new();
    for (position, declaration) in module.enum_declarations.iter().enumerate() {
        if !declaration.type_parameters.is_empty() {
            generics.entry(declaration.carrier_name()).or_insert(position);
        }
    }
    generics
}

/// Visit checked aggregate positions, retaining only closed enum uses and their exact declared argument order.
fn collect_type(
    module: &BodyIrModule,
    generics: &HashMap<String, usize>,
    ty: &IncanType,
    instances: &mut Vec<EnumInstance>,
    ancestors: &[String],
) -> Result<(), String> {
    match ty {
        IncanType::Generic { base, args } => {
            for argument in args {
                collect_type(module, generics, argument, instances, ancestors)?;
            }
            if !closed_arguments(args) {
                return Ok(());
            }
            if let Some(declaration) = generics
                .get(base)
                .and_then(|position| module.enum_declarations.get(*position))
            {
                if declaration.type_parameters.len() != args.len() {
                    return Err(format!("unsupported Body IR enum type argument arity on {base}"));
                }
                if ancestors.contains(&declaration.canonical.render_compact()) {
                    return Err(format!(
                        "unsupported Body IR recursive enum payload instantiation on {base}"
                    ));
                }
                retain_instance(declaration, args.clone(), instances, ancestors);
                return Ok(());
            }
        }
        IncanType::Tuple(elements) => {
            for element in elements {
                collect_type(module, generics, element, instances, ancestors)?;
            }
        }
        IncanType::Ref(inner) | IncanType::RefMut(inner) => {
            return collect_type(module, generics, inner, instances, ancestors);
        }
        _ => {}
    }
    Ok(())
}

/// Deduplicate a layout by its canonical declaration and arguments, independently of a written alias.
fn retain_instance(
    declaration: &EnumDeclaration,
    arguments: Vec<IncanType>,
    instances: &mut Vec<EnumInstance>,
    ancestors: &[String],
) {
    if instances
        .iter()
        .any(|retained| retained.declaration.canonical == declaration.canonical && retained.arguments == arguments)
    {
        return;
    }
    instances.push(EnumInstance {
        declaration: declaration.clone(),
        arguments,
        ancestors: ancestors.to_vec(),
    });
}

/// Keep unresolved placeholders out of executable enum layouts, including nested aggregate arguments.
fn closed_arguments(arguments: &[IncanType]) -> bool {
    arguments.iter().all(|argument| match argument {
        IncanType::TypeVar(_)
        | IncanType::SelfType
        | IncanType::Unknown
        | IncanType::Infer
        | IncanType::Ref(_)
        | IncanType::RefMut(_) => false,
        IncanType::Generic { args, .. } => closed_arguments(args),
        IncanType::Tuple(elements) => closed_arguments(elements),
        _ => true,
    })
}

/// Substitute payload fields while keeping each source variant's declaration and canonical identities.
fn close_layout(instance: &EnumInstance) -> Result<EnumDeclaration, String> {
    let mut result = instance.declaration.clone();
    result.name = nominal_symbol(&instance.declaration.carrier_name(), &instance.arguments);
    result.native_type_arguments = instance.arguments.clone();
    result.variants.clear();
    for variant in &instance.declaration.variants {
        let mut closed = variant.clone();
        closed.fields.clear();
        for field in &variant.fields {
            closed.fields.push(substitute(
                field,
                &instance.declaration.type_parameters,
                &instance.arguments,
            )?);
        }
        result.variants.push(closed);
    }
    Ok(result)
}

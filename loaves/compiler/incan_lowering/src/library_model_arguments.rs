//! Native SDK model arguments retained from the exact checked nominal instances that scheduled their layouts.

use incan_mir_plan_native::{EnumDeclaration, PlanType};
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::{BodyIrModule, NominalDeclaration};

use crate::layout_types::native_source_type;
use crate::native_type_identity::ConstructorIndex;
use crate::nominal_instances::{NominalInstance, nominal_symbol};

/// Select the retained template of a local closed carrier, so its storage aliases the ADT published by this Loaf.
pub fn local_generic_item(declaration: &NominalDeclaration, instances: &[NominalInstance]) -> Result<String, String> {
    for instance in instances {
        if instance.declaration.canonical == declaration.canonical
            && nominal_symbol(&instance.declaration.name, &instance.arguments) == declaration.name
        {
            return Ok(format!("crate::{}", instance.declaration.name));
        }
    }
    Err("invalid Body IR local generic layout without its template".into())
}

/// Transport retained instance arguments against the enclosing layout pass's ordered union inventory.
pub fn library_model_arguments(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    declaration: &NominalDeclaration,
    instances: &[NominalInstance],
    unions: &[IncanType],
    intrinsics: &[EnumDeclaration],
) -> Result<Vec<PlanType>, String> {
    let native_item = declaration.native_item.clone();
    if declaration.type_parameters.is_empty() {
        return native_arguments(constructors, module, declaration, unions, intrinsics)
            .map_err(|reason| format!("{reason} in native arguments of {native_item}"));
    }
    let mut selected: Option<&NominalInstance> = None;
    for instance in instances {
        if instance.declaration.canonical != declaration.canonical {
            continue;
        }
        if nominal_symbol(&instance.declaration.name, &instance.arguments) != declaration.name {
            continue;
        }
        if instance.arguments.len() != declaration.type_parameters.len() {
            return Err("the standard library model type argument count is unsupported".into());
        }
        match selected {
            None => selected = Some(instance),
            Some(previous) => {
                if previous.arguments != instance.arguments {
                    return Err("the standard library model has conflicting type arguments".into());
                }
            }
        }
    }
    let Some(instance) = selected else {
        return Err(format!(
            "standard library model {} has no checked instance type arguments",
            declaration.name
        ));
    };
    instance_native_arguments(constructors, module, declaration, instance, unions, intrinsics)
        .map_err(|reason| format!("{reason} in native instance arguments of {native_item}"))
}

fn native_arguments(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    declaration: &NominalDeclaration,
    unions: &[IncanType],
    intrinsics: &[EnumDeclaration],
) -> Result<Vec<PlanType>, String> {
    declaration
        .native_type_arguments
        .iter()
        .map(|argument| {
            native_source_type(
                constructors,
                &module.nominal_declarations,
                &module.enum_declarations,
                unions,
                argument,
                intrinsics,
                "",
            )
        })
        .collect()
}

/// Close native templates using the caller's union inventory and preserve each argument's field storage.
fn instance_native_arguments(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    declaration: &NominalDeclaration,
    instance: &NominalInstance,
    unions: &[IncanType],
    intrinsics: &[EnumDeclaration],
) -> Result<Vec<PlanType>, String> {
    let mut templates = instance.declaration.native_type_arguments.clone();
    if templates.is_empty() {
        templates = instance
            .declaration
            .type_parameters
            .iter()
            .map(|name| IncanType::TypeVar(name.clone()))
            .collect();
    }
    let closed = if declaration.native_type_arguments.is_empty() {
        instance.arguments.clone()
    } else {
        declaration.native_type_arguments.clone()
    };
    if templates.len() != closed.len() {
        return Err("invalid Body IR native owner argument templates".into());
    }
    let mut arguments = Vec::with_capacity(closed.len());
    for (index, argument) in closed.iter().enumerate() {
        let ty = native_source_type(
            constructors,
            &module.nominal_declarations,
            &module.enum_declarations,
            unions,
            argument,
            intrinsics,
            "",
        )?;
        arguments.push(stored_argument_type(
            declaration,
            &instance.declaration,
            &templates[index],
            ty,
        )?);
    }
    Ok(arguments)
}

/// Transport a stored generic callable's owning ABI into its native owner argument, using the same field facts.
fn stored_argument_type(
    declaration: &NominalDeclaration,
    template: &NominalDeclaration,
    argument: &IncanType,
    ty: PlanType,
) -> Result<PlanType, String> {
    for (slot, field) in template.field_types.iter().enumerate() {
        if !declaration.owns_callable_field(slot as i64) {
            continue;
        }
        match field {
            IncanType::TypeVar(_) => {
                if field != argument {
                    continue;
                }
                return match ty {
                    PlanType::FunctionPointer(signature) => Ok(PlanType::Closure(signature)),
                    _ => Err("invalid Body IR stored callable owner argument".into()),
                };
            }
            _ => return Err("invalid Body IR stored callable field template".into()),
        }
    }
    Ok(ty)
}

//! Source-body symbols for checker-selected nominal display methods.

use incan_mir_plan_native::{CalleeKind, EnumDeclaration, PlanType};
use incan_semantics_core::body_ir::BodyIrModule;
use incan_semantics_core::{IncanPrimitiveType, IncanType};

use crate::classes::{frame_type, function_name};
use crate::layout_types::native_source_type;
use crate::native_type_identity::ConstructorIndex;
use crate::program_context::DisplayFunction;

/// Resolve only the retained selected identity with its exact shared receiver and string-returning signature.
pub fn display_functions(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    intrinsics: &[EnumDeclaration],
) -> Result<Vec<DisplayFunction>, String> {
    let mut functions: Vec<DisplayFunction> = Vec::new();
    let nominals = &module.nominal_declarations;
    let enums = &module.enum_declarations;
    for (owner, declaration) in nominals.iter().enumerate() {
        let Some(selected) = &declaration.display_method else {
            continue;
        };
        let receiver_type = PlanType::ModelRef(owner as i64, declaration.name.clone());
        for body in &module.bodies {
            if body.canonical.as_ref() != Some(selected) || !body.type_parameters.is_empty() {
                continue;
            }
            if body.params.len() != 1 || body.locals.is_empty() || !string_result(&body.return_type) {
                continue;
            }
            if frame_type(constructors, nominals, enums, &body.locals[0], intrinsics, "")? != receiver_type {
                continue;
            }
            if functions.iter().any(|retained| retained.owner == owner as i64) {
                return Err(format!(
                    "unsupported Body IR ambiguous checked display body on {}",
                    declaration.name
                ));
            }
            functions.push(DisplayFunction {
                owner: owner as i64,
                callee: CalleeKind::Planned(function_name(module, body)),
            });
        }
        let mut native_display = false;
        for delegation in &module.loaf_callables {
            if delegation.canonical != *selected {
                continue;
            }
            if !delegation.type_parameters.is_empty() || !delegation.native_type_arguments.is_empty() {
                continue;
            }
            if delegation.parameters.len() != 1 || !string_result(&delegation.return_type) {
                continue;
            }
            let receiver = native_source_type(
                constructors,
                nominals,
                enums,
                &[],
                &delegation.parameters[0],
                intrinsics,
                "",
            )?;
            if receiver != receiver_type {
                continue;
            }
            let Some(path) = delegation.rust_path() else {
                return Err("the selected standard library display method has no checked native identity".into());
            };
            if native_display {
                return Err(format!(
                    "unsupported Body IR ambiguous checked display body on {}",
                    declaration.name
                ));
            }
            native_display = true;
            // One selected source body and its published native entry describe the same method. Prefer the checked
            // body, but validate every native entry before choosing either form.
            if !functions.iter().any(|retained| retained.owner == owner as i64) {
                functions.push(DisplayFunction {
                    owner: owner as i64,
                    callee: CalleeKind::External(path),
                });
            }
        }
    }
    Ok(functions)
}

/// Require the exact checked string result without permitting an implicit formatting conversion.
fn string_result(ty: &IncanType) -> bool {
    matches!(ty, IncanType::Primitive(IncanPrimitiveType::Str))
}

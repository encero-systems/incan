//! Canonical normal-enum layouts and identity-checked native construction and pattern admission.

use incan_mir_plan_native::PlanType;
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::EnumDeclaration;

use crate::nominal_instances::nominal_symbol;
use crate::symbols::model_symbol;

/// A retained source enum, recognized before the model and scalar resolver runs.
pub fn enum_type(declarations: &[EnumDeclaration], ty: &IncanType) -> Option<PlanType> {
    let name = match ty {
        IncanType::Named(name) => name.clone(),
        IncanType::Generic { base, args } => nominal_symbol(base, args),
        _ => return None,
    };
    declarations
        .iter()
        .position(|declaration| declaration.carrier_name() == name)
        .map(|index| PlanType::Enum(index as i64, model_symbol(&name)))
}

use incan_mir_plan_native::EnumDeclaration as PlanEnumDeclaration;
use incan_semantics_core::body_ir::BodyIrModule;

use crate::native_type_identity::ConstructorIndex;
use crate::pending::untranslated;

/// Copy checked source-order variants and derives into the native declaration plan.
pub fn lower_enum_declarations(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    file: &str,
    intrinsics: &[PlanEnumDeclaration],
) -> Result<Vec<PlanEnumDeclaration>, String> {
    let _ = (constructors, file, intrinsics);
    if !module.enum_declarations.is_empty() || !module.nominal_declarations.is_empty() {
        return Err(untranslated("enums::lower_enum_declarations"));
    }
    Ok(Vec::new())
}

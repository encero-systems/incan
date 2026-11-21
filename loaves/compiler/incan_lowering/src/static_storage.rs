//! Computed and literal module statics planned as guarded adapter cells (RFC 052).

use incan_mir_plan_native::{EnumDeclaration, PlanType, StaticCell};
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::{BodyIrModule, StaticInitializer};

use crate::closures::callable_copy_type;
use crate::lowering::Inventories;
use crate::numerics::numeric_carrier;
use crate::pending::untranslated;
use crate::program_context::ProgramContext;

/// Plan computed statics and effect-free literal lists or dictionaries as guarded adapter cells.
///
/// A continuation starts at the first newly resolved declaration so preserved authored cells are never lowered twice.
#[allow(clippy::too_many_arguments)]
pub fn lower_computed_statics(
    module: &BodyIrModule,
    program: &mut ProgramContext,
    unions: &[IncanType],
    sources: &[String],
    files: &[String],
    owners: &[String],
    intrinsics: &mut Vec<EnumDeclaration>,
    inventories: &mut Inventories,
    start_slot: usize,
) -> Result<Vec<StaticCell>, String> {
    let _ = (program, unions, sources, files, owners, intrinsics, inventories);
    let cells = Vec::new();
    for declaration in module.static_declarations.iter().skip(start_slot) {
        if module
            .decorated_bindings
            .iter()
            .any(|binding| binding.original_body.canonical.as_ref() == Some(&declaration.canonical))
        {
            return Err(untranslated("static_storage::lower_computed_statics original callable"));
        }
        match declaration.initial {
            StaticInitializer::Computed(_) => {
                return Err(untranslated(
                    "static_storage::lower_computed_statics computed initializer",
                ));
            }
            StaticInitializer::List(_) | StaticInitializer::Dict(_) => {
                return Err(untranslated("static_storage::lower_literal_static"));
            }
            _ => {}
        }
    }
    Ok(cells)
}

/// Whether a loaded static value owns resources its frame must drop, rather than being a plain copy.
pub fn snapshot_owns(ty: &PlanType) -> bool {
    if numeric_carrier(ty) || matches!(ty, PlanType::Bool | PlanType::Unit | PlanType::FrozenStr) {
        return false;
    }
    !callable_copy_type(ty)
}

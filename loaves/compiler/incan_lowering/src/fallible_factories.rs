//! Fallible iterator factories and the callback storage their carriers retain.

use incan_mir_plan_native::{ExternalFunction, Function, ModelDeclaration};
use incan_semantics_core::body_ir::BodyIrModule;

use crate::pending::untranslated;

/// The runtime path of the fallible error-map factory in the reference runtime.
pub const FALLIBLE_MAP_ERR: &str = "incan_native_runtime::fallible_map_err";

/// Carry an authenticated factory's proven shared callback ABI into its concrete return carrier and generic call slot.
pub fn retain_factory_callable_storage(
    module: &BodyIrModule,
    functions: &mut Vec<Function>,
    models: &mut Vec<ModelDeclaration>,
    externals: &mut Vec<ExternalFunction>,
) -> Result<(), String> {
    let _ = (module, functions, models);
    if externals.iter().any(|external| external.path == FALLIBLE_MAP_ERR) {
        return Err(untranslated("fallible_factories::retain_factory_callable_storage"));
    }
    Ok(())
}

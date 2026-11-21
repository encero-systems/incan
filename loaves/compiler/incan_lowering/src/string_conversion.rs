//! Selected raw string-conversion chains rooted in the checked TryFrom contract.

use std::sync::Arc;

use incan_mir_plan_native::{EnumDeclaration, ExternalFunction, Function};
use incan_semantics_core::body_ir::BodyIrModule;

use crate::native_type_identity::ConstructorIndex;
use crate::pending::untranslated;
use crate::string_conversion_contracts::conversion_trait;

/// Plan selected raw chain roots from `start`, leaving a continuation's unchanged roots intact.
pub fn lower_string_conversions(
    constructors: &Arc<ConstructorIndex>,
    module: &BodyIrModule,
    file: &str,
    intrinsics: &mut Vec<EnumDeclaration>,
    start: usize,
) -> Result<(Vec<Function>, Vec<ExternalFunction>), String> {
    let _ = (constructors, file, intrinsics);
    if conversion_trait(module)?.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    if module.nominal_declarations.len() > start {
        return Err(untranslated("string_conversion::lower_string_conversions"));
    }
    Ok((Vec::new(), Vec::new()))
}

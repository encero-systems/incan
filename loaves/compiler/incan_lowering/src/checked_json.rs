//! Checked serde reconstruction rooted in exact source hook bodies and authored Incan error mapping.

use std::sync::Arc;

use incan_mir_plan_native::{EnumDeclaration, ExternalFunction, Function};
use incan_semantics_core::SemanticSourceTargetKind;
use incan_semantics_core::body_ir::BodyIrModule;

use crate::decls::library_item;
use crate::native_type_identity::ConstructorIndex;
use crate::nominal_instances::NominalInstance;
use crate::pending::untranslated;
use crate::type_parameters::Instance;

/// Close checked ingress roots from `start`, so a resumed plan visits only newly resolved declarations.
pub fn lower_checked_deserialize(
    constructors: &Arc<ConstructorIndex>,
    module: &BodyIrModule,
    instances: &[NominalInstance],
    file: &str,
    intrinsics: &mut Vec<EnumDeclaration>,
    start: usize,
) -> Result<(Vec<Function>, Vec<ExternalFunction>, Vec<Instance>), String> {
    let _ = (constructors, instances, file, intrinsics);
    for declaration in module.nominal_declarations.iter().skip(start) {
        if declaration.canonical.kind != SemanticSourceTargetKind::Newtype || !library_item(declaration).is_empty() {
            continue;
        }
        return Err(untranslated("checked_json::lower_checked_deserialize"));
    }
    Ok((Vec::new(), Vec::new(), Vec::new()))
}

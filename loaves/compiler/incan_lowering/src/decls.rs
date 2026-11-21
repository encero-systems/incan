//! Model and enum layout admission and publication.

use incan_semantics_core::body_ir::{EnumDeclaration, NominalDeclaration};

/// The compiled item that owns a layout a linked Loaf declares, or nothing for a layout declared here.
pub fn library_item(declaration: &NominalDeclaration) -> &str {
    &declaration.native_item
}

/// The compiled item that owns an enum a linked Loaf declares, or nothing for an enum declared here.
pub fn library_enum_item(declaration: &EnumDeclaration) -> &str {
    &declaration.native_item
}

use incan_mir_plan_native::{EnumDeclaration as PlanEnumDeclaration, ModelDeclaration};
use incan_semantics_core::body_ir::BodyIrModule;

use crate::native_type_identity::ConstructorIndex;
use crate::nominal_instances::NominalInstance;
use crate::pending::untranslated;

/// Expose canonical native ABI items for a compiled Loaf; its checked manifest retains source visibility.
pub fn publish_local_layouts(models: &mut [ModelDeclaration], enums: &mut [PlanEnumDeclaration]) {
    for model in models.iter_mut() {
        if model.library_item.is_empty() {
            model.public = true;
        }
    }
    for declaration in enums.iter_mut() {
        if declaration.library_item.is_empty() {
            declaration.public = true;
        }
    }
}

/// Retain checked field layout and derives for admitted plain model, class, and newtype declarations.
pub fn lower_model_declarations(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    file: &str,
    intrinsics: &mut Vec<PlanEnumDeclaration>,
    nominal_instances: &[NominalInstance],
    publish_generics: bool,
    start: usize,
) -> Result<Vec<ModelDeclaration>, String> {
    let _ = (constructors, file, intrinsics, nominal_instances, publish_generics);
    if !module.nominal_declarations.is_empty() || !module.enum_declarations.is_empty() || start != 0 {
        return Err(untranslated("decls::lower_model_declarations"));
    }
    Ok(Vec::new())
}

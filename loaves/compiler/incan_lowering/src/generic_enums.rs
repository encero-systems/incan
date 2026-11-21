//! The generic enum templates a library publishes, with each closed instance aliased to its template.

use incan_mir_plan_native::{EnumDeclaration, GenericEnumDeclaration};
use incan_semantics_core::body_ir::{BodyIrModule, EnumDeclaration as SourceEnumDeclaration};

use crate::generic_layouts::referenced_templates;
use crate::native_type_identity::ConstructorIndex;
use crate::pending::untranslated;

/// The generic enum templates a library publishes; each closed instance's entry in `layouts` becomes their alias.
pub fn generic_enums(
    constructors: &ConstructorIndex,
    source: &BodyIrModule,
    closed: &BodyIrModule,
    layouts: &[EnumDeclaration],
    intrinsics: &[EnumDeclaration],
    file: &str,
) -> Result<Vec<GenericEnumDeclaration>, String> {
    let _ = (constructors, layouts, intrinsics, file);
    let referenced = referenced_templates(source);
    for declaration in &source.enum_declarations {
        if published(declaration, closed, &referenced) {
            return Err(untranslated("generic_enums::generic_enums"));
        }
    }
    Ok(Vec::new())
}

/// Whether a source enum gets a template: it is generic, defined in Incan, and exported, lowered or referenced.
fn published(declaration: &SourceEnumDeclaration, closed: &BodyIrModule, referenced: &[String]) -> bool {
    if declaration.type_parameters.is_empty() || !declaration.native_item.is_empty() {
        return false;
    }
    if declaration.public || referenced.contains(&declaration.carrier_name()) {
        return true;
    }
    closed
        .enum_declarations
        .iter()
        .any(|instance| instance.canonical == declaration.canonical)
}

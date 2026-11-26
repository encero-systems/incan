//! Authenticate primitive-owned metadata declarations before planning generated reflection.

use incan_semantics_core::body_ir::NominalDeclaration;
use incan_semantics_core::{SemanticSourceTargetKind, SymbolOrigin};

/// Only the checked primitive model supplies the generated metadata layout; an equal name grants nothing.
pub fn is_field_metadata_carrier(declaration: &NominalDeclaration) -> bool {
    let identity = &declaration.canonical;
    if identity.kind != SemanticSourceTargetKind::Model || identity.declaration_name != "FieldInfo" {
        return false;
    }
    match &identity.origin {
        SymbolOrigin::Package { library, module_path } => {
            library == "incan_stdlib_primitives" && module_path == &["primitives", "reflection"]
        }
        _ => false,
    }
}

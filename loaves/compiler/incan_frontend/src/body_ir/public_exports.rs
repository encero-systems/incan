//! The names a module re-exports with `pub from ... import`, bound to the declarations import resolution proved.
//!
//! A module's own public declarations need no entry here: a library unit emits them under their source names. Only a
//! re-export publishes a declaration under a name its own module does not declare.

use incan_semantics_core::body_ir::PublicExport;

use crate::ast;
use crate::typechecker::TypeCheckInfo;

/// Collect every public `from ... import` binding of `program` whose target import resolution proved.
///
/// Source, `pub::` library and `rust::` imports re-export the same way. A binding import resolution did not prove is
/// left out rather than recovered from its written path; a consumer that needs it refuses on the missing name.
pub(super) fn collect(program: &ast::Program, type_info: &TypeCheckInfo) -> Vec<PublicExport> {
    let mut exports = Vec::new();
    for declaration in &program.declarations {
        let ast::Declaration::Import(import) = &declaration.node else {
            continue;
        };
        if import.visibility != ast::Visibility::Public {
            continue;
        }
        let items = match &import.kind {
            ast::ImportKind::From { items, .. }
            | ast::ImportKind::PubFrom { items, .. }
            | ast::ImportKind::RustFrom { items, .. } => items,
            _ => continue,
        };
        for item in items {
            let name = item.alias.as_ref().unwrap_or(&item.name);
            if let Some(target) = type_info.resolved_import_identity(name) {
                exports.push(PublicExport {
                    name: name.clone(),
                    target: target.clone(),
                });
            }
        }
    }
    exports
}

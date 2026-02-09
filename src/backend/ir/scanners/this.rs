//! Detect `import this` usage.

use crate::frontend::ast::{Declaration, ImportKind, Program};
use incan_core::lang::stdlib;

/// Check for `import this` usage.
pub fn check_for_this_import(program: &Program) -> bool {
    for decl in &program.declarations {
        let Declaration::Import(import) = &decl.node else {
            continue;
        };
        let ImportKind::Module(path) = &import.kind else {
            continue;
        };
        if path.segments.len() == 1 && path.segments[0] == stdlib::STDLIB_THIS {
            return true;
        }
    }
    false
}

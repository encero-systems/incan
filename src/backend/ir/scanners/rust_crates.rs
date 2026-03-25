//! Rust interop crate collection (`import rust::...` / `from rust::...`).

use std::collections::HashSet;

use crate::frontend::ast::{Declaration, ImportKind, Program};
use incan_core::lang::stdlib;

/// Collect Rust crates imported via `import rust::` or `from rust::`.
pub fn collect_rust_crates(program: &Program) -> HashSet<String> {
    let mut crates = HashSet::new();
    for decl in &program.declarations {
        if let Declaration::Import(import) = &decl.node {
            match &import.kind {
                ImportKind::RustCrate { crate_name, .. } | ImportKind::RustFrom { crate_name, .. }
                    if crate_name != stdlib::STDLIB_ROOT =>
                {
                    crates.insert(crate_name.clone());
                }
                _ => {}
            }
        }
    }
    crates
}

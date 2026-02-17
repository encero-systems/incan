//! Runtime requirement detection — fully registry-driven.
//!
//! Instead of hardcoding module names or specific keyword IDs, these scanners ask the semantics registry whether each
//! import and each surface modifier implies a particular runtime requirement.

use crate::ast::{Declaration, ImportKind, Program, SurfaceModifier};
use incan_core::lang::stdlib::STDLIB_ROOT;
use incan_semantics_core::{RuntimeRequirement, SurfaceSemanticsRegistry};

/// Detect whether the async runtime is required for the given program.
///
/// Two sources of async requirement are checked:
///
/// 1. **Imports**: `import std.<module>` or `from std.<module> import ...` where the registry says that module requires
///    async runtime.
/// 2. **Declaration modifiers**: `async def` or `async` on methods, where the registry says the surface modifier
///    requires async runtime.
pub fn needs_async_runtime(program: &Program, registry: &SurfaceSemanticsRegistry<'_>) -> bool {
    // ---- Check imports ----
    for decl in &program.declarations {
        let Declaration::Import(import) = &decl.node else {
            continue;
        };
        let segments = match &import.kind {
            ImportKind::Module(path) => &path.segments,
            ImportKind::From { module, .. } => &module.segments,
            _ => continue,
        };
        if segments.len() >= 2
            && segments[0] == STDLIB_ROOT
            && registry.import_runtime_requirement(&segments[1]) == RuntimeRequirement::AsyncRuntime
        {
            return true;
        }
    }

    // ---- Check declaration-level surface modifiers (async def, async methods) ----
    let has_async_modifier = |modifiers: &[SurfaceModifier]| -> bool {
        modifiers
            .iter()
            .any(|m| registry.modifier_runtime_requirement(&m.key) == RuntimeRequirement::AsyncRuntime)
    };

    program.declarations.iter().any(|decl| match &decl.node {
        Declaration::Function(f) => has_async_modifier(&f.surface_modifiers),
        Declaration::Model(m) => m
            .methods
            .iter()
            .any(|method| has_async_modifier(&method.node.surface_modifiers)),
        Declaration::Class(c) => c
            .methods
            .iter()
            .any(|method| has_async_modifier(&method.node.surface_modifiers)),
        _ => false,
    })
}

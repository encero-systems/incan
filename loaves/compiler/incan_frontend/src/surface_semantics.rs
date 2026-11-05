//! Unified surface-semantics metadata for soft keywords and decorators.
//!
//! This module centralizes import-driven activation and feature-key routing for language-surface features. It is
//! intentionally lightweight so parser/typechecker/lowering can share one source of truth.

use std::collections::HashSet;

use crate::ast::{Declaration, Expr, ImportKind, Program};
use crate::ast_walk;
use incan_semantics_core::SurfaceFeatureKey;

use crate::semantics_registry::semantics_registry;

/// Shared context for import-driven surface semantics.
#[derive(Debug, Clone, Default)]
pub struct SurfaceContext {
    /// Normalized module imports (`std.testing`, `std.async`, ...).
    imported_modules: HashSet<String>,
}

impl SurfaceContext {
    /// Build surface context from a parsed program.
    pub fn from_program(program: &Program) -> Self {
        let mut imported_modules = HashSet::new();

        for decl in &program.declarations {
            let Declaration::Import(import_decl) = &decl.node else {
                continue;
            };
            match &import_decl.kind {
                ImportKind::Module(path) => {
                    imported_modules.insert(path.segments.join("."));
                }
                ImportKind::From { module, .. } => {
                    imported_modules.insert(module.segments.join("."));
                }
                _ => {}
            }
        }

        Self { imported_modules }
    }

    pub fn has_imported_module(&self, module_path: &[String]) -> bool {
        self.imported_modules.contains(&module_path.join("."))
    }

    pub fn decorator_feature_for_path(&self, resolved_path: &[String]) -> Option<SurfaceFeatureKey> {
        semantics_registry().decorator_feature_for_path(resolved_path)
    }
}

/// Return whether the program may use the ambient `std.logging` logger binding.
///
/// This deliberately detects the binding candidate, not a fixed list of `Logger` methods. The typechecker still lets
/// local or imported `log` bindings shadow the ambient value; this scan only ensures the stdlib module is available
/// when a file contains an unqualified `log` expression.
pub fn uses_ambient_log_surface(program: &Program) -> bool {
    ast_walk::any_expr_in_program(program, |expr| match expr {
        Expr::Ident(name) => name == "log",
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::SurfaceContext;
    use crate::{lexer, parser};
    use incan_semantics_core::{DecoratorFeature, SurfaceFeatureKey};

    #[test]
    fn records_async_module_imports() -> Result<(), String> {
        let source = "import std.async\n";
        let tokens = lexer::lex(source).map_err(|e| format!("{e:?}"))?;
        let program = parser::parse(&tokens).map_err(|e| format!("{e:?}"))?;
        let context = SurfaceContext::from_program(&program);
        if !context.has_imported_module(&["std".to_string(), "async".to_string()]) {
            return Err("expected `std.async` to be recorded as an imported module".to_string());
        }
        Ok(())
    }

    #[test]
    fn classifies_stdlib_decorator_functions() {
        let context = SurfaceContext::default();
        let feature =
            context.decorator_feature_for_path(&["std".to_string(), "future".to_string(), "describe".to_string()]);
        assert_eq!(
            feature,
            Some(SurfaceFeatureKey::Decorator(DecoratorFeature::StdlibDecoratorFunction))
        );
    }

    #[test]
    fn detects_ambient_log_binding_candidates() -> Result<(), String> {
        let source = "def run() -> None:\n    log.info(\"started\")\n";
        let tokens = lexer::lex(source).map_err(|e| format!("{e:?}"))?;
        let program = parser::parse(&tokens).map_err(|e| format!("{e:?}"))?;
        if !super::uses_ambient_log_surface(&program) {
            return Err("expected ambient log candidate to activate std.logging".to_string());
        }
        Ok(())
    }
}

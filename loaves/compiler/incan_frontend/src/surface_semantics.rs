//! Unified surface-semantics metadata for soft keywords and decorators.
//!
//! This module centralizes import-driven activation and feature-key routing for language-surface features. It is
//! intentionally lightweight so parser/typechecker/lowering can share one source of truth.

use std::collections::HashSet;

use crate::ast::{Declaration, ImportKind, Program};
use incan_lang::lang::keywords::KeywordId;
use incan_lang::lang::stdlib;
use incan_semantics_core::SurfaceFeatureKey;

use crate::semantics_registry::semantics_registry;

/// Shared context for import-driven surface semantics.
#[derive(Debug, Clone, Default)]
pub struct SurfaceContext {
    active_soft_keywords: HashSet<KeywordId>,
    /// Normalized module imports (`std.testing`, `std.async`, ...).
    imported_modules: HashSet<String>,
}

impl SurfaceContext {
    /// Build surface context from a parsed program.
    pub fn from_program(program: &Program) -> Self {
        let mut active_soft_keywords = HashSet::new();
        let mut imported_modules = HashSet::new();

        for decl in &program.declarations {
            let Declaration::Import(import_decl) = &decl.node else {
                continue;
            };
            match &import_decl.kind {
                ImportKind::Module(path) => {
                    imported_modules.insert(path.segments.join("."));
                    for kw in stdlib::soft_keywords_for_import(&path.segments) {
                        active_soft_keywords.insert(kw);
                    }
                }
                ImportKind::From { module, .. } => {
                    imported_modules.insert(module.segments.join("."));
                    for kw in stdlib::soft_keywords_for_import(&module.segments) {
                        active_soft_keywords.insert(kw);
                    }
                }
                _ => {}
            }
        }

        Self {
            active_soft_keywords,
            imported_modules,
        }
    }

    pub fn is_soft_keyword_active(&self, keyword: KeywordId) -> bool {
        self.active_soft_keywords.contains(&keyword)
    }

    pub fn has_imported_module(&self, module_path: &[String]) -> bool {
        self.imported_modules.contains(&module_path.join("."))
    }

    pub fn soft_keyword_feature(&self, keyword: KeywordId) -> Option<SurfaceFeatureKey> {
        let registry = semantics_registry();
        registry
            .statement_feature_for_soft_keyword(keyword)
            .or_else(|| registry.expression_feature_for_soft_keyword(keyword))
            .or_else(|| registry.modifier_feature_for_soft_keyword(keyword))
    }

    pub fn decorator_feature_for_path(&self, resolved_path: &[String]) -> Option<SurfaceFeatureKey> {
        semantics_registry().decorator_feature_for_path(resolved_path)
    }
}

#[cfg(test)]
mod tests {
    use super::SurfaceContext;
    use crate::{lexer, parser};
    use incan_lang::lang::keywords::KeywordId;
    use incan_semantics_core::{DecoratorFeature, SurfaceFeatureKey};

    #[test]
    fn async_imports_do_not_activate_language_keywords() -> Result<(), String> {
        let source = "import std.async\n";
        let tokens = lexer::lex(source).map_err(|e| format!("{e:?}"))?;
        let program = parser::parse(&tokens).map_err(|e| format!("{e:?}"))?;
        let context = SurfaceContext::from_program(&program);
        if context.is_soft_keyword_active(KeywordId::Async) {
            return Err("expected `async` to remain language-owned after `import std.async`".to_string());
        }
        if context.is_soft_keyword_active(KeywordId::Await) {
            return Err("expected `await` to remain language-owned after `import std.async`".to_string());
        }
        let async_feature = context.soft_keyword_feature(KeywordId::Async);
        if async_feature != Some(SurfaceFeatureKey::SoftKeyword(KeywordId::Async)) {
            return Err("expected the async surface feature key to remain registered".to_string());
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
}

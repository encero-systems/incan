//! Unified surface-semantics metadata for soft keywords and decorators.
//!
//! This module centralizes import-driven activation and feature-key routing for language-surface features.
//! It is intentionally lightweight so parser/typechecker/lowering can share one source of truth.

use std::collections::{HashMap, HashSet};

use crate::frontend::ast::{Declaration, Expr, ImportKind, Program};
use crate::frontend::ast_walk;
use crate::frontend::decorator_resolution;
use incan_core::lang::keywords::KeywordId;
use incan_core::lang::stdlib;
use incan_semantics_core::SurfaceFeatureKey;

use crate::semantics_registry::semantics_registry;

/// Shared context for import-driven surface semantics.
#[derive(Debug, Clone, Default)]
pub struct SurfaceContext {
    active_soft_keywords: HashSet<KeywordId>,
    /// Normalized module imports (`std.testing`, `std.async`, ...).
    imported_modules: HashSet<String>,
    import_aliases: HashMap<String, Vec<String>>,
}

impl SurfaceContext {
    /// Build surface context from a parsed program.
    pub fn from_program(program: &Program) -> Self {
        let mut active_soft_keywords = HashSet::new();
        let mut imported_modules = HashSet::new();
        let import_aliases = decorator_resolution::collect_import_aliases(program);

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
            import_aliases,
        }
    }

    pub fn is_soft_keyword_active(&self, keyword: KeywordId) -> bool {
        self.active_soft_keywords.contains(&keyword)
    }

    pub fn import_aliases(&self) -> &HashMap<String, Vec<String>> {
        &self.import_aliases
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

/// Return whether `method` is one of the std.logging event methods.
pub(crate) fn is_std_logging_event_method(method: &str) -> bool {
    matches!(method, "trace" | "debug" | "info" | "warning" | "error" | "critical")
}

/// Structured field value families accepted by source-defined `std.logging.Logger` methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StdLoggingFieldValueKind {
    TelemetryValue,
    String,
    Bool,
    Int,
    Float,
    None,
}

const STD_LOGGING_FIELD_VALUE_KINDS: &[StdLoggingFieldValueKind] = &[
    StdLoggingFieldValueKind::TelemetryValue,
    StdLoggingFieldValueKind::String,
    StdLoggingFieldValueKind::Bool,
    StdLoggingFieldValueKind::Int,
    StdLoggingFieldValueKind::Float,
    StdLoggingFieldValueKind::None,
];

/// Return the structured field value families accepted by source-defined `std.logging.Logger` methods.
pub(crate) fn std_logging_field_value_kinds() -> &'static [StdLoggingFieldValueKind] {
    STD_LOGGING_FIELD_VALUE_KINDS
}

/// Return whether `method` is callable through the ambient std.logging `log` surface.
pub(crate) fn is_std_logging_ambient_method(method: &str) -> bool {
    is_std_logging_event_method(method) || matches!(method, "is_enabled" | "child" | "bind")
}

/// Return whether the program uses ambient `log.<method>(...)`.
pub(crate) fn uses_ambient_log_surface(program: &Program) -> bool {
    ast_walk::any_expr_in_program(program, |expr| match expr {
        Expr::MethodCall(receiver, method, _, _) => {
            matches!(&receiver.node, Expr::Ident(name) if name == "log")
                && is_std_logging_ambient_method(method.as_str())
        }
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        StdLoggingFieldValueKind, SurfaceContext, is_std_logging_ambient_method, is_std_logging_event_method,
        std_logging_field_value_kinds,
    };
    use crate::frontend::{lexer, parser};
    use incan_core::lang::keywords::KeywordId;
    use incan_semantics_core::{DecoratorFeature, SurfaceFeatureKey};

    #[test]
    fn activates_async_soft_keywords_from_imports() -> Result<(), String> {
        let source = "import std.async\n";
        let tokens = lexer::lex(source).map_err(|e| format!("{e:?}"))?;
        let program = parser::parse(&tokens).map_err(|e| format!("{e:?}"))?;
        let context = SurfaceContext::from_program(&program);
        if !context.is_soft_keyword_active(KeywordId::Async) {
            return Err("expected `async` to be activated by `import std.async`".to_string());
        }
        if !context.is_soft_keyword_active(KeywordId::Await) {
            return Err("expected `await` to be activated by `import std.async`".to_string());
        }
        let async_feature = context.soft_keyword_feature(KeywordId::Async);
        if async_feature != Some(SurfaceFeatureKey::SoftKeyword(KeywordId::Async)) {
            return Err("expected async soft-keyword feature key to be registered".to_string());
        }
        Ok(())
    }

    #[test]
    fn classifies_stdlib_decorator_functions() {
        let context = SurfaceContext::default();
        let feature =
            context.decorator_feature_for_path(&["std".to_string(), "testing".to_string(), "parametrize".to_string()]);
        assert_eq!(
            feature,
            Some(SurfaceFeatureKey::Decorator(DecoratorFeature::StdlibDecoratorFunction))
        );
    }

    #[test]
    fn classifies_std_logging_methods() {
        assert!(is_std_logging_event_method("info"));
        assert!(is_std_logging_ambient_method("bind"));
        assert!(!is_std_logging_event_method("bind"));
        assert!(!is_std_logging_ambient_method("unknown"));
        assert_eq!(
            std_logging_field_value_kinds(),
            &[
                StdLoggingFieldValueKind::TelemetryValue,
                StdLoggingFieldValueKind::String,
                StdLoggingFieldValueKind::Bool,
                StdLoggingFieldValueKind::Int,
                StdLoggingFieldValueKind::Float,
                StdLoggingFieldValueKind::None,
            ]
        );
    }
}

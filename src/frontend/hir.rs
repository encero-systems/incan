//! Frontend bridge into Incan HIR v0.
//!
//! This module builds the first declaration-level HIR snapshot from parsed AST plus `TypeCheckInfo`. It does not lower
//! bodies or replace the Rust-source backend; it gives the v0.5 middle-end a deterministic shape to grow from.

use crate::frontend::ast::{self, Declaration};
use crate::frontend::typechecker::TypeCheckInfo;
use incan_semantics_core::{
    CompilerNodeId, HirDeclaration, HirDeclarationKind, HirModule, HirSourceSpan, SemanticFactStore,
    SemanticModuleSnapshot,
};

/// Build declaration-level HIR v0 for a typechecked module.
pub fn build_hir_v0(program: &ast::Program, module_path: &[String], type_info: &TypeCheckInfo) -> HirModule {
    let module_identity = hir_module_identity(module_path);
    let facts = type_info.semantic_fact_store(module_path);
    build_hir_v0_with_facts(program, module_identity, &facts)
}

/// Build the bundled semantic module snapshot v0 for a typechecked module.
pub fn build_semantic_module_snapshot_v0(
    program: &ast::Program,
    module_path: &[String],
    type_info: &TypeCheckInfo,
) -> SemanticModuleSnapshot {
    let module_identity = hir_module_identity(module_path);
    let facts = type_info.semantic_fact_store(module_path);
    let hir = build_hir_v0_with_facts(program, module_identity, &facts);
    SemanticModuleSnapshot { hir, facts }
}

/// Build declaration-level HIR after semantic facts have already been collected.
fn build_hir_v0_with_facts(program: &ast::Program, module_identity: String, facts: &SemanticFactStore) -> HirModule {
    let declarations = program
        .declarations
        .iter()
        .map(|decl| {
            let (kind, name) = hir_decl_kind_and_name(&decl.node);
            let id = name
                .as_deref()
                .map(|name| hir_named_decl_id(&module_identity, name))
                .unwrap_or_else(|| hir_span_decl_id(&module_identity, decl.span));
            let type_fact_subject = facts.type_facts_for(&id).next().is_some().then_some(id.clone());
            HirDeclaration {
                id,
                kind,
                name,
                span: HirSourceSpan::new(decl.span.start, decl.span.end),
                type_fact_subject,
            }
        })
        .collect();

    HirModule {
        id: CompilerNodeId::module(module_identity.clone()),
        path: module_identity,
        declarations,
    }
}

/// Map a frontend declaration to the HIR v0 declaration category and optional name.
fn hir_decl_kind_and_name(decl: &Declaration) -> (HirDeclarationKind, Option<String>) {
    match decl {
        Declaration::Import(_) => (HirDeclarationKind::Import, None),
        Declaration::Const(decl) => (HirDeclarationKind::Const, Some(decl.name.clone())),
        Declaration::Static(decl) => (HirDeclarationKind::Static, Some(decl.name.clone())),
        Declaration::Model(decl) => (HirDeclarationKind::Model, Some(decl.name.clone())),
        Declaration::Class(decl) => (HirDeclarationKind::Class, Some(decl.name.clone())),
        Declaration::Trait(decl) => (HirDeclarationKind::Trait, Some(decl.name.clone())),
        Declaration::Alias(decl) => (HirDeclarationKind::Alias, Some(decl.name.clone())),
        Declaration::Partial(decl) => (HirDeclarationKind::Partial, Some(decl.name.clone())),
        Declaration::TypeAlias(decl) => (HirDeclarationKind::TypeAlias, Some(decl.name.clone())),
        Declaration::Newtype(decl) => (
            if decl.is_rusttype {
                HirDeclarationKind::Rusttype
            } else {
                HirDeclarationKind::Newtype
            },
            Some(decl.name.clone()),
        ),
        Declaration::Enum(decl) => (HirDeclarationKind::Enum, Some(decl.name.clone())),
        Declaration::Function(decl) => (HirDeclarationKind::Function, Some(decl.name.clone())),
        Declaration::TestModule(decl) => (HirDeclarationKind::TestModule, Some(decl.name.clone())),
        Declaration::Docstring(_) => (HirDeclarationKind::Docstring, None),
    }
}

/// Render a module path into the semantic module identity used by HIR v0.
fn hir_module_identity(module_path: &[String]) -> String {
    if module_path.is_empty() {
        "<module>".to_string()
    } else {
        module_path.join("::")
    }
}

/// Build the HIR declaration identity for a named declaration.
fn hir_named_decl_id(module_identity: &str, name: &str) -> CompilerNodeId {
    CompilerNodeId::declaration(module_identity, name)
}

/// Build the HIR declaration identity for an anonymous declaration.
fn hir_span_decl_id(module_identity: &str, span: ast::Span) -> CompilerNodeId {
    CompilerNodeId::declaration_span(module_identity, span.start, span.end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::typechecker::TypeChecker;
    use crate::frontend::{lexer, parser};

    #[test]
    fn build_hir_v0_renders_deterministic_declaration_snapshot() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
model User:
  name: str

enum Status:
  Active

def add(x: int, y: int = 1) -> int:
  return x + y
"#;
        let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let module_path = vec!["facts".to_string(), "hir".to_string()];
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        checker
            .check_program(&program)
            .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

        let first = build_hir_v0(&program, &module_path, checker.type_info()).render_snapshot();
        let second = build_hir_v0(&program, &module_path, checker.type_info()).render_snapshot();

        assert_eq!(first, second);
        assert!(first.contains("module facts::hir module:facts::hir\n"));
        assert!(first.contains("decl model User decl:facts::hir::User"));
        assert!(first.contains("decl enum Status decl:facts::hir::Status"));
        assert!(first.contains("decl function add decl:facts::hir::add"));
        assert!(first.contains("type_fact=decl:facts::hir::add"));
        assert!(!first.contains("type_fact=decl:facts::hir::User"));
        assert!(!first.contains("type_fact=decl:facts::hir::Status"));
        Ok(())
    }

    #[test]
    fn build_semantic_module_snapshot_v0_renders_hir_and_fact_sections() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
def add(x: int, y: int = 1) -> int:
  return x + y
"#;
        let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let module_path = vec!["facts".to_string(), "snapshot".to_string()];
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        checker
            .check_program(&program)
            .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

        let snapshot = build_semantic_module_snapshot_v0(&program, &module_path, checker.type_info()).render_snapshot();

        assert!(snapshot.contains("module facts::snapshot module:facts::snapshot\n"));
        assert!(snapshot.contains("decl function add decl:facts::snapshot::add"));
        assert!(snapshot.contains("\nfacts\n"));
        assert!(snapshot.contains("decl:facts::snapshot::add type=(int, int) -> int"));
        Ok(())
    }

    #[test]
    fn build_semantic_module_snapshot_v0_preserves_imported_source_targets() -> Result<(), Box<dyn std::error::Error>> {
        let helper_source = r#"
pub def helper() -> int:
  return 1
"#;
        let main_source = r#"
from helpers import helper

def run() -> int:
  return helper()
"#;
        let helper_tokens = lexer::lex(helper_source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let helper_program =
            parser::parse(&helper_tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let main_tokens = lexer::lex(main_source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let main_program = parser::parse(&main_tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let module_path = vec!["app".to_string()];
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        checker
            .check_with_imports(&main_program, &[("helpers", &helper_program)])
            .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

        let snapshot =
            build_semantic_module_snapshot_v0(&main_program, &module_path, checker.type_info()).render_snapshot();

        assert!(snapshot.contains("\nfacts\n"));
        assert!(snapshot.contains("symbol_target=function:helpers::helper"));
        assert!(!snapshot.contains("symbol_target=function:app::helper"));
        Ok(())
    }
}

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
    build_hir_v0_with_facts(program, module_identity, type_info, &facts)
}

/// Build the bundled semantic module snapshot v0 for a typechecked module.
pub fn build_semantic_module_snapshot_v0(
    program: &ast::Program,
    module_path: &[String],
    type_info: &TypeCheckInfo,
) -> SemanticModuleSnapshot {
    let module_identity = hir_module_identity(module_path);
    let facts = type_info.semantic_fact_store(module_path);
    let hir = build_hir_v0_with_facts(program, module_identity, type_info, &facts);
    SemanticModuleSnapshot { hir, facts }
}

/// Build declaration-level HIR after semantic facts have already been collected.
///
/// Each declaration's RFC 120 identity is *consumed* from the typechecker's span-keyed checked binding handoff,
/// never re-derived here from module path plus spelling. A multi-item import contributes one declaration record per
/// checked binding. A declaration with no exported identity carries none.
fn build_hir_v0_with_facts(
    program: &ast::Program,
    module_identity: String,
    type_info: &TypeCheckInfo,
    facts: &SemanticFactStore,
) -> HirModule {
    let declarations = program
        .declarations
        .iter()
        .flat_map(|decl| hir_declarations_for(decl, &module_identity, type_info, facts))
        .collect();

    HirModule {
        id: CompilerNodeId::module(module_identity.clone()),
        path: module_identity,
        declarations,
    }
}

/// Lower one syntax declaration into its checked declaration-level HIR records.
fn hir_declarations_for(
    decl: &ast::Spanned<Declaration>,
    module_identity: &str,
    type_info: &TypeCheckInfo,
    facts: &SemanticFactStore,
) -> Vec<HirDeclaration> {
    let key = (decl.span.start, decl.span.end);
    let span = HirSourceSpan::new(decl.span.start, decl.span.end);
    let bindings = type_info
        .declarations
        .hir_bindings_by_span
        .get(&key)
        .map(Vec::as_slice)
        .unwrap_or_default();

    if matches!(&decl.node, Declaration::Import(_)) {
        return bindings
            .iter()
            .enumerate()
            .map(|(ordinal, binding)| HirDeclaration {
                id: CompilerNodeId::declaration_binding_span(module_identity, decl.span.start, decl.span.end, ordinal),
                kind: HirDeclarationKind::Import,
                name: Some(binding.local_name.clone()),
                span,
                type_fact_subject: hir_type_fact_subject(facts, module_identity, &binding.local_name),
                canonical: binding.canonical.clone(),
            })
            .collect();
    }

    let name = hir_decl_name(&decl.node).map(str::to_string);
    let type_fact_subject = name
        .as_deref()
        .and_then(|name| hir_type_fact_subject(facts, module_identity, name));
    let canonical = match &decl.node {
        Declaration::Function(_) => type_info
            .declarations
            .function_bindings_by_span
            .get(&key)
            .and_then(|binding| binding.identity.clone()),
        _ => match bindings {
            [binding] => binding.canonical.clone(),
            _ => None,
        },
    };
    vec![HirDeclaration {
        id: hir_span_decl_id(module_identity, decl.span),
        kind: hir_decl_kind(&decl.node),
        name,
        span,
        type_fact_subject,
        canonical,
    }]
}

/// Return the compatibility semantic-fact subject for one checked local spelling, when a type fact exists.
///
/// Semantic type facts remain name-keyed during Slice 4. This link is deliberately separate from the span-derived
/// HIR node id; callers must not treat it as declaration identity.
fn hir_type_fact_subject(facts: &SemanticFactStore, module_identity: &str, local_name: &str) -> Option<CompilerNodeId> {
    let subject = CompilerNodeId::declaration(module_identity, local_name);
    let has_type_fact = facts.type_facts_for(&subject).next().is_some();
    has_type_fact.then_some(subject)
}

/// Map a frontend declaration to the HIR v0 declaration category.
fn hir_decl_kind(decl: &Declaration) -> HirDeclarationKind {
    match decl {
        Declaration::Import(_) => HirDeclarationKind::Import,
        Declaration::Const(_) => HirDeclarationKind::Const,
        Declaration::Static(_) => HirDeclarationKind::Static,
        Declaration::Model(_) => HirDeclarationKind::Model,
        Declaration::Capability(_) => HirDeclarationKind::Capability,
        Declaration::Class(_) => HirDeclarationKind::Class,
        Declaration::Trait(_) => HirDeclarationKind::Trait,
        Declaration::Alias(_) => HirDeclarationKind::Alias,
        Declaration::Partial(_) => HirDeclarationKind::Partial,
        Declaration::TypeAlias(_) => HirDeclarationKind::TypeAlias,
        Declaration::Newtype(decl) => {
            if decl.is_rusttype {
                HirDeclarationKind::Rusttype
            } else {
                HirDeclarationKind::Newtype
            }
        }
        Declaration::Enum(_) => HirDeclarationKind::Enum,
        Declaration::Function(_) => HirDeclarationKind::Function,
        Declaration::TestModule(_) => HirDeclarationKind::TestModule,
        Declaration::VocabBlock(_) | Declaration::Docstring(_) => HirDeclarationKind::Docstring,
    }
}

/// Return a non-import declaration's source spelling when it has one.
fn hir_decl_name(decl: &Declaration) -> Option<&str> {
    match decl {
        Declaration::Import(_) | Declaration::VocabBlock(_) | Declaration::Docstring(_) => None,
        Declaration::Const(decl) => Some(&decl.name),
        Declaration::Static(decl) => Some(&decl.name),
        Declaration::Model(decl) => Some(&decl.name),
        Declaration::Capability(decl) => Some(&decl.name),
        Declaration::Class(decl) => Some(&decl.name),
        Declaration::Trait(decl) => Some(&decl.name),
        Declaration::Alias(decl) => Some(&decl.name),
        Declaration::Partial(decl) => Some(&decl.name),
        Declaration::TypeAlias(decl) => Some(&decl.name),
        Declaration::Newtype(decl) => Some(&decl.name),
        Declaration::Enum(decl) => Some(&decl.name),
        Declaration::Function(decl) => Some(&decl.name),
        Declaration::TestModule(decl) => Some(&decl.name),
    }
}

/// Render a module path into the semantic module identity used by HIR v0.
fn hir_module_identity(module_path: &[String]) -> String {
    incan_semantics_core::module_identity_for_path(module_path)
}

/// Build the HIR node identity for a source declaration.
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
        assert!(first.contains("decl model User decl:facts::hir#decl."));
        assert!(first.contains("decl enum Status decl:facts::hir#decl."));
        assert!(first.contains("decl function add decl:facts::hir#decl."));
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
        assert!(snapshot.contains("decl function add decl:facts::snapshot#decl."));
        assert!(snapshot.contains("\nfacts\n"));
        assert!(snapshot.contains("decl:facts::snapshot::add type=(int, int) -> int"));
        Ok(())
    }

    /// RFC 120: a declaration's HIR record carries its minted identity, and an aliased import carries the *declaring*
    /// module's identity — so the import and its target declaration are visibly one symbol in the HIR handoff
    /// without consulting spellings.
    #[test]
    fn build_hir_v0_attaches_canonical_identities_to_declarations_and_single_imports()
    -> Result<(), Box<dyn std::error::Error>> {
        let helper_source = r#"
pub def helper() -> int:
  return 1
"#;
        let main_source = r#"
from helpers import helper as h

def run() -> int:
  return h()
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

        let hir = build_hir_v0(&main_program, &module_path, checker.type_info());

        let import_decl = hir
            .declarations
            .iter()
            .find(|decl| decl.kind == incan_semantics_core::HirDeclarationKind::Import)
            .ok_or("import declaration missing from HIR")?;
        assert_eq!(
            import_decl.name.as_deref(),
            Some("h"),
            "a single-binding import is named by its local binding"
        );
        let import_identity = import_decl
            .canonical
            .as_ref()
            .ok_or("single-binding import must carry its target's identity")?;
        assert_eq!(import_identity.declaration_name, "helper");
        assert_eq!(
            import_identity.module_path(),
            Some(["helpers".to_string()].as_slice()),
            "the import carries the declaring module's identity, not the consumer's"
        );

        let run_decl = hir
            .declarations
            .iter()
            .find(|decl| decl.name.as_deref() == Some("run"))
            .ok_or("run declaration missing from HIR")?;
        let run_identity = run_decl
            .canonical
            .as_ref()
            .ok_or("local declaration must carry its identity")?;
        assert_eq!(run_identity.module_path(), Some(["app".to_string()].as_slice()));

        let snapshot = hir.render_snapshot();
        assert!(
            snapshot.contains("identity=function:helpers::helper"),
            "the snapshot renders the import's declaring identity: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn build_hir_v0_emits_each_multi_item_import_binding_in_checked_source_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let helper_source = r#"
pub def first() -> int:
  return 1

pub def second() -> int:
  return 2
"#;
        let main_source = r#"
from helpers import first as left, second

def run() -> int:
  return left() + second()
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

        let hir = build_hir_v0(&main_program, &module_path, checker.type_info());
        let imports = hir
            .declarations
            .iter()
            .filter(|decl| decl.kind == HirDeclarationKind::Import)
            .collect::<Vec<_>>();

        assert_eq!(
            imports.len(),
            2,
            "one source import declaration introduces two HIR bindings"
        );
        assert_eq!(imports[0].name.as_deref(), Some("left"));
        assert_eq!(imports[1].name.as_deref(), Some("second"));
        assert_ne!(imports[0].id, imports[1].id);
        assert!(imports[0].id.path().ends_with(".binding.0"));
        assert!(imports[1].id.path().ends_with(".binding.1"));
        assert_eq!(
            imports[0]
                .canonical
                .as_ref()
                .map(|identity| identity.declaration_name.as_str()),
            Some("first")
        );
        assert_eq!(
            imports[1]
                .canonical
                .as_ref()
                .map(|identity| identity.declaration_name.as_str()),
            Some("second")
        );
        assert_eq!(
            imports[0]
                .canonical
                .as_ref()
                .and_then(|identity| identity.module_path()),
            Some(&["helpers".to_string()][..])
        );
        assert_eq!(
            imports[1]
                .canonical
                .as_ref()
                .and_then(|identity| identity.module_path()),
            Some(&["helpers".to_string()][..])
        );
        Ok(())
    }

    #[test]
    fn build_hir_v0_preserves_a_proven_module_binding_identity() -> Result<(), Box<dyn std::error::Error>> {
        let helper_source = "pub def helper() -> int:\n  return 1\n";
        let main_source = "import helpers as support\n\ndef run() -> int:\n  return 1\n";
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

        let hir = build_hir_v0(&main_program, &module_path, checker.type_info());
        let import = hir
            .declarations
            .iter()
            .find(|decl| decl.kind == HirDeclarationKind::Import)
            .ok_or("module import binding missing from HIR")?;

        assert_eq!(import.name.as_deref(), Some("support"));
        assert!(import.id.path().ends_with(".binding.0"));
        let canonical = import
            .canonical
            .as_ref()
            .ok_or("a resolved module binding must retain its canonical path identity")?;
        assert_eq!(canonical.namespace, incan_semantics_core::SymbolNamespace::ModulePath);
        assert_eq!(
            canonical.origin,
            incan_semantics_core::SymbolOrigin::Module(vec!["helpers".to_string()])
        );
        assert_eq!(canonical.declaration_name, "helpers");
        assert_eq!(canonical.kind, incan_semantics_core::SemanticSourceTargetKind::Module);
        assert_eq!(
            canonical.declaration_span,
            incan_semantics_core::HirSourceSpan::new(0, 0)
        );
        Ok(())
    }

    #[test]
    fn build_hir_v0_keeps_overload_nodes_and_canonical_identities_span_distinct()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
def render(value: int) -> int:
  return value

def render(value: str) -> str:
  return value
"#;
        let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let module_path = vec!["app".to_string(), "overloads".to_string()];
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        checker
            .check_program(&program)
            .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

        let hir = build_hir_v0(&program, &module_path, checker.type_info());
        let declarations = hir
            .declarations
            .iter()
            .filter(|decl| decl.name.as_deref() == Some("render"))
            .collect::<Vec<_>>();

        assert_eq!(declarations.len(), 2);
        assert_ne!(declarations[0].id, declarations[1].id);
        assert_ne!(declarations[0].canonical, declarations[1].canonical);
        for declaration in declarations {
            assert!(declaration.id.path().contains("#decl."));
            let canonical = declaration
                .canonical
                .as_ref()
                .ok_or("each overload must retain its own declaration identity")?;
            assert_eq!(canonical.declaration_name, "render");
            assert_eq!(declaration.span, canonical.declaration_span);
        }
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

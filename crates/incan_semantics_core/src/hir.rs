//! Incan HIR v0 data model.
//!
//! HIR v0 is a typed, source-mapped middle-end handoff keyed by stable compiler IDs. This first slice is deliberately
//! declaration-level so it can run beside the Rust-source backend while later slices add statements, expressions, and
//! body ownership facts.

use std::fmt::Write;

use crate::{CompilerNodeId, SemanticFactStore};

/// A source byte range attached to a HIR node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HirSourceSpan {
    pub start: usize,
    pub end: usize,
}

impl HirSourceSpan {
    /// Build a source span from inclusive-exclusive byte offsets.
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

/// Declaration-level HIR module snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirModule {
    pub id: CompilerNodeId,
    pub path: String,
    pub declarations: Vec<HirDeclaration>,
}

impl HirModule {
    /// Render a deterministic maintainer-facing snapshot.
    pub fn render_snapshot(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(&mut out, "module {} {}", self.path, self.id);
        for decl in &self.declarations {
            let name = decl.name.as_deref().unwrap_or("<anonymous>");
            let _ = write!(
                &mut out,
                "decl {} {} {} span={}..{}",
                decl.kind.as_str(),
                name,
                decl.id,
                decl.span.start,
                decl.span.end
            );
            if let Some(type_fact_subject) = &decl.type_fact_subject {
                let _ = write!(&mut out, " type_fact={type_fact_subject}");
            }
            out.push('\n');
        }
        out
    }
}

/// Combined v0 semantic module handoff containing HIR plus semantic facts.
#[derive(Debug, Clone)]
pub struct SemanticModuleSnapshot {
    pub hir: HirModule,
    pub facts: SemanticFactStore,
}

impl SemanticModuleSnapshot {
    /// Render a deterministic maintainer-facing module snapshot.
    pub fn render_snapshot(&self) -> String {
        let mut out = self.hir.render_snapshot();
        out.push_str("facts\n");
        out.push_str(&self.facts.render_snapshot());
        out
    }
}

/// One top-level declaration in HIR v0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirDeclaration {
    pub id: CompilerNodeId,
    pub kind: HirDeclarationKind,
    pub name: Option<String>,
    pub span: HirSourceSpan,
    /// Subject ID to query for a [`crate::SemanticFactKind::Type`] fact, when this declaration has one.
    pub type_fact_subject: Option<CompilerNodeId>,
}

/// Top-level declaration categories represented by HIR v0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HirDeclarationKind {
    Import,
    Const,
    Static,
    Model,
    Class,
    Trait,
    Alias,
    Partial,
    TypeAlias,
    Newtype,
    Rusttype,
    Enum,
    Function,
    TestModule,
    Docstring,
}

impl HirDeclarationKind {
    /// Return the compact snapshot spelling for this declaration kind.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Import => "import",
            Self::Const => "const",
            Self::Static => "static",
            Self::Model => "model",
            Self::Class => "class",
            Self::Trait => "trait",
            Self::Alias => "alias",
            Self::Partial => "partial",
            Self::TypeAlias => "type_alias",
            Self::Newtype => "newtype",
            Self::Rusttype => "rusttype",
            Self::Enum => "enum",
            Self::Function => "function",
            Self::TestModule => "test_module",
            Self::Docstring => "docstring",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CompilerNodeId, CompilerNodeKind};

    #[test]
    fn hir_module_snapshot_is_deterministic() {
        let decl_id = CompilerNodeId::new(CompilerNodeKind::Declaration, "facts::hir::run");
        let module = HirModule {
            id: CompilerNodeId::new(CompilerNodeKind::Module, "facts::hir"),
            path: "facts::hir".to_string(),
            declarations: vec![HirDeclaration {
                id: decl_id.clone(),
                kind: HirDeclarationKind::Function,
                name: Some("run".to_string()),
                span: HirSourceSpan::new(1, 24),
                type_fact_subject: Some(decl_id),
            }],
        };

        assert_eq!(
            module.render_snapshot(),
            "module facts::hir module:facts::hir\n\
             decl function run decl:facts::hir::run span=1..24 type_fact=decl:facts::hir::run\n"
        );
    }

    #[test]
    fn semantic_module_snapshot_renders_hir_and_facts() {
        let decl_id = CompilerNodeId::new(CompilerNodeKind::Declaration, "facts::hir::run");
        let mut facts = SemanticFactStore::new();
        facts.insert(crate::SemanticFact::new(
            decl_id.clone(),
            crate::SemanticFactKind::Type,
            crate::SemanticFactValue::semantic_type(crate::IncanType::Primitive(crate::IncanPrimitiveType::Int)),
        ));
        let snapshot = SemanticModuleSnapshot {
            hir: HirModule {
                id: CompilerNodeId::new(CompilerNodeKind::Module, "facts::hir"),
                path: "facts::hir".to_string(),
                declarations: vec![HirDeclaration {
                    id: decl_id.clone(),
                    kind: HirDeclarationKind::Function,
                    name: Some("run".to_string()),
                    span: HirSourceSpan::new(1, 24),
                    type_fact_subject: Some(decl_id),
                }],
            },
            facts,
        };

        assert_eq!(
            snapshot.render_snapshot(),
            "module facts::hir module:facts::hir\n\
             decl function run decl:facts::hir::run span=1..24 type_fact=decl:facts::hir::run\n\
             facts\n\
             decl:facts::hir::run type=int\n"
        );
    }
}

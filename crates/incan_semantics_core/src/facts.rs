//! Backend-neutral semantic fact identifiers.
//!
//! These types are the first shared vocabulary for the v0.5 middle-end foundation. They deliberately live outside the
//! Rust-source backend so HIR, diagnostics, inspection, and future backends can talk about the same compiler-owned
//! subjects without using emitted Rust tokens as identity.

use std::collections::BTreeMap;
use std::fmt::{self, Write};

use crate::IncanType;

/// Kind of compiler-owned node that can receive semantic facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CompilerNodeKind {
    Module,
    Declaration,
    Statement,
    Expression,
    Local,
    Type,
}

impl CompilerNodeKind {
    /// Return the compact snapshot spelling for this node kind.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Module => "module",
            Self::Declaration => "decl",
            Self::Statement => "stmt",
            Self::Expression => "expr",
            Self::Local => "local",
            Self::Type => "type",
        }
    }
}

/// Stable compiler-owned identity for a module, declaration, statement, expression, local, or type.
///
/// The `path` is intentionally semantic rather than Rust-shaped. Current bridge code may derive it from spans or
/// source paths at first, but consumers should treat the rendered form as a compiler identity, not as an emitted Rust
/// item path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CompilerNodeId {
    kind: CompilerNodeKind,
    path: String,
}

impl CompilerNodeId {
    /// Build an identity from an explicit kind and semantic path.
    pub fn new(kind: CompilerNodeKind, path: impl Into<String>) -> Self {
        Self {
            kind,
            path: path.into(),
        }
    }

    /// Build a module identity from its semantic module path.
    pub fn module(module_identity: impl Into<String>) -> Self {
        Self::new(CompilerNodeKind::Module, module_identity)
    }

    /// Build a named declaration identity scoped to a module.
    pub fn declaration(module_identity: &str, name: &str) -> Self {
        Self::new(CompilerNodeKind::Declaration, format!("{module_identity}::{name}"))
    }

    /// Build an anonymous declaration identity from its module and source byte span.
    pub fn declaration_span(module_identity: &str, start: usize, end: usize) -> Self {
        Self::new(
            CompilerNodeKind::Declaration,
            format!("{module_identity}#decl.{start}..{end}"),
        )
    }

    /// Build an expression identity from its module and source byte span.
    pub fn expression_span(module_identity: &str, start: usize, end: usize) -> Self {
        Self::new(
            CompilerNodeKind::Expression,
            format!("{module_identity}#{start}..{end}"),
        )
    }

    /// Build a statement identity from its module and source byte span.
    pub fn statement_span(module_identity: &str, start: usize, end: usize) -> Self {
        Self::new(
            CompilerNodeKind::Statement,
            format!("{module_identity}#stmt.{start}..{end}"),
        )
    }

    /// Build a local binding identity scoped to a module.
    pub fn local(module_identity: &str, name: &str) -> Self {
        Self::new(CompilerNodeKind::Local, format!("{module_identity}::{name}"))
    }

    /// Build a source type identity scoped to a module.
    pub fn type_identity(module_identity: &str, name: &str) -> Self {
        Self::new(CompilerNodeKind::Type, format!("{module_identity}::{name}"))
    }

    /// Return the category of compiler node this identity names.
    pub const fn kind(&self) -> CompilerNodeKind {
        self.kind
    }

    /// Return the semantic path inside this compiler-owned identity.
    pub fn path(&self) -> &str {
        &self.path
    }
}

impl fmt::Display for CompilerNodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind.as_str(), self.path)
    }
}

/// Semantic fact category owned by the compiler middle end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SemanticFactKind {
    Type,
    SymbolTarget,
    RuntimeRequirement,
    Diagnostic,
    BackendObligation,
}

impl SemanticFactKind {
    /// Return the compact snapshot spelling for this semantic fact kind.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Type => "type",
            Self::SymbolTarget => "symbol_target",
            Self::RuntimeRequirement => "runtime_requirement",
            Self::Diagnostic => "diagnostic",
            Self::BackendObligation => "backend_obligation",
        }
    }
}

/// Initial fact payload shape.
///
/// This is intentionally small. Type facts carry [`IncanType`] and source-target facts carry
/// [`SemanticSourceTarget`]; text remains available for diagnostics and other payloads until those facts gain their
/// own semantic structures.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SemanticFactValue {
    Text(String),
    Type(IncanType),
    SourceTarget(SemanticSourceTarget),
    Flag(bool),
}

impl SemanticFactValue {
    /// Build a text payload fact value.
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }

    /// Build a structured Incan type fact value.
    pub fn semantic_type(value: IncanType) -> Self {
        Self::Type(value)
    }

    /// Build a structured source target fact value.
    pub fn source_target(value: SemanticSourceTarget) -> Self {
        Self::SourceTarget(value)
    }

    /// Render a deterministic maintainer-facing fact payload snapshot.
    pub fn render_snapshot(&self) -> String {
        match self {
            Self::Text(value) => format!("{value:?}"),
            Self::Type(value) => value.to_string(),
            Self::SourceTarget(value) => value.to_string(),
            Self::Flag(value) => value.to_string(),
        }
    }
}

/// Compiler-proven source declaration target.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SemanticSourceTarget {
    pub module_path: Vec<String>,
    pub name: String,
    pub kind: SemanticSourceTargetKind,
}

impl SemanticSourceTarget {
    /// Build a source target from structured module path, name, and kind fields.
    pub fn new(module_path: Vec<String>, name: impl Into<String>, kind: SemanticSourceTargetKind) -> Self {
        Self {
            module_path,
            name: name.into(),
            kind,
        }
    }

    /// Build a source target while accepting the current frontend declaration kind spelling.
    pub fn from_kind_str(module_path: Vec<String>, name: impl Into<String>, kind: &str) -> Self {
        Self::new(module_path, name, SemanticSourceTargetKind::from_kind_str(kind))
    }
}

impl fmt::Display for SemanticSourceTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.module_path.is_empty() {
            write!(f, "{}:<module>::{}", self.kind, self.name)
        } else {
            write!(f, "{}:{}::{}", self.kind, self.module_path.join("::"), self.name)
        }
    }
}

/// Semantic target declaration category.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SemanticSourceTargetKind {
    Function,
    Model,
    Class,
    Newtype,
    Rusttype,
    Enum,
    Other(String),
}

impl SemanticSourceTargetKind {
    /// Convert the current frontend declaration kind spelling into a semantic target kind.
    pub fn from_kind_str(kind: &str) -> Self {
        match kind {
            "function" => Self::Function,
            "model" => Self::Model,
            "class" => Self::Class,
            "newtype" => Self::Newtype,
            "rusttype" => Self::Rusttype,
            "enum" => Self::Enum,
            other => Self::Other(other.to_string()),
        }
    }

    /// Return the compact snapshot spelling for this source target kind.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Function => "function",
            Self::Model => "model",
            Self::Class => "class",
            Self::Newtype => "newtype",
            Self::Rusttype => "rusttype",
            Self::Enum => "enum",
            Self::Other(kind) => kind,
        }
    }
}

impl fmt::Display for SemanticSourceTargetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One backend-neutral fact about a compiler-owned node.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SemanticFact {
    pub subject: CompilerNodeId,
    pub kind: SemanticFactKind,
    pub value: SemanticFactValue,
}

impl SemanticFact {
    /// Build one semantic fact for a compiler-owned subject.
    pub fn new(subject: CompilerNodeId, kind: SemanticFactKind, value: SemanticFactValue) -> Self {
        Self { subject, kind, value }
    }

    /// Render a deterministic maintainer-facing single-fact snapshot line.
    pub fn render_snapshot(&self) -> String {
        format!(
            "{} {}={}",
            self.subject,
            self.kind.as_str(),
            self.value.render_snapshot()
        )
    }
}

/// Deterministic in-memory semantic fact store.
#[derive(Debug, Clone, Default)]
pub struct SemanticFactStore {
    facts: BTreeMap<CompilerNodeId, Vec<SemanticFact>>,
}

impl SemanticFactStore {
    /// Build an empty deterministic fact store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert one fact, keeping facts for the subject in deterministic order.
    pub fn insert(&mut self, fact: SemanticFact) {
        let facts = self.facts.entry(fact.subject.clone()).or_default();
        facts.push(fact);
        facts.sort();
    }

    /// Return all facts recorded for one subject.
    pub fn facts_for(&self, subject: &CompilerNodeId) -> &[SemanticFact] {
        self.facts.get(subject).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Return all facts of the requested kind in deterministic store order.
    pub fn facts_by_kind(&self, kind: SemanticFactKind) -> impl Iterator<Item = &SemanticFact> {
        self.iter().filter(move |fact| fact.kind == kind)
    }

    /// Return facts for a subject filtered by semantic fact kind.
    pub fn facts_for_kind(
        &self,
        subject: &CompilerNodeId,
        kind: SemanticFactKind,
    ) -> impl Iterator<Item = &SemanticFact> {
        self.facts_for(subject).iter().filter(move |fact| fact.kind == kind)
    }

    /// Return structured semantic type facts for a subject.
    pub fn type_facts_for(&self, subject: &CompilerNodeId) -> impl Iterator<Item = &IncanType> {
        self.facts_for_kind(subject, SemanticFactKind::Type)
            .filter_map(|fact| match &fact.value {
                SemanticFactValue::Type(ty) => Some(ty),
                _ => None,
            })
    }

    /// Return structured source-target facts for a subject.
    pub fn source_targets_for(&self, subject: &CompilerNodeId) -> impl Iterator<Item = &SemanticSourceTarget> {
        self.facts_for_kind(subject, SemanticFactKind::SymbolTarget)
            .filter_map(|fact| match &fact.value {
                SemanticFactValue::SourceTarget(target) => Some(target),
                _ => None,
            })
    }

    /// Return all subjects that have at least one fact.
    pub fn subjects(&self) -> impl Iterator<Item = &CompilerNodeId> {
        self.facts.keys()
    }

    /// Iterate over every fact in deterministic subject and fact order.
    pub fn iter(&self) -> impl Iterator<Item = &SemanticFact> {
        self.facts.values().flat_map(|facts| facts.iter())
    }

    /// Render all facts in deterministic store order.
    pub fn render_snapshot(&self) -> String {
        let mut out = String::new();
        for fact in self.iter() {
            let _ = writeln!(&mut out, "{}", fact.render_snapshot());
        }
        out
    }

    /// Return whether the store contains no facts.
    pub fn is_empty(&self) -> bool {
        self.facts.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiler_node_ids_render_kind_prefixed_identity() {
        let id = CompilerNodeId::new(CompilerNodeKind::Declaration, "pkg::module::build");

        assert_eq!(id.kind(), CompilerNodeKind::Declaration);
        assert_eq!(id.path(), "pkg::module::build");
        assert_eq!(id.to_string(), "decl:pkg::module::build");
        assert_eq!(CompilerNodeId::module("pkg::module").to_string(), "module:pkg::module");
        assert_eq!(
            CompilerNodeId::declaration("pkg::module", "build").to_string(),
            "decl:pkg::module::build"
        );
        assert_eq!(
            CompilerNodeId::expression_span("pkg::module", 7, 11).to_string(),
            "expr:pkg::module#7..11"
        );
        assert_eq!(
            CompilerNodeId::statement_span("pkg::module", 11, 19).to_string(),
            "stmt:pkg::module#stmt.11..19"
        );
        assert_eq!(
            CompilerNodeId::local("pkg::module", "value").to_string(),
            "local:pkg::module::value"
        );
        assert_eq!(
            CompilerNodeId::type_identity("pkg::module", "User").to_string(),
            "type:pkg::module::User"
        );
    }

    #[test]
    fn semantic_fact_store_iterates_subjects_deterministically() {
        let expr = CompilerNodeId::new(CompilerNodeKind::Expression, "pkg::main#expr.2");
        let decl = CompilerNodeId::new(CompilerNodeKind::Declaration, "pkg::main");
        let mut store = SemanticFactStore::new();

        store.insert(SemanticFact::new(
            expr.clone(),
            SemanticFactKind::Type,
            SemanticFactValue::semantic_type(IncanType::Primitive(crate::IncanPrimitiveType::Int)),
        ));
        store.insert(SemanticFact::new(
            decl.clone(),
            SemanticFactKind::RuntimeRequirement,
            SemanticFactValue::text("hosted_std"),
        ));

        let subjects = store.subjects().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(subjects, vec!["decl:pkg::main", "expr:pkg::main#expr.2"]);
        assert_eq!(store.facts_for(&decl).len(), 1);
        assert!(
            store
                .facts_for(&CompilerNodeId::new(CompilerNodeKind::Type, "missing"))
                .is_empty()
        );
    }

    #[test]
    fn semantic_fact_store_sorts_facts_for_the_same_subject() {
        let expr = CompilerNodeId::new(CompilerNodeKind::Expression, "pkg::main#expr.2");
        let mut store = SemanticFactStore::new();

        store.insert(SemanticFact::new(
            expr.clone(),
            SemanticFactKind::SymbolTarget,
            SemanticFactValue::source_target(SemanticSourceTarget::from_kind_str(
                vec!["pkg".to_string()],
                "main",
                "function",
            )),
        ));
        store.insert(SemanticFact::new(
            expr.clone(),
            SemanticFactKind::Type,
            SemanticFactValue::semantic_type(IncanType::Primitive(crate::IncanPrimitiveType::Int)),
        ));

        let kinds = store.facts_for(&expr).iter().map(|fact| fact.kind).collect::<Vec<_>>();
        assert_eq!(kinds, vec![SemanticFactKind::Type, SemanticFactKind::SymbolTarget]);
    }

    #[test]
    fn semantic_fact_store_queries_facts_by_kind_deterministically() {
        let decl = CompilerNodeId::declaration("pkg::main", "build");
        let expr = CompilerNodeId::expression_span("pkg::main", 3, 8);
        let mut store = SemanticFactStore::new();

        store.insert(SemanticFact::new(
            expr.clone(),
            SemanticFactKind::SymbolTarget,
            SemanticFactValue::source_target(SemanticSourceTarget::from_kind_str(
                vec!["pkg".to_string()],
                "build",
                "function",
            )),
        ));
        store.insert(SemanticFact::new(
            decl.clone(),
            SemanticFactKind::Type,
            SemanticFactValue::semantic_type(IncanType::Named("Builder".to_string())),
        ));
        store.insert(SemanticFact::new(
            expr.clone(),
            SemanticFactKind::Type,
            SemanticFactValue::semantic_type(IncanType::Primitive(crate::IncanPrimitiveType::Int)),
        ));

        let type_subjects = store
            .facts_by_kind(SemanticFactKind::Type)
            .map(|fact| fact.subject.to_string())
            .collect::<Vec<_>>();
        assert_eq!(type_subjects, vec!["decl:pkg::main::build", "expr:pkg::main#3..8"]);

        let expr_kinds = store
            .facts_for_kind(&expr, SemanticFactKind::SymbolTarget)
            .map(|fact| fact.kind)
            .collect::<Vec<_>>();
        assert_eq!(expr_kinds, vec![SemanticFactKind::SymbolTarget]);

        assert_eq!(
            store
                .facts_for_kind(&CompilerNodeId::module("missing"), SemanticFactKind::Type)
                .count(),
            0
        );
    }

    #[test]
    fn semantic_fact_store_extracts_typed_payloads() {
        let expr = CompilerNodeId::expression_span("pkg::main", 3, 8);
        let target = SemanticSourceTarget::from_kind_str(vec!["pkg".to_string()], "build", "function");
        let mut store = SemanticFactStore::new();

        store.insert(SemanticFact::new(
            expr.clone(),
            SemanticFactKind::Type,
            SemanticFactValue::semantic_type(IncanType::Primitive(crate::IncanPrimitiveType::Int)),
        ));
        store.insert(SemanticFact::new(
            expr.clone(),
            SemanticFactKind::Type,
            SemanticFactValue::text("legacy diagnostic payload"),
        ));
        store.insert(SemanticFact::new(
            expr.clone(),
            SemanticFactKind::SymbolTarget,
            SemanticFactValue::source_target(target.clone()),
        ));

        let type_facts = store.type_facts_for(&expr).cloned().collect::<Vec<_>>();
        assert_eq!(type_facts, vec![IncanType::Primitive(crate::IncanPrimitiveType::Int)]);

        let source_targets = store.source_targets_for(&expr).cloned().collect::<Vec<_>>();
        assert_eq!(source_targets, vec![target]);
    }

    #[test]
    fn semantic_fact_store_renders_deterministic_snapshot() {
        let expr = CompilerNodeId::expression_span("pkg::main", 3, 8);
        let mut store = SemanticFactStore::new();

        store.insert(SemanticFact::new(
            expr.clone(),
            SemanticFactKind::SymbolTarget,
            SemanticFactValue::source_target(SemanticSourceTarget::from_kind_str(
                vec!["pkg".to_string()],
                "build",
                "function",
            )),
        ));
        store.insert(SemanticFact::new(
            expr,
            SemanticFactKind::Type,
            SemanticFactValue::semantic_type(IncanType::Primitive(crate::IncanPrimitiveType::Int)),
        ));
        store.insert(SemanticFact::new(
            CompilerNodeId::module("pkg::main"),
            SemanticFactKind::Diagnostic,
            SemanticFactValue::text("line one\nline two"),
        ));

        assert_eq!(
            store.render_snapshot(),
            "module:pkg::main diagnostic=\"line one\\nline two\"\n\
             expr:pkg::main#3..8 type=int\n\
             expr:pkg::main#3..8 symbol_target=function:pkg::build\n"
        );
    }

    #[test]
    fn semantic_source_target_kind_preserves_known_and_unknown_kinds() {
        assert_eq!(
            SemanticSourceTargetKind::from_kind_str("function"),
            SemanticSourceTargetKind::Function
        );
        assert_eq!(SemanticSourceTargetKind::from_kind_str("macro").as_str(), "macro");
    }
}

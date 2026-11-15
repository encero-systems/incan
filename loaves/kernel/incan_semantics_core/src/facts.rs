//! Backend-neutral semantic fact identifiers.
//!
//! These types are the first shared vocabulary for the v0.5 middle-end foundation. They deliberately live outside the
//! native backend so HIR, diagnostics, inspection, and lowering can talk about the same compiler-owned
//! subjects without using native symbol names as identity.

use std::collections::BTreeMap;
use std::fmt::{self, Write};

use serde::{Deserialize, Serialize};

use crate::IncanType;

mod authority_decisions;
mod canonical_symbols;
mod registry_values;
mod source_targets;

pub use authority_decisions::*;
pub use canonical_symbols::*;
pub use registry_values::*;
pub use source_targets::*;

/// Kind of compiler-owned node that can receive semantic facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub enum CompilerNodeKind {
    /// A package selected at a command/session boundary.
    Package,
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
            Self::Package => "package",
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
/// source paths at first, but consumers should treat the rendered form as a compiler identity, not as a native Rust
/// item path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub struct CompilerNodeId {
    kind: CompilerNodeKind,
    path: String,
}

impl CompilerNodeId {
    /// Distinguish generated nodes sharing a source range while preserving authored identities byte for byte.
    pub fn with_expansion(mut self, expansion: Option<[u8; 32]>) -> Self {
        if let Some(expansion) = expansion {
            self.path.push_str("/expansion.");
            self.path.push_str(&hex::encode(expansion));
        }
        self
    }

    /// Address a declaration using its complete checked span rather than its diagnostic range alone.
    pub fn declaration_at(module: &str, span: crate::HirSourceSpan) -> Self {
        Self::declaration_span(module, span.start, span.end).with_expansion(span.expansion)
    }

    /// Address one binding of a generated or authored declaration without losing its expansion context.
    pub fn declaration_binding_at(module: &str, span: crate::HirSourceSpan, ordinal: usize) -> Self {
        Self::declaration_binding_span(module, span.start, span.end, ordinal).with_expansion(span.expansion)
    }

    /// Address an expression using the same full span as the checker's expression facts.
    pub fn expression_at(module: &str, span: crate::HirSourceSpan) -> Self {
        Self::expression_span(module, span.start, span.end).with_expansion(span.expansion)
    }

    /// Address a selected protocol hook within its receiver's complete expansion context.
    pub fn protocol_call_at(module: &str, span: crate::HirSourceSpan, role: &str) -> Self {
        Self::protocol_call_span(module, span.start, span.end, role).with_expansion(span.expansion)
    }

    /// Address a checked type component within the expression that owns it.
    pub fn expression_type_component_at(module: &str, span: crate::HirSourceSpan, indices: &[usize]) -> Self {
        Self::expression_type_component(module, span.start, span.end, indices).with_expansion(span.expansion)
    }

    /// Address a statement while retaining its generated-node identity and original diagnostic range.
    pub fn statement_at(module: &str, span: crate::HirSourceSpan) -> Self {
        Self::statement_span(module, span.start, span.end).with_expansion(span.expansion)
    }

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

    /// Build a package identity supplied by the compilation/session boundary.
    pub fn package(package_identity: impl Into<String>) -> Self {
        Self::new(CompilerNodeKind::Package, package_identity)
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

    /// Build one declaration-binding identity for a source declaration that introduces multiple bindings.
    ///
    /// The ordinal is the binding's checked source order within the declaration. It is not a source spelling and
    /// carries no resolution meaning; canonical symbol identity remains the authority for what the binding names.
    pub fn declaration_binding_span(module_identity: &str, start: usize, end: usize, binding_ordinal: usize) -> Self {
        Self::new(
            CompilerNodeKind::Declaration,
            format!("{module_identity}#decl.{start}..{end}.binding.{binding_ordinal}"),
        )
    }

    /// Build an expression identity from its module and source byte span.
    pub fn expression_span(module_identity: &str, start: usize, end: usize) -> Self {
        Self::new(
            CompilerNodeKind::Expression,
            format!("{module_identity}#{start}..{end}"),
        )
    }

    /// Address a checker-selected protocol call separately from its written receiver expression and sibling hooks.
    /// The role names the checked hook, while its target remains a separate canonical identity fact.
    pub fn protocol_call_span(module_identity: &str, start: usize, end: usize, role: &str) -> Self {
        Self::new(
            CompilerNodeKind::Expression,
            format!("{module_identity}#protocol.{start}..{end}.{role}"),
        )
    }

    /// Address one named leaf of an expression's checked type by structural child indices, independent of aliases.
    pub fn expression_type_component(module_identity: &str, start: usize, end: usize, indices: &[usize]) -> Self {
        let path = indices.iter().map(usize::to_string).collect::<Vec<_>>().join(".");
        Self::new(
            CompilerNodeKind::Type,
            format!("{module_identity}#expr.{start}..{end}/type.{path}"),
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
    SymbolIdentity,
    /// Identity of a checked declaring context, including fields, methods and properties.
    DeclarationIdentity,
    /// Unique nearest checked declaration owning one reference site.
    ReferenceOwner,
    /// Stable owner plus collision-local ordinal for a nested declaration target.
    StableDeclarationContext,
    /// Source anchor for a checked reference without parsing an opaque compiler node id.
    ReferenceSpan,
    /// Declaring type that requires a field or variant layout and its checked defaults.
    RequiredMemberOwner,
    /// Retained declaration context required by a compiler-generated reference target.
    RequiredReferenceTarget,
    Registry,
    RuntimeRequirement,
    Diagnostic,
    BackendObligation,
    AuthorityDecision,
}

impl SemanticFactKind {
    /// Return the compact snapshot spelling for this semantic fact kind.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Type => "type",
            Self::SymbolTarget => "symbol_target",
            Self::SymbolIdentity => "symbol_identity",
            Self::DeclarationIdentity => "declaration_identity",
            Self::ReferenceOwner => "reference_owner",
            Self::StableDeclarationContext => "stable_declaration_context",
            Self::ReferenceSpan => "reference_span",
            Self::RequiredMemberOwner => "required_member_owner",
            Self::RequiredReferenceTarget => "required_reference_target",
            Self::Registry => "registry",
            Self::RuntimeRequirement => "runtime_requirement",
            Self::Diagnostic => "diagnostic",
            Self::BackendObligation => "backend_obligation",
            Self::AuthorityDecision => "authority_decision",
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
    CanonicalIdentity(CanonicalSymbolId),
    StableDeclarationContext(CanonicalStableDeclarationContext),
    /// Original checked reference anchor used by inspection projections.
    SourceSpan(crate::HirSourceSpan),
    RegistryEntry(SemanticRegistryEntry),
    AuthorityDecision(Box<AuthorityDecision>),
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

    /// Build a canonical symbol-identity fact value.
    pub fn canonical_identity(value: CanonicalSymbolId) -> Self {
        Self::CanonicalIdentity(value)
    }

    /// Build a checked stable-declaration context sidecar.
    pub fn stable_declaration_context(value: CanonicalStableDeclarationContext) -> Self {
        Self::StableDeclarationContext(value)
    }

    /// Build one checked typed-registry entry fact.
    pub fn registry_entry(value: SemanticRegistryEntry) -> Self {
        Self::RegistryEntry(value)
    }

    /// Build one RFC 104 authority-decision fact value.
    pub fn authority_decision(value: AuthorityDecision) -> Self {
        Self::AuthorityDecision(Box::new(value))
    }

    /// Render a deterministic maintainer-facing fact payload snapshot.
    pub fn render_snapshot(&self) -> String {
        match self {
            Self::Text(value) => format!("{value:?}"),
            Self::Type(value) => value.to_string(),
            Self::SourceTarget(value) => value.to_string(),
            Self::CanonicalIdentity(value) => value.render_compact(),
            Self::StableDeclarationContext(value) => value.to_string(),
            Self::RegistryEntry(value) => value.to_string(),
            Self::AuthorityDecision(value) => value.to_string(),
            Self::Flag(value) => value.to_string(),
            Self::SourceSpan(span) => format!("{}..{}", span.start, span.end),
        }
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

    /// Return compiler-owned canonical symbol identities for one source node.
    pub fn symbol_identities_for(&self, subject: &CompilerNodeId) -> impl Iterator<Item = &CanonicalSymbolId> {
        self.facts_for_kind(subject, SemanticFactKind::SymbolIdentity)
            .filter_map(|fact| match &fact.value {
                SemanticFactValue::CanonicalIdentity(identity) => Some(identity),
                _ => None,
            })
    }

    /// Return checked stable-declaration contexts for one source node.
    pub fn stable_declaration_contexts_for(
        &self,
        subject: &CompilerNodeId,
    ) -> impl Iterator<Item = &CanonicalStableDeclarationContext> {
        self.facts_for_kind(subject, SemanticFactKind::StableDeclarationContext)
            .filter_map(|fact| match &fact.value {
                SemanticFactValue::StableDeclarationContext(context) => Some(context),
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
mod tests;

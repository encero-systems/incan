//! Backend-neutral semantic fact identifiers.
//!
//! These types are the first shared vocabulary for the v0.5 middle-end foundation. They deliberately live outside the
//! Rust-source backend so HIR, diagnostics, inspection, and future backends can talk about the same compiler-owned
//! subjects without using emitted Rust tokens as identity.

use std::collections::BTreeMap;
use std::fmt::{self, Write};

use serde::{Deserialize, Serialize};

use crate::IncanType;

/// Kind of compiler-owned node that can receive semantic facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
            Self::RegistryEntry(value) => value.to_string(),
            Self::AuthorityDecision(value) => value.to_string(),
            Self::Flag(value) => value.to_string(),
        }
    }
}

/// Compiler-recognised registry subject category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SemanticRegistrySubjectKind {
    Function,
    Method,
    CompilationUnit,
    Package,
}

impl SemanticRegistrySubjectKind {
    /// Return the stable machine spelling of this registry subject category.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Method => "method",
            Self::CompilationUnit => "compilation_unit",
            Self::Package => "package",
        }
    }
}

impl fmt::Display for SemanticRegistrySubjectKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A source-level value that the compiler can snapshot without evaluating user code.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SemanticRegistryValue {
    Int(i64),
    Float(String),
    Bool(bool),
    String(String),
    Bytes(Vec<u8>),
    None,
    /// A concrete Incan type token retained as its canonical checked spelling.
    Type(String),
    Option(Box<SemanticRegistryValue>),
    List(Vec<SemanticRegistryValue>),
    Dict(Vec<(SemanticRegistryValue, SemanticRegistryValue)>),
    ConstRef(Vec<String>),
    Newtype {
        name: String,
        value: Box<SemanticRegistryValue>,
    },
    Model {
        name: String,
        fields: Vec<(String, SemanticRegistryValue)>,
    },
}

impl fmt::Display for SemanticRegistryValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(value) => write!(f, "{value}"),
            Self::Float(value) => f.write_str(value),
            Self::Bool(value) => write!(f, "{value}"),
            Self::String(value) => write!(f, "{value:?}"),
            Self::Bytes(value) => write!(f, "bytes:{value:?}"),
            Self::None => f.write_str("None"),
            Self::Type(value) => write!(f, "Type[{value}]"),
            Self::Option(value) => write!(f, "Some({value})"),
            Self::List(values) => {
                f.write_str("[")?;
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    value.fmt(f)?;
                }
                f.write_str("]")
            }
            Self::Dict(entries) => {
                f.write_str("{")?;
                for (index, (key, value)) in entries.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    key.fmt(f)?;
                    f.write_str(": ")?;
                    value.fmt(f)?;
                }
                f.write_str("}")
            }
            Self::ConstRef(path) => f.write_str(&path.join(".")),
            Self::Newtype { name, value } => write!(f, "{name}({value})"),
            Self::Model { name, fields } => {
                write!(f, "{name}(")?;
                for (index, (field, value)) in fields.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{field}={value}")?;
                }
                f.write_str(")")
            }
        }
    }
}

/// One checked registry entry, preserved as structured source data rather than a rendered descriptor string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SemanticRegistryEntry {
    pub registry: CompilerNodeId,
    pub key: SemanticRegistryValue,
    pub descriptor: SemanticRegistryValue,
    pub subject_kind: SemanticRegistrySubjectKind,
    pub subject_identity: String,
}

impl fmt::Display for SemanticRegistryEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "registry={} key={} descriptor={} subject={}:{}",
            self.registry, self.key, self.descriptor, self.subject_kind, self.subject_identity
        )
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

/// Declaration category a canonical identity or semantic target names.
///
/// This is RFC 120's `kind` field. It deliberately covers every binding form the identity model reaches, not only the
/// declaration kinds today's codegraph targets happen to record, so a member and a local never have to be told apart
/// by a string. [`Self::Other`] remains for a frontend spelling this vocabulary has not adopted yet; it is a gap
/// marker, never a category.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticSourceTargetKind {
    /// A `def` declaration, free or associated.
    Function,
    /// A `model` declaration.
    Model,
    /// A `class` declaration.
    Class,
    /// A `newtype` declaration.
    Newtype,
    /// A `rusttype` declaration binding a Rust type.
    Rusttype,
    /// An `enum` declaration.
    Enum,
    /// A `type X = ...` alias.
    TypeAlias,
    /// A `partial` projection declaration.
    Partial,
    /// One variant of an enum.
    Variant,
    /// A `trait` declaration.
    Trait,
    /// A `capability` declaration naming an RFC 104 ambient runtime authority.
    Capability,
    /// A field on a nominal type.
    Field,
    /// A method on a nominal type or trait.
    Method,
    /// A computed property on a nominal type.
    Property,
    /// A `const` declaration.
    Const,
    /// A `static` storage cell.
    Static,
    /// A binding introduced inside a body by `let`, `mut`, assignment, `for`, `with ... as`, or `except ... as`.
    Local,
    /// A declared callable parameter.
    Parameter,
    /// A receiver binding (`self` or `cls`).
    Receiver,
    /// A generic type parameter, scoped to the declaration that introduces it.
    GenericBinder,
    /// A module.
    Module,
    /// An item reached through `rust::`.
    RustItem,
    /// A compiler-owned builtin beneath the ordinary lexical scope chain.
    Builtin,
    /// A frontend declaration spelling this vocabulary has not adopted. A gap marker, not a category: a consumer that
    /// branches on it is branching on a string.
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
            "type_alias" => Self::TypeAlias,
            "partial" => Self::Partial,
            "variant" => Self::Variant,
            "trait" => Self::Trait,
            "capability" => Self::Capability,
            "field" => Self::Field,
            "method" => Self::Method,
            "property" => Self::Property,
            "const" => Self::Const,
            "static" => Self::Static,
            "local" => Self::Local,
            "parameter" => Self::Parameter,
            "receiver" => Self::Receiver,
            "generic_binder" => Self::GenericBinder,
            "module" => Self::Module,
            "rust_item" => Self::RustItem,
            "builtin" => Self::Builtin,
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
            Self::TypeAlias => "type_alias",
            Self::Partial => "partial",
            Self::Variant => "variant",
            Self::Trait => "trait",
            Self::Capability => "capability",
            Self::Field => "field",
            Self::Method => "method",
            Self::Property => "property",
            Self::Const => "const",
            Self::Static => "static",
            Self::Local => "local",
            Self::Parameter => "parameter",
            Self::Receiver => "receiver",
            Self::GenericBinder => "generic_binder",
            Self::Module => "module",
            Self::RustItem => "rust_item",
            Self::Builtin => "builtin",
            Self::Other(kind) => kind,
        }
    }
}

/// RFC 104 run mode.
///
/// The mode is part of the decision rather than ambient context: the same request produces a different outcome under
/// `Governed` than under `Permissive`, and a consumer reading a stored decision must be able to tell which rule
/// produced it without re-deriving the run's configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityMode {
    /// Operations run normally with authority reporting disabled.
    Permissive,
    /// Operations run normally and receipts are emitted.
    Observe,
    /// Operations require granted capabilities and receipts are emitted.
    Governed,
}

impl Default for AuthorityMode {
    /// Observe authority-bearing operations unless a project-owned policy selects another mode.
    fn default() -> Self {
        Self::Observe
    }
}

impl AuthorityMode {
    /// Return the compact snapshot spelling for this mode.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Permissive => "permissive",
            Self::Observe => "observe",
            Self::Governed => "governed",
        }
    }
}

/// Why a governed authority request was denied.
///
/// This is the machine-usable denial reason RFC 104 requires. A consumer branches on the variant; the prose belongs to
/// the diagnostic that renders it, never to this fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityDenialReason {
    /// The invocation never requested this capability.
    NotGranted,
    /// The invocation requested the capability, but a host ceiling did not permit it.
    OutsideCeiling,
    /// The capability was granted, but not for the scope this operation requested.
    OutOfScope,
    /// A budget for this capability was exhausted before the operation ran.
    BudgetExhausted,
    /// Replay required a recorded fixture that was not available.
    FixtureRequired,
}

impl AuthorityDenialReason {
    /// Return the compact snapshot spelling for this denial reason.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotGranted => "not_granted",
            Self::OutsideCeiling => "outside_ceiling",
            Self::OutOfScope => "out_of_scope",
            Self::BudgetExhausted => "budget_exhausted",
            Self::FixtureRequired => "fixture_required",
        }
    }
}

/// The effect of an authority decision on one operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityOutcome {
    /// The operation may perform its authority-bearing behavior.
    Allowed,
    /// The operation must fail before performing its authority-bearing behavior.
    Denied(AuthorityDenialReason),
}

/// The grant context a decision was reached against.
///
/// RFC 104 makes the ceiling a distinct grant source from the per-invocation request, and requires the effective grant
/// to be their **intersection, never their union**: an invocation can only ever receive less than its ceiling allows,
/// regardless of what it asks for. The durable fact retains both the resulting grant set and the exact ceiling, because
/// `Allowed` under a ceiling and `Allowed` with no constraint are different facts about the run.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AuthorityGrantContext {
    /// Scope dimensions the operation requested, as `(dimension, value)` in the capability's declaration order.
    pub requested_scope: Vec<(String, String)>,
    /// The invocation's effective canonical capability grants after project policy and any host ceiling were
    /// intersected.
    pub effective_grants: Vec<CanonicalSymbolId>,
    /// The host-supplied capability ceiling that constrained this invocation, when one applied.
    pub ceiling: Option<Vec<CanonicalSymbolId>>,
}

/// Enough provenance to raise a source-owned governed denial diagnostic.
///
/// RFC 104 requires a denial to identify the required capability and to be reportable against the source that asked
/// for it. The requesting operation's canonical identity and the use-site span are what make that possible without a
/// consumer re-reading source text.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AuthorityProvenance {
    /// Canonical identity of the operation that requested the authority.
    pub operation: CanonicalSymbolId,
    /// The use site the request came from, which is where a denial is reported.
    pub request_span: crate::HirSourceSpan,
    /// Grant spelling to suggest in a denial diagnostic, such as `host.http.request`.
    pub suggested_grant: String,
}

/// One RFC 104 authority decision about one operation.
///
/// This is deliberately generic over capability publishers and provider operations: both the capability and the
/// requesting operation are named by [`CanonicalSymbolId`], so the stdlib, a library-defined domain capability, and a
/// provider operation all produce the same fact. A consumer can act on an allowed or denied decision without
/// consulting source text or emitted Rust, which is what lets a provider backend avoid inventing its own grant model.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AuthorityDecision {
    /// The capability whose authority was requested.
    pub capability: CanonicalSymbolId,
    /// The mode this decision was reached under.
    pub mode: AuthorityMode,
    /// Whether the operation may proceed.
    pub outcome: AuthorityOutcome,
    /// The grant context the decision was reached against.
    pub grant: AuthorityGrantContext,
    /// Where the decision can be reported in source.
    pub provenance: AuthorityProvenance,
}

impl AuthorityDecision {
    /// Build an allowed decision.
    pub fn allowed(
        capability: CanonicalSymbolId,
        mode: AuthorityMode,
        grant: AuthorityGrantContext,
        provenance: AuthorityProvenance,
    ) -> Self {
        Self {
            capability,
            mode,
            outcome: AuthorityOutcome::Allowed,
            grant,
            provenance,
        }
    }

    /// Build a denied decision carrying its machine-usable reason.
    pub fn denied(
        capability: CanonicalSymbolId,
        mode: AuthorityMode,
        reason: AuthorityDenialReason,
        grant: AuthorityGrantContext,
        provenance: AuthorityProvenance,
    ) -> Self {
        Self {
            capability,
            mode,
            outcome: AuthorityOutcome::Denied(reason),
            grant,
            provenance,
        }
    }

    /// Whether the operation may perform its authority-bearing behavior.
    pub const fn is_allowed(&self) -> bool {
        matches!(self.outcome, AuthorityOutcome::Allowed)
    }

    /// The denial reason, when this decision denied the operation.
    pub const fn denial_reason(&self) -> Option<AuthorityDenialReason> {
        match self.outcome {
            AuthorityOutcome::Allowed => None,
            AuthorityOutcome::Denied(reason) => Some(reason),
        }
    }
}

impl std::fmt::Display for AuthorityDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let outcome = match self.outcome {
            AuthorityOutcome::Allowed => "allowed".to_string(),
            AuthorityOutcome::Denied(reason) => format!("denied:{}", reason.as_str()),
        };
        let grants = render_authority_grants(&self.grant.effective_grants);
        let ceiling = self
            .grant
            .ceiling
            .as_ref()
            .map_or_else(|| "none".to_string(), |values| render_authority_grants(values));
        write!(
            f,
            "{} {} {} grants=[{}] ceiling=[{}] <- {}",
            self.capability.declaration_name,
            self.mode.as_str(),
            outcome,
            grants,
            ceiling,
            self.provenance.operation.declaration_name
        )
    }
}

/// Render canonical grant identities for an inspectable maintainer-facing fact snapshot.
fn render_authority_grants(grants: &[CanonicalSymbolId]) -> String {
    grants.iter().map(render_authority_grant).collect::<Vec<_>>().join(",")
}

/// Render every identity component that distinguishes one canonical capability grant from another.
fn render_authority_grant(grant: &CanonicalSymbolId) -> String {
    let origin = match &grant.origin {
        SymbolOrigin::Module(path) => format!("module:{}", path.join(".")),
        SymbolOrigin::Package { library, module_path } => {
            format!("package:{library}:{}", module_path.join("."))
        }
        SymbolOrigin::RustCrate(path) => format!("rust:{}", path.join(".")),
        SymbolOrigin::Builtin => "builtin".to_string(),
    };
    format!(
        "{origin}:{}:{}@{}..{}",
        grant.kind.as_str(),
        grant.declaration_name,
        grant.declaration_span.start,
        grant.declaration_span.end
    )
}

/// Render a module path into the identity string used by HIR, Body IR, and declaration identities.
///
/// One spelling, in one place: an empty path is a real case (a module checked without a path), and the frontend and
/// the data model silently disagreeing about whether that is `"<module>"` or `""` would produce two identities for
/// one declaration.
pub fn module_identity_for_path(module_path: &[String]) -> String {
    if module_path.is_empty() {
        "<module>".to_string()
    } else {
        module_path.join("::")
    }
}

/// Which of RFC 120's three namespaces a binding lives in.
///
/// Namespaces are distinguished by *how* a name is looked up, not by what kind of thing it names: a model name and a
/// function name share one namespace, exactly as ordinary Python-like lexical lookup expects. Carrying this in an
/// identity is what keeps a field named `items` and a local named `items` from ever comparing equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolNamespace {
    /// Module-level declarations, imports, aliases and re-exports, bare enum variant names, generic binders,
    /// parameters and receivers, and locals. Looked up innermost scope outward, then the builtin fallback tier.
    OrdinaryLexical,
    /// Fields, methods, computed properties, method aliases, and qualified enum variants. Reached `.`-directed from a
    /// resolved owner type, never through the scope chain.
    Member,
    /// Project module paths, the `std` root, `rust::` crate roots, and `pub::` library roots. Path-directed from a
    /// namespace root.
    ModulePath,
}

/// What owns a declaration, independent of who references it.
///
/// An import, alias, or re-export carries its *target's* origin, never the referencing module's. That is the property
/// that makes three different spellings of one declaration compare equal.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolOrigin {
    /// A project source module, by canonical module path.
    Module(Vec<String>),
    /// A `pub::` library, and the module path owning the declaration inside it.
    Package {
        /// Library root name.
        library: String,
        /// Module path within the library.
        module_path: Vec<String>,
    },
    /// A `rust::` crate root and item path.
    RustCrate(Vec<String>),
    /// The compiler-owned builtin registry beneath the ordinary lexical scope chain.
    Builtin,
}

/// Distinguishes bindings that are not unique within their origin.
///
/// Module-level declarations are already unique within their origin and carry no discriminant. Locals, parameters,
/// receivers, and generic binders are not: two `x` bindings in sibling blocks of one module must not collapse to one
/// identity, so those carry the scope that introduced them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ScopeDiscriminant(pub usize);

/// RFC 120 canonical symbol identity: what a resolved reference *means*.
///
/// Assigned once at a declaration site and unchanged by how the declaration is later referenced. A local declaration,
/// an import, an alias, and a re-export of one declaration all carry this same value; none of them creates a second
/// identity for the thing they name.
///
/// Two properties are load-bearing. Equality is decidable structurally, without comparing source spellings or emitted
/// names — no phase may recover what a reference means by parsing generated Rust. And identity is stable across the
/// stages of *one* compilation, not across edits: [`Self::declaration_span`] moves when the file does, so a consumer
/// needing cross-edit continuity must re-resolve rather than cache.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CanonicalSymbolId {
    /// Which namespace the binding lives in.
    pub namespace: SymbolNamespace,
    /// The module, package, crate, or registry owning the declaration.
    pub origin: SymbolOrigin,
    /// The spelling at the *declaration* site, never at a reference site.
    pub declaration_name: String,
    /// Declaration category.
    pub kind: SemanticSourceTargetKind,
    /// Present only for bindings that are not unique within their origin.
    pub scope_discriminant: Option<ScopeDiscriminant>,
    /// Provenance anchor: the one declaration site.
    pub declaration_span: crate::HirSourceSpan,
}

impl CanonicalSymbolId {
    /// Build the identity of a module-level declaration in a project source module.
    ///
    /// Module-level declarations are unique within their origin, so this deliberately takes no scope discriminant.
    pub fn module_declaration(
        module_path: Vec<String>,
        declaration_name: impl Into<String>,
        kind: SemanticSourceTargetKind,
        declaration_span: crate::HirSourceSpan,
    ) -> Self {
        Self {
            namespace: SymbolNamespace::OrdinaryLexical,
            origin: SymbolOrigin::Module(module_path),
            declaration_name: declaration_name.into(),
            kind,
            scope_discriminant: None,
            declaration_span,
        }
    }

    /// Return the owning module path when this identity is owned by a project source module.
    pub fn module_path(&self) -> Option<&[String]> {
        match &self.origin {
            SymbolOrigin::Module(path) => Some(path),
            _ => None,
        }
    }

    /// Render a deterministic, compact single-line spelling for maintainer-facing snapshots.
    ///
    /// This is a projection of the identity for humans; nothing may compare or dispatch on it. The shape is
    /// `<kind>:<origin>::<name>[#<scope>]@<start>..<end>`, with member- and path-namespace identities prefixed by
    /// their namespace so a member and a lexical binding sharing a spelling render visibly differently.
    pub fn render_compact(&self) -> String {
        let origin = match &self.origin {
            SymbolOrigin::Module(path) => module_identity_for_path(path),
            SymbolOrigin::Package { library, module_path } => {
                let mut parts = vec![format!("pub::{library}")];
                parts.extend(module_path.iter().cloned());
                parts.join("::")
            }
            SymbolOrigin::RustCrate(path) => format!("rust::{}", path.join("::")),
            SymbolOrigin::Builtin => "builtin".to_string(),
        };
        let namespace = match self.namespace {
            SymbolNamespace::OrdinaryLexical => "",
            SymbolNamespace::Member => "member/",
            SymbolNamespace::ModulePath => "path/",
        };
        let scope = self
            .scope_discriminant
            .map(|ScopeDiscriminant(scope)| format!("#{scope}"))
            .unwrap_or_default();
        format!(
            "{namespace}{}:{origin}::{}{scope}@{}..{}",
            self.kind.as_str(),
            self.declaration_name,
            self.declaration_span.start,
            self.declaration_span.end
        )
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

    /// Return compiler-owned canonical symbol identities for one source node.
    pub fn symbol_identities_for(&self, subject: &CompilerNodeId) -> impl Iterator<Item = &CanonicalSymbolId> {
        self.facts_for_kind(subject, SemanticFactKind::SymbolIdentity)
            .filter_map(|fact| match &fact.value {
                SemanticFactValue::CanonicalIdentity(identity) => Some(identity),
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
    /// Every declaration category round-trips through its own spelling.
    ///
    /// The two arms are hand-written and 22 variants long; a typo in either would silently reclassify a declaration
    /// as `Other`, which compares unequal to the variant it came from and would split one declaration's identity.
    /// Build a capability identity and a requesting-operation identity for authority-decision tests.
    fn authority_fixture() -> (super::CanonicalSymbolId, super::AuthorityProvenance) {
        use super::{AuthorityProvenance, CanonicalSymbolId, SemanticSourceTargetKind};

        let capability = CanonicalSymbolId::module_declaration(
            vec!["host".to_string(), "http".to_string()],
            "request",
            SemanticSourceTargetKind::Capability,
            crate::HirSourceSpan::new(10, 20),
        );
        let operation = CanonicalSymbolId::module_declaration(
            vec!["app".to_string(), "billing".to_string()],
            "charge",
            SemanticSourceTargetKind::Function,
            crate::HirSourceSpan::new(80, 96),
        );
        let provenance = AuthorityProvenance {
            operation,
            request_span: crate::HirSourceSpan::new(120, 140),
            suggested_grant: "host.http.request".to_string(),
        };
        (capability, provenance)
    }

    /// Build a distinct canonical capability for ceiling and identity-separation tests.
    fn fs_read_capability() -> super::CanonicalSymbolId {
        use super::{CanonicalSymbolId, SemanticSourceTargetKind};

        CanonicalSymbolId::module_declaration(
            vec!["host".to_string(), "fs".to_string()],
            "read",
            SemanticSourceTargetKind::Capability,
            crate::HirSourceSpan::new(30, 40),
        )
    }

    /// An allowed decision must be actionable without a consumer re-reading source or emitted Rust.
    #[test]
    fn an_allowed_authority_decision_carries_its_mode_and_grant_context() {
        use super::{AuthorityDecision, AuthorityGrantContext, AuthorityMode};

        let (capability, provenance) = authority_fixture();
        let decision = AuthorityDecision::allowed(
            capability.clone(),
            AuthorityMode::Governed,
            AuthorityGrantContext {
                requested_scope: vec![("host".to_string(), "api.example.com".to_string())],
                effective_grants: vec![capability.clone()],
                ceiling: Some(vec![capability.clone()]),
            },
            provenance,
        );

        assert!(decision.is_allowed());
        assert_eq!(decision.denial_reason(), None);
        assert_eq!(decision.mode, AuthorityMode::Governed);
        assert_eq!(decision.grant.effective_grants, vec![capability.clone()]);
        assert_eq!(decision.grant.ceiling, Some(vec![capability]));
        assert_eq!(decision.provenance.suggested_grant, "host.http.request");
    }

    /// A denied decision must carry a machine-usable reason and enough provenance to raise a source-owned diagnostic.
    #[test]
    fn a_denied_authority_decision_carries_a_machine_usable_reason_and_its_use_site() {
        use super::{AuthorityDecision, AuthorityDenialReason, AuthorityGrantContext, AuthorityMode};

        let (capability, provenance) = authority_fixture();
        let ceiling = fs_read_capability();
        let decision = AuthorityDecision::denied(
            capability,
            AuthorityMode::Governed,
            AuthorityDenialReason::OutsideCeiling,
            AuthorityGrantContext {
                requested_scope: Vec::new(),
                effective_grants: Vec::new(),
                ceiling: Some(vec![ceiling]),
            },
            provenance,
        );

        assert!(!decision.is_allowed());
        assert_eq!(decision.denial_reason(), Some(AuthorityDenialReason::OutsideCeiling));
        assert_eq!(
            decision.provenance.request_span,
            crate::HirSourceSpan::new(120, 140),
            "a denial is reported at the use site, not at the capability declaration",
        );
        assert_eq!(
            decision.provenance.operation.declaration_name, "charge",
            "the requesting operation stays identified so the diagnostic can name it",
        );
    }

    /// The fact must be generic over capability publishers rather than assuming a stdlib host capability.
    #[test]
    fn authority_decisions_work_for_a_library_defined_capability() {
        use super::{
            AuthorityDecision, AuthorityDenialReason, AuthorityGrantContext, AuthorityMode, CanonicalSymbolId,
            SemanticSourceTargetKind,
        };

        let (_, provenance) = authority_fixture();
        let package_capability = CanonicalSymbolId::module_declaration(
            vec!["acme".to_string(), "ledger".to_string()],
            "post_entry",
            SemanticSourceTargetKind::Capability,
            crate::HirSourceSpan::new(4, 14),
        );
        let decision = AuthorityDecision::denied(
            package_capability,
            AuthorityMode::Governed,
            AuthorityDenialReason::NotGranted,
            AuthorityGrantContext {
                requested_scope: Vec::new(),
                effective_grants: Vec::new(),
                ceiling: None,
            },
            provenance,
        );

        assert_eq!(decision.capability.kind, SemanticSourceTargetKind::Capability);
        assert_eq!(decision.capability.declaration_name, "post_entry");
        assert_eq!(decision.denial_reason(), Some(AuthorityDenialReason::NotGranted));
    }

    /// Every mode and denial reason needs a distinct snapshot spelling.
    #[test]
    fn authority_modes_and_denial_reasons_have_distinct_spellings() {
        use super::{AuthorityDenialReason as R, AuthorityMode as M};

        let modes = [M::Permissive, M::Observe, M::Governed];
        let mode_spellings: std::collections::HashSet<&str> = modes.iter().map(|m| m.as_str()).collect();
        assert_eq!(mode_spellings.len(), modes.len(), "two modes share one spelling");

        let reasons = [
            R::NotGranted,
            R::OutsideCeiling,
            R::OutOfScope,
            R::BudgetExhausted,
            R::FixtureRequired,
        ];
        let reason_spellings: std::collections::HashSet<&str> = reasons.iter().map(|r| r.as_str()).collect();
        assert_eq!(
            reason_spellings.len(),
            reasons.len(),
            "two denial reasons share one spelling",
        );
    }

    #[test]
    fn every_source_target_kind_round_trips_through_its_spelling() {
        use super::SemanticSourceTargetKind as K;
        let all = [
            K::Function,
            K::Model,
            K::Class,
            K::Newtype,
            K::Rusttype,
            K::Enum,
            K::TypeAlias,
            K::Partial,
            K::Variant,
            K::Trait,
            K::Capability,
            K::Field,
            K::Method,
            K::Property,
            K::Const,
            K::Static,
            K::Local,
            K::Parameter,
            K::Receiver,
            K::GenericBinder,
            K::Module,
            K::RustItem,
            K::Builtin,
        ];
        for kind in &all {
            assert_eq!(K::from_kind_str(kind.as_str()), *kind, "round trip failed for {kind}");
        }
        let spellings: std::collections::HashSet<&str> = all.iter().map(|kind| kind.as_str()).collect();
        assert_eq!(spellings.len(), all.len(), "two categories share one spelling");
    }

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
            CompilerNodeId::declaration_binding_span("pkg::module", 3, 17, 1).to_string(),
            "decl:pkg::module#decl.3..17.binding.1"
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
        let identity = CanonicalSymbolId::module_declaration(
            vec!["pkg".to_string()],
            "build",
            SemanticSourceTargetKind::Function,
            crate::HirSourceSpan::new(10, 25),
        );
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
        store.insert(SemanticFact::new(
            expr.clone(),
            SemanticFactKind::SymbolIdentity,
            SemanticFactValue::canonical_identity(identity.clone()),
        ));

        let type_facts = store.type_facts_for(&expr).cloned().collect::<Vec<_>>();
        assert_eq!(type_facts, vec![IncanType::Primitive(crate::IncanPrimitiveType::Int)]);

        let source_targets = store.source_targets_for(&expr).cloned().collect::<Vec<_>>();
        assert_eq!(source_targets, vec![target]);

        let identities = store.symbol_identities_for(&expr).cloned().collect::<Vec<_>>();
        assert_eq!(identities, vec![identity]);
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

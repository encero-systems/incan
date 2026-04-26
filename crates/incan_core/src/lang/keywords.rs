//! Define the reserved keyword vocabulary for the Incan language.
//!
//! This module is the single source of truth for reserved words: a stable identifier ([`KeywordId`]) plus a const
//! metadata table ([`KEYWORDS`]) that records canonical spellings, aliases, categories, provenance, and examples.
//!
//! ## Notes
//! - Lookup via [`from_str`] is **case-sensitive**, except where explicit aliases are defined.
//! - This registry is intentionally **pure** (no AST/IO/side effects).
//! - Some reserved words are also “word operators” (e.g. `and`). If you need operator precedence/fixity, use
//!   [`crate::lang::operators`].
//!
//! ## Examples
//! ```rust
//! use incan_core::lang::keywords::{self, KeywordId};
//!
//! assert_eq!(keywords::from_str("if"), Some(KeywordId::If));
//! assert_eq!(keywords::as_str(KeywordId::If), "if");
//! ```
//!
//! ## See also
//! - [`crate::lang::operators`] for operator precedence/fixity metadata.

use super::registry::{Example, RFC, RfcId, Since, Stability};

/// Stable identifier for every reserved keyword.
///
/// ## Notes
/// - The canonical spelling is accessible via [`as_str`].
/// - Accepted aliases are accessible via [`aliases`].
///
/// ## Examples
/// ```rust
/// use incan_core::lang::keywords::{self, KeywordId};
///
/// assert_eq!(keywords::from_str("def"), Some(KeywordId::Def));
/// assert_eq!(keywords::from_str("fn"), Some(KeywordId::Def)); // alias
/// assert_eq!(keywords::as_str(KeywordId::Def), "def");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeywordId {
    // Control flow / statements
    If,
    Else,
    Elif,
    Match,
    Case,
    Loop,
    While,
    For,
    Break,
    Continue,
    Return,
    Yield,
    Pass,
    Assert,

    // Definitions / declarations
    Def,
    Async,
    Await,
    Class,
    Model,
    Trait,
    Enum,
    Type,
    Newtype,
    With,
    Extends,
    Pub,

    // Imports / modules / interop
    Import,
    From,
    As,
    Rust,
    Python,
    Super,
    Crate,

    // Bindings / receivers
    Const,
    Static,
    Let,
    Mut,
    SelfKw,

    // Literals
    True,
    False,
    None,

    // Word operators
    And,
    Or,
    Not,
    In,
    Is,
}

/// High-level grouping for documentation and tooling.
///
/// ## Notes
/// - Categories are metadata only; they do not enforce parsing context.
/// - Prefer [`KeywordDescriptor::category`] when presenting keyword tables in docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeywordCategory {
    ControlFlow,
    Definition,
    Import,
    Binding,
    Literal,
    Operator,
}

/// Usage context hints (not enforced here; parser/lexer own context).
///
/// ## Notes
/// - These are intended for formatter/tooling and better diagnostics (“expected …”).
/// - The lexer/parser remain the source of truth for syntactic legality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeywordUsage {
    Statement,
    Expression,
    Modifier,
    Operator,
    ReceiverOnly,
}

/// Activation mode for a keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeywordActivation {
    /// Hard keyword: always reserved.
    Hard,
    /// Contextual keyword: always available in a parser-owned syntactic position without being reserved elsewhere.
    Contextual,
    /// Soft keyword: reserved only after importing the activating stdlib namespace.
    Soft { namespace: &'static str },
}

/// Parser handoff shape for registry-driven surface keywords.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeywordSurfaceKind {
    StatementKeywordArgs,
    PrefixExpression,
    DeclarationModifier,
}

/// Metadata for a keyword.
///
/// ## Notes
/// - `canonical` is the preferred spelling for docs and emission.
/// - `aliases` are additional spellings accepted by the compiler.
/// - `examples` are intended for generated documentation; keep them small and focused.
#[derive(Debug, Clone, Copy)]
pub struct KeywordDescriptor {
    pub id: KeywordId,
    pub canonical: &'static str,
    pub aliases: &'static [&'static str],
    pub activation: KeywordActivation,
    /// Parser handoff metadata for surface dispatch.
    pub surface_kind: Option<KeywordSurfaceKind>,
    pub category: KeywordCategory,
    pub usage: &'static [KeywordUsage],
    pub introduced_in_rfc: RfcId,
    pub since: Since,
    pub stability: Stability,
    pub examples: &'static [Example],
}

/// Registry of all keywords.
///
/// ## Notes
/// - The ordering is not semantically meaningful, but is grouped for readability.
pub const KEYWORDS: &[KeywordDescriptor] = &[
    // Control flow / statements
    info(
        KeywordId::If,
        "if",
        &[],
        KeywordCategory::ControlFlow,
        &[KeywordUsage::Statement, KeywordUsage::Expression],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::Else,
        "else",
        &[],
        KeywordCategory::ControlFlow,
        &[KeywordUsage::Statement],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::Elif,
        "elif",
        &[],
        KeywordCategory::ControlFlow,
        &[KeywordUsage::Statement],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::Match,
        "match",
        &[],
        KeywordCategory::ControlFlow,
        &[KeywordUsage::Statement, KeywordUsage::Expression],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::Case,
        "case",
        &[],
        KeywordCategory::ControlFlow,
        &[KeywordUsage::Statement],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::Loop,
        "loop",
        &[],
        KeywordCategory::ControlFlow,
        &[KeywordUsage::Statement, KeywordUsage::Expression],
        RFC::_016,
        Since(0, 3),
    ),
    info(
        KeywordId::While,
        "while",
        &[],
        KeywordCategory::ControlFlow,
        &[KeywordUsage::Statement],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::For,
        "for",
        &[],
        KeywordCategory::ControlFlow,
        &[KeywordUsage::Statement],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::Break,
        "break",
        &[],
        KeywordCategory::ControlFlow,
        &[KeywordUsage::Statement],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::Continue,
        "continue",
        &[],
        KeywordCategory::ControlFlow,
        &[KeywordUsage::Statement],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::Return,
        "return",
        &[],
        KeywordCategory::ControlFlow,
        &[KeywordUsage::Statement],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::Yield,
        "yield",
        &[],
        KeywordCategory::ControlFlow,
        &[KeywordUsage::Statement, KeywordUsage::Expression],
        RFC::_001,
        Since(0, 1),
    ),
    info(
        KeywordId::Pass,
        "pass",
        &[],
        KeywordCategory::ControlFlow,
        &[KeywordUsage::Statement],
        RFC::_000,
        Since(0, 1),
    ),
    info_contextual(
        KeywordId::Assert,
        "assert",
        KeywordSurfaceKind::StatementKeywordArgs,
        KeywordCategory::ControlFlow,
        &[KeywordUsage::Statement],
        RFC::_018,
        Since(0, 3),
        Stability::Draft,
    ),
    // Definitions / declarations
    info_with_aliases(
        KeywordId::Def,
        "def",
        &["fn"],
        KeywordCategory::Definition,
        &[KeywordUsage::Statement],
        RFC::_000,
        Since(0, 1),
    ),
    info_soft(
        KeywordId::Async,
        "async",
        "async",
        KeywordSurfaceKind::DeclarationModifier,
        KeywordCategory::Definition,
        &[KeywordUsage::Modifier],
        RFC::_000,
        Since(0, 1),
        Stability::Stable,
    ),
    info_soft(
        KeywordId::Await,
        "await",
        "async",
        KeywordSurfaceKind::PrefixExpression,
        KeywordCategory::Definition,
        &[KeywordUsage::Expression],
        RFC::_000,
        Since(0, 1),
        Stability::Stable,
    ),
    info(
        KeywordId::Class,
        "class",
        &[],
        KeywordCategory::Definition,
        &[KeywordUsage::Statement],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::Model,
        "model",
        &[],
        KeywordCategory::Definition,
        &[KeywordUsage::Statement],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::Trait,
        "trait",
        &[],
        KeywordCategory::Definition,
        &[KeywordUsage::Statement],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::Enum,
        "enum",
        &[],
        KeywordCategory::Definition,
        &[KeywordUsage::Statement],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::Type,
        "type",
        &[],
        KeywordCategory::Definition,
        &[KeywordUsage::Statement],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::Newtype,
        "newtype",
        &[],
        KeywordCategory::Definition,
        &[KeywordUsage::Statement],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::With,
        "with",
        &[],
        KeywordCategory::Definition,
        &[KeywordUsage::Modifier],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::Extends,
        "extends",
        &[],
        KeywordCategory::Definition,
        &[KeywordUsage::Modifier],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::Pub,
        "pub",
        &[],
        KeywordCategory::Definition,
        &[KeywordUsage::Modifier],
        RFC::_000,
        Since(0, 1),
    ),
    // Imports / modules / interop
    info(
        KeywordId::Import,
        "import",
        &[],
        KeywordCategory::Import,
        &[KeywordUsage::Statement],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::From,
        "from",
        &[],
        KeywordCategory::Import,
        &[KeywordUsage::Statement],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::As,
        "as",
        &[],
        KeywordCategory::Import,
        &[KeywordUsage::Modifier],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::Rust,
        "rust",
        &[],
        KeywordCategory::Import,
        &[KeywordUsage::Modifier],
        RFC::_005,
        Since(0, 1),
    ),
    info(
        KeywordId::Python,
        "python",
        &[],
        KeywordCategory::Import,
        &[KeywordUsage::Modifier],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::Super,
        "super",
        &[],
        KeywordCategory::Import,
        &[KeywordUsage::Expression],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::Crate,
        "crate",
        &[],
        KeywordCategory::Import,
        &[KeywordUsage::Expression],
        RFC::_005,
        Since(0, 1),
    ),
    // Bindings / receivers
    info(
        KeywordId::Const,
        "const",
        &[],
        KeywordCategory::Binding,
        &[KeywordUsage::Statement],
        RFC::_008,
        Since(0, 1),
    ),
    info(
        KeywordId::Static,
        "static",
        &[],
        KeywordCategory::Binding,
        &[KeywordUsage::Statement],
        RFC::_052,
        Since(0, 2),
    ),
    info(
        KeywordId::Let,
        "let",
        &[],
        KeywordCategory::Binding,
        &[KeywordUsage::Statement],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::Mut,
        "mut",
        &[],
        KeywordCategory::Binding,
        &[KeywordUsage::Modifier],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::SelfKw,
        "self",
        &[],
        KeywordCategory::Binding,
        &[KeywordUsage::ReceiverOnly],
        RFC::_000,
        Since(0, 1),
    ),
    // Literals
    info_with_aliases(
        KeywordId::True,
        "true",
        &["True"],
        KeywordCategory::Literal,
        &[KeywordUsage::Expression],
        RFC::_000,
        Since(0, 1),
    ),
    info_with_aliases(
        KeywordId::False,
        "false",
        &["False"],
        KeywordCategory::Literal,
        &[KeywordUsage::Expression],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::None,
        "None",
        &[],
        KeywordCategory::Literal,
        &[KeywordUsage::Expression],
        RFC::_000,
        Since(0, 1),
    ),
    // Word operators
    info(
        KeywordId::And,
        "and",
        &[],
        KeywordCategory::Operator,
        &[KeywordUsage::Operator],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::Or,
        "or",
        &[],
        KeywordCategory::Operator,
        &[KeywordUsage::Operator],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::Not,
        "not",
        &[],
        KeywordCategory::Operator,
        &[KeywordUsage::Operator],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::In,
        "in",
        &[],
        KeywordCategory::Operator,
        &[KeywordUsage::Operator],
        RFC::_000,
        Since(0, 1),
    ),
    info(
        KeywordId::Is,
        "is",
        &[],
        KeywordCategory::Operator,
        &[KeywordUsage::Operator],
        RFC::_000,
        Since(0, 1),
    ),
];

/// Canonical spelling.
///
/// ## Parameters
/// - `id`: Keyword identifier.
///
/// ## Returns
/// - The canonical spelling for `id`.
pub fn as_str(id: KeywordId) -> &'static str {
    info_for(id).canonical
}

/// Aliases.
///
/// ## Parameters
/// - `id`: Keyword identifier.
///
/// ## Returns
/// - A slice of accepted alias spellings.
pub fn aliases(id: KeywordId) -> &'static [&'static str] {
    info_for(id).aliases
}

/// Category.
///
/// ## Parameters
/// - `id`: Keyword identifier.
///
/// ## Returns
/// - The keyword's [`KeywordCategory`].
pub fn category(id: KeywordId) -> KeywordCategory {
    info_for(id).category
}

/// Usage hints.
///
/// ## Parameters
/// - `id`: Keyword identifier.
///
/// ## Returns
/// - A slice of usage hints for the keyword.
pub fn usage(id: KeywordId) -> &'static [KeywordUsage] {
    info_for(id).usage
}

/// Parser surface handoff metadata for a keyword.
pub fn surface_kind(id: KeywordId) -> Option<KeywordSurfaceKind> {
    info_for(id).surface_kind
}

/// Whether a keyword supports the given surface handoff kind.
pub fn supports_surface_kind(id: KeywordId, kind: KeywordSurfaceKind) -> bool {
    surface_kind(id) == Some(kind)
}

/// Activation namespace for soft keywords.
///
/// ## Returns
/// - `Some("namespace")` if this keyword is import-activated.
/// - `None` if this keyword is hard or contextual.
pub fn activation(id: KeywordId) -> Option<&'static str> {
    match info_for(id).activation {
        KeywordActivation::Hard => None,
        KeywordActivation::Contextual => None,
        KeywordActivation::Soft { namespace } => Some(namespace),
    }
}

/// Full activation metadata for a keyword.
pub fn activation_kind(id: KeywordId) -> KeywordActivation {
    info_for(id).activation
}

/// Whether a keyword is soft at the lexer layer.
///
/// Contextual and import-activated keywords both lex as identifiers until the parser accepts them in an allowed
/// context.
pub fn is_soft(id: KeywordId) -> bool {
    matches!(
        activation_kind(id),
        KeywordActivation::Contextual | KeywordActivation::Soft { .. }
    )
}

/// Whether a keyword is lexically reserved in all contexts.
pub fn is_hard(id: KeywordId) -> bool {
    matches!(activation_kind(id), KeywordActivation::Hard)
}

/// Full metadata.
///
/// ## Parameters
/// - `id`: Keyword identifier.
///
/// ## Returns
/// - The associated [`KeywordDescriptor`] from [`KEYWORDS`].
///
/// ## Panics
/// - If the registry is missing an entry for `id` (this indicates a programming error).
pub fn info_for(id: KeywordId) -> &'static KeywordDescriptor {
    match KEYWORDS.iter().find(|k| k.id == id) {
        Some(info) => info,
        None => panic!("keyword info missing for {:?}", id),
    }
}

/// Lookup by spelling (canonical or alias).
///
/// ## Parameters
/// - `s`: Candidate keyword spelling (canonical or alias).
///
/// ## Returns
/// - `Some(KeywordId)` if the spelling matches this registry.
/// - `None` otherwise.
///
/// ## Notes
/// - Matching is **case-sensitive**, except where aliases are explicitly defined.
pub fn from_str(s: &str) -> Option<KeywordId> {
    if let Some(k) = KEYWORDS.iter().find(|k| k.canonical == s) {
        return Some(k.id);
    }
    KEYWORDS
        .iter()
        .find(|k| {
            let aliases: &[&str] = k.aliases;
            aliases.contains(&s)
        })
        .map(|k| k.id)
}

/// Lookup by spelling, excluding contextual and soft keywords.
///
/// Used by the lexer to reserve only hard keywords globally.
pub fn from_str_hard_only(s: &str) -> Option<KeywordId> {
    from_str(s).filter(|id| is_hard(*id))
}

/// Resolve soft keywords activated by a stdlib namespace.
pub fn soft_keywords_for_namespace(namespace: &str) -> Vec<KeywordId> {
    KEYWORDS
        .iter()
        .filter_map(|k| match k.activation {
            KeywordActivation::Soft {
                namespace: activation_namespace,
            } if activation_namespace == namespace => Some(k.id),
            _ => None,
        })
        .collect()
}

// --- helpers -----------------------------------------------------------------

/// Build metadata for a hard keyword without aliases or examples.
const fn info(
    id: KeywordId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    category: KeywordCategory,
    usage: &'static [KeywordUsage],
    introduced_in_rfc: RfcId,
    since: Since,
) -> KeywordDescriptor {
    KeywordDescriptor {
        id,
        canonical,
        aliases,
        activation: KeywordActivation::Hard,
        surface_kind: None,
        category,
        usage,
        introduced_in_rfc,
        since,
        stability: Stability::Stable,
        examples: &[],
    }
}

/// Build metadata for a hard keyword with accepted aliases.
const fn info_with_aliases(
    id: KeywordId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    category: KeywordCategory,
    usage: &'static [KeywordUsage],
    introduced_in_rfc: RfcId,
    since: Since,
) -> KeywordDescriptor {
    info(id, canonical, aliases, category, usage, introduced_in_rfc, since)
}

#[allow(clippy::too_many_arguments)] // const table constructor mirrors `info()` — a struct param would be awkward here
/// Build metadata for an import-activated soft keyword.
const fn info_soft(
    id: KeywordId,
    canonical: &'static str,
    activation_namespace: &'static str,
    surface_kind: KeywordSurfaceKind,
    category: KeywordCategory,
    usage: &'static [KeywordUsage],
    introduced_in_rfc: RfcId,
    since: Since,
    stability: Stability,
) -> KeywordDescriptor {
    KeywordDescriptor {
        id,
        canonical,
        aliases: &[],
        activation: KeywordActivation::Soft {
            namespace: activation_namespace,
        },
        surface_kind: Some(surface_kind),
        category,
        usage,
        introduced_in_rfc,
        since,
        stability,
        examples: &[],
    }
}

#[allow(clippy::too_many_arguments)] // const table constructor mirrors the keyword metadata shape
/// Build metadata for an always-available contextual keyword.
const fn info_contextual(
    id: KeywordId,
    canonical: &'static str,
    surface_kind: KeywordSurfaceKind,
    category: KeywordCategory,
    usage: &'static [KeywordUsage],
    introduced_in_rfc: RfcId,
    since: Since,
    stability: Stability,
) -> KeywordDescriptor {
    KeywordDescriptor {
        id,
        canonical,
        aliases: &[],
        activation: KeywordActivation::Contextual,
        surface_kind: Some(surface_kind),
        category,
        usage,
        introduced_in_rfc,
        since,
        stability,
        examples: &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assert_keyword_is_contextual_and_not_import_activated() {
        assert_eq!(activation_kind(KeywordId::Assert), KeywordActivation::Contextual);
        assert_eq!(activation(KeywordId::Assert), None);
        assert!(is_soft(KeywordId::Assert));
        assert!(!soft_keywords_for_namespace("testing").contains(&KeywordId::Assert));
    }

    #[test]
    fn hard_only_lookup_excludes_contextual_assert() {
        assert_eq!(from_str("assert"), Some(KeywordId::Assert));
        assert_eq!(from_str_hard_only("assert"), None);
    }
}

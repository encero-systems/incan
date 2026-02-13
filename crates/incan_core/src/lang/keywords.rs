//! Define the reserved keyword vocabulary for the Incan language.
//!
//! This module is the single source of truth for reserved words: a stable identifier
//! ([`KeywordId`]) plus a const metadata table ([`KEYWORDS`]) that records canonical spellings,
//! aliases, categories, provenance, and examples.
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
    While,
    For,
    Break,
    Continue,
    Return,
    Yield,
    Pass,

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
/// - Prefer [`KeywordInfo::category`] when presenting keyword tables in docs.
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

/// Metadata for a keyword.
///
/// ## Notes
/// - `canonical` is the preferred spelling for docs and emission.
/// - `aliases` are additional spellings accepted by the compiler.
/// - `examples` are intended for generated documentation; keep them small and focused.
#[derive(Debug, Clone, Copy)]
pub struct KeywordInfo {
    pub id: KeywordId,
    pub canonical: &'static str,
    pub aliases: &'static [&'static str],
    /// Optional stdlib namespace that activates this keyword as a soft keyword.
    ///
    /// `None` means this is a hard keyword that is always reserved.
    pub activation: Option<&'static str>,
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
pub const KEYWORDS: &[KeywordInfo] = &[
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
    KeywordInfo {
        id: KeywordId::Async,
        canonical: "async",
        aliases: &[],
        activation: Some("async"),
        category: KeywordCategory::Definition,
        usage: &[KeywordUsage::Modifier],
        introduced_in_rfc: RFC::_000,
        since: Since(0, 1),
        stability: Stability::Stable,
        examples: &[],
    },
    KeywordInfo {
        id: KeywordId::Await,
        canonical: "await",
        aliases: &[],
        activation: Some("async"),
        category: KeywordCategory::Definition,
        usage: &[KeywordUsage::Expression],
        introduced_in_rfc: RFC::_000,
        since: Since(0, 1),
        stability: Stability::Stable,
        examples: &[],
    },
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

/// Activation namespace for soft keywords.
///
/// ## Returns
/// - `Some("namespace")` if this keyword is import-activated.
/// - `None` if this is a hard keyword.
pub fn activation(id: KeywordId) -> Option<&'static str> {
    info_for(id).activation
}

/// Whether a keyword is soft (import-activated).
pub fn is_soft(id: KeywordId) -> bool {
    activation(id).is_some()
}

/// Full metadata.
///
/// ## Parameters
/// - `id`: Keyword identifier.
///
/// ## Returns
/// - The associated [`KeywordInfo`] from [`KEYWORDS`].
///
/// ## Panics
/// - If the registry is missing an entry for `id` (this indicates a programming error).
pub fn info_for(id: KeywordId) -> &'static KeywordInfo {
    KEYWORDS.iter().find(|k| k.id == id).expect("keyword info missing")
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

/// Lookup by spelling, excluding soft keywords.
///
/// Used by the lexer to reserve only hard keywords globally.
pub fn from_str_hard_only(s: &str) -> Option<KeywordId> {
    from_str(s).filter(|id| !is_soft(*id))
}

// --- helpers -----------------------------------------------------------------

const fn info(
    id: KeywordId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    category: KeywordCategory,
    usage: &'static [KeywordUsage],
    introduced_in_rfc: RfcId,
    since: Since,
) -> KeywordInfo {
    KeywordInfo {
        id,
        canonical,
        aliases,
        activation: None,
        category,
        usage,
        introduced_in_rfc,
        since,
        stability: Stability::Stable,
        examples: &[],
    }
}

const fn info_with_aliases(
    id: KeywordId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    category: KeywordCategory,
    usage: &'static [KeywordUsage],
    introduced_in_rfc: RfcId,
    since: Since,
) -> KeywordInfo {
    info(id, canonical, aliases, category, usage, introduced_in_rfc, since)
}

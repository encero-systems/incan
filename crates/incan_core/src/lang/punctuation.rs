//! Punctuation vocabulary.
//!
//! This module defines the canonical set of non-operator punctuation tokens used by the
//! lexer/parser: delimiters, separators, access/path markers, and a few structural markers.
//!
//! ## Notes
//! - Lookup via [`from_str`] is **case-sensitive**.
//! - This module is vocabulary only (spellings + metadata). It does not tokenize source text.
//!
//! ## Examples
//! ```rust
//! use incan_core::lang::punctuation::{self, PunctuationId};
//!
//! assert_eq!(punctuation::from_str("::"), Some(PunctuationId::ColonColon));
//! assert_eq!(punctuation::as_str(PunctuationId::FatArrow), "=>");
//! ```

use super::registry::{Example, RFC, RfcId, Since, Stability};

/// Broad syntactic grouping for punctuation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PunctuationCategory {
    /// Brackets and braces.
    Delimiter,
    /// Separators like `,` and `:`.
    Separator,
    /// Access/path markers like `.` and `::`.
    Access,
    /// Arrow markers like `->` and `=>`.
    Arrow,
    /// Misc markers like `?`, `@`, `...`.
    Marker,
}

/// Stable identifier for punctuation tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PunctuationId {
    // Separators / markers
    Comma,
    Colon,
    Question,
    At,
    Pipe,

    // Access / path
    Dot,
    ColonColon,

    // Structural arrows
    Arrow,
    FatArrow,

    // Special markers
    Ellipsis,

    // Delimiters
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
}

/// Metadata for a punctuation token.
#[derive(Debug, Clone, Copy)]
pub struct PunctuationInfo {
    pub id: PunctuationId,
    pub canonical: &'static str,
    pub aliases: &'static [&'static str],
    pub category: PunctuationCategory,
    pub introduced_in_rfc: RfcId,
    pub since: Since,
    pub stability: Stability,
    pub examples: &'static [Example],
}

/// Registry of all punctuation tokens.
pub const PUNCTUATION: &[PunctuationInfo] = &[
    // Separators / markers
    info(
        PunctuationId::Comma,
        ",",
        &[],
        PunctuationCategory::Separator,
        RFC::_000,
        Since(0, 1),
    ),
    info(
        PunctuationId::Colon,
        ":",
        &[],
        PunctuationCategory::Separator,
        RFC::_000,
        Since(0, 1),
    ),
    info(
        PunctuationId::Question,
        "?",
        &[],
        PunctuationCategory::Marker,
        RFC::_000,
        Since(0, 1),
    ),
    info(
        PunctuationId::At,
        "@",
        &[],
        PunctuationCategory::Marker,
        RFC::_000,
        Since(0, 1),
    ),
    info(
        PunctuationId::Pipe,
        "|",
        &[],
        PunctuationCategory::Marker,
        RFC::_000,
        Since(0, 1),
    ),
    // Access / path
    info(
        PunctuationId::Dot,
        ".",
        &[],
        PunctuationCategory::Access,
        RFC::_000,
        Since(0, 1),
    ),
    info(
        PunctuationId::ColonColon,
        "::",
        &[],
        PunctuationCategory::Access,
        RFC::_000,
        Since(0, 1),
    ),
    // Arrows
    info(
        PunctuationId::Arrow,
        "->",
        &[],
        PunctuationCategory::Arrow,
        RFC::_000,
        Since(0, 1),
    ),
    info(
        PunctuationId::FatArrow,
        "=>",
        &[],
        PunctuationCategory::Arrow,
        RFC::_000,
        Since(0, 1),
    ),
    // Special markers
    info(
        PunctuationId::Ellipsis,
        "...",
        &[],
        PunctuationCategory::Marker,
        RFC::_000,
        Since(0, 1),
    ),
    // Delimiters
    info(
        PunctuationId::LParen,
        "(",
        &[],
        PunctuationCategory::Delimiter,
        RFC::_000,
        Since(0, 1),
    ),
    info(
        PunctuationId::RParen,
        ")",
        &[],
        PunctuationCategory::Delimiter,
        RFC::_000,
        Since(0, 1),
    ),
    info(
        PunctuationId::LBracket,
        "[",
        &[],
        PunctuationCategory::Delimiter,
        RFC::_000,
        Since(0, 1),
    ),
    info(
        PunctuationId::RBracket,
        "]",
        &[],
        PunctuationCategory::Delimiter,
        RFC::_000,
        Since(0, 1),
    ),
    info(
        PunctuationId::LBrace,
        "{",
        &[],
        PunctuationCategory::Delimiter,
        RFC::_000,
        Since(0, 1),
    ),
    info(
        PunctuationId::RBrace,
        "}",
        &[],
        PunctuationCategory::Delimiter,
        RFC::_000,
        Since(0, 1),
    ),
];

/// Return the canonical spelling for a punctuation token.
pub fn as_str(id: PunctuationId) -> &'static str {
    info_for(id).canonical
}

/// Return the accepted aliases for a punctuation token.
pub fn aliases(id: PunctuationId) -> &'static [&'static str] {
    info_for(id).aliases
}

/// Return the category for a punctuation token.
pub fn category(id: PunctuationId) -> PunctuationCategory {
    info_for(id).category
}

/// Return the full metadata entry for a punctuation token.
///
/// The lookup is exhaustive over the closed enum, so adding punctuation requires updating this match at compile time.
pub fn info_for(id: PunctuationId) -> PunctuationInfo {
    match id {
        PunctuationId::Comma => PUNCTUATION[0],
        PunctuationId::Colon => PUNCTUATION[1],
        PunctuationId::Question => PUNCTUATION[2],
        PunctuationId::At => PUNCTUATION[3],
        PunctuationId::Pipe => PUNCTUATION[4],
        PunctuationId::Dot => PUNCTUATION[5],
        PunctuationId::ColonColon => PUNCTUATION[6],
        PunctuationId::Arrow => PUNCTUATION[7],
        PunctuationId::FatArrow => PUNCTUATION[8],
        PunctuationId::Ellipsis => PUNCTUATION[9],
        PunctuationId::LParen => PUNCTUATION[10],
        PunctuationId::RParen => PUNCTUATION[11],
        PunctuationId::LBracket => PUNCTUATION[12],
        PunctuationId::RBracket => PUNCTUATION[13],
        PunctuationId::LBrace => PUNCTUATION[14],
        PunctuationId::RBrace => PUNCTUATION[15],
    }
}

/// Resolve a punctuation spelling to its identifier.
///
/// ## Notes
/// - Matching is **case-sensitive**.
pub fn from_str(s: &str) -> Option<PunctuationId> {
    if let Some(p) = PUNCTUATION.iter().find(|p| p.canonical == s) {
        return Some(p.id);
    }
    PUNCTUATION
        .iter()
        .find(|p| {
            let aliases: &[&str] = p.aliases;
            aliases.contains(&s)
        })
        .map(|p| p.id)
}

const fn info(
    id: PunctuationId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    category: PunctuationCategory,
    introduced_in_rfc: RfcId,
    since: Since,
) -> PunctuationInfo {
    PunctuationInfo {
        id,
        canonical,
        aliases,
        category,
        introduced_in_rfc,
        since,
        stability: Stability::Stable,
        examples: &[],
    }
}

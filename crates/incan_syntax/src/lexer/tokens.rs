//! Token types for the Incan lexer.
//!
//! The lexer uses **registry-backed IDs** for language vocabulary:
//! - `Keyword(KeywordId)` for reserved words
//! - `Operator(OperatorId)` for operators (including word-operators like `and`)
//! - `Punctuation(PunctuationId)` for punctuation tokens
//!
//! ## Notes
//! - ID-bearing tokens avoid stringly-typed checks in the parser and compiler.
//! - Use `crate::token_helpers` for ergonomic token matching at call sites.

use crate::ast::{FloatLiteral, IntLiteral, Span};
use incan_core::lang::keywords::{self, KeywordId};
use incan_core::lang::operators::OperatorId;
use incan_core::lang::punctuation::PunctuationId;

// ============================================================================
// TOKEN TYPES
// ============================================================================

/// Kind of token produced by the lexer.
///
/// ## Notes
/// - Keyword/operator/punctuation tokens carry stable IDs from `incan_core::lang`.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // ========== Keyword / operator / punctuation (ID-based) ==========
    Keyword(KeywordId),
    Operator(OperatorId),
    Punctuation(PunctuationId),

    // ========== Identifiers and Literals ==========
    Ident(String),
    Int(IntLiteral),
    Float(FloatLiteral),
    String(String),
    Bytes(Vec<u8>),
    FString(Vec<FStringPart>),

    // ========== Indentation ==========
    Newline,
    Indent,
    Dedent,

    // ========== Special ==========
    Ellipsis, // ...
    Eof,      // end of file
}

/// Part of an f-string.
#[derive(Debug, Clone, PartialEq)]
pub enum FStringPart {
    Literal(String),
    Expr {
        text: String,
        /// Byte offset of the opening `{` in source.
        offset: usize,
    },
}

/// A token with its kind and source span.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    /// Construct a new token.
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span }
    }
}
/// Resolve an identifier spelling to a keyword id, if reserved.
pub fn keyword_id(name: &str) -> Option<KeywordId> {
    keywords::from_str_hard_only(name)
}

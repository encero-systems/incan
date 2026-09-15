//! Small helper APIs for working with `Token` / `TokenKind`.
//!
//! These helpers exist to reduce repetitive `matches!(...)` at call sites and to make it easy to work with ID-based
//! tokens.

use crate::lexer::{Token, TokenKind};
use incan_core::lang::keywords::KeywordId;
use incan_core::lang::operators::OperatorId;
use incan_core::lang::punctuation::PunctuationId;

impl TokenKind {
    /// Return the keyword id, if this is a keyword token.
    pub fn keyword_id(&self) -> Option<KeywordId> {
        match self {
            TokenKind::Keyword(id) => Some(*id),
            _ => None,
        }
    }

    /// Return `true` if this is the given keyword.
    pub fn is_keyword(&self, id: KeywordId) -> bool {
        matches!(self, TokenKind::Keyword(k) if *k == id)
    }

    /// Return the operator id, if this is an operator token.
    pub fn operator_id(&self) -> Option<OperatorId> {
        match self {
            TokenKind::Operator(id) => Some(*id),
            _ => None,
        }
    }

    /// Return `true` if this is the given operator.
    pub fn is_operator(&self, id: OperatorId) -> bool {
        matches!(self, TokenKind::Operator(o) if *o == id)
    }

    /// Return the punctuation id, if this is a punctuation token.
    pub fn punctuation_id(&self) -> Option<PunctuationId> {
        match self {
            TokenKind::Punctuation(id) => Some(*id),
            _ => None,
        }
    }

    /// Return `true` if this is the given punctuation.
    pub fn is_punctuation(&self, id: PunctuationId) -> bool {
        matches!(self, TokenKind::Punctuation(p) if *p == id)
    }

    /// Return `true` if this token is trivia/control flow in the token stream.
    pub fn is_layout(&self) -> bool {
        matches!(self, TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent)
    }
}

impl Token {
    /// Convenience wrapper for `self.kind.keyword_id()`.
    pub fn keyword_id(&self) -> Option<KeywordId> {
        self.kind.keyword_id()
    }

    /// Convenience wrapper for `self.kind.operator_id()`.
    pub fn operator_id(&self) -> Option<OperatorId> {
        self.kind.operator_id()
    }

    /// Convenience wrapper for `self.kind.punctuation_id()`.
    pub fn punctuation_id(&self) -> Option<PunctuationId> {
        self.kind.punctuation_id()
    }
}

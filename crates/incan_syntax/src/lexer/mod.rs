//! Lexer for the Incan programming language
//!
//! Handles tokenization including:
//! - Keywords (def, async, await, class, model, trait, etc.)
//! - Identifiers and literals (int, float, string, f-string)
//! - Operators and punctuation (::, =>, ?, etc.)
//! - Indentation-based blocks (INDENT/DEDENT tokens)
//!
//! ## Module Structure
//!
//! - `tokens` - Token types (TokenKind, Token, FStringPart)
//! - `strings` - String/f-string/byte-string scanning
//! - `numbers` - Numeric literal scanning
//! - `indent` - INDENT/DEDENT handling

mod indent;
mod numbers;
mod strings;
pub mod tokens;

pub use tokens::{FStringPart, Token, TokenKind, keyword_id};

use crate::ast::Span;
use crate::diagnostics::{CompileError, errors};
use incan_core::lang::operators::OperatorId;
use incan_core::lang::punctuation::PunctuationId;

// ============================================================================
// LEXER STATE
// ----------------------------------------------------------------------------
// Lexer state diagram (simplfied):
//
// [Start of line] → count spaces → [Inside code]
//                                       ↓
//                                      see '(' → [bracket_depth++]
//                                       ↓
//                                      see '\n' → skip (inside brackets)
//                                       ↓
//                                      see ')' → [bracket_depth--]
// ============================================================================

/// Lexer for Incan source code.
///
/// Converts source text into a stream of tokens, handling:
/// - Keywords and identifiers
/// - Numeric and string literals (including f-strings and byte strings)
/// - Operators and punctuation
/// - Python-style indentation (INDENT/DEDENT tokens)
/// - Implicit line continuation inside brackets
pub struct Lexer<'a> {
    source: &'a str,
    chars: std::iter::Peekable<std::str::CharIndices<'a>>,
    current_pos: usize,
    indent_stack: Vec<usize>,
    pending_dedents: usize,
    at_line_start: bool,
    /// Bracket depth for implicit line continuation (parens, brackets, braces)
    bracket_depth: usize,
    tokens: Vec<Token>,
    errors: Vec<CompileError>,
}

impl<'a> Lexer<'a> {
    /// Create a new lexer for the given source code.
    pub fn new(source: &'a str) -> Self {
        Self {
            source,
            chars: source.char_indices().peekable(),
            current_pos: 0,
            indent_stack: vec![0],
            pending_dedents: 0,
            at_line_start: true,
            bracket_depth: 0,
            tokens: Vec::new(),
            errors: Vec::new(),
        }
    }

    /// Tokenize the entire source code.
    ///
    /// Returns a vector of tokens on success, or a vector of errors on failure.
    /// The token stream always ends with an `Eof` token.
    pub fn tokenize(mut self) -> Result<Vec<Token>, Vec<CompileError>> {
        while !self.is_at_end() {
            self.scan_token();
        }

        // Emit remaining dedents at EOF
        while self.indent_stack.len() > 1 {
            self.indent_stack.pop();
            self.tokens.push(Token::new(
                TokenKind::Dedent,
                Span::new(self.current_pos, self.current_pos),
            ));
        }

        self.tokens.push(Token::new(
            TokenKind::Eof,
            Span::new(self.current_pos, self.current_pos),
        ));

        if self.errors.is_empty() {
            Ok(self.tokens)
        } else {
            Err(self.errors)
        }
    }

    // ========================================================================
    // Core character handling
    // ========================================================================

    fn is_at_end(&mut self) -> bool {
        self.chars.peek().is_none()
    }

    fn peek(&mut self) -> Option<char> {
        self.chars.peek().map(|(_, c)| *c)
    }

    fn peek_next(&self) -> Option<char> {
        let mut iter = self.source[self.current_pos..].char_indices();
        iter.next(); // skip current
        iter.next().map(|(_, c)| c)
    }

    fn advance(&mut self) -> Option<char> {
        if let Some((pos, c)) = self.chars.next() {
            self.current_pos = pos + c.len_utf8();
            Some(c)
        } else {
            None
        }
    }

    // ========================================================================
    // Main scanning dispatch
    // ========================================================================

    fn scan_token(&mut self) {
        // Handle pending dedents first
        if self.pending_dedents > 0 {
            self.pending_dedents -= 1;
            self.tokens.push(Token::new(
                TokenKind::Dedent,
                Span::new(self.current_pos, self.current_pos),
            ));
            return;
        }

        // Handle indentation at line start
        if self.at_line_start {
            self.handle_indentation();
            return;
        }

        // Skip whitespace (but not newlines)
        while let Some(c) = self.peek() {
            if c == ' ' || c == '\t' {
                self.advance();
            } else {
                break;
            }
        }

        let start = self.current_pos;

        let Some(c) = self.advance() else {
            return;
        };

        match c {
            // Comments
            '#' => {
                while let Some(c) = self.peek() {
                    if c == '\n' {
                        break;
                    }
                    self.advance();
                }
            }

            // Newlines
            '\n' => {
                // Implicit line continuation: skip newlines inside brackets
                if self.bracket_depth > 0 {
                    // Inside brackets - don't emit newline, don't trigger indentation
                    return;
                }
                // Skip blank lines (don't emit newline if we're already at line start)
                if !self.at_line_start {
                    self.tokens
                        .push(Token::new(TokenKind::Newline, Span::new(start, self.current_pos)));
                }
                self.at_line_start = true;
            }

            // Skip carriage return
            '\r' => {}

            // Operators and punctuation
            '+' => self.operator(start, OperatorId::Plus, &[('=', OperatorId::PlusEq)]),
            '-' => {
                if self.match_char('>') {
                    self.add_punct(PunctuationId::Arrow, start);
                } else if self.match_char('=') {
                    self.add_op(OperatorId::MinusEq, start);
                } else {
                    self.add_op(OperatorId::Minus, start);
                }
            }
            '*' => self.operator(
                start,
                OperatorId::Star,
                &[('*', OperatorId::StarStar), ('=', OperatorId::StarEq)],
            ),
            '/' => self.scan_slash(start),
            '%' => self.operator(start, OperatorId::Percent, &[('=', OperatorId::PercentEq)]),
            '?' => self.add_punct(PunctuationId::Question, start),
            '@' => self.add_punct(PunctuationId::At, start),
            ',' => self.add_punct(PunctuationId::Comma, start),
            '(' => self.open_bracket(PunctuationId::LParen, start),
            ')' => self.close_bracket(PunctuationId::RParen, start),
            '[' => self.open_bracket(PunctuationId::LBracket, start),
            ']' => self.close_bracket(PunctuationId::RBracket, start),
            '{' => self.open_bracket(PunctuationId::LBrace, start),
            '}' => self.close_bracket(PunctuationId::RBrace, start),
            ':' => {
                if self.match_char(':') {
                    self.add_punct(PunctuationId::ColonColon, start);
                } else {
                    self.add_punct(PunctuationId::Colon, start);
                }
            }
            '=' => {
                if self.match_char('=') {
                    self.add_op(OperatorId::EqEq, start);
                } else if self.match_char('>') {
                    self.add_punct(PunctuationId::FatArrow, start);
                } else {
                    self.add_op(OperatorId::Eq, start);
                }
            }
            '!' => {
                if self.match_char('=') {
                    self.add_op(OperatorId::NotEq, start);
                } else {
                    self.errors
                        .push(errors::unexpected_bang(Span::new(start, self.current_pos)));
                }
            }
            '<' => self.operator(start, OperatorId::Lt, &[('=', OperatorId::LtEq)]),
            '>' => self.operator(start, OperatorId::Gt, &[('=', OperatorId::GtEq)]),
            '.' => {
                if self.match_char('.') {
                    if self.match_char('.') {
                        self.add_punct(PunctuationId::Ellipsis, start);
                    } else if self.match_char('=') {
                        self.add_op(OperatorId::DotDotEq, start);
                    } else {
                        self.add_op(OperatorId::DotDot, start);
                    }
                } else {
                    self.add_punct(PunctuationId::Dot, start);
                }
            }

            // Strings
            '"' => self.scan_string(start, '"'),
            '\'' => self.scan_string(start, '\''),

            // f-strings
            'f' if self.peek() == Some('"') || self.peek() == Some('\'') => {
                // Safe: we just checked peek() is Some quote char
                let quote = self.advance().expect("f-string quote after peek check");
                self.scan_fstring(start, quote);
            }

            // b-strings (byte strings)
            'b' if self.peek() == Some('"') || self.peek() == Some('\'') => {
                // Safe: we just checked peek() is Some quote char
                let quote = self.advance().expect("b-string quote after peek check");
                self.scan_byte_string(start, quote);
            }

            // Numbers
            '0'..='9' => self.scan_number(start, c),

            // Identifiers and keywords
            _ if is_ident_start(c) => self.scan_identifier(start, c),

            _ => {
                self.errors
                    .push(errors::unexpected_character(c, Span::new(start, self.current_pos)));
            }
        }
    }

    // ========================================================================
    // Operator helpers
    // ========================================================================

    fn match_char(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn add_token(&mut self, kind: TokenKind, start: usize) {
        self.tokens.push(Token::new(kind, Span::new(start, self.current_pos)));
    }

    fn add_op(&mut self, id: OperatorId, start: usize) {
        self.add_token(TokenKind::Operator(id), start);
    }

    fn add_punct(&mut self, id: PunctuationId, start: usize) {
        self.add_token(TokenKind::Punctuation(id), start);
    }

    /// Try to match compound operator, fallback to simple.
    fn operator(&mut self, start: usize, simple: OperatorId, compounds: &[(char, OperatorId)]) {
        for (c, id) in compounds {
            if self.match_char(*c) {
                self.add_op(*id, start);
                return;
            }
        }
        self.add_op(simple, start);
    }

    /// Scan slash operators: `/`, `/=`, `//`, `//=`.
    fn scan_slash(&mut self, start: usize) {
        if self.match_char('/') {
            // `//` or `//=`
            if self.match_char('=') {
                self.add_op(OperatorId::SlashSlashEq, start);
            } else {
                self.add_op(OperatorId::SlashSlash, start);
            }
        } else if self.match_char('=') {
            // `/=`
            self.add_op(OperatorId::SlashEq, start);
        } else {
            // `/`
            self.add_op(OperatorId::Slash, start);
        }
    }

    /// Emit a bracket token and track bracket depth.
    fn open_bracket(&mut self, kind: PunctuationId, start: usize) {
        self.bracket_depth += 1;
        self.add_punct(kind, start);
    }

    /// Emit a closing bracket token and decrement bracket depth.
    /// Produces an error if there's no matching opening bracket.
    fn close_bracket(&mut self, kind: PunctuationId, start: usize) {
        if self.bracket_depth == 0 {
            self.errors
                .push(errors::unmatched_closing_bracket(Span::new(start, self.current_pos)));
        } else {
            self.bracket_depth -= 1;
        }
        self.add_punct(kind, start);
    }

    // ========================================================================
    // Identifier scanning
    // ========================================================================

    fn scan_identifier(&mut self, start: usize, _first: char) {
        while let Some(c) = self.peek() {
            if is_ident_continue(c) {
                self.advance();
            } else {
                break;
            }
        }

        let spelling = &self.source[start..self.current_pos];

        // Look up identifier spelling in the reserved-word registry (no allocation for keywords).
        if let Some(id) = keyword_id(spelling) {
            self.add_token(TokenKind::Keyword(id), start);
        } else {
            self.add_token(TokenKind::Ident(spelling.to_string()), start);
        }
    }
}

// ============================================================================
// Helper functions
// ============================================================================

/// Check if a character can start an identifier (ASCII-only).
fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

/// Check if a character can continue an identifier (ASCII-only).
fn is_ident_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Convenience function to lex a source string.
///
/// This is a shorthand for `Lexer::new(source).tokenize()`.
#[tracing::instrument(skip_all, fields(source_len = source.len()))]
pub fn lex(source: &str) -> Result<Vec<Token>, Vec<CompileError>> {
    Lexer::new(source).tokenize()
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use incan_core::lang::keywords::KeywordId;
    use incan_core::lang::operators::OperatorId;

    #[test]
    fn test_punctuation_registry_parity() {
        use incan_core::lang::punctuation::{self, PunctuationId};

        for p in punctuation::PUNCTUATION {
            match p.id {
                // Closing delimiters error when unmatched; use a matching pair.
                PunctuationId::LParen | PunctuationId::RParen => {
                    let tokens = lex("()").unwrap();
                    assert!(tokens[0].kind.is_punctuation(PunctuationId::LParen));
                    assert!(tokens[1].kind.is_punctuation(PunctuationId::RParen));
                }
                PunctuationId::LBracket | PunctuationId::RBracket => {
                    let tokens = lex("[]").unwrap();
                    assert!(tokens[0].kind.is_punctuation(PunctuationId::LBracket));
                    assert!(tokens[1].kind.is_punctuation(PunctuationId::RBracket));
                }
                PunctuationId::LBrace | PunctuationId::RBrace => {
                    let tokens = lex("{}").unwrap();
                    assert!(tokens[0].kind.is_punctuation(PunctuationId::LBrace));
                    assert!(tokens[1].kind.is_punctuation(PunctuationId::RBrace));
                }

                // Everything else should tokenize cleanly as a single token.
                _ => {
                    let tokens = lex(p.canonical).unwrap_or_else(|errs| {
                        panic!("lex({:?}) failed: {:?}", p.canonical, errs);
                    });
                    assert!(!tokens.is_empty(), "lex({:?}) produced no tokens", p.canonical);

                    match p.id {
                        PunctuationId::Comma => assert!(tokens[0].kind.is_punctuation(PunctuationId::Comma)),
                        PunctuationId::Colon => assert!(tokens[0].kind.is_punctuation(PunctuationId::Colon)),
                        PunctuationId::Question => assert!(tokens[0].kind.is_punctuation(PunctuationId::Question)),
                        PunctuationId::At => assert!(tokens[0].kind.is_punctuation(PunctuationId::At)),

                        PunctuationId::Dot => assert!(tokens[0].kind.is_punctuation(PunctuationId::Dot)),
                        PunctuationId::ColonColon => assert!(tokens[0].kind.is_punctuation(PunctuationId::ColonColon)),

                        PunctuationId::Arrow => assert!(tokens[0].kind.is_punctuation(PunctuationId::Arrow)),
                        PunctuationId::FatArrow => assert!(tokens[0].kind.is_punctuation(PunctuationId::FatArrow)),

                        PunctuationId::Ellipsis => assert!(tokens[0].kind.is_punctuation(PunctuationId::Ellipsis)),

                        // Delimiters handled above.
                        PunctuationId::LParen
                        | PunctuationId::RParen
                        | PunctuationId::LBracket
                        | PunctuationId::RBracket
                        | PunctuationId::LBrace
                        | PunctuationId::RBrace => unreachable!("handled above"),
                    }
                }
            }
        }
    }

    #[test]
    fn test_keyword_registry_parity() {
        use incan_core::lang::keywords;

        for k in keywords::KEYWORDS {
            let tokens = lex(k.canonical).unwrap_or_else(|errs| panic!("lex({:?}) failed: {:?}", k.canonical, errs));
            assert!(
                tokens.len() >= 2,
                "expected token + EOF for keyword {:?}, got {:?}",
                k.id,
                tokens
            );
            assert!(matches!(tokens.last().map(|t| &t.kind), Some(TokenKind::Eof)));

            let tokens = &tokens[..tokens.len() - 1];
            assert_eq!(
                tokens.len(),
                1,
                "expected single non-EOF token for keyword {:?}, got {:?}",
                k.id,
                tokens
            );
            assert!(tokens[0].kind.is_keyword(k.id));
        }
    }

    #[test]
    fn test_operator_registry_parity() {
        use incan_core::lang::operators;

        for o in operators::OPERATORS {
            for &sp in o.spellings {
                let tokens = lex(sp).unwrap_or_else(|errs| panic!("lex({:?}) failed: {:?}", sp, errs));
                assert!(
                    tokens.len() >= 2,
                    "expected token + EOF for operator spelling {:?}, got {:?}",
                    sp,
                    tokens
                );
                assert!(matches!(tokens.last().map(|t| &t.kind), Some(TokenKind::Eof)));

                let tokens = &tokens[..tokens.len() - 1];
                assert_eq!(
                    tokens.len(),
                    1,
                    "expected single non-EOF token for operator spelling {:?}, got {:?}",
                    sp,
                    tokens
                );

                if o.is_keyword_spelling {
                    // Word operators are lexed as keywords.
                    let expected_kw = match o.id {
                        operators::OperatorId::And => KeywordId::And,
                        operators::OperatorId::Or => KeywordId::Or,
                        operators::OperatorId::Not => KeywordId::Not,
                        operators::OperatorId::In => KeywordId::In,
                        operators::OperatorId::Is => KeywordId::Is,
                        _ => panic!("unexpected keyword-spelling operator {:?}", o.id),
                    };
                    assert!(tokens[0].kind.is_keyword(expected_kw));
                } else {
                    assert!(tokens[0].kind.is_operator(o.id));
                }
            }
        }
    }

    #[test]
    fn test_keywords() {
        let tokens = lex("def async await class model trait").unwrap();
        assert!(matches!(tokens[0].kind, TokenKind::Keyword(KeywordId::Def)));
        assert!(matches!(tokens[1].kind, TokenKind::Keyword(KeywordId::Async)));
        assert!(matches!(tokens[2].kind, TokenKind::Keyword(KeywordId::Await)));
        assert!(matches!(tokens[3].kind, TokenKind::Keyword(KeywordId::Class)));
        assert!(matches!(tokens[4].kind, TokenKind::Keyword(KeywordId::Model)));
        assert!(matches!(tokens[5].kind, TokenKind::Keyword(KeywordId::Trait)));
    }

    #[test]
    fn test_operators() {
        let tokens = lex("+ - * / :: => -> ? @ == !=").unwrap();
        assert!(matches!(tokens[0].kind, TokenKind::Operator(OperatorId::Plus)));
        assert!(matches!(tokens[1].kind, TokenKind::Operator(OperatorId::Minus)));
        assert!(matches!(tokens[2].kind, TokenKind::Operator(OperatorId::Star)));
        assert!(matches!(tokens[3].kind, TokenKind::Operator(OperatorId::Slash)));
        assert!(matches!(
            tokens[4].kind,
            TokenKind::Punctuation(PunctuationId::ColonColon)
        ));
        assert!(matches!(
            tokens[5].kind,
            TokenKind::Punctuation(PunctuationId::FatArrow)
        ));
        assert!(matches!(tokens[6].kind, TokenKind::Punctuation(PunctuationId::Arrow)));
        assert!(matches!(
            tokens[7].kind,
            TokenKind::Punctuation(PunctuationId::Question)
        ));
        assert!(matches!(tokens[8].kind, TokenKind::Punctuation(PunctuationId::At)));
        assert!(matches!(tokens[9].kind, TokenKind::Operator(OperatorId::EqEq)));
        assert!(matches!(tokens[10].kind, TokenKind::Operator(OperatorId::NotEq)));
    }

    #[test]
    #[allow(clippy::approx_constant)]
    fn test_numbers() {
        let tokens = lex("42 3.14 1_000_000 1e10").unwrap();
        assert!(matches!(tokens[0].kind, TokenKind::Int(42)));
        assert!(matches!(tokens[1].kind, TokenKind::Float(f) if (f - 3.14).abs() < 0.001));
        assert!(matches!(tokens[2].kind, TokenKind::Int(1000000)));
        assert!(matches!(tokens[3].kind, TokenKind::Float(_)));
    }

    #[test]
    fn test_strings() {
        let tokens = lex(r#""hello" 'world'"#).unwrap();
        assert!(matches!(&tokens[0].kind, TokenKind::String(s) if s == "hello"));
        assert!(matches!(&tokens[1].kind, TokenKind::String(s) if s == "world"));
    }

    #[test]
    fn test_indentation() {
        let source = "def foo():\n  x = 1\n  y = 2\nx = 3";
        let tokens = lex(source).unwrap();

        // Find indent and dedent tokens
        let indent_count = tokens.iter().filter(|t| matches!(t.kind, TokenKind::Indent)).count();
        let dedent_count = tokens.iter().filter(|t| matches!(t.kind, TokenKind::Dedent)).count();

        assert_eq!(indent_count, 1, "Should have 1 INDENT token");
        assert_eq!(dedent_count, 1, "Should have 1 DEDENT token");
    }

    #[test]
    fn test_import_path() {
        let tokens = lex("import polars::prelude as pl").unwrap();
        assert!(matches!(tokens[0].kind, TokenKind::Keyword(KeywordId::Import)));
        assert!(matches!(&tokens[1].kind, TokenKind::Ident(s) if s == "polars"));
        assert!(matches!(
            tokens[2].kind,
            TokenKind::Punctuation(PunctuationId::ColonColon)
        ));
        assert!(matches!(&tokens[3].kind, TokenKind::Ident(s) if s == "prelude"));
        assert!(matches!(tokens[4].kind, TokenKind::Keyword(KeywordId::As)));
        assert!(matches!(&tokens[5].kind, TokenKind::Ident(s) if s == "pl"));
    }

    #[test]
    fn test_fstring() {
        let tokens = lex(r#"f"Hello {name}!""#).unwrap();
        match &tokens[0].kind {
            TokenKind::FString(parts) => {
                assert_eq!(parts.len(), 3);
                assert!(matches!(&parts[0], FStringPart::Literal(s) if s == "Hello "));
                assert!(matches!(&parts[1], FStringPart::Expr(s) if s == "name"));
                assert!(matches!(&parts[2], FStringPart::Literal(s) if s == "!"));
            }
            _ => panic!("Expected FString token"),
        }
    }

    #[test]
    fn test_unicode_identifier_rejected() {
        // Unicode characters should not be valid identifiers (ASCII-only)
        let result = lex("π = 1");
        assert!(result.is_err(), "Unicode identifier should produce an error");
        let errors = result.unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("Unexpected character"));
    }

    #[test]
    fn test_unmatched_closing_bracket() {
        // Closing bracket without matching open should produce an error
        let result = lex(")");
        assert!(result.is_err(), "Unmatched ) should produce an error");
        let errors = result.unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("Unmatched closing bracket"));

        // Same for ] and }
        let result = lex("]");
        assert!(result.is_err());
        assert!(result.unwrap_err()[0].message.contains("Unmatched closing bracket"));

        let result = lex("}");
        assert!(result.is_err());
        assert!(result.unwrap_err()[0].message.contains("Unmatched closing bracket"));
    }

    #[test]
    fn test_matched_brackets_ok() {
        // Properly matched brackets should work fine
        let tokens = lex("(x)").unwrap();
        assert!(matches!(tokens[0].kind, TokenKind::Punctuation(PunctuationId::LParen)));
        assert!(matches!(&tokens[1].kind, TokenKind::Ident(s) if s == "x"));
        assert!(matches!(tokens[2].kind, TokenKind::Punctuation(PunctuationId::RParen)));
    }

    #[test]
    fn test_multiple_dedents() {
        // Multiple dedent levels in one line
        let source = "def foo():\n  if True:\n    x = 1\ny = 2";
        let tokens = lex(source).unwrap();

        let indent_count = tokens.iter().filter(|t| matches!(t.kind, TokenKind::Indent)).count();
        let dedent_count = tokens.iter().filter(|t| matches!(t.kind, TokenKind::Dedent)).count();

        assert_eq!(indent_count, 2, "Should have 2 INDENT tokens");
        assert_eq!(dedent_count, 2, "Should have 2 DEDENT tokens");
    }

    #[test]
    fn test_tabs_as_spaces() {
        // Tabs count as 2 spaces
        let source = "def foo():\n\tx = 1"; // Tab indentation
        let tokens = lex(source).unwrap();

        let indent_count = tokens.iter().filter(|t| matches!(t.kind, TokenKind::Indent)).count();
        assert_eq!(indent_count, 1, "Tab should produce INDENT");
    }

    #[test]
    fn test_newlines_inside_brackets() {
        // Newlines inside brackets should NOT emit Newline tokens (implicit continuation)
        let source = "foo(\n  x,\n  y\n)";
        let tokens = lex(source).unwrap();

        let newline_count = tokens.iter().filter(|t| matches!(t.kind, TokenKind::Newline)).count();
        assert_eq!(newline_count, 0, "No Newline tokens inside brackets");
    }

    #[test]
    fn test_range_not_float() {
        // 1..2 should be Int, DotDot, Int - not a float
        let tokens = lex("1..2").unwrap();
        assert!(matches!(tokens[0].kind, TokenKind::Int(1)));
        assert!(matches!(tokens[1].kind, TokenKind::Operator(OperatorId::DotDot)));
        assert!(matches!(tokens[2].kind, TokenKind::Int(2)));
    }

    #[test]
    fn test_inclusive_range() {
        // 1..=5 should work
        let tokens = lex("1..=5").unwrap();
        assert!(matches!(tokens[0].kind, TokenKind::Int(1)));
        assert!(matches!(tokens[1].kind, TokenKind::Operator(OperatorId::DotDotEq)));
        assert!(matches!(tokens[2].kind, TokenKind::Int(5)));
    }
}

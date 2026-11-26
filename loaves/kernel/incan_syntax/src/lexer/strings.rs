//! String scanning for the Incan lexer
//!
//! Handles regular strings, f-strings, and byte strings.

use super::Lexer;
use super::tokens::{FStringPart, Token, TokenKind};
use crate::ast::Span;
use crate::diagnostics::errors::{self, BidirectionalControlContext};
use crate::source_security::bidirectional_control_name;

// ============================================================================
// Escape sequence handling
// ============================================================================

/// Result of processing an escape sequence
pub enum EscapeResult {
    /// Successfully parsed escape character
    Char(char),
    /// Unknown escape - preserve as-is (backslash + char)
    Unknown(char),
    /// A `\u{...}` spelling that does not name one Unicode scalar value.
    InvalidUnicode {
        /// Authored escape text, which is ASCII and therefore safe to display.
        spelling: String,
        /// Why the authored spelling is not a scalar escape.
        reason: &'static str,
        /// Complete byte span of the malformed escape.
        span: Span,
    },
    /// End of input during escape
    Eof,
}

/// Result of processing a byte escape sequence
pub enum ByteEscapeResult {
    /// Successfully parsed escape byte
    Byte(u8),
    /// Unknown escape - preserve as-is (backslash + char)
    Unknown(u8),
    /// Hex escape (\xNN)
    Hex(u8),
    /// Error parsing hex escape
    HexError(String),
    /// End of input during escape
    Eof,
}

impl<'a> Lexer<'a> {
    /// Process a text escape sequence (for strings and f-strings). Called after consuming the backslash.
    fn scan_text_escape(&mut self, quote: char) -> EscapeResult {
        let escape_start = self.current_pos.saturating_sub(1);
        match self.advance() {
            Some('n') => EscapeResult::Char('\n'),
            Some('t') => EscapeResult::Char('\t'),
            Some('r') => EscapeResult::Char('\r'),
            Some('\\') => EscapeResult::Char('\\'),
            Some('u') if self.peek() == Some('{') => self.scan_unicode_scalar_escape(escape_start, quote),
            Some(q) if q == quote => EscapeResult::Char(q),
            Some(c) => EscapeResult::Unknown(c),
            None => EscapeResult::Eof,
        }
    }

    /// Decode a braced Unicode scalar escape after its `\u` prefix has been consumed.
    fn scan_unicode_scalar_escape(&mut self, start: usize, quote: char) -> EscapeResult {
        self.advance();
        let mut spelling = String::from("\\u{");
        let mut digits = String::new();
        let mut invalid_character = false;
        let mut closed = false;
        while let Some(character) = self.peek() {
            if character == '}' {
                self.advance();
                spelling.push('}');
                closed = true;
                break;
            }
            if character == quote || character == '\n' {
                break;
            }
            self.advance();
            spelling.push(character);
            if character.is_ascii_hexdigit() {
                digits.push(character);
            } else {
                invalid_character = true;
            }
        }

        let span = Span::new(start, self.current_pos);
        if !closed {
            return EscapeResult::InvalidUnicode {
                spelling,
                reason: "the closing `}` is missing",
                span,
            };
        }
        if invalid_character {
            return EscapeResult::InvalidUnicode {
                spelling,
                reason: "only hexadecimal digits are allowed",
                span,
            };
        }
        if digits.is_empty() || digits.len() > 6 {
            return EscapeResult::InvalidUnicode {
                spelling,
                reason: "the escape needs one to six hexadecimal digits",
                span,
            };
        }
        let Ok(value) = u32::from_str_radix(&digits, 16) else {
            return EscapeResult::InvalidUnicode {
                spelling,
                reason: "the digits do not name a Unicode scalar value",
                span,
            };
        };
        match char::from_u32(value) {
            Some(character) => EscapeResult::Char(character),
            None => EscapeResult::InvalidUnicode {
                spelling,
                reason: "surrogates and values above U+10FFFF are not Unicode scalar values",
                span,
            },
        }
    }

    /// Process a byte escape sequence. Called after consuming the backslash.
    fn scan_byte_escape(&mut self, quote: char) -> ByteEscapeResult {
        match self.advance() {
            Some('n') => ByteEscapeResult::Byte(b'\n'),
            Some('t') => ByteEscapeResult::Byte(b'\t'),
            Some('r') => ByteEscapeResult::Byte(b'\r'),
            Some('\\') => ByteEscapeResult::Byte(b'\\'),
            Some('0') => ByteEscapeResult::Byte(0),
            Some('x') => {
                // Hex escape \xNN
                let mut hex = String::new();
                if let Some(c) = self.advance() {
                    hex.push(c);
                }
                if let Some(c) = self.advance() {
                    hex.push(c);
                }
                match u8::from_str_radix(&hex, 16) {
                    Ok(byte) => ByteEscapeResult::Hex(byte),
                    Err(_) => ByteEscapeResult::HexError(hex),
                }
            }
            Some(q) if q == quote => ByteEscapeResult::Byte(q as u8),
            Some(c) => ByteEscapeResult::Unknown(c as u8),
            None => ByteEscapeResult::Eof,
        }
    }
}

// ============================================================================
// String scanning
// ============================================================================

impl<'a> Lexer<'a> {
    pub(super) fn scan_string(&mut self, start: usize, quote: char) {
        // Check for triple-quoted string
        let triple = if self.peek() == Some(quote) {
            if self.peek_next() == Some(quote) {
                self.advance(); // consume second quote
                self.advance(); // consume third quote
                true
            } else {
                false
            }
        } else {
            false
        };

        let mut value = String::new();

        loop {
            match self.peek() {
                None => {
                    self.errors
                        .push(errors::unterminated_string(Span::new(start, self.current_pos)));
                    break;
                }
                Some(c) if c == quote => {
                    if triple {
                        // Need three quotes to close
                        self.advance();
                        if self.peek() == Some(quote) {
                            self.advance();
                            if self.peek() == Some(quote) {
                                self.advance();
                                break;
                            } else {
                                value.push(quote);
                                value.push(quote);
                            }
                        } else {
                            value.push(quote);
                        }
                    } else {
                        self.advance();
                        break;
                    }
                }
                Some('\n') if !triple => {
                    self.errors
                        .push(errors::unterminated_string_newline(Span::new(start, self.current_pos)));
                    break;
                }
                Some('\\') => {
                    self.advance();
                    match self.scan_text_escape(quote) {
                        EscapeResult::Char(c) => value.push(c),
                        EscapeResult::Unknown(c) => {
                            value.push('\\');
                            value.push(c);
                        }
                        EscapeResult::InvalidUnicode { spelling, reason, span } => self
                            .errors
                            .push(errors::malformed_unicode_scalar_escape(&spelling, reason, span)),
                        EscapeResult::Eof => {
                            self.errors
                                .push(errors::unterminated_escape_sequence(Span::new(start, self.current_pos)));
                            break;
                        }
                    }
                }
                Some(c) => {
                    if bidirectional_control_name(c).is_some() {
                        self.errors.push(errors::bidirectional_control_in_source(
                            c,
                            BidirectionalControlContext::String,
                            Span::new(self.current_pos, self.current_pos + c.len_utf8()),
                        ));
                    }
                    value.push(c);
                    self.advance();
                }
            }
        }

        self.tokens
            .push(Token::new(TokenKind::String(value), Span::new(start, self.current_pos)));
    }

    pub(super) fn scan_byte_string(&mut self, start: usize, quote: char) {
        let mut value = Vec::new();

        loop {
            match self.peek() {
                None => {
                    self.errors
                        .push(errors::unterminated_byte_string(Span::new(start, self.current_pos)));
                    break;
                }
                Some(c) if c == quote => {
                    self.advance();
                    break;
                }
                Some('\n') => {
                    self.errors.push(errors::unterminated_byte_string_newline(Span::new(
                        start,
                        self.current_pos,
                    )));
                    break;
                }
                Some('\\') => {
                    self.advance();
                    match self.scan_byte_escape(quote) {
                        ByteEscapeResult::Byte(b) | ByteEscapeResult::Hex(b) => value.push(b),
                        ByteEscapeResult::Unknown(c) => {
                            value.push(b'\\');
                            value.push(c);
                        }
                        ByteEscapeResult::HexError(hex) => {
                            self.errors
                                .push(errors::invalid_hex_escape(&hex, Span::new(start, self.current_pos)));
                        }
                        ByteEscapeResult::Eof => {
                            self.errors
                                .push(errors::unterminated_escape_sequence(Span::new(start, self.current_pos)));
                            break;
                        }
                    }
                }
                Some(c) => {
                    if bidirectional_control_name(c).is_some() {
                        self.errors.push(errors::bidirectional_control_in_source(
                            c,
                            BidirectionalControlContext::String,
                            Span::new(self.current_pos, self.current_pos + c.len_utf8()),
                        ));
                    } else if c.is_ascii() {
                        // Byte strings should only contain ASCII.
                        value.push(c as u8);
                    } else {
                        self.errors
                            .push(errors::non_ascii_in_byte_string(c, Span::new(start, self.current_pos)));
                    }
                    self.advance();
                }
            }
        }

        self.tokens
            .push(Token::new(TokenKind::Bytes(value), Span::new(start, self.current_pos)));
    }

    pub(super) fn scan_fstring(&mut self, start: usize, quote: char) {
        let mut parts = Vec::new();
        let mut literal = String::new();

        loop {
            match self.peek() {
                None => {
                    self.errors
                        .push(errors::unterminated_fstring(Span::new(start, self.current_pos)));
                    break;
                }
                Some(c) if c == quote => {
                    self.advance();
                    break;
                }
                Some('{') => {
                    self.advance();
                    if self.peek() == Some('{') {
                        // Escaped brace
                        self.advance();
                        literal.push('{');
                    } else {
                        let expr_offset = self.current_pos.saturating_sub(1);
                        // Push current literal
                        if !literal.is_empty() {
                            parts.push(FStringPart::Literal(std::mem::take(&mut literal)));
                        }
                        // Scan expression
                        let expr = self.scan_fstring_expr();
                        parts.push(FStringPart::Expr {
                            text: expr,
                            offset: expr_offset,
                        });
                    }
                }
                Some('}') => {
                    self.advance();
                    if self.peek() == Some('}') {
                        self.advance();
                        literal.push('}');
                    } else {
                        self.errors.push(errors::unmatched_right_brace_in_fstring(Span::new(
                            start,
                            self.current_pos,
                        )));
                    }
                }
                Some('\\') => {
                    self.advance();
                    match self.scan_text_escape(quote) {
                        EscapeResult::Char(c) => literal.push(c),
                        EscapeResult::Unknown(c) => {
                            literal.push('\\');
                            literal.push(c);
                        }
                        EscapeResult::InvalidUnicode { spelling, reason, span } => self
                            .errors
                            .push(errors::malformed_unicode_scalar_escape(&spelling, reason, span)),
                        EscapeResult::Eof => {
                            self.errors
                                .push(errors::unterminated_fstring_escape(Span::new(start, self.current_pos)));
                            break;
                        }
                    }
                }
                Some('\n') => {
                    self.errors
                        .push(errors::unterminated_fstring(Span::new(start, self.current_pos)));
                    break;
                }
                Some(c) => {
                    if bidirectional_control_name(c).is_some() {
                        self.errors.push(errors::bidirectional_control_in_source(
                            c,
                            BidirectionalControlContext::String,
                            Span::new(self.current_pos, self.current_pos + c.len_utf8()),
                        ));
                    }
                    literal.push(c);
                    self.advance();
                }
            }
        }

        if !literal.is_empty() {
            parts.push(FStringPart::Literal(literal));
        }

        self.tokens.push(Token::new(
            TokenKind::FString(parts),
            Span::new(start, self.current_pos),
        ));
    }

    pub(super) fn scan_fstring_expr(&mut self) -> String {
        let mut expr = String::new();
        let mut depth = 1; // We're already past the opening {

        while depth > 0 {
            match self.peek() {
                None => break,
                Some('{') => {
                    expr.push('{');
                    self.advance();
                    depth += 1;
                }
                Some('}') => {
                    depth -= 1;
                    if depth > 0 {
                        expr.push('}');
                    }
                    self.advance();
                }
                Some(c) => {
                    if bidirectional_control_name(c).is_some() {
                        self.errors.push(errors::bidirectional_control_in_source(
                            c,
                            BidirectionalControlContext::Code,
                            Span::new(self.current_pos, self.current_pos + c.len_utf8()),
                        ));
                    }
                    expr.push(c);
                    self.advance();
                }
            }
        }

        expr
    }
}

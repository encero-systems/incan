//! String scanning for the Incan lexer
//!
//! Handles regular strings, f-strings, and byte strings.

use super::Lexer;
use super::tokens::{FStringPart, Token, TokenKind};
use crate::ast::Span;
use crate::diagnostics::errors;

// ============================================================================
// Escape sequence handling
// ============================================================================

/// Result of processing an escape sequence
pub enum EscapeResult {
    /// Successfully parsed escape character
    Char(char),
    /// Unknown escape - preserve as-is (backslash + char)
    Unknown(char),
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
    /// Process a text escape sequence (for strings and f-strings).
    /// Called after consuming the backslash.
    fn scan_text_escape(&mut self, quote: char) -> EscapeResult {
        match self.advance() {
            Some('n') => EscapeResult::Char('\n'),
            Some('t') => EscapeResult::Char('\t'),
            Some('r') => EscapeResult::Char('\r'),
            Some('\\') => EscapeResult::Char('\\'),
            Some(q) if q == quote => EscapeResult::Char(q),
            Some(c) => EscapeResult::Unknown(c),
            None => EscapeResult::Eof,
        }
    }

    /// Process a byte escape sequence.
    /// Called after consuming the backslash.
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
                        EscapeResult::Eof => {
                            self.errors
                                .push(errors::unterminated_escape_sequence(Span::new(start, self.current_pos)));
                            break;
                        }
                    }
                }
                Some(c) => {
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
                    // Byte strings should only contain ASCII
                    if c.is_ascii() {
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
                        // Push current literal
                        if !literal.is_empty() {
                            parts.push(FStringPart::Literal(std::mem::take(&mut literal)));
                        }
                        // Scan expression
                        let expr = self.scan_fstring_expr();
                        parts.push(FStringPart::Expr(expr));
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
                    expr.push(c);
                    self.advance();
                }
            }
        }

        expr
    }
}

//! Number scanning for the Incan lexer
//!
//! Handles integer and floating-point literals.

use super::Lexer;
use super::tokens::TokenKind;
use crate::ast::{FloatLiteral, IntLiteral, Span};
use crate::diagnostics::errors;

impl<'a> Lexer<'a> {
    pub(super) fn scan_number(&mut self, start: usize, first: char) {
        let mut value = String::from(first);
        let mut is_float = false;

        // Integer part
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == '_' {
                if c != '_' {
                    value.push(c);
                }
                self.advance();
            } else {
                break;
            }
        }

        // Decimal part
        if self.peek() == Some('.') {
            // Look ahead to ensure it's not `..` (range) or method call
            if self.peek_next().is_some_and(|c| c.is_ascii_digit()) {
                is_float = true;
                value.push('.');
                self.advance(); // consume .
                while let Some(c) = self.peek() {
                    if c.is_ascii_digit() || c == '_' {
                        if c != '_' {
                            value.push(c);
                        }
                        self.advance();
                    } else {
                        break;
                    }
                }
            }
        }

        // Exponent part
        if self.peek() == Some('e') || self.peek() == Some('E') {
            is_float = true;
            value.push('e');
            self.advance();
            if let Some(sign) = self.peek()
                && (sign == '+' || sign == '-')
            {
                value.push(sign);
                self.advance();
            }
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    value.push(c);
                    self.advance();
                } else {
                    break;
                }
            }
        }

        if is_float {
            let end = self.current_pos;
            let repr = self.source.get(start..end).unwrap_or("").to_string();
            // `value` omits `_` (Rust float parsing); `repr` is the exact source substring for faithful formatting.
            match value.parse::<f64>() {
                Ok(f) => self.add_token(TokenKind::Float(FloatLiteral { value: f, repr }), start),
                Err(_) => {
                    self.errors
                        .push(errors::invalid_float_literal(&repr, Span::new(start, end)));
                }
            }
        } else {
            let end = self.current_pos;
            let repr = self.source.get(start..end).unwrap_or("").to_string();
            match value.parse::<i64>() {
                Ok(i) => self.add_token(TokenKind::Int(IntLiteral { value: i, repr }), start),
                Err(_) => {
                    self.errors
                        .push(errors::invalid_integer_literal(&repr, Span::new(start, end)));
                }
            }
        }
    }
}

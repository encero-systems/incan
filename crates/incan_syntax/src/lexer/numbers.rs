//! Number scanning for the Incan lexer
//!
//! Handles integer and floating-point literals.

use super::Lexer;
use super::tokens::TokenKind;
use crate::ast::Span;
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
            match value.parse::<f64>() {
                Ok(f) => self.add_token(TokenKind::Float(f), start),
                Err(_) => {
                    self.errors.push(errors::invalid_float_literal(
                        &value,
                        Span::new(start, self.current_pos),
                    ));
                }
            }
        } else {
            match value.parse::<i64>() {
                Ok(i) => self.add_token(TokenKind::Int(i), start),
                Err(_) => {
                    self.errors.push(errors::invalid_integer_literal(
                        &value,
                        Span::new(start, self.current_pos),
                    ));
                }
            }
        }
    }
}

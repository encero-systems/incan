//! Number scanning for the Incan lexer
//!
//! Handles integer and floating-point literals.

use super::Lexer;
use super::tokens::TokenKind;
use crate::ast::{DecimalLiteral, FloatLiteral, IntLiteral, Span};
use crate::diagnostics::errors;
use incan_core::numeric_strings::normalize_numeric_string;

impl<'a> Lexer<'a> {
    /// Scan an integer, float, or decimal literal after the first digit has been consumed.
    pub(super) fn scan_number(&mut self, start: usize) {
        let mut is_float = false;

        // Integer part
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == '_' {
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
                self.advance(); // consume .
                while let Some(c) = self.peek() {
                    if c.is_ascii_digit() || c == '_' {
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
            self.advance();
            if let Some(sign) = self.peek()
                && (sign == '+' || sign == '-')
            {
                self.advance();
            }
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() || c == '_' {
                    self.advance();
                } else {
                    break;
                }
            }
        }

        if self.peek() == Some('d') {
            self.advance();
            let end = self.current_pos;
            let repr = self.source.get(start..end).unwrap_or("").to_string();
            let numeric_repr = repr.strip_suffix('d').unwrap_or(&repr);
            let Some(body) = normalize_numeric_string(numeric_repr) else {
                self.errors
                    .push(errors::invalid_decimal_literal(&repr, Span::new(start, end)));
                return;
            };
            self.add_token(
                TokenKind::Decimal(DecimalLiteral {
                    body: body.into_owned(),
                    repr,
                }),
                start,
            );
        } else if is_float {
            let end = self.current_pos;
            let repr = self.source.get(start..end).unwrap_or("").to_string();
            let Some(value) = normalize_numeric_string(&repr) else {
                self.errors
                    .push(errors::invalid_float_literal(&repr, Span::new(start, end)));
                return;
            };
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
            let Some(value) = normalize_numeric_string(&repr) else {
                self.errors
                    .push(errors::invalid_integer_literal(&repr, Span::new(start, end)));
                return;
            };
            match value.parse::<u128>() {
                Ok(magnitude) => self.add_token(
                    TokenKind::Int(IntLiteral {
                        value: magnitude.try_into().unwrap_or(i64::MAX),
                        magnitude,
                        repr,
                    }),
                    start,
                ),
                Err(_) => {
                    self.errors
                        .push(errors::invalid_integer_literal(&repr, Span::new(start, end)));
                }
            }
        }
    }
}

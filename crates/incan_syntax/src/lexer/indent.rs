//! Indentation handling for the Incan lexer
//!
//! Implements Python-style INDENT/DEDENT tokens.

use super::Lexer;
use super::tokens::{Token, TokenKind};
use crate::ast::Span;
use crate::diagnostics::errors;

impl<'a> Lexer<'a> {
    pub(super) fn handle_indentation(&mut self) {
        let start = self.current_pos;
        let mut indent = 0;

        // Count leading spaces/tabs
        while let Some(c) = self.peek() {
            match c {
                ' ' => {
                    indent += 1;
                    self.advance();
                }
                '\t' => {
                    // Treat tab as 4 spaces (Incan uses 4-space indentation)
                    indent += 4;
                    self.advance();
                }
                '#' => {
                    // Comment line - skip to end
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.advance();
                    }
                    if self.peek() == Some('\n') {
                        self.advance();
                    }
                    return; // Stay at line start
                }
                '\n' => {
                    // Blank line - skip
                    self.advance();
                    return; // Stay at line start
                }
                '\r' => {
                    self.advance();
                }
                _ => break,
            }
        }

        // At end of file?
        if self.is_at_end() {
            self.at_line_start = false;
            return;
        }

        let current_indent = *self.indent_stack.last().unwrap_or(&0);

        if indent > current_indent {
            self.indent_stack.push(indent);
            self.tokens
                .push(Token::new(TokenKind::Indent, Span::new(start, self.current_pos)));
        } else if indent < current_indent {
            // Count how many dedents we need BEFORE modifying the stack
            let mut count = 0;
            for &level in self.indent_stack.iter().rev() {
                if indent >= level {
                    break;
                }
                count += 1;
            }

            // Pop indent levels
            while let Some(&top) = self.indent_stack.last() {
                if indent >= top {
                    break;
                }
                self.indent_stack.pop();
                if self.indent_stack.is_empty() {
                    self.indent_stack.push(0);
                    break;
                }
            }

            // Verify we landed on a valid indent level
            let final_indent = *self.indent_stack.last().unwrap_or(&0);
            if indent != final_indent {
                self.errors.push(errors::inconsistent_indentation(
                    final_indent,
                    indent,
                    Span::new(start, self.current_pos),
                ));
            }

            // Emit dedent tokens
            if count > 0 {
                self.tokens
                    .push(Token::new(TokenKind::Dedent, Span::new(start, self.current_pos)));
                if count > 1 {
                    self.pending_dedents = count - 1;
                }
            }
        }

        self.at_line_start = false;
    }
}

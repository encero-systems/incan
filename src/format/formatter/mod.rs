//! Core formatting logic for Incan source code.
//!
//! Walks the AST and emits properly formatted source code. The heavy lifting is split across focused submodules:
//!
//! - [`declarations`]: imports, models, classes, traits, enums, newtypes, functions, methods, decorators, fields,
//!   params, type params
//! - [`statements`]: assignments, control flow (if/elif/else, while, for), compound statements
//! - [`expressions`]: expressions, literals, operators, patterns, match arms, types

mod declarations;
mod expressions;
mod statements;

use super::config::FormatConfig;
use super::writer::FormatWriter;
use crate::frontend::ast::*;

/// Formatter that transforms AST back to formatted source code.
pub struct Formatter {
    writer: FormatWriter,
}

impl Formatter {
    /// Create a new formatter with the given config.
    pub fn new(config: FormatConfig) -> Self {
        Self {
            writer: FormatWriter::new(config),
        }
    }

    /// Format a program and return the formatted source.
    pub fn format(mut self, program: &Program) -> String {
        self.format_program(program);
        self.writer.finish()
    }

    /// Write the visibility of a declaration.
    fn write_visibility(&mut self, visibility: Visibility) {
        if matches!(visibility, Visibility::Public) {
            self.writer.write("pub ");
        }
    }

    /// Format a program.
    fn format_program(&mut self, program: &Program) {
        let mut first = true;
        let mut prev_was_docstring = false;

        for decl in &program.declarations {
            if !first {
                if prev_was_docstring {
                    self.writer.newline();
                } else {
                    self.writer.blank_lines(2);
                }
            }

            prev_was_docstring = matches!(&decl.node, Declaration::Docstring(_));
            self.format_declaration(&decl.node);
            first = false;
        }

        // Ensure file ends with newline
        self.writer.newline();
    }
}

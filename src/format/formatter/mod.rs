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
        let mut prev_decl: Option<Declaration> = None;
        let mut idx = 0usize;

        while idx < program.declarations.len() {
            let (decl, consumed) = self.coalesce_top_level_decl(&program.declarations, idx);
            if !first {
                let extra_newlines = prev_decl
                    .as_ref()
                    .map(|prev| self.top_level_spacing(prev, &decl))
                    .unwrap_or_default();
                self.writer.blank_lines(extra_newlines);
            }

            prev_decl = Some(decl.clone());
            self.format_declaration(&decl);
            first = false;
            idx += consumed;
        }

        // Ensure file ends with newline
        self.writer.newline();
    }

    /// Coalesce adjacent compatible top-level imports for cleaner Black-style output.
    ///
    /// Today this merges contiguous `from rust::... import ...` declarations that share the same
    /// crate/path/version/features, so repeated imports from one Rust module format as one import block instead of many
    /// visually noisy lines.
    fn coalesce_top_level_decl(&self, decls: &[Spanned<Declaration>], start: usize) -> (Declaration, usize) {
        let Some(base_decl) = decls.get(start).map(|d| &d.node) else {
            return (Declaration::Docstring(String::new()), 1);
        };

        let Declaration::Import(base_import) = base_decl else {
            return (base_decl.clone(), 1);
        };
        let ImportKind::RustFrom {
            crate_name: base_crate,
            path: base_path,
            version: base_version,
            features: base_features,
            items: base_items,
        } = &base_import.kind
        else {
            return (base_decl.clone(), 1);
        };

        let mut merged_items = base_items.clone();
        let mut consumed = 1usize;
        let mut cursor = start + 1;
        while let Some(next_decl) = decls.get(cursor).map(|d| &d.node) {
            let Declaration::Import(next_import) = next_decl else {
                break;
            };
            let ImportKind::RustFrom {
                crate_name,
                path,
                version,
                features,
                items,
            } = &next_import.kind
            else {
                break;
            };

            if crate_name != base_crate || path != base_path || version != base_version || features != base_features {
                break;
            }

            merged_items.extend(items.iter().cloned());
            consumed += 1;
            cursor += 1;
        }

        if consumed == 1 {
            return (base_decl.clone(), 1);
        }

        (
            Declaration::Import(ImportDecl {
                visibility: base_import.visibility,
                kind: ImportKind::RustFrom {
                    crate_name: base_crate.clone(),
                    path: base_path.clone(),
                    version: base_version.clone(),
                    features: base_features.clone(),
                    items: merged_items,
                },
                alias: base_import.alias.clone(),
            }),
            consumed,
        )
    }

    /// Determine extra blank lines to insert between two top-level declarations.
    ///
    /// The declarations themselves already emit a trailing newline, so this returns only the additional newlines needed
    /// to get the desired vertical spacing.
    fn top_level_spacing(&self, prev: &Declaration, next: &Declaration) -> usize {
        if matches!(prev, Declaration::Docstring(_)) {
            // Module docstring is followed by a single blank line.
            return 1;
        }

        match (Self::decl_group(prev), Self::decl_group(next)) {
            (DeclGroup::Import, DeclGroup::Import) => 0,
            (DeclGroup::Import, _) | (_, DeclGroup::Import) => 1,
            (DeclGroup::Const, DeclGroup::Const) => 0,
            (DeclGroup::BlockLike, DeclGroup::BlockLike)
            | (DeclGroup::Const, DeclGroup::BlockLike)
            | (DeclGroup::BlockLike, DeclGroup::Const) => self.writer.config().blank_lines_top_level,
            (DeclGroup::Docstring, _) | (_, DeclGroup::Docstring) => 1,
        }
    }

    fn decl_group(decl: &Declaration) -> DeclGroup {
        match decl {
            Declaration::Import(_) => DeclGroup::Import,
            Declaration::Const(_) => DeclGroup::Const,
            Declaration::Docstring(_) => DeclGroup::Docstring,
            Declaration::Model(_)
            | Declaration::Class(_)
            | Declaration::Trait(_)
            | Declaration::TypeAlias(_)
            | Declaration::Newtype(_)
            | Declaration::Enum(_)
            | Declaration::Function(_) => DeclGroup::BlockLike,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeclGroup {
    Import,
    Const,
    BlockLike,
    Docstring,
}

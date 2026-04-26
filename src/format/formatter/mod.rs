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

pub(super) const RFC053_TOP_LEVEL_BLANK_LINES: usize = 2;
pub(super) const RFC053_METHOD_BLANK_LINES: usize = 1;

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

        // Top-level declarations already end their emitted text with a trailing newline (`writeln`, `newline`, etc.).
        // An extra newline here produced two blank lines at EOF after `reattach_comments` normalized output (#189).
        if program.declarations.is_empty() {
            self.writer.newline();
        }
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
            return if Self::decl_needs_wide_top_level_spacing(next) {
                RFC053_TOP_LEVEL_BLANK_LINES
            } else {
                1
            };
        }

        if Self::decl_needs_wide_top_level_spacing(prev) || Self::decl_needs_wide_top_level_spacing(next) {
            return RFC053_TOP_LEVEL_BLANK_LINES;
        }

        match (Self::decl_spacing_class(prev), Self::decl_spacing_class(next)) {
            (DeclSpacingClass::Docstring, _) | (_, DeclSpacingClass::Docstring) => 1,
            (DeclSpacingClass::Import, DeclSpacingClass::Import)
            | (DeclSpacingClass::ConstLike, DeclSpacingClass::ConstLike) => 0,
            _ => 1,
        }
    }

    fn decl_spacing_class(decl: &Declaration) -> DeclSpacingClass {
        match decl {
            Declaration::Import(_) => DeclSpacingClass::Import,
            Declaration::Const(_) | Declaration::Static(_) => DeclSpacingClass::ConstLike,
            Declaration::Docstring(_) => DeclSpacingClass::Docstring,
            Declaration::TypeAlias(_) | Declaration::Newtype(_) => DeclSpacingClass::TypeLike,
            Declaration::Model(_)
            | Declaration::Class(_)
            | Declaration::Trait(_)
            | Declaration::Enum(_)
            | Declaration::Function(_)
            | Declaration::TestModule(_) => DeclSpacingClass::BodyBearing,
        }
    }

    fn decl_needs_wide_top_level_spacing(decl: &Declaration) -> bool {
        matches!(
            decl,
            Declaration::TypeAlias(_)
                | Declaration::Newtype(_)
                | Declaration::Model(_)
                | Declaration::Class(_)
                | Declaration::Trait(_)
                | Declaration::Enum(_)
                | Declaration::Function(_)
                | Declaration::TestModule(_)
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeclSpacingClass {
    Import,
    ConstLike,
    TypeLike,
    BodyBearing,
    Docstring,
}

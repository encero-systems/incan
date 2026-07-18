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

    /// Write a vocab block keyword exactly as the parser resolved it, including compound keyword tokens such as
    /// `GROUP BY` or `WINDOW BY`, followed by any header expressions.
    fn format_vocab_block_header(&mut self, block: &VocabBlockStmt) {
        self.writer.write(&block.keyword);
        for token in &block.keyword_binding.compound_tokens {
            self.writer.write(" ");
            self.writer.write(token);
        }
        for arg in &block.header_args {
            self.writer.write(" ");
            self.format_expr(&arg.node);
        }
    }

    /// Write an expression-list item without deciding statement terminators.
    fn format_vocab_expression_item_contents(&mut self, item: &VocabExpressionItemStmt) {
        self.format_expr(&item.expr.node);
        if let Some(alias) = &item.alias {
            self.writer.write(" as ");
            self.writer.write(alias);
        }
        for modifier in &item.modifiers {
            self.writer.write(" ");
            self.writer.write(&modifier.keyword);
            self.writer.write(" ");
            self.format_expr(&modifier.value.node);
        }
    }

    /// Write an expression-position vocab block with the brace/no-colon surface used by vocab expressions.
    fn format_expression_vocab_block_braced(&mut self, block: &VocabBlockStmt) {
        self.format_vocab_block_header(block);
        self.writer.write(" {");
        if block.body.is_empty() {
            self.writer.write("}");
            return;
        }

        self.writer.newline();
        self.writer.indent();
        for stmt in &block.body {
            self.format_braced_vocab_statement(stmt);
        }
        self.writer.dedent();
        self.writer.write("}");
    }

    /// Format one statement nested inside an expression-position braced vocab block.
    fn format_braced_vocab_statement(&mut self, stmt: &Spanned<Statement>) {
        self.writer.blank_lines(stmt.leading_blank_lines as usize);
        match &stmt.node {
            Statement::VocabBlock(block) => self.format_braced_vocab_child(block),
            Statement::VocabExpressionItem(item) => {
                self.format_vocab_expression_item_contents(item);
                self.writer.newline();
            }
            Statement::Expr(expr) => {
                self.format_expr(&expr.node);
                self.writer.newline();
            }
            _ => self.format_statement(stmt),
        }
    }

    /// Format a child clause within an expression-position braced vocab block.
    fn format_braced_vocab_child(&mut self, block: &VocabBlockStmt) {
        self.format_vocab_block_header(block);
        match block.keyword_binding.clause_body_kind {
            Some(incan_vocab::ClauseBodyKind::Expression) => {
                if let Some(stmt) = block.body.first()
                    && block.body.len() == 1
                    && self.format_braced_vocab_inline_expression(stmt)
                {
                    self.writer.newline();
                    return;
                }
                self.writer.newline();
                self.writer.indent();
                for stmt in &block.body {
                    self.format_braced_vocab_statement(stmt);
                }
                self.writer.dedent();
            }
            Some(incan_vocab::ClauseBodyKind::ExpressionList) => {
                if block.body.len() == 1 && !block.body_item_trailing_commas.first().copied().unwrap_or(false) {
                    let checkpoint = self.writer.checkpoint();
                    if self.format_braced_vocab_inline_expression(&block.body[0])
                        && !self.writer.output_since_contains_newline(checkpoint)
                        && !self.writer.line_length_exceeded()
                    {
                        self.writer.newline();
                        return;
                    }
                    self.writer.restore(checkpoint);
                }

                self.writer.newline();
                self.writer.indent();
                for (idx, stmt) in block.body.iter().enumerate() {
                    self.format_braced_vocab_expression_list_item(stmt);
                    if block.body_item_trailing_commas.get(idx).copied().unwrap_or(false) {
                        self.writer.write(",");
                    }
                    self.writer.newline();
                }
                self.writer.dedent();
            }
            _ => {
                self.writer.newline();
                self.writer.indent();
                for stmt in &block.body {
                    self.format_braced_vocab_statement(stmt);
                }
                self.writer.dedent();
            }
        }
    }

    /// Try to write a single expression-clause body inline after its keyword.
    fn format_braced_vocab_inline_expression(&mut self, stmt: &Spanned<Statement>) -> bool {
        match &stmt.node {
            Statement::Expr(expr) => {
                self.writer.write(" ");
                self.format_expr(&expr.node);
                true
            }
            Statement::VocabExpressionItem(item) => {
                self.writer.write(" ");
                self.format_vocab_expression_item_contents(item);
                true
            }
            _ => false,
        }
    }

    /// Write one braced expression-list item without a trailing newline.
    fn format_braced_vocab_expression_list_item(&mut self, stmt: &Spanned<Statement>) {
        match &stmt.node {
            Statement::Expr(expr) => self.format_expr(&expr.node),
            Statement::VocabExpressionItem(item) => self.format_vocab_expression_item_contents(item),
            _ => self.format_statement(stmt),
        }
    }

    /// Format a program.
    fn format_program(&mut self, program: &Program) {
        let mut first = true;
        let mut prev_decl: Option<Declaration> = None;
        let mut idx = 0usize;

        if let Some(first_declaration) = program.declarations.first()
            && first_declaration.required_features.is_empty()
            && let Declaration::Docstring(doc) = &first_declaration.node
        {
            self.format_docstring(doc);
            prev_decl = Some(Declaration::Docstring(doc.clone()));
            first = false;
            idx = 1;
        }

        if let Some(rust_module_path) = &program.rust_module_path {
            if !first {
                self.writer.blank_lines(1);
            }
            self.writer.write("rust.module(\"");
            self.writer.write(&rust_module_path.node);
            self.writer.writeln("\")");
            first = false;
        }

        while idx < program.declarations.len() {
            let required_features = program.declarations[idx].required_features.clone();
            if !required_features.is_empty() {
                let group_end = program.declarations[idx..]
                    .iter()
                    .position(|declaration| declaration.required_features != required_features)
                    .map(|offset| idx + offset)
                    .unwrap_or(program.declarations.len());
                let first_decl = program.declarations[idx].node.clone();
                if !first {
                    let extra_newlines = prev_decl
                        .as_ref()
                        .map(|prev| self.top_level_spacing(prev, &first_decl))
                        .unwrap_or_else(|| usize::from(program.rust_module_path.is_some()));
                    self.writer.blank_lines(extra_newlines);
                }
                self.format_feature_condition_header(&required_features);
                self.writer.indent();
                let mut inner_idx = idx;
                let mut inner_prev: Option<Declaration> = None;
                while inner_idx < group_end {
                    let (decl, consumed) = self.coalesce_top_level_decl(&program.declarations, inner_idx);
                    if let Some(previous) = inner_prev.as_ref() {
                        self.writer.blank_lines(self.top_level_spacing(previous, &decl));
                    }
                    self.format_declaration(&decl);
                    inner_prev = Some(decl);
                    inner_idx += consumed;
                }
                self.writer.dedent();
                prev_decl = inner_prev;
                first = false;
                idx = group_end;
                continue;
            }

            let (decl, consumed) = self.coalesce_top_level_decl(&program.declarations, idx);
            if !first {
                let extra_newlines = prev_decl
                    .as_ref()
                    .map(|prev| self.top_level_spacing(prev, &decl))
                    .unwrap_or_else(|| usize::from(program.rust_module_path.is_some()));
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

    /// Write the normalized positive conjunction that owns a feature-conditioned declaration group.
    pub(super) fn format_feature_condition_header(&mut self, required_features: &[String]) {
        self.writer.write("when ");
        for (index, feature) in required_features.iter().enumerate() {
            if index > 0 {
                self.writer.write(" and ");
            }
            self.writer.write("feature(\"");
            self.writer.write(feature);
            self.writer.write("\")");
        }
        self.writer.writeln(":");
    }

    /// Coalesce adjacent compatible top-level imports for cleaner Black-style output.
    ///
    /// Today this merges contiguous `from rust::... import ...` declarations that share the same
    /// crate/path/version/features, so repeated imports from one Rust module format as one import block instead of many
    /// visually noisy lines.
    fn coalesce_top_level_decl(&self, decls: &[Spanned<Declaration>], start: usize) -> (Declaration, usize) {
        let Some(base_spanned) = decls.get(start) else {
            return (Declaration::Docstring(String::new()), 1);
        };
        let base_decl = &base_spanned.node;

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
        while let Some(next_spanned) = decls.get(cursor) {
            if next_spanned.required_features != base_spanned.required_features {
                break;
            }
            let next_decl = &next_spanned.node;
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

    /// Classify declarations for formatter blank-line spacing.
    fn decl_spacing_class(decl: &Declaration) -> DeclSpacingClass {
        match decl {
            Declaration::Import(_) => DeclSpacingClass::Import,
            Declaration::Const(_) | Declaration::Static(_) | Declaration::Alias(_) | Declaration::Partial(_) => {
                DeclSpacingClass::ConstLike
            }
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

    /// Return whether a declaration needs wider top-level spacing.
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

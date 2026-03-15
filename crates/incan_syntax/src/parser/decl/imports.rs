/// Import parsing (`import ...`, `from ... import ...`, `rust::`, `python ...`).
impl<'a> Parser<'a> {
    fn expect_pub_namespace_separator(&mut self, form: errors::PubImportForm) -> Result<(), CompileError> {
        if self.match_punct(PunctuationId::ColonColon) {
            return Ok(());
        }
        if self.match_punct(PunctuationId::Dot) {
            return Err(errors::pub_import_expected_namespace_separator(self.current_span(), form));
        }
        Err(errors::pub_import_expected_namespace_separator(self.current_span(), form))
    }

    fn import_decl(&mut self, visibility: Visibility) -> Result<ImportDecl, CompileError> {
        // Check for "from ... import ..." syntax
        if self.match_keyword(KeywordId::From) {
            // Check for "from rust::crate import ..." syntax
            if self.match_keyword(KeywordId::Rust) {
                // RFC 005: dot-notation `from rust.crate import ...` — warn and recover by treating `.` as `::`.
                if self.check_punct(PunctuationId::Dot) {
                    self.warnings
                        .push(errors::rust_import_dot_notation(self.current_span(), errors::RustImportForm::From));
                    self.match_punct(PunctuationId::Dot);
                } else {
                    self.expect_punct(PunctuationId::ColonColon, "Expected '::' after 'rust'")?;
                }
                let (crate_name, path) = self.rust_crate_path()?;
                let (version, features) = self.rust_import_spec()?;
                self.expect_keyword(KeywordId::Import, "Expected 'import' after rust crate path")?;

                let items = self.parse_import_items()?;

                return Ok(ImportDecl {
                    visibility,
                    kind: ImportKind::RustFrom {
                        crate_name,
                        path,
                        version,
                        features,
                        items,
                    },
                    alias: None,
                });
            }

            // Check for "from pub::library import ..." syntax
            if self.match_keyword(KeywordId::Pub) {
                self.expect_pub_namespace_separator(errors::PubImportForm::From)?;
                let library = self.identifier()?;
                if self.check_punct(PunctuationId::ColonColon) || self.check_punct(PunctuationId::Dot) {
                    return Err(errors::pub_import_submodule_not_supported(self.current_span()));
                }
                self.expect_keyword(KeywordId::Import, "Expected 'import' after pub library path")?;

                let items = self.parse_import_items()?;

                return Ok(ImportDecl {
                    visibility,
                    kind: ImportKind::PubFrom { library, items },
                    alias: None,
                });
            }

            // Regular from import
            let module = self.import_path()?;
            self.expect_keyword(KeywordId::Import, "Expected 'import' after module path")?;

            // Parse import items: `ItemA, ItemB as alias` or `(ItemA, ItemB as alias,)`.
            let items = self.parse_import_items()?;

            return Ok(ImportDecl {
                visibility,
                kind: ImportKind::From { module, items },
                alias: None,
            });
        }

        // Regular import syntax (Rust-style)
        self.expect_keyword(KeywordId::Import, "Expected 'import'")?;

        let kind = if self.match_keyword(KeywordId::Python) {
            // Python import: import python "package" as alias
            let pkg = self.string_literal()?;
            ImportKind::Python(pkg)
        } else if self.match_keyword(KeywordId::Rust) {
            // Rust crate import: import rust::serde_json or import rust::serde_json::Value
            // RFC 005: dot-notation `import rust.crate` — warn and recover by treating `.` as `::`.
            if self.check_punct(PunctuationId::Dot) {
                self.warnings
                    .push(errors::rust_import_dot_notation(self.current_span(), errors::RustImportForm::Import));
                self.match_punct(PunctuationId::Dot);
            } else {
                self.expect_punct(PunctuationId::ColonColon, "Expected '::' after 'rust'")?;
            }
            let (crate_name, path) = self.rust_crate_path()?;
            let (version, features) = self.rust_import_spec()?;
            ImportKind::RustCrate {
                crate_name,
                path,
                version,
                features,
            }
        } else if self.match_keyword(KeywordId::Pub) {
            self.expect_pub_namespace_separator(errors::PubImportForm::Import)?;
            let library = self.identifier()?;
            if self.check_punct(PunctuationId::ColonColon) || self.check_punct(PunctuationId::Dot) {
                return Err(errors::pub_import_submodule_not_supported(self.current_span()));
            }
            ImportKind::PubLibrary { library }
        } else {
            // Module import: import foo::bar::baz or import super::foo or import crate::foo
            let path = self.import_path()?;
            ImportKind::Module(path)
        };

        let alias = if self.match_keyword(KeywordId::As) {
            Some(self.identifier()?)
        } else {
            None
        };

        Ok(ImportDecl {
            visibility,
            kind,
            alias,
        })
    }

    /// Parse a Rust crate path after `rust::` (or `rust.` recovery)
    /// Returns (crate_name, optional_path_within_crate)
    /// Examples:
    /// - `serde_json` -> ("serde_json", [])
    /// - `serde_json::Value` -> ("serde_json", ["Value"])
    /// - `std::collections::HashMap` -> ("std", ["collections", "HashMap"])
    ///
    /// Both `::` and `.` are accepted as separators here to support dot-notation recovery
    /// (`from rust.std.time import Instant` → same result as `from rust::std::time import Instant`).
    fn rust_crate_path(&mut self) -> Result<(String, Vec<Ident>), CompileError> {
        let crate_name = self.identifier()?;
        let mut path = Vec::new();

        while self.match_punct(PunctuationId::ColonColon) || self.match_punct(PunctuationId::Dot) {
            let segment = self.identifier()?;
            path.push(segment);
        }

        Ok((crate_name, path))
    }

    /// Parse optional `@ "version"` and `with ["feature"]` specifiers for rust imports.
    fn rust_import_spec(&mut self) -> Result<(Option<String>, Vec<String>), CompileError> {
        let mut version = None;
        let mut features = Vec::new();

        if self.match_punct(PunctuationId::At) {
            version = Some(self.string_literal()?);

            if self.match_keyword(KeywordId::With) {
                features = self.string_list()?;
            }
        } else if self.check_keyword(KeywordId::With) {
            return Err(errors::rust_import_features_require_version(self.current_span()));
        }

        Ok((version, features))
    }

    /// Parse a bracketed list of string literals, e.g. `["a", "b"]`.
    fn string_list(&mut self) -> Result<Vec<String>, CompileError> {
        self.expect_punct(PunctuationId::LBracket, "Expected '[' to start feature list")?;
        let mut items = Vec::new();

        if self.match_punct(PunctuationId::RBracket) {
            return Ok(items);
        }

        loop {
            items.push(self.string_literal()?);
            if self.match_punct(PunctuationId::Comma) {
                continue;
            }
            self.expect_punct(PunctuationId::RBracket, "Expected ']' after feature list")?;
            break;
        }

        Ok(items)
    }

    /// Parse an import path, supporting:
    /// - Simple: `models`, `utils::helpers`
    /// - Relative with dots: `..common`, `...shared.utils`
    /// - Relative with super: `super::common`, `super::super::utils`
    /// - Absolute with crate: `crate::config`
    /// - Dotted paths: `db.models`, `api.handlers.auth`
    fn import_path(&mut self) -> Result<ImportPath, CompileError> {
        let mut parent_levels = 0;
        let mut is_absolute = false;
        let mut segments = Vec::new();

        // Check for leading `..` (Python-style parent navigation)
        while self.match_op(OperatorId::DotDot) {
            parent_levels += 1;
        }

        // Check for `crate` (absolute path)
        if parent_levels == 0 && self.match_keyword(KeywordId::Crate) {
            is_absolute = true;
            // Expect :: or . after crate
            if !self.match_punct(PunctuationId::ColonColon) && !self.match_punct(PunctuationId::Dot) {
                return Err(errors::import_path_expected_separator_after_crate(
                    self.current_span(),
                ));
            }
        }

        // Check for `super` (Rust-style parent navigation)
        while self.match_keyword(KeywordId::Super) {
            parent_levels += 1;
            // Expect :: or . after super
            if !self.match_punct(PunctuationId::ColonColon) && !self.match_punct(PunctuationId::Dot) {
                // Could be end of path if no more segments
                if !self.check_keyword(KeywordId::Import)
                    && !self.check_keyword(KeywordId::As)
                    && !self.check(&TokenKind::Newline)
                {
                    return Err(errors::import_path_expected_separator_after_super(
                        self.current_span(),
                    ));
                }
            }
        }

        // Parse the actual path segments
        // First segment
        if let Ok(first) = self.identifier_or_import_keyword() {
            segments.push(first);

            // Continue with :: or . separators
            loop {
                if self.match_punct(PunctuationId::ColonColon) || self.match_punct(PunctuationId::Dot) {
                    segments.push(self.identifier_or_import_keyword()?);
                } else {
                    break;
                }
            }
        }

        Ok(ImportPath {
            parent_levels,
            is_absolute,
            segments,
        })
    }

    /// Parse a comma-separated list of import items, with optional parenthesization.
    ///
    /// Accepts both:
    /// - Bare list: `ItemA, ItemB as alias, ItemC`
    /// - Parenthesized list: `(\n    ItemA,\n    ItemB as alias,\n    ItemC,\n)`
    ///
    /// The lexer's `bracket_depth` tracking suppresses `Newline`/`Indent`/`Dedent` tokens inside
    /// `(...)`, so no explicit newline handling is needed here — multi-line layouts parse
    /// identically to single-line layouts.
    fn parse_import_items(&mut self) -> Result<Vec<ImportItem>, CompileError> {
        let parenthesized = self.match_punct(PunctuationId::LParen);
        let mut items = Vec::new();

        // ---- Empty parenthesized list: `from db import ()` ----
        if parenthesized && self.match_punct(PunctuationId::RParen) {
            return Err(errors::import_list_empty(self.current_span()));
        }

        // ---- Parse one item at a time ----
        loop {
            let name = self.identifier()?;
            let alias = if self.match_keyword(KeywordId::As) {
                Some(self.identifier()?)
            } else {
                None
            };
            items.push(ImportItem { name, alias });

            if !self.match_punct(PunctuationId::Comma) {
                break;
            }
            // Allow a trailing comma before `)` without requiring another identifier.
            if parenthesized && self.check_punct(PunctuationId::RParen) {
                break;
            }
        }

        if parenthesized {
            self.expect_punct(PunctuationId::RParen, "Expected ')' to close import list")?;
        }

        Ok(items)
    }
}

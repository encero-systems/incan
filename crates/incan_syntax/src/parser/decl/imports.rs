/// Import parsing (`import ...`, `from ... import ...`, `rust::`, `python ...`).
impl<'a> Parser<'a> {
    fn import_decl(&mut self) -> Result<ImportDecl, CompileError> {
        // Check for "from ... import ..." syntax
        if self.match_keyword(KeywordId::From) {
            // Check for "from rust::crate import ..." syntax
            if self.match_keyword(KeywordId::Rust) {
                self.expect_punct(PunctuationId::ColonColon, "Expected '::' after 'rust'")?;
                let (crate_name, path) = self.rust_crate_path()?;
                let (version, features) = self.rust_import_spec()?;
                self.expect_keyword(KeywordId::Import, "Expected 'import' after rust crate path")?;

                // Parse import items
                let mut items = Vec::new();
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
                }

                return Ok(ImportDecl {
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

            // Regular from import
            let module = self.import_path()?;
            self.expect_keyword(KeywordId::Import, "Expected 'import' after module path")?;

            // Parse import items: item1, item2 as alias, item3, ...
            let mut items = Vec::new();
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
            }

            return Ok(ImportDecl {
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
            self.expect_punct(PunctuationId::ColonColon, "Expected '::' after 'rust'")?;
            let (crate_name, path) = self.rust_crate_path()?;
            let (version, features) = self.rust_import_spec()?;
            ImportKind::RustCrate {
                crate_name,
                path,
                version,
                features,
            }
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

        Ok(ImportDecl { kind, alias })
    }

    /// Parse a Rust crate path after `rust::`
    /// Returns (crate_name, optional_path_within_crate)
    /// Examples:
    /// - `serde_json` -> ("serde_json", [])
    /// - `serde_json::Value` -> ("serde_json", ["Value"])
    /// - `std::collections::HashMap` -> ("std", ["collections", "HashMap"])
    fn rust_crate_path(&mut self) -> Result<(String, Vec<Ident>), CompileError> {
        let crate_name = self.identifier()?;
        let mut path = Vec::new();

        while self.match_punct(PunctuationId::ColonColon) {
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
}

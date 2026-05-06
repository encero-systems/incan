/// Parser core types and entrypoint.
///
/// This chunk defines the [`Parser`] type and its top-level `parse()` entrypoint.
/// It also contains a few small internal helper types shared across the other parser chunks.
///
/// ## Notes
/// - This file is `include!`'d into `crate::parser` to keep all parser methods in a
///   single module while avoiding a single “god file”.
type FieldsAndMethods = (
    Vec<Spanned<FieldDecl>>,
    Vec<Spanned<MethodAliasDecl>>,
    Vec<Spanned<PropertyDecl>>,
    Vec<Spanned<MethodDecl>>,
);

/// Result of parsing `[...]` postfix syntax: either a single index or a slice.
enum IndexOrSlice {
    Index(Spanned<Expr>),
    Slice(SliceExpr),
}

#[derive(Debug, Clone)]
struct ActiveImportedKeywordSpec {
    keyword_name: String,
    dependency_key: String,
    activation_namespace: String,
    valid_decorators: Vec<String>,
    surface_kind: incan_vocab::KeywordSurfaceKind,
    placement: incan_vocab::KeywordPlacement,
}

#[derive(Debug, Clone)]
struct ActiveScopedSurfaceDescriptor {
    dependency_key: String,
    descriptor: incan_vocab::ScopedSurfaceDescriptor,
}

#[derive(Debug, Clone)]
struct ScopedCallArgumentContext {
    call: String,
}

/// Parser state.
///
/// ## Notes
/// - The parser is intentionally single-pass and recovers from errors where possible by
///   synchronizing at statement/declaration boundaries.
/// - Most parsing helpers are implemented on `Parser` but split across multiple files.
pub struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    errors: Vec<CompileError>,
    /// Non-fatal warnings accumulated during parsing (e.g. style nudges that don't block compilation).
    warnings: Vec<CompileError>,
    active_soft_keywords: std::collections::HashSet<KeywordId>,
    active_imported_keyword_specs: std::collections::HashMap<String, Vec<ActiveImportedKeywordSpec>>,
    vocab_block_stack: Vec<String>,
    module_path: Option<String>,
    library_imported_vocab: ImportedLibraryVocab,
    library_imported_dsl_surfaces: ImportedLibraryDslSurfaces,
    active_scoped_surface_descriptors: Vec<ActiveScopedSurfaceDescriptor>,
    scoped_call_argument_stack: Vec<ScopedCallArgumentContext>,
    /// Blank-line intent consumed by an inner block immediately before its `Dedent`.
    ///
    /// The next outer statement should receive this as `leading_blank_lines`; otherwise a readable gap after a nested
    /// suite is lost before the outer block can see it.
    pending_dedent_blank_lines: u8,
}

/// Compares a path segment to an expected spelling for parser path-context checks.
#[cfg(windows)]
fn path_segment_eq(expected: &str, actual: &std::ffi::OsStr) -> bool {
    actual
        .to_str()
        .map(|value| value.eq_ignore_ascii_case(expected))
        .unwrap_or(false)
}

/// Compares a path segment to an expected spelling for parser path-context checks.
#[cfg(not(windows))]
fn path_segment_eq(expected: &str, actual: &std::ffi::OsStr) -> bool {
    actual == std::ffi::OsStr::new(expected)
}

impl<'a> Parser<'a> {
    /// Create a new parser for a token stream.
    ///
    /// ## Parameters
    /// - `tokens`: Token stream produced by `incan_syntax::lexer`.
    pub fn new(tokens: &'a [Token]) -> Self {
        Self::new_with_context(tokens, None, None, None)
    }

    /// Create a new parser for a token stream with optional module path context.
    pub fn new_with_module_path(tokens: &'a [Token], module_path: Option<String>) -> Self {
        Self::new_with_context(tokens, module_path, None, None)
    }

    /// Create a new parser for a token stream with optional module path and library keyword context.
    pub fn new_with_context(
        tokens: &'a [Token],
        module_path: Option<String>,
        library_imported_vocab: Option<&ImportedLibraryVocab>,
        library_imported_dsl_surfaces: Option<&ImportedLibraryDslSurfaces>,
    ) -> Self {
        Self {
            tokens,
            pos: 0,
            errors: Vec::new(),
            warnings: Vec::new(),
            active_soft_keywords: std::collections::HashSet::new(),
            active_imported_keyword_specs: std::collections::HashMap::new(),
            vocab_block_stack: Vec::new(),
            module_path,
            library_imported_vocab: library_imported_vocab.cloned().unwrap_or_default(),
            library_imported_dsl_surfaces: library_imported_dsl_surfaces.cloned().unwrap_or_default(),
            active_scoped_surface_descriptors: Vec::new(),
            scoped_call_argument_stack: Vec::new(),
            pending_dedent_blank_lines: 0,
        }
    }

    /// Parse the entire token stream into a [`Program`].
    ///
    /// ## Errors
    /// Returns a list of [`CompileError`]s if parsing fails. The parser attempts to recover and continue after an error
    /// to report multiple issues in one pass.
    pub fn parse(mut self) -> Result<Program, Vec<CompileError>> {
        let mut declarations = Vec::new();
        let mut rust_module_path: Option<Spanned<String>> = None;
        let mut seen_non_doc_decl = false;
        let mut seen_test_module = false;

        // Skip leading newlines
        self.skip_newlines();
        // Stray top-level DEDENT can appear after error recovery (e.g. unexpected indentation).
        // Ignore it at the module level to avoid cascaded errors.
        self.skip_dedents();

        while !self.is_at_end() {
            // ---- Context: `rust.module("...")` directive (RFC 023) ----
            if self.check_keyword(KeywordId::Rust)
                && self.peek_next().kind == TokenKind::Punctuation(PunctuationId::Dot)
            {
                match self.rust_module_directive() {
                    Ok(directive) => {
                        if seen_non_doc_decl {
                            self.errors.push(errors::rust_module_not_at_top(directive.span));
                        }
                        if rust_module_path.is_some() {
                            self.errors.push(errors::duplicate_rust_module(directive.span));
                        } else {
                            rust_module_path = Some(directive);
                        }
                    }
                    Err(e) => {
                        self.errors.push(e);
                        self.synchronize();
                    }
                }
                self.skip_newlines();
                self.skip_dedents();
                continue;
            }

            // ---- Context: normal declarations ----
            match self.declaration() {
                Ok(decl) => {
                    if matches!(decl.node, Declaration::TestModule(_)) {
                        if seen_test_module {
                            self.errors.push(CompileError::syntax(
                                "Only one `module tests:` block is allowed per file".to_string(),
                                decl.span,
                            ));
                        }
                        seen_test_module = true;
                    }
                    self.activate_soft_keywords_for_declaration(&decl.node);
                    if !matches!(decl.node, Declaration::Docstring(_)) {
                        seen_non_doc_decl = true;
                    }
                    declarations.push(decl)
                }
                Err(e) => {
                    self.errors.push(e);
                    self.synchronize();
                }
            }
            self.skip_newlines();
            // Same rationale as above: at the module level we should not see DEDENT tokens,
            // but the lexer may emit them and recovery may leave us positioned on them.
            self.skip_dedents();
        }

        if self.errors.is_empty() {
            Ok(Program {
                declarations,
                rust_module_path,
                warnings: self.warnings,
            })
        } else {
            // Fold non-fatal warnings into the error list so callers don't silently lose them when parsing fails.
            // Warnings retain their `ErrorKind::Warning` kind so callers can still distinguish them from errors if needed.
            self.errors.append(&mut self.warnings);
            Err(self.errors)
        }
    }

    /// Parse a `rust.module("path::to::module")` directive.
    ///
    /// Expects the current token to be `Keyword(Rust)`. Consumes `rust . module ( "..." )`.
    fn rust_module_directive(&mut self) -> Result<Spanned<String>, CompileError> {
        let start = self.current_span().start;

        // Consume `rust`
        self.expect_keyword(KeywordId::Rust, "Expected 'rust'")?;

        // Consume `.`
        self.expect_punct(PunctuationId::Dot, "Expected '.' after 'rust'")?;

        // Consume `module` (an identifier, not a keyword)
        let name = self.identifier_spanned()?;
        if name.node != "module" {
            return Err(errors::expected_token_message(
                "Expected 'module' after 'rust.'",
                &name.node,
                name.span,
            ));
        }

        // Consume `(` string_literal `)`
        self.expect_punct(PunctuationId::LParen, "Expected '(' after 'rust.module'")?;
        let path = self.string_literal()?;
        self.expect_punct(PunctuationId::RParen, "Expected ')' after rust.module path")?;

        let end = self.tokens[self.pos.saturating_sub(1)].span.end;
        Ok(Spanned::new(path, Span::new(start, end)))
    }

    /// Whether the parser is currently parsing a module under `src/`.
    ///
    /// This gates [`Visibility::Public`] on `from ... import ...` (RFC 031). Callers must pass a filesystem-style module path (as the CLI and LSP do) so the parser can enforce that `pub from` appears only in source modules.
    ///
    /// On Windows, path-segment checks are ASCII case-insensitive so editor URIs that normalize path casing still match.
    fn is_src_module(&self) -> bool {
        let Some(module_path) = self.module_path.as_deref() else {
            return false;
        };

        let path = std::path::Path::new(module_path);
        if path.file_name().is_none() {
            return false;
        }

        path.ancestors()
            .skip(1)
            .filter_map(std::path::Path::file_name)
            .any(|segment| path_segment_eq("src", segment))
    }

    /// Activate soft keywords introduced by stdlib or library imports in this declaration.
    fn activate_soft_keywords_for_declaration(&mut self, decl: &Declaration) {
        if let Declaration::Import(import) = decl {
            match &import.kind {
                ImportKind::Module(path) => {
                    for kw in incan_core::lang::stdlib::soft_keywords_for_import(&path.segments) {
                        self.active_soft_keywords.insert(kw);
                    }
                }
                ImportKind::From { module, .. } => {
                    for kw in incan_core::lang::stdlib::soft_keywords_for_import(&module.segments) {
                        self.active_soft_keywords.insert(kw);
                    }
                }
                ImportKind::PubLibrary { library } => {
                    self.activate_imported_keywords_for_library(library);
                }
                ImportKind::PubFrom { library, .. } => {
                    self.activate_imported_keywords_for_library(library);
                }
                _ => {}
            }
        }
    }

    /// Activate keyword registrations contributed by a `pub::` library dependency.
    ///
    /// This bridges serialized vocab metadata into parser state by:
    /// - recording compatible soft-keyword ids in `active_soft_keywords` (for existing parser flows), and
    /// - recording imported keyword surface specs in `active_imported_keyword_specs`
    ///   (for surface-kind checks driven by imported metadata).
    fn activate_imported_keywords_for_library(&mut self, library: &str) {
        if let Some(surfaces) = self.library_imported_dsl_surfaces.get(library) {
            for surface in surfaces {
                if !dsl_surface_applies_to_pub_import(surface, library) {
                    continue;
                }
                self.active_scoped_surface_descriptors.extend(
                    surface
                        .scoped_surfaces
                        .iter()
                        .cloned()
                        .map(|descriptor| ActiveScopedSurfaceDescriptor {
                            dependency_key: library.to_string(),
                            descriptor,
                        }),
                );
            }
        }

        let Some(registrations) = self.library_imported_vocab.get(library) else {
            return;
        };

        for registration in registrations {
            if !registration_applies_to_pub_import(registration, library) {
                continue;
            }

            for keyword in &registration.keywords {
                let specs = self
                    .active_imported_keyword_specs
                    .entry(keyword.name.clone())
                    .or_default();
                specs.push(ActiveImportedKeywordSpec {
                    keyword_name: keyword.name.clone(),
                    dependency_key: library.to_string(),
                    activation_namespace: match &registration.activation {
                        incan_vocab::KeywordActivation::OnImport { namespace } => namespace.clone(),
                        _ => library.to_string(),
                    },
                    valid_decorators: registration.valid_decorators.clone(),
                    surface_kind: keyword.surface_kind,
                    placement: keyword.placement.clone(),
                });
                if let Some(id) = incan_core::lang::keywords::from_str(&keyword.name)
                    && incan_core::lang::keywords::is_soft(id)
                {
                    self.active_soft_keywords.insert(id);
                }
            }
        }
    }
}

/// Return `true` when a DSL surface should activate for a `pub::library` import.
fn dsl_surface_applies_to_pub_import(surface: &incan_vocab::DslSurface, library: &str) -> bool {
    match &surface.activation {
        incan_vocab::KeywordActivation::Always => true,
        incan_vocab::KeywordActivation::OnImport { namespace } => namespace_matches_pub_library(namespace, library),
        _ => false,
    }
}

/// Return `true` when a registration should be activated for `pub::library` imports.
///
/// `OnImport` namespaces match either the library key exactly (`widgets`) or one of its child namespaces
/// (`widgets.dsl`).
fn registration_applies_to_pub_import(registration: &incan_vocab::KeywordRegistration, library: &str) -> bool {
    match &registration.activation {
        incan_vocab::KeywordActivation::Always => true,
        incan_vocab::KeywordActivation::OnImport { namespace } => namespace_matches_pub_library(namespace, library),
        _ => false,
    }
}

/// Return whether an `OnImport` namespace activates for a `pub::library` import.
fn namespace_matches_pub_library(namespace: &str, library: &str) -> bool {
    let trimmed = namespace.trim();
    !trimmed.is_empty() && (trimmed == library || trimmed.starts_with(&format!("{library}.")))
}

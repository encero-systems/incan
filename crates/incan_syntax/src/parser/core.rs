/// Parser core types and entrypoint.
///
/// This chunk defines the [`Parser`] type and its top-level `parse()` entrypoint.
/// It also contains a few small internal helper types shared across the other parser chunks.
///
/// ## Notes
/// - This file is `include!`'d into `crate::parser` to keep all parser methods in a single module while avoiding a
///   single “god file”.
type FieldsAndMethods = (
    Vec<Spanned<FieldDecl>>,
    Vec<Spanned<MethodAliasDecl>>,
    Vec<Spanned<MethodPartialDecl>>,
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
    compound_tokens: Vec<String>,
    dependency_key: String,
    activation_namespace: String,
    valid_decorators: Vec<String>,
    surface_kind: incan_vocab::KeywordSurfaceKind,
    placement: incan_vocab::KeywordPlacement,
    declaration_head_kind: incan_vocab::DeclarationHeadKind,
    desugar_target: incan_vocab::DesugarTarget,
    is_declaration_owned_clause: bool,
    clause_body_kind: Option<incan_vocab::ClauseBodyKind>,
    expression_item_modifiers: Vec<incan_vocab::ExpressionItemModifierSurface>,
}

#[derive(Debug, Clone)]
struct ActiveScopedSurfaceDescriptor {
    dependency_key: String,
    descriptor: incan_vocab::ScopedSurfaceDescriptor,
}

#[derive(Debug, Clone)]
struct ActiveScopedSymbolDescriptor {
    dependency_key: String,
    descriptor: incan_vocab::ScopedSymbolDescriptor,
}

/// One embedded-fragment descriptor (RFC 081) activated by an import in the current file.
#[derive(Debug, Clone)]
struct ActiveEmbeddedFragmentDescriptor {
    dependency_key: String,
    descriptor: incan_vocab::EmbeddedFragmentDescriptor,
}

#[derive(Debug, Clone)]
struct ScopedCallArgumentContext {
    call: String,
}

/// Parser state.
///
/// ## Notes
/// - The parser is intentionally single-pass and recovers from errors where possible by synchronizing at
///   statement/declaration boundaries.
/// - Most parsing helpers are implemented on `Parser` but split across multiple files.
pub struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    /// Original source text, when available.
    ///
    /// This is `None` for every existing parser entrypoint (`new`, `new_with_module_path`, `new_with_context`) —
    /// they only ever received a token stream, and this field is purely additive so none of their behavior
    /// changes. It is `Some` only via [`Parser::new_with_source`], used by the RFC 081 (`#1023`) embedded-fragment
    /// mechanism: a descriptor-gated lexical submode re-tokenizes the fragment body's *raw source slice* directly
    /// (see `parser/embedded/mod.rs`), because token forms like `#1166ff`, `16px`, or `<section>` cannot be
    /// faithfully reconstructed from whatever the ordinary lexer already did to that byte range (for example, `#`
    /// silently starts a line comment in ordinary Incan lexing). The byte range itself is still recovered purely
    /// from existing `Indent`/`Dedent` token spans — no lexer change is needed to compute it.
    source: Option<&'a str>,
    errors: Vec<CompileError>,
    /// Non-fatal warnings accumulated during parsing (e.g. style nudges that don't block compilation).
    warnings: Vec<CompileError>,
    /// Byte spans of every embedded fragment (RFC 081, `#1023`) successfully claimed during this parse.
    ///
    /// Populated by `Parser::try_embedded_fragment_body` on each successful claim. `parse()` uses this to decide
    /// which of `pending_lex_errors` are expected noise (the ordinary lexer's honest confusion about foreign
    /// submode syntax it was never meant to understand) versus real user mistakes elsewhere in the file.
    claimed_embedded_fragment_spans: Vec<Span>,
    /// Lex errors collected by a tolerant pre-lex pass, reconciled against `claimed_embedded_fragment_spans` at
    /// the end of `parse()` rather than surfaced unconditionally.
    ///
    /// Only ever non-empty via [`Parser::with_pending_lex_errors`], used by the RFC 081 (`#1023`) production
    /// parsing entrypoint (`parser::parse_with_source_and_lex_errors`) for source that the ordinary strict lexer
    /// could not tokenize outright. A lex error whose span falls inside a fragment this parse actually claimed is
    /// expected -- the fragment's own submode tokenizer re-scans that byte range independently and never consults
    /// the ordinary lexer's output for it. A lex error outside every claimed fragment is a real mistake and must
    /// still reach the user.
    pending_lex_errors: Vec<CompileError>,
    active_soft_keywords: std::collections::HashSet<KeywordId>,
    active_imported_keyword_specs: std::collections::HashMap<String, Vec<ActiveImportedKeywordSpec>>,
    vocab_block_stack: Vec<String>,
    vocab_body_kind_stack: Vec<Option<incan_vocab::ClauseBodyKind>>,
    vocab_expression_item_modifier_stack: Vec<Vec<incan_vocab::ExpressionItemModifierSurface>>,
    module_path: Option<String>,
    library_imported_vocab: ImportedLibraryVocab,
    library_imported_dsl_surfaces: ImportedLibraryDslSurfaces,
    std_async_vocab_active: bool,
    active_scoped_surface_descriptors: Vec<ActiveScopedSurfaceDescriptor>,
    active_scoped_symbol_descriptors: Vec<ActiveScopedSymbolDescriptor>,
    active_embedded_fragment_descriptors: Vec<ActiveEmbeddedFragmentDescriptor>,
    scoped_call_argument_stack: Vec<ScopedCallArgumentContext>,
    /// Blank-line intent consumed by an inner block immediately before its `Dedent`.
    ///
    /// The next outer statement should receive this as `leading_blank_lines`; otherwise a readable gap after a nested
    /// suite is lost before the outer block can see it.
    pending_dedent_blank_lines: u8,
}

/// Whether `inner` lies entirely within `outer` (RFC 081, `#1023` tolerant-lex-error reconciliation).
///
/// Used to decide whether a tolerant-lexer error falls inside a successfully-claimed embedded fragment. Byte
/// ranges are compared, not source positions, so this is exact regardless of line/column rendering.
fn span_contains(outer: Span, inner: Span) -> bool {
    inner.start >= outer.start && inner.end <= outer.end
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
            source: None,
            pos: 0,
            errors: Vec::new(),
            warnings: Vec::new(),
            claimed_embedded_fragment_spans: Vec::new(),
            pending_lex_errors: Vec::new(),
            active_soft_keywords: std::collections::HashSet::new(),
            active_imported_keyword_specs: std::collections::HashMap::new(),
            vocab_block_stack: Vec::new(),
            vocab_body_kind_stack: Vec::new(),
            vocab_expression_item_modifier_stack: Vec::new(),
            module_path,
            library_imported_vocab: library_imported_vocab.cloned().unwrap_or_default(),
            library_imported_dsl_surfaces: library_imported_dsl_surfaces.cloned().unwrap_or_default(),
            std_async_vocab_active: false,
            active_scoped_surface_descriptors: Vec::new(),
            active_scoped_symbol_descriptors: Vec::new(),
            active_embedded_fragment_descriptors: Vec::new(),
            scoped_call_argument_stack: Vec::new(),
            pending_dedent_blank_lines: 0,
        }
    }

    /// Create a new parser with full contextual information plus the original source text.
    ///
    /// This is purely additive relative to [`Parser::new_with_context`]: every other constructor keeps `source`
    /// unset and is completely unaffected. `source` is required for RFC 081 (`#1023`) descriptor-gated embedded
    /// fragments — when a descriptor claims a lexical submode for a vocab-block position, the parser slices the
    /// fragment's raw byte range directly out of `source` (using the enclosing `Indent`/`Dedent` token spans) and
    /// re-tokenizes it with a dedicated submode tokenizer, rather than trying to reinterpret whatever the ordinary
    /// lexer already produced for that range. Without `source`, embedded-fragment descriptors are inert: their
    /// vocab-block body still parses exactly as an ordinary RFC 040/045 statement-list body.
    pub fn new_with_source(
        tokens: &'a [Token],
        module_path: Option<String>,
        library_imported_vocab: Option<&ImportedLibraryVocab>,
        library_imported_dsl_surfaces: Option<&ImportedLibraryDslSurfaces>,
        source: &'a str,
    ) -> Self {
        let mut parser =
            Self::new_with_context(tokens, module_path, library_imported_vocab, library_imported_dsl_surfaces);
        parser.source = Some(source);
        parser
    }

    /// Attach lex errors from a tolerant pre-lex pass (RFC 081, `#1023`) to be reconciled at the end of `parse()`.
    ///
    /// Use this only when `tokens` came from [`crate::lexer::lex_tolerant`] rather than the ordinary
    /// [`crate::lexer::lex`] -- for example after the strict lexer failed outright on source containing embedded
    /// fragment content the ordinary Incan lexer was never meant to tokenize (`;`, `` ` ``, `$`, and similar).
    /// `parse()` drops every error here whose span falls inside a fragment this parse successfully claims, since
    /// that fragment's own submode tokenizer re-scans the byte range independently; every remaining error still
    /// surfaces as a real diagnostic.
    #[must_use]
    pub fn with_pending_lex_errors(mut self, lex_errors: Vec<CompileError>) -> Self {
        self.pending_lex_errors = lex_errors;
        self
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

            // ---- Context: compilation-unit feature projection (RFC 114) ----
            if self.starts_feature_condition() {
                match self.feature_conditioned_declarations(&[]) {
                    Ok(conditioned) => {
                        for decl in conditioned {
                            if matches!(decl.node, Declaration::TestModule(_)) {
                                if seen_test_module {
                                    self.errors.push(CompileError::syntax(
                                        "Only one `module tests:` block is allowed per file".to_string(),
                                        decl.span,
                                    ));
                                }
                                seen_test_module = true;
                            }
                            if !matches!(decl.node, Declaration::Docstring(_)) {
                                seen_non_doc_decl = true;
                            }
                            declarations.push(decl);
                        }
                    }
                    Err(error) => {
                        self.errors.push(error);
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

        // ---- Reconcile tolerant-lex errors against claimed embedded fragments (RFC 081, #1023) ----
        // Any `pending_lex_errors` entry whose span falls inside a fragment this parse actually claimed is
        // expected noise: that byte range was re-tokenized independently by the fragment's own submode grammar,
        // so the ordinary lexer's confusion about it never reflects a real user mistake. Everything else is real
        // and must still reach the user, exactly as if the strict lexer had produced it directly.
        for lex_error in self.pending_lex_errors.drain(..) {
            let is_expected_fragment_noise = self
                .claimed_embedded_fragment_spans
                .iter()
                .any(|fragment_span| span_contains(*fragment_span, lex_error.span));
            if !is_expected_fragment_noise {
                self.errors.push(lex_error);
            }
        }

        if self.errors.is_empty() {
            Ok(Program {
                declarations,
                source_path: self.module_path.clone(),
                rust_module_path,
                warnings: self.warnings,
            })
        } else {
            // Fold non-fatal warnings into the error list so callers don't silently lose them when parsing fails.
            // Warnings retain their `ErrorKind::Warning` kind so callers can still distinguish them from errors when
            // needed.
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
    /// This gates [`Visibility::Public`] on `from ... import ...` (RFC 031). Callers must pass a filesystem-style
    /// module path, as the CLI and LSP do, so the parser can enforce that `pub from` appears only in source
    /// modules.
    ///
    /// On Windows, path-segment checks are ASCII case-insensitive so editor URIs that normalize path casing still
    /// match.
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
                    if import_path_activates_std_async(&path.segments) {
                        self.std_async_vocab_active = true;
                    }
                    for kw in incan_core::lang::stdlib::soft_keywords_for_import(&path.segments) {
                        self.active_soft_keywords.insert(kw);
                    }
                    self.activate_imported_keywords_for_import_path(&path.segments);
                }
                ImportKind::From { module, .. } => {
                    if import_path_activates_std_async(&module.segments) {
                        self.std_async_vocab_active = true;
                    }
                    for kw in incan_core::lang::stdlib::soft_keywords_for_import(&module.segments) {
                        self.active_soft_keywords.insert(kw);
                    }
                    self.activate_imported_keywords_for_import_path(&module.segments);
                }
                ImportKind::PubLibrary { library, path } => {
                    let segments: Vec<String> = std::iter::once(library.clone()).chain(path.iter().cloned()).collect();
                    self.activate_imported_keywords_for_import_path(&segments);
                }
                ImportKind::PubFrom { library, path, .. } => {
                    let segments: Vec<String> = std::iter::once(library.clone()).chain(path.iter().cloned()).collect();
                    self.activate_imported_keywords_for_import_path(&segments);
                }
                _ => {}
            }
        }
    }

    /// Activate keyword registrations contributed by an imported namespace.
    ///
    /// This bridges serialized vocab metadata into parser state by:
    /// - recording compatible soft-keyword ids in `active_soft_keywords` (for existing parser flows), and
    /// - recording imported keyword surface specs in `active_imported_keyword_specs` (for surface-kind checks driven by
    ///   imported metadata).
    fn activate_imported_keywords_for_import_path(&mut self, import_path: &[String]) {
        let mut provider_keys = self
            .library_imported_vocab
            .keys()
            .chain(self.library_imported_dsl_surfaces.keys())
            .filter(|key| import_path_activates_namespace(import_path, key))
            .cloned()
            .collect::<Vec<_>>();
        provider_keys.sort();
        provider_keys.dedup();

        for provider_key in provider_keys {
            self.activate_imported_keywords_for_provider(&provider_key, import_path);
        }
    }

    /// Activate one provider's registered vocabulary after its import namespace matched.
    fn activate_imported_keywords_for_provider(&mut self, provider_key: &str, import_path: &[String]) {
        if let Some(surfaces) = self.library_imported_dsl_surfaces.get(provider_key) {
            for surface in surfaces {
                if !dsl_surface_applies_to_import_path(surface, import_path) {
                    continue;
                }
                self.active_scoped_surface_descriptors
                    .extend(
                        surface
                            .scoped_surfaces
                            .iter()
                            .cloned()
                            .map(|descriptor| ActiveScopedSurfaceDescriptor {
                                dependency_key: provider_key.to_string(),
                                descriptor,
                            }),
                    );
                self.active_scoped_symbol_descriptors
                    .extend(
                        surface
                            .scoped_symbols
                            .iter()
                            .cloned()
                            .map(|descriptor| ActiveScopedSymbolDescriptor {
                                dependency_key: provider_key.to_string(),
                                descriptor,
                            }),
                    );
                self.active_embedded_fragment_descriptors.extend(
                    surface
                        .embedded_fragments
                        .iter()
                        .cloned()
                        .map(|descriptor| ActiveEmbeddedFragmentDescriptor {
                            dependency_key: provider_key.to_string(),
                            descriptor,
                        }),
                );
            }
        }

        let Some(registrations) = self.library_imported_vocab.get(provider_key) else {
            return;
        };

        for registration in registrations {
            if !registration_applies_to_import_path(registration, import_path) {
                continue;
            }

            for keyword in &registration.keywords {
                let declaration_surface =
                    self.active_declaration_surface_for_keyword(provider_key, keyword, import_path);
                let desugar_target = declaration_surface
                    .map(|declaration| declaration.desugars_to)
                    .unwrap_or(incan_vocab::DesugarTarget::Statements);
                let declaration_head_kind = declaration_surface
                    .map(|declaration| declaration.head_kind)
                    .unwrap_or_default();
                let clause_surface = self.active_clause_surface_for_keyword(provider_key, keyword, import_path);
                let is_declaration_owned_clause = clause_surface.is_some();
                let (clause_body_kind, expression_item_modifiers) = clause_surface
                    .map(|clause| (Some(clause.body_kind), clause.expression_item_modifiers.clone()))
                    .unwrap_or((None, Vec::new()));
                let specs = self
                    .active_imported_keyword_specs
                    .entry(keyword.name.clone())
                    .or_default();
                specs.push(ActiveImportedKeywordSpec {
                    keyword_name: keyword.name.clone(),
                    compound_tokens: keyword.compound_tokens.clone(),
                    dependency_key: provider_key.to_string(),
                    activation_namespace: match &registration.activation {
                        incan_vocab::KeywordActivation::OnImport { namespace } => namespace.clone(),
                        _ => provider_key.to_string(),
                    },
                    valid_decorators: registration.valid_decorators.clone(),
                    surface_kind: keyword.surface_kind,
                    placement: keyword.placement.clone(),
                    declaration_head_kind,
                    desugar_target,
                    is_declaration_owned_clause,
                    clause_body_kind,
                    expression_item_modifiers,
                });
                if let Some(id) = incan_core::lang::keywords::from_str(&keyword.name)
                    && incan_core::lang::keywords::is_soft(id)
                {
                    self.active_soft_keywords.insert(id);
                }
            }
        }
    }

    /// Return the declaration surface declared by a rich DSL surface for one imported keyword.
    ///
    /// Keyword registrations are still the parser activation index, but declaration-only contract such as the desugar
    /// target lives on the richer `DslSurface`. Joining them here keeps expression-position vocab parsing driven by
    /// metadata instead of keyword spellings.
    fn active_declaration_surface_for_keyword(
        &self,
        provider_key: &str,
        keyword: &incan_vocab::KeywordSpec,
        import_path: &[String],
    ) -> Option<&incan_vocab::DeclarationSurface> {
        let surfaces = self.library_imported_dsl_surfaces.get(provider_key)?;

        /// Recursively locate a declaration surface matching the activated keyword registration.
        fn find<'a>(
            declarations: &'a [incan_vocab::DeclarationSurface],
            keyword: &incan_vocab::KeywordSpec,
        ) -> Option<&'a incan_vocab::DeclarationSurface> {
            for declaration in declarations {
                if declaration.keyword == keyword.name
                    && declaration.compound_tokens == keyword.compound_tokens
                    && declaration.placement == keyword.placement
                {
                    return Some(declaration);
                }
                if let Some(declaration) = find(&declaration.declarations, keyword) {
                    return Some(declaration);
                }
            }
            None
        }

        for surface in surfaces {
            if !dsl_surface_applies_to_import_path(surface, import_path) {
                continue;
            }
            if let Some(declaration) = find(&surface.declarations, keyword) {
                return Some(declaration);
            }
        }
        None
    }

    /// Return the clause surface declared by a rich DSL surface for one imported keyword.
    ///
    /// Low-level keyword registrations do not carry clause-body structure. When the same library also provides the
    /// author-facing `DslSurface`, parser-only forms such as expression-list item modifiers can be gated by the richer
    /// public contract instead of guessed later by the AST bridge.
    fn active_clause_surface_for_keyword(
        &self,
        provider_key: &str,
        keyword: &incan_vocab::KeywordSpec,
        import_path: &[String],
    ) -> Option<&incan_vocab::ClauseSurface> {
        let surfaces = self.library_imported_dsl_surfaces.get(provider_key)?;

        /// Recursively locate the clause surface nested under the activated declaration.
        fn find<'a>(
            declarations: &'a [incan_vocab::DeclarationSurface],
            keyword: &incan_vocab::KeywordSpec,
        ) -> Option<&'a incan_vocab::ClauseSurface> {
            let incan_vocab::KeywordPlacement::InBlock(parents) = &keyword.placement else {
                return None;
            };
            for declaration in declarations {
                if parents.iter().any(|parent| parent == &declaration.keyword)
                    && let Some(clause) = declaration.clauses.iter().find(|clause| {
                        clause.keyword == keyword.name && clause.compound_tokens == keyword.compound_tokens
                    })
                {
                    return Some(clause);
                }
                if let Some(clause) = find(&declaration.declarations, keyword) {
                    return Some(clause);
                }
            }
            None
        }

        for surface in surfaces {
            if !dsl_surface_applies_to_import_path(surface, import_path) {
                continue;
            }
            if let Some(clause) = find(&surface.declarations, keyword) {
                return Some(clause);
            }
        }
        None
    }
}

/// Return `true` when a DSL surface should activate for an imported namespace.
fn dsl_surface_applies_to_import_path(surface: &incan_vocab::DslSurface, import_path: &[String]) -> bool {
    match &surface.activation {
        incan_vocab::KeywordActivation::Always => true,
        incan_vocab::KeywordActivation::OnImport { namespace } => import_path_activates_namespace(import_path, namespace),
        _ => false,
    }
}

/// Return `true` when a registration should be activated for an imported namespace.
fn registration_applies_to_import_path(
    registration: &incan_vocab::KeywordRegistration,
    import_path: &[String],
) -> bool {
    match &registration.activation {
        incan_vocab::KeywordActivation::Always => true,
        incan_vocab::KeywordActivation::OnImport { namespace } => import_path_activates_namespace(import_path, namespace),
        _ => false,
    }
}

/// Return whether an `OnImport` namespace is activated by an import path.
fn import_path_activates_namespace(import_path: &[String], namespace: &str) -> bool {
    let imported = import_path.join(".");
    let trimmed = namespace.trim();
    !trimmed.is_empty() && (trimmed == imported || trimmed.starts_with(&format!("{imported}.")))
}

/// Return whether an import path activates `std.async` vocabulary in this file.
fn import_path_activates_std_async(path: &[String]) -> bool {
    matches!(path, [root, namespace, ..] if root == "std" && namespace == "async")
}

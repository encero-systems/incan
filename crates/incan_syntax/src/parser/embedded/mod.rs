// RFC 081 (#1023) — descriptor-gated lexical submodes and typed embedded fragments.
//
// This chunk implements the re-entrant submode tokenizer/parser the main parser invokes once a descriptor claims
// an eligible position (see `Parser::try_embedded_fragment_body` in `stmts.rs`, and
// `Parser::active_embedded_fragment_descriptor_for_declaration_body` in `expr.rs`). It operates on a raw source
// slice, not on the ordinary token stream — see the rustdoc on `Parser::source` in `core.rs` for why.
//
// Split into one file per submode grammar, plus this shared cursor/hole-re-entry chunk, to keep functions small
// per the readable-Rust convention.

include!("cursor.rs");
include!("markup.rs");
include!("style.rs");
include!("raw_text.rs");
include!("regex_template.rs");
include!("selector_declaration_value.rs");
include!("type_position.rs");
include!("tests.rs");

/// Parse one descriptor-claimed embedded fragment's raw source slice into its structural node tree.
///
/// `raw` is the exact original source text between the fragment's enclosing `Indent`/`Dedent` tokens; `base_offset`
/// is that slice's starting absolute byte offset in the original file, so every span produced below anchors back
/// into real source coordinates. This is the single dispatch point across the fixed six-submode catalog RFC 081
/// (`#1023`) implements — a descriptor can only ever select one of these kinds, never author its own grammar.
///
/// ## Errors
/// Returns a [`CompileError`] wherever the fragment's content does not match its claimed submode's grammar.
/// Unrecognized syntax inside a claimed submode is always a parse error, never a silent reinterpretation.
fn parse_embedded_fragment_body(
    submode: incan_vocab::EmbeddedFragmentSubmode,
    raw: &str,
    base_offset: usize,
) -> Result<Vec<Spanned<EmbeddedNode>>, CompileError> {
    use incan_vocab::EmbeddedFragmentSubmode as Submode;
    match submode {
        Submode::Markup => parse_markup_fragment(raw, base_offset),
        Submode::Style => parse_style_fragment(raw, base_offset),
        Submode::RawText => parse_raw_text_fragment(raw, base_offset),
        Submode::RegexTemplate => parse_regex_template_fragment(raw, base_offset),
        Submode::SelectorDeclarationValue => parse_selector_declaration_value_fragment(raw, base_offset),
        Submode::TypePosition => parse_type_position_fragment(raw, base_offset),
        // `EmbeddedFragmentSubmode` is `#[non_exhaustive]` so `incan_vocab` can grow the catalog later without an
        // immediate breaking change here. Until this parser is updated with a matching grammar, refuse rather than
        // silently guessing — this is the same "hard-error on unrecognized submode" discipline #1023 applies at
        // lowering (see `lower_embedded_fragment_expr`).
        _ => Err(CompileError::syntax(
            "This embedded-fragment submode kind is not implemented by this parser version".to_string(),
            Span::new(base_offset, base_offset + raw.len()),
        )),
    }
}

impl<'a> Parser<'a> {
    /// Attempt to parse a vocab-block body as a descriptor-claimed embedded fragment (RFC 081, `#1023`).
    ///
    /// Called from `try_vocab_block` right after the block header's suite indent has been consumed (so
    /// `self.pos` sits on the body's first token) and before the ordinary `self.block()` statement-list parse
    /// would run. Returns `Ok(None)` whenever no embedded-fragment descriptor claims this position, or when the
    /// original source text is unavailable (`self.source` is `None`) — in both cases the caller must fall back to
    /// ordinary RFC 040/045 statement-list parsing, leaving parser position and behavior completely unchanged.
    ///
    /// On a successful claim, this locates the suite's matching `Dedent` purely from existing token spans (no
    /// lexer change needed — see `Parser::source`'s rustdoc), slices the raw source between the body's start and
    /// that `Dedent`, and re-tokenizes it with the claimed submode's dedicated grammar. `self.pos` is left
    /// positioned exactly on the matching `Dedent` token, so the caller's existing
    /// `self.expect(&TokenKind::Dedent, ...)` continues to work unchanged.
    pub(super) fn try_embedded_fragment_body(
        &mut self,
        keyword_name: &str,
    ) -> Result<Option<Vec<Spanned<Statement>>>, CompileError> {
        let body_start_span = self.current_span();
        let Some(descriptor) =
            self.active_embedded_fragment_descriptor_for_declaration_body(keyword_name, body_start_span)?
        else {
            return Ok(None);
        };
        let Some(source) = self.source else {
            return Ok(None);
        };
        let submode = descriptor.submode;
        let dependency_key = self
            .active_embedded_fragment_descriptors
            .iter()
            .find(|active| active.descriptor.key == descriptor.key)
            .map(|active| active.dependency_key.clone())
            .unwrap_or_default();
        let descriptor_key = descriptor.key.clone();
        let layout_sensitive = descriptor.format_hint.layout_sensitive;

        let dedent_idx = self.find_matching_dedent_index()?;
        // Use the just-consumed `Indent` token's span end, not the next token's span start: when the fragment's
        // first byte is not a valid ordinary-Incan token start (for example a template string's leading
        // backtick), the next *token* begins strictly after it, which would silently truncate the raw slice.
        // `Indent`'s span always ends exactly at the first non-whitespace byte of the line regardless of whether
        // that byte tokenizes (see `Lexer::handle_indentation` in `lexer/indent.rs`), so it is the correct anchor.
        let indent_idx = self.pos.checked_sub(1).ok_or_else(|| {
            CompileError::syntax(
                "Internal error: no preceding `Indent` token for an embedded-fragment body".to_string(),
                body_start_span,
            )
        })?;
        let body_start = self.tokens[indent_idx].span.end;
        let body_end = self.tokens[dedent_idx].span.start;
        let fragment_span = Span::new(body_start, body_end);
        let raw = source.get(body_start..body_end).ok_or_else(|| {
            CompileError::syntax(
                "Internal error: embedded-fragment source span is not aligned to a character boundary".to_string(),
                fragment_span,
            )
        })?;
        if let Some(swallowed_offset) = find_swallowed_top_level_declaration(raw, body_start) {
            return Err(CompileError::syntax(
                "This embedded fragment's boundary could not be determined reliably: its content appears to \
                 contain an unmatched bracket-like character (`(`, `[`, or `{`) inside a comment or string that \
                 the ordinary Incan tokenizer misread as real punctuation, which made the fragment boundary \
                 extend past its real end and swallow unrelated subsequent source. Rewrite the fragment to avoid \
                 an unmatched bracket character, even inside a comment or string literal."
                    .to_string(),
                Span::new(swallowed_offset, swallowed_offset),
            ));
        }

        let nodes = parse_embedded_fragment_body(submode, raw, body_start)?;
        self.pos = dedent_idx;
        self.claimed_embedded_fragment_spans.push(fragment_span);

        let fragment = EmbeddedFragmentExpr {
            key: SurfaceFeatureKey::ScopedDslSurface {
                dependency_key,
                descriptor_key,
            },
            submode,
            nodes,
            source_text: raw.to_string(),
            layout_sensitive,
        };
        let embedded_expr = Spanned::new(Expr::Embedded(Box::new(fragment)), fragment_span);
        Ok(Some(vec![Spanned::new(
            Statement::Expr(embedded_expr),
            fragment_span,
        )]))
    }

    /// Return the token index of the `Dedent` that closes the suite starting at the current parser position.
    ///
    /// This scans forward counting nested `Indent`/`Dedent` tokens without interpreting any other token, so it
    /// works even when the intervening bytes are not meaningful ordinary-Incan tokens. It never advances
    /// `self.pos`.
    ///
    /// ## Errors
    /// Returns a [`CompileError`] if the token stream ends (`Eof`) before the matching `Dedent` is found, which
    /// would indicate a lexer/parser desynchronization bug rather than a user-facing syntax error.
    fn find_matching_dedent_index(&self) -> Result<usize, CompileError> {
        let mut depth: i32 = 0;
        let mut idx = self.pos;
        loop {
            match self.tokens.get(idx).map(|token| &token.kind) {
                Some(TokenKind::Indent) => {
                    depth += 1;
                    idx += 1;
                }
                Some(TokenKind::Dedent) => {
                    if depth == 0 {
                        return Ok(idx);
                    }
                    depth -= 1;
                    idx += 1;
                }
                Some(TokenKind::Eof) | None => {
                    return Err(CompileError::syntax(
                        "Internal error: reached end of file while scanning for the end of an embedded fragment's \
                         suite"
                            .to_string(),
                        self.current_span(),
                    ));
                }
                Some(_) => idx += 1,
            }
        }
    }
}

/// Column-0 line prefixes this scan treats as unambiguous proof a fragment's computed body range swallowed a
/// real subsequent top-level declaration (see [`find_swallowed_top_level_declaration`]'s rustdoc for why).
const TOP_LEVEL_DECLARATION_STARTS: [&str; 9] = [
    "def ", "class ", "model ", "trait ", "import ", "from ", "enum ", "type ", "pub ",
];

/// Detect whether a fragment's computed body range swallowed a real subsequent top-level declaration (RFC 081,
/// `#1023`).
///
/// `find_matching_dedent_index` trusts the ordinary lexer's `Indent`/`Dedent` token stream, which can desync when
/// a fragment's own content confuses the ordinary lexer's bracket-depth tracking -- for example an unmatched
/// `(`/`[`/`{`-like character inside a `Style` submode's `/* ... */` comment, which the ordinary lexer does not
/// understand as a comment at all and instead scans as a real, permanently-unbalanced bracket token. When that
/// happens, `Indent`/`Dedent`/`Newline` emission can stop entirely for the rest of the file, and the fragment's
/// computed end can silently extend past its real boundary, swallowing unrelated subsequent declarations into the
/// fragment's raw text rather than letting the ordinary parser see them.
///
/// An embedded fragment is always the indented body of a `keyword:` block, so legitimate fragment content is
/// never itself at column 0 -- a column-0 line starting a real declaration keyword inside the computed range is
/// therefore decisive proof of a swallowed declaration, not a false positive from ordinary multi-line content
/// inside the fragment (for example a balanced, multi-line bracket pair in a template literal, which does not
/// trigger this check).
///
/// Returns the byte offset of the first such line, for use as the resulting diagnostic's span.
///
/// Deliberately starts scanning *after* `raw`'s own first line, never at `offset == 0`: `raw` is sliced starting
/// right after the block's `Indent` token, so its first line alone -- unlike every subsequent line -- never
/// carries the block's leading indentation in this string, even when the fragment is correctly bounded. Checking
/// it here would false-positive on any legitimate fragment whose very first line happens to start with one of
/// `TOP_LEVEL_DECLARATION_STARTS`'s words (for example `RawText` content that begins with the literal text
/// `"def "`).
fn find_swallowed_top_level_declaration(raw: &str, body_start: usize) -> Option<usize> {
    let mut offset = raw.find('\n')? + 1;
    while offset < raw.len() {
        let rest = &raw[offset..];
        let line_len = rest.find('\n').map_or(rest.len(), |i| i + 1);
        if TOP_LEVEL_DECLARATION_STARTS
            .iter()
            .any(|keyword| rest[..line_len].starts_with(keyword))
        {
            return Some(body_start + offset);
        }
        offset += line_len;
    }
    None
}

/// Parse one expression hole's contents as ordinary Incan, re-entering the main expression grammar.
///
/// This mirrors the existing f-string interpolation precedent (`Parser::parse_fstring_expr` in `expr.rs`): lex and
/// parse `text` with a fresh, independent [`Parser`], then rebase the resulting spans into outer-source
/// coordinates. Unlike the f-string helper, real lex/parse errors are propagated rather than silently falling back
/// to `Expr::Ident` — an embedded expression hole must genuinely fail to compile when its contents are broken,
/// since RFC 081 requires holes to flow through real typecheck/lowering like any other Incan expression.
///
/// ## Errors
/// Returns a [`CompileError`] if `text` does not lex or parse as a complete Incan expression.
fn parse_embedded_hole_expr(text: &str, base_offset: usize) -> Result<Spanned<Expr>, CompileError> {
    let hole_span = Span::new(base_offset, base_offset + text.len());
    if text.trim().is_empty() {
        return Err(CompileError::syntax(
            "Expression hole must not be empty".to_string(),
            hole_span,
        ));
    }

    let mut tokens = crate::lexer::lex(text).map_err(|mut lex_errors| {
        lex_errors
            .drain(..)
            .next()
            .unwrap_or_else(|| CompileError::syntax("Invalid expression inside embedded hole".to_string(), hole_span))
    })?;
    if !matches!(tokens.last().map(|token| &token.kind), Some(TokenKind::Eof)) {
        tokens.push(Token::new(TokenKind::Eof, Span::default()));
    }

    let mut parser = Parser::new(&tokens);
    let mut expr = parser.expression()?;
    if !matches!(parser.peek().kind, TokenKind::Eof) {
        return Err(CompileError::syntax(
            "Unexpected trailing tokens inside embedded expression hole".to_string(),
            hole_span,
        ));
    }
    parser.shift_spanned_expr(&mut expr, base_offset);
    Ok(expr)
}

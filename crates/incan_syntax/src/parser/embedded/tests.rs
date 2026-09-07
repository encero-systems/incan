// Parser unit tests for RFC 081 (#1023) descriptor-gated embedded fragments.
//
// Follows the existing scoped-surface fixture model in `parser/tests.rs` (hand-built `KeywordRegistration`/
// `DslSurface` maps, no manifest file, no WASM): one accept fixture per submode's grammar, source-span assertions,
// rejection-boundary fixtures (unrecognized syntax inside a submode, and the same spelling outside an eligible
// position), and a same-depth ambiguity fixture, per `research-notes.md` §5's fixture precedent.

#[cfg(test)]
mod embedded_fragment_tests {
    use super::*;
    use crate::lexer;

    type KeywordMap = std::collections::HashMap<String, Vec<incan_vocab::KeywordRegistration>>;
    type SurfaceMap = std::collections::HashMap<String, Vec<incan_vocab::DslSurface>>;

    /// Build a minimal keyword + DSL-surface registration map that activates one embedded-fragment descriptor
    /// claiming `keyword`'s declaration body under the given `submode`.
    fn embedded_fixture_maps(
        keyword: &str,
        provider: &str,
        submode: incan_vocab::EmbeddedFragmentSubmode,
    ) -> (KeywordMap, SurfaceMap) {
        let namespace = format!("{provider}.{keyword}");
        let mut keyword_map = KeywordMap::new();
        keyword_map.insert(
            provider.to_string(),
            vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: namespace.clone(),
                },
                keywords: vec![incan_vocab::KeywordSpec::block(keyword)],
                valid_decorators: Vec::new(),
            }],
        );
        let mut surface_map = SurfaceMap::new();
        surface_map.insert(
            provider.to_string(),
            vec![
                incan_vocab::DslSurface::on_import(&namespace)
                    .with_declaration(incan_vocab::DeclarationSurface::named(keyword))
                    .with_embedded_fragment(
                        incan_vocab::EmbeddedFragmentDescriptor::new(
                            &format!("{keyword}.fragment"),
                            submode,
                            "fragment",
                        )
                        .in_declaration_body(keyword),
                    ),
            ],
        );
        (keyword_map, surface_map)
    }

    /// Lex and parse `source` with `source` also threaded through for embedded-fragment support (see
    /// `Parser::new_with_source`), using the given fixture maps.
    ///
    /// Uses [`lexer::lex_tolerant`] rather than [`lexer::lex`]: embedded-fragment raw content commonly contains
    /// characters (`;`, `` ` ``, `$`, `!`) that are not valid ordinary Incan tokens, and the whole-file upfront lex
    /// pass would otherwise discard the token stream entirely before the parser ever gets a chance to route that
    /// content through the submode tokenizer instead of ordinary tokenization. See `Lexer::tokenize_tolerant`'s
    /// rustdoc for why this is safe: `Indent`/`Dedent` boundaries stay correct regardless.
    fn parse_embedded_fixture(
        source: &str,
        keyword_map: &KeywordMap,
        surface_map: &SurfaceMap,
    ) -> Result<Program, Vec<CompileError>> {
        let (tokens, _lex_errors) = lexer::lex_tolerant(source);
        parse_with_source(&tokens, None, Some(keyword_map), Some(surface_map), source)
    }

    /// Dig the `Expr::Embedded` fragment out of `def configure() -> None:\n  <keyword>:\n    ...\n`-shaped source.
    fn embedded_fragment_from_program(program: &Program) -> Result<&EmbeddedFragmentExpr, String> {
        let Declaration::Function(function) = &program.declarations[1].node else {
            return Err("expected a function declaration at index 1".to_string());
        };
        let Statement::VocabBlock(block) = &function.body[0].node else {
            return Err(format!("expected a vocab block statement, got {:?}", function.body[0].node));
        };
        let Statement::Expr(expr) = &block.body[0].node else {
            return Err(format!(
                "expected an expression statement inside the vocab block, got {:?}",
                block.body[0].node
            ));
        };
        let Expr::Embedded(fragment) = &expr.node else {
            return Err(format!("expected an embedded fragment expression, got {:?}", expr.node));
        };
        Ok(fragment)
    }

    // ---- Markup submode ----

    #[test]
    fn markup_fragment_accepts_element_attrs_text_entity_comment_and_hole() -> Result<(), Box<dyn std::error::Error>>
    {
        let (keyword_map, surface_map) =
            embedded_fixture_maps("html", "webkit", incan_vocab::EmbeddedFragmentSubmode::Markup);
        let source = "import pub::webkit\n\ndef render(title: str) -> None:\n  html:\n    <section class=\"card\">\n      <h1>{title}</h1>\n      Copyright &amp; co.\n      <!-- trailer -->\n      <img src={title} alt=\"Preview\" />\n    </section>\n";
        let program = parse_embedded_fixture(source, &keyword_map, &surface_map)
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let fragment = embedded_fragment_from_program(&program)?;
        assert_eq!(fragment.submode, incan_vocab::EmbeddedFragmentSubmode::Markup);
        assert!(matches!(
            &fragment.key,
            incan_semantics_core::SurfaceFeatureKey::ScopedDslSurface { dependency_key, descriptor_key }
                if dependency_key == "webkit" && descriptor_key == "html.fragment"
        ));
        // The fragment's raw byte range includes any trailing newline/indentation before the closing `Dedent`
        // (preserved verbatim, per the formatter's future layout-preserving fallback), so a trailing whitespace-only
        // `Text` node after the top-level element is expected here rather than an exact node count.
        let EmbeddedNode::Element(section) = &fragment.nodes[0].node else {
            return Err(format!("expected a top-level <section> element, got {:?}", fragment.nodes[0].node).into());
        };
        assert_eq!(section.name, "section");
        assert_eq!(section.attrs.len(), 1);
        assert_eq!(section.attrs[0].name, "class");
        assert!(matches!(&section.attrs[0].value, Some(spanned) if matches!(&spanned.node, EmbeddedNode::Text(t) if t == "card")));

        let h1 = section
            .children
            .iter()
            .find_map(|child| match &child.node {
                EmbeddedNode::Element(el) if el.name == "h1" => Some(el),
                _ => None,
            })
            .ok_or("expected a nested <h1> element")?;
        let EmbeddedNode::Hole(hole_expr) = &h1.children.first().ok_or("expected <h1> to have a hole child")?.node
        else {
            return Err("expected <h1>'s child to be an expression hole".into());
        };
        assert!(matches!(&hole_expr.node, Expr::Ident(name) if name == "title"));

        let has_entity = section
            .children
            .iter()
            .any(|child| matches!(&child.node, EmbeddedNode::EntityRef(name) if name == "amp"));
        assert!(has_entity, "expected an `&amp;` entity reference among the section's children");

        let has_comment = section
            .children
            .iter()
            .any(|child| matches!(&child.node, EmbeddedNode::Comment(text) if text == " trailer "));
        assert!(has_comment, "expected a preserved `<!-- trailer -->` comment");

        let img = section
            .children
            .iter()
            .find_map(|child| match &child.node {
                EmbeddedNode::Element(el) if el.name == "img" => Some(el),
                _ => None,
            })
            .ok_or("expected a self-closing <img> element")?;
        assert!(img.self_closing);
        assert!(img.children.is_empty());
        assert_eq!(img.attrs.len(), 2);
        assert!(matches!(&img.attrs[0].value, Some(spanned) if matches!(&spanned.node, EmbeddedNode::Hole(_))));

        // Source text is preserved verbatim for the formatter's layout-preserving fallback (#1022).
        assert!(fragment.source_text.contains("<section class=\"card\">"));
        assert!(fragment.source_text.contains("<!-- trailer -->"));
        Ok(())
    }

    #[test]
    fn markup_fragment_hole_span_matches_exact_subregion() -> Result<(), Box<dyn std::error::Error>> {
        let (keyword_map, surface_map) =
            embedded_fixture_maps("html", "webkit", incan_vocab::EmbeddedFragmentSubmode::Markup);
        let source = "import pub::webkit\n\ndef render(title: str) -> None:\n  html:\n    <h1>{title}</h1>\n";
        let program = parse_embedded_fixture(source, &keyword_map, &surface_map)
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let fragment = embedded_fragment_from_program(&program)?;
        let EmbeddedNode::Element(h1) = &fragment.nodes[0].node else {
            return Err("expected a top-level <h1> element".into());
        };
        let hole = h1.children.first().ok_or("expected a hole child")?;
        let expected_start = source.find("{title}").ok_or("fixture source must contain `{title}`")?;
        let expected_end = expected_start + "{title}".len();
        assert_eq!(hole.span.start, expected_start, "hole span should start exactly at `{{`");
        assert_eq!(hole.span.end, expected_end, "hole span should end exactly after `}}`");
        Ok(())
    }

    #[test]
    fn markup_fragment_rejects_mismatched_closing_tag() -> Result<(), Box<dyn std::error::Error>> {
        let (keyword_map, surface_map) =
            embedded_fixture_maps("html", "webkit", incan_vocab::EmbeddedFragmentSubmode::Markup);
        let source = "import pub::webkit\n\ndef render() -> None:\n  html:\n    <section></div>\n";
        let errors = parse_embedded_fixture(source, &keyword_map, &surface_map)
            .err()
            .ok_or("expected a parse error for a mismatched closing tag")?;
        assert!(
            errors.iter().any(|error| error.message.contains("Mismatched closing tag")),
            "expected a mismatched-closing-tag diagnostic, got {errors:?}"
        );
        Ok(())
    }

    #[test]
    fn markup_shaped_content_is_rejected_as_ordinary_incan_when_the_vocab_is_never_imported()
    -> Result<(), Box<dyn std::error::Error>> {
        // No import at all: the same `html:`-block spelling with markup-shaped content must not silently become an
        // embedded fragment. It falls back to ordinary RFC 040/045 statement-list parsing, which fails as ordinary
        // Incan syntax because `<section>` is not a valid Incan expression.
        let source = "def render() -> None:\n  html:\n    <section></section>\n";
        let tokens = lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;
        let result = parse(&tokens);
        assert!(
            result.is_err(),
            "expected ordinary parsing to reject markup-shaped content when no descriptor claims it"
        );
        Ok(())
    }

    #[test]
    fn type_position_shaped_content_is_rejected_as_ordinary_incan_when_the_vocab_is_never_imported()
    -> Result<(), Box<dyn std::error::Error>> {
        // No import at all: `Foo<Bar>` is not silently reinterpreted as a type shape. Without an active
        // `TypePosition` descriptor, and with no `typeof` vocab keyword registered either, `Foo<Bar>` alone falls
        // back to ordinary Incan expression parsing -- a chained less-than/greater-than comparison missing its
        // final operand -- and correctly fails as ordinary Incan, not as a silently-accepted type shape.
        let source = "def render() -> None:\n  typeof:\n    Foo<Bar>\n";
        let tokens = lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;
        let result = parse(&tokens);
        assert!(
            result.is_err(),
            "expected ordinary parsing to reject type-shaped content when no descriptor claims it"
        );
        Ok(())
    }

    #[test]
    fn style_shaped_content_is_rejected_as_ordinary_incan_when_the_vocab_is_never_imported()
    -> Result<(), Box<dyn std::error::Error>> {
        // No import at all: the same `css:`-block spelling with style-shaped content must not silently become an
        // embedded fragment. `.card { color: red; }` is not valid ordinary Incan syntax -- `;` is not even an
        // ordinary Incan token -- so it is rejected either at lexing or parsing, never silently reinterpreted as a
        // style rule.
        let source = "def render() -> None:\n  css:\n    .card { color: red; }\n";
        let is_rejected = match lexer::lex(source) {
            Err(_) => true,
            Ok(tokens) => parse(&tokens).is_err(),
        };
        assert!(
            is_rejected,
            "expected style-shaped content to be rejected by ordinary lexing/parsing when no descriptor claims it"
        );
        Ok(())
    }

    #[test]
    fn embedded_fragment_descriptor_does_not_activate_for_a_position_it_was_never_registered_for()
    -> Result<(), Box<dyn std::error::Error>> {
        // Unlike the three `_is_rejected_as_ordinary_incan_when_the_vocab_is_never_imported` tests above, this
        // registers the `html` vocab keyword AND an embedded-fragment descriptor for the same provider -- the
        // import is active and the keyword is recognized -- but the descriptor's own `in_declaration_body(...)`
        // eligibility names a *different* declaration ("card", not "html"). This exercises the actual empty-match
        // branch of `Parser::active_embedded_fragment_descriptor_for_declaration_body` (`expr.rs`), proving a
        // descriptor that is active but not eligible *here* correctly falls back to ordinary RFC 040/045
        // statement-list parsing rather than silently claiming a position it was never registered for.
        let namespace = "webkit.html";
        let mut keyword_map = KeywordMap::new();
        keyword_map.insert(
            "webkit".to_string(),
            vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: namespace.to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec::block("html")],
                valid_decorators: Vec::new(),
            }],
        );
        let mut surface_map = SurfaceMap::new();
        surface_map.insert(
            "webkit".to_string(),
            vec![
                incan_vocab::DslSurface::on_import(namespace)
                    .with_declaration(incan_vocab::DeclarationSurface::named("html"))
                    .with_embedded_fragment(
                        incan_vocab::EmbeddedFragmentDescriptor::new(
                            "html.fragment",
                            incan_vocab::EmbeddedFragmentSubmode::Markup,
                            "fragment",
                        )
                        .in_declaration_body("card"),
                    ),
            ],
        );
        let source = "import pub::webkit\n\ndef render() -> None:\n  html:\n    <section></section>\n";
        let errors = parse_embedded_fixture(source, &keyword_map, &surface_map)
            .err()
            .ok_or("expected markup-shaped content to still be rejected as ordinary Incan when the only active descriptor is eligible for a different position")?;
        assert!(
            !errors.iter().any(|error| error.message.contains("Ambiguous")),
            "expected an ordinary-parsing rejection, not an ambiguity diagnostic: {errors:?}"
        );
        Ok(())
    }

    // ---- Style submode ----

    #[test]
    fn style_fragment_accepts_selector_list_declarations_custom_property_and_color() -> Result<(), Box<dyn std::error::Error>>
    {
        let (keyword_map, surface_map) =
            embedded_fixture_maps("css", "webkit", incan_vocab::EmbeddedFragmentSubmode::Style);
        let source = "import pub::webkit\n\ndef render() -> None:\n  css:\n    .card:hover, #title {\n      --accent-color: #1166ff;\n      color: var(--accent-color);\n      padding: 16px;\n    }\n";
        let program = parse_embedded_fixture(source, &keyword_map, &surface_map)
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let fragment = embedded_fragment_from_program(&program)?;
        assert_eq!(fragment.nodes.len(), 1);
        let EmbeddedNode::StyleRule(rule) = &fragment.nodes[0].node else {
            return Err("expected a top-level style rule".into());
        };
        assert_eq!(rule.selectors.len(), 2);
        assert!(matches!(&rule.selectors[0].node, EmbeddedNode::Value(EmbeddedValue::Selector(s)) if s == ".card:hover"));
        assert!(matches!(&rule.selectors[1].node, EmbeddedNode::Value(EmbeddedValue::Selector(s)) if s == "#title"));
        assert_eq!(rule.declarations.len(), 3);
        let EmbeddedNode::Declaration(custom_prop) = &rule.declarations[0].node else {
            return Err("expected the first declaration".into());
        };
        assert_eq!(custom_prop.property, "--accent-color");
        assert!(matches!(&custom_prop.value[0].node, EmbeddedNode::Value(EmbeddedValue::Color(c)) if c == "#1166ff"));
        let EmbeddedNode::Declaration(color_decl) = &rule.declarations[1].node else {
            return Err("expected the second declaration".into());
        };
        assert!(matches!(
            &color_decl.value[0].node,
            EmbeddedNode::Value(EmbeddedValue::CustomPropertyRef(name)) if name == "--accent-color"
        ));
        let EmbeddedNode::Declaration(padding_decl) = &rule.declarations[2].node else {
            return Err("expected the third declaration".into());
        };
        assert!(matches!(
            &padding_decl.value[0].node,
            EmbeddedNode::Value(EmbeddedValue::Dimension { number, unit }) if number == "16" && unit == "px"
        ));
        Ok(())
    }

    #[test]
    fn style_fragment_accepts_a_comment_between_declarations() -> Result<(), Box<dyn std::error::Error>> {
        let (keyword_map, surface_map) =
            embedded_fixture_maps("css", "webkit", incan_vocab::EmbeddedFragmentSubmode::Style);
        let source = "import pub::webkit\n\ndef render() -> None:\n  css:\n    .card {\n      /* keep in sync with the theme tokens */\n      color: red;\n    }\n";
        let program = parse_embedded_fixture(source, &keyword_map, &surface_map)
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let fragment = embedded_fragment_from_program(&program)?;
        let EmbeddedNode::StyleRule(rule) = &fragment.nodes[0].node else {
            return Err("expected a top-level style rule".into());
        };
        // Comments are structural siblings within the declaration block, in source order, not filtered out.
        assert_eq!(rule.declarations.len(), 2);
        assert!(matches!(
            &rule.declarations[0].node,
            EmbeddedNode::Comment(text) if text.contains("keep in sync")
        ));
        let EmbeddedNode::Declaration(color_decl) = &rule.declarations[1].node else {
            return Err("expected the color declaration".into());
        };
        assert_eq!(color_decl.property, "color");
        Ok(())
    }

    #[test]
    fn style_fragment_rejects_an_unterminated_comment() -> Result<(), Box<dyn std::error::Error>> {
        let (keyword_map, surface_map) =
            embedded_fixture_maps("css", "webkit", incan_vocab::EmbeddedFragmentSubmode::Style);
        let source = "import pub::webkit\n\ndef render() -> None:\n  css:\n    .card {\n      /* never closed\n      color: red;\n    }\n";
        let errors = parse_embedded_fixture(source, &keyword_map, &surface_map)
            .err()
            .ok_or("expected an unterminated-comment parse error")?;
        assert!(
            errors.iter().any(|error| error.message.contains("Unterminated comment")),
            "expected an unterminated-comment diagnostic, got {errors:?}"
        );
        Ok(())
    }

    #[test]
    fn style_fragment_rejects_unterminated_declaration_block() -> Result<(), Box<dyn std::error::Error>> {
        let (keyword_map, surface_map) =
            embedded_fixture_maps("css", "webkit", incan_vocab::EmbeddedFragmentSubmode::Style);
        let source = "import pub::webkit\n\ndef render() -> None:\n  css:\n    .card {\n      color: red;\n";
        let errors = parse_embedded_fixture(source, &keyword_map, &surface_map)
            .err()
            .ok_or("expected a parse error for an unterminated declaration block")?;
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("Expected `}`") || error.message.contains("declaration")),
            "expected an unterminated-block diagnostic, got {errors:?}"
        );
        Ok(())
    }

    #[test]
    fn style_fragment_boundary_desync_from_an_unmatched_bracket_in_a_comment_is_refused_not_silently_swallowed()
    -> Result<(), Box<dyn std::error::Error>> {
        // Regression test: an unmatched `(` inside a `/* ... */` comment -- valid, documented `Style`-submode
        // syntax -- is not a comment to the *ordinary* Incan lexer at all (Incan's own comments are `#`-prefixed).
        // The ordinary lexer reads it as a real, permanently-unbalanced bracket token, which silences its own
        // `Indent`/`Dedent`/`Newline` emission for the rest of the file. Before this test's corresponding fix,
        // `find_matching_dedent_index` (which trusts that ordinary token stream) ran all the way to EOF, and the
        // fragment silently swallowed `def after_fragment()` into its own raw text -- dropping a real declaration
        // from the AST with no error at all. It must now refuse loudly instead.
        let (keyword_map, surface_map) =
            embedded_fixture_maps("css", "webkit", incan_vocab::EmbeddedFragmentSubmode::Style);
        let source = "import pub::webkit\n\ndef card_theme() -> None:\n  css:\n    .card {\n      /* ( */\n      color: red;\n    }\n\ndef after_fragment() -> None:\n  let x = 1\n  print(x)\n";
        let errors = parse_embedded_fixture(source, &keyword_map, &surface_map).err().ok_or(
            "expected the boundary-desync diagnostic, not a successful parse that silently dropped \
             `after_fragment`",
        )?;
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("boundary could not be determined reliably")),
            "expected the boundary-desync diagnostic, got {errors:?}"
        );
        Ok(())
    }

    #[test]
    fn style_fragment_starting_with_a_declaration_keyword_word_is_not_a_false_positive_boundary_desync()
    -> Result<(), Box<dyn std::error::Error>> {
        // Regression test for the boundary-desync check itself: a fragment's raw slice starts right after the
        // block's `Indent` token, so unlike every later line, its *first* line alone carries no leading
        // indentation in that string -- even when the fragment is correctly bounded. A naive check that also
        // scanned the first line would false-positive here, since `.def-card` starts with the literal text
        // `"def "`-adjacent bytes... use a declaration keyword outright to make the risk concrete: a `RawText`
        // fragment whose entire, correctly-bounded content is the single word `"class"` on its own first line.
        let (keyword_map, surface_map) =
            embedded_fixture_maps("note", "webkit", incan_vocab::EmbeddedFragmentSubmode::RawText);
        let source = "import pub::webkit\n\ndef render() -> None:\n  note:\n    class assignment\n\ndef after_fragment() -> None:\n  let x = 1\n";
        let program = parse_embedded_fixture(source, &keyword_map, &surface_map)
            .map_err(|errs| format!("expected no false-positive boundary-desync error, got: {errs:?}"))?;
        let fragment = embedded_fragment_from_program(&program)?;
        assert!(matches!(&fragment.nodes[0].node, EmbeddedNode::Text(t) if t.contains("class assignment")));
        Ok(())
    }

    // ---- RawText submode ----

    #[test]
    fn raw_text_fragment_preserves_verbatim_text_and_supports_a_hole() -> Result<(), Box<dyn std::error::Error>> {
        let (keyword_map, surface_map) =
            embedded_fixture_maps("note", "webkit", incan_vocab::EmbeddedFragmentSubmode::RawText);
        let source = "import pub::webkit\n\ndef render(who: str) -> None:\n  note:\n    TODO({who}): finish this <<weird>> text\n";
        let program = parse_embedded_fixture(source, &keyword_map, &surface_map)
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let fragment = embedded_fragment_from_program(&program)?;
        assert!(fragment.nodes.len() >= 3);
        assert!(matches!(&fragment.nodes[0].node, EmbeddedNode::Text(t) if t == "TODO("));
        assert!(matches!(&fragment.nodes[1].node, EmbeddedNode::Hole(expr) if matches!(&expr.node, Expr::Ident(n) if n == "who")));
        assert!(fragment.source_text.contains("<<weird>>"));
        Ok(())
    }

    // ---- RegexTemplate submode ----

    #[test]
    fn regex_template_fragment_accepts_a_regex_literal() -> Result<(), Box<dyn std::error::Error>> {
        let (keyword_map, surface_map) =
            embedded_fixture_maps("script", "webkit", incan_vocab::EmbeddedFragmentSubmode::RegexTemplate);
        let source = "import pub::webkit\n\ndef render() -> None:\n  script:\n    /^[a-z]+\\/[0-9]+$/gi\n";
        let program = parse_embedded_fixture(source, &keyword_map, &surface_map)
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let fragment = embedded_fragment_from_program(&program)?;
        assert_eq!(fragment.nodes.len(), 1);
        assert!(matches!(
            &fragment.nodes[0].node,
            EmbeddedNode::Regex { pattern, flags } if pattern == "^[a-z]+\\/[0-9]+$" && flags == "gi"
        ));
        Ok(())
    }

    #[test]
    fn regex_template_fragment_accepts_a_template_string_with_a_hole() -> Result<(), Box<dyn std::error::Error>> {
        let (keyword_map, surface_map) =
            embedded_fixture_maps("script", "webkit", incan_vocab::EmbeddedFragmentSubmode::RegexTemplate);
        let source = "import pub::webkit\n\ndef render(name: str) -> None:\n  script:\n    `hello ${name}!`\n";
        let program = parse_embedded_fixture(source, &keyword_map, &surface_map)
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let fragment = embedded_fragment_from_program(&program)?;
        assert_eq!(fragment.nodes.len(), 3);
        assert!(matches!(&fragment.nodes[0].node, EmbeddedNode::Text(t) if t == "hello "));
        assert!(matches!(&fragment.nodes[1].node, EmbeddedNode::Hole(expr) if matches!(&expr.node, Expr::Ident(n) if n == "name")));
        assert!(matches!(&fragment.nodes[2].node, EmbeddedNode::Text(t) if t == "!"));
        Ok(())
    }

    #[test]
    fn regex_template_fragment_rejects_content_that_is_neither_form() -> Result<(), Box<dyn std::error::Error>> {
        let (keyword_map, surface_map) =
            embedded_fixture_maps("script", "webkit", incan_vocab::EmbeddedFragmentSubmode::RegexTemplate);
        let source = "import pub::webkit\n\ndef render() -> None:\n  script:\n    not_a_regex_or_template\n";
        let errors = parse_embedded_fixture(source, &keyword_map, &surface_map)
            .err()
            .ok_or("expected a parse error for content matching neither accepted form")?;
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("regex literal") || error.message.contains("template")),
            "expected a form-mismatch diagnostic, got {errors:?}"
        );
        Ok(())
    }

    // ---- Production-entrypoint tolerant-lex reconciliation (RFC 081, `#1023`) ----
    //
    // The parser-level fixtures above all call `parse_embedded_fixture`, which unconditionally lexes with
    // `lex_tolerant` and discards its collected errors. That mirrors the parser's own unit-test convenience, not
    // the real production entrypoint (`CompilationSession::parse_source_for_collection` in
    // `src/cli/commands/common.rs`), which only falls back to `lex_tolerant` after the strict `lex()` fails, and
    // then reconciles the tolerant lexer's errors through `parse_with_source_and_lex_errors` rather than
    // dropping them outright. The test below exercises that exact reconciliation path directly.

    #[test]
    fn tolerant_lex_errors_inside_a_claimed_fragment_are_reconciled_away_but_real_errors_elsewhere_survive()
    -> Result<(), Box<dyn std::error::Error>> {
        let (keyword_map, surface_map) =
            embedded_fixture_maps("script", "webkit", incan_vocab::EmbeddedFragmentSubmode::RegexTemplate);
        // The `script:` fragment's template-string content (backtick, `$`, `{`) is exactly the kind of byte the
        // ordinary Incan lexer cannot tokenize on its own -- this is why the production entrypoint needs the
        // tolerant-lex fallback at all. The stray backtick in `broken()`, well outside the fragment, is a genuine
        // mistake and must still surface as a real error after reconciliation, not get silently dropped along
        // with the fragment's expected noise.
        let source = "import pub::webkit\n\ndef render(name: str) -> None:\n  script:\n    `hello ${name}!`\n\ndef broken() -> None:\n    let x = `oops\n";

        // Confirm the premise: the strict lexer really does fail outright on this file, so production would
        // actually take the tolerant-lex fallback branch for it (see `parse_source_for_collection`).
        assert!(
            lexer::lex(source).is_err(),
            "expected the strict lexer to fail outright on the fragment's template-string bytes"
        );

        let (tokens, lex_errors) = lexer::lex_tolerant(source);
        assert!(
            !lex_errors.is_empty(),
            "expected the tolerant lexer to still record errors for the exotic bytes"
        );

        let errors = parse_with_source_and_lex_errors(
            &tokens,
            None,
            Some(&keyword_map),
            Some(&surface_map),
            source,
            lex_errors,
        )
        .err()
        .ok_or("expected the stray backtick in `broken()` to still surface as a real error")?;

        // Reconciliation must drop every error whose span falls inside the successfully-claimed `script:`
        // fragment (the template string's own backtick/`$`/`{` bytes) while keeping the one real mistake in
        // `broken()`.
        let broken_offset = source.find("`oops").ok_or("fixture source must contain the stray backtick")?;
        assert!(!errors.is_empty(), "expected at least one surviving real error");
        assert!(
            errors.iter().all(|error| error.span.start >= broken_offset),
            "expected only the real error in `broken()` to survive reconciliation, got: {errors:?}"
        );
        Ok(())
    }

    // ---- SelectorDeclarationValue submode ----

    #[test]
    fn selector_declaration_value_fragment_accepts_a_dimension() -> Result<(), Box<dyn std::error::Error>> {
        let (keyword_map, surface_map) = embedded_fixture_maps(
            "spacing",
            "webkit",
            incan_vocab::EmbeddedFragmentSubmode::SelectorDeclarationValue,
        );
        let source = "import pub::webkit\n\ndef render() -> None:\n  spacing:\n    2rem\n";
        let program = parse_embedded_fixture(source, &keyword_map, &surface_map)
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let fragment = embedded_fragment_from_program(&program)?;
        assert_eq!(fragment.nodes.len(), 1);
        assert!(matches!(
            &fragment.nodes[0].node,
            EmbeddedNode::Value(EmbeddedValue::Dimension { number, unit }) if number == "2" && unit == "rem"
        ));
        Ok(())
    }

    #[test]
    fn selector_declaration_value_fragment_rejects_trailing_content() -> Result<(), Box<dyn std::error::Error>> {
        let (keyword_map, surface_map) = embedded_fixture_maps(
            "spacing",
            "webkit",
            incan_vocab::EmbeddedFragmentSubmode::SelectorDeclarationValue,
        );
        let source = "import pub::webkit\n\ndef render() -> None:\n  spacing:\n    2rem 4px\n";
        let errors = parse_embedded_fixture(source, &keyword_map, &surface_map)
            .err()
            .ok_or("expected a parse error: only one declaration-value token is accepted per fragment")?;
        assert!(
            errors.iter().any(|error| error.message.contains("trailing content")),
            "expected a trailing-content diagnostic, got {errors:?}"
        );
        Ok(())
    }

    // ---- TypePosition submode ----

    #[test]
    fn type_position_fragment_accepts_generic_nullable_array_and_union() -> Result<(), Box<dyn std::error::Error>> {
        let (keyword_map, surface_map) =
            embedded_fixture_maps("typeof", "webkit", incan_vocab::EmbeddedFragmentSubmode::TypePosition);
        let source = "import pub::webkit\n\ndef render() -> None:\n  typeof:\n    a.b.Foo<Bar[]>? | Baz\n";
        let program = parse_embedded_fixture(source, &keyword_map, &surface_map)
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let fragment = embedded_fragment_from_program(&program)?;
        assert_eq!(fragment.nodes.len(), 1);
        let EmbeddedNode::TypeShape(EmbeddedTypeShape::Union(members)) = &fragment.nodes[0].node else {
            return Err("expected a top-level union type shape".into());
        };
        assert_eq!(members.len(), 2);
        let EmbeddedTypeShape::Nullable(inner) = &members[0] else {
            return Err("expected the first union member to be nullable".into());
        };
        let EmbeddedTypeShape::Generic(base, args) = inner.as_ref() else {
            return Err("expected the nullable member to wrap a generic type".into());
        };
        assert!(matches!(base.as_ref(), EmbeddedTypeShape::Name(segments) if segments == &["a".to_string(), "b".to_string(), "Foo".to_string()]));
        assert!(matches!(&args[0], EmbeddedTypeShape::Array(elem) if matches!(elem.as_ref(), EmbeddedTypeShape::Name(segments) if segments == &["Bar".to_string()])));
        assert!(matches!(&members[1], EmbeddedTypeShape::Name(segments) if segments == &["Baz".to_string()]));
        Ok(())
    }

    #[test]
    fn type_position_fragment_rejects_an_unclosed_generic() -> Result<(), Box<dyn std::error::Error>> {
        let (keyword_map, surface_map) =
            embedded_fixture_maps("typeof", "webkit", incan_vocab::EmbeddedFragmentSubmode::TypePosition);
        let source = "import pub::webkit\n\ndef render() -> None:\n  typeof:\n    Foo<Bar\n";
        let errors = parse_embedded_fixture(source, &keyword_map, &surface_map)
            .err()
            .ok_or("expected a parse error for an unclosed generic argument list")?;
        assert!(
            errors.iter().any(|error| error.message.contains("Expected `>`")),
            "expected an unclosed-generic diagnostic, got {errors:?}"
        );
        Ok(())
    }

    // ---- Cross-submode: ambiguity and ownership ----

    #[test]
    fn same_depth_embedded_fragment_descriptor_ambiguity_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        // Mirrors `test_same_depth_scoped_symbol_ambiguity_is_rejected` (parser/tests.rs): two independently
        // imported providers each register their own `markup` keyword and their own embedded-fragment descriptor
        // claiming its declaration body, so both are active at the same depth for the same eligible position.
        let mut keyword_map = KeywordMap::new();
        keyword_map.insert(
            "alpha".to_string(),
            vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "alpha.markup".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec::block("markup")],
                valid_decorators: Vec::new(),
            }],
        );
        keyword_map.insert(
            "beta".to_string(),
            vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "beta.markup".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec::block("markup")],
                valid_decorators: Vec::new(),
            }],
        );
        let mut surface_map = SurfaceMap::new();
        surface_map.insert(
            "alpha".to_string(),
            vec![
                incan_vocab::DslSurface::on_import("alpha.markup")
                    .with_declaration(incan_vocab::DeclarationSurface::named("markup"))
                    .with_embedded_fragment(
                        incan_vocab::EmbeddedFragmentDescriptor::new(
                            "alpha.markup.fragment",
                            incan_vocab::EmbeddedFragmentSubmode::Markup,
                            "fragment",
                        )
                        .in_declaration_body("markup"),
                    ),
            ],
        );
        surface_map.insert(
            "beta".to_string(),
            vec![
                incan_vocab::DslSurface::on_import("beta.markup")
                    .with_declaration(incan_vocab::DeclarationSurface::named("markup"))
                    .with_embedded_fragment(
                        incan_vocab::EmbeddedFragmentDescriptor::new(
                            "beta.markup.fragment",
                            incan_vocab::EmbeddedFragmentSubmode::Markup,
                            "fragment",
                        )
                        .in_declaration_body("markup"),
                    ),
            ],
        );
        let source = "import pub::alpha\nimport pub::beta\n\ndef render() -> None:\n  markup:\n    <p></p>\n";
        let errors = parse_embedded_fixture(source, &keyword_map, &surface_map)
            .err()
            .ok_or("expected an ambiguous same-depth descriptor error")?;
        assert!(
            errors.iter().any(|error| error.message.contains("Ambiguous embedded-fragment descriptors")),
            "expected an ambiguity diagnostic, got {errors:?}"
        );
        Ok(())
    }

    // ---- Conformance: `examples/pro/vocab_*` end-to-end example packages parse ----
    //
    // These tie the real consumer `.incn` files under `examples/pro/` to compiler correctness: each fixture uses
    // the exact keyword/descriptor shape the matching producer's `vocab_companion` crate registers (mirrored here
    // by hand, since parser tests do not load a real manifest), and parses the real file content via
    // `include_str!`. A grammar mismatch or typo in the example content fails here, not just in documentation.

    #[test]
    fn vocab_styleforge_consumer_example_parses() -> Result<(), Box<dyn std::error::Error>> {
        let source = include_str!("../../../../../examples/pro/vocab_styleforge/consumer/src/main.incn");
        let mut keyword_map = KeywordMap::new();
        keyword_map.insert(
            "styleforge".to_string(),
            vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "styleforge".to_string(),
                },
                keywords: vec![
                    incan_vocab::KeywordSpec::block("style"),
                    incan_vocab::KeywordSpec::block("spacing"),
                ],
                valid_decorators: Vec::new(),
            }],
        );
        let mut surface_map = SurfaceMap::new();
        surface_map.insert(
            "styleforge".to_string(),
            vec![
                incan_vocab::DslSurface::on_import("styleforge")
                    .with_declaration(incan_vocab::DeclarationSurface::named("style"))
                    .with_declaration(incan_vocab::DeclarationSurface::named("spacing"))
                    .with_embedded_fragment(
                        incan_vocab::EmbeddedFragmentDescriptor::new(
                            "style.fragment",
                            incan_vocab::EmbeddedFragmentSubmode::Style,
                            "style_rules",
                        )
                        .in_declaration_body("style"),
                    )
                    .with_embedded_fragment(
                        incan_vocab::EmbeddedFragmentDescriptor::new(
                            "spacing.fragment",
                            incan_vocab::EmbeddedFragmentSubmode::SelectorDeclarationValue,
                            "spacing_value",
                        )
                        .in_declaration_body("spacing"),
                    ),
            ],
        );
        parse_embedded_fixture(source, &keyword_map, &surface_map)
            .map_err(|errs| format!("vocab_styleforge consumer example failed to parse: {errs:?}"))?;
        Ok(())
    }

    #[test]
    fn vocab_markform_consumer_example_parses() -> Result<(), Box<dyn std::error::Error>> {
        let source = include_str!("../../../../../examples/pro/vocab_markform/consumer/src/main.incn");
        let mut keyword_map = KeywordMap::new();
        keyword_map.insert(
            "markform".to_string(),
            vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "markform".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec::block("markup")],
                valid_decorators: Vec::new(),
            }],
        );
        let mut surface_map = SurfaceMap::new();
        surface_map.insert(
            "markform".to_string(),
            vec![
                incan_vocab::DslSurface::on_import("markform")
                    .with_declaration(incan_vocab::DeclarationSurface::named("markup"))
                    .with_embedded_fragment(
                        incan_vocab::EmbeddedFragmentDescriptor::new(
                            "markup.fragment",
                            incan_vocab::EmbeddedFragmentSubmode::Markup,
                            "markup_nodes",
                        )
                        .in_declaration_body("markup"),
                    ),
            ],
        );
        parse_embedded_fixture(source, &keyword_map, &surface_map)
            .map_err(|errs| format!("vocab_markform consumer example failed to parse: {errs:?}"))?;
        Ok(())
    }

    #[test]
    fn vocab_scriptkit_consumer_example_parses() -> Result<(), Box<dyn std::error::Error>> {
        let source = include_str!("../../../../../examples/pro/vocab_scriptkit/consumer/src/main.incn");
        let mut keyword_map = KeywordMap::new();
        keyword_map.insert(
            "scriptkit".to_string(),
            vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "scriptkit".to_string(),
                },
                keywords: vec![
                    incan_vocab::KeywordSpec::block("pattern"),
                    incan_vocab::KeywordSpec::block("shape"),
                ],
                valid_decorators: Vec::new(),
            }],
        );
        let mut surface_map = SurfaceMap::new();
        surface_map.insert(
            "scriptkit".to_string(),
            vec![
                incan_vocab::DslSurface::on_import("scriptkit")
                    .with_declaration(incan_vocab::DeclarationSurface::named("pattern"))
                    .with_declaration(incan_vocab::DeclarationSurface::named("shape"))
                    .with_embedded_fragment(
                        incan_vocab::EmbeddedFragmentDescriptor::new(
                            "pattern.fragment",
                            incan_vocab::EmbeddedFragmentSubmode::RegexTemplate,
                            "pattern_nodes",
                        )
                        .in_declaration_body("pattern"),
                    )
                    .with_embedded_fragment(
                        incan_vocab::EmbeddedFragmentDescriptor::new(
                            "shape.fragment",
                            incan_vocab::EmbeddedFragmentSubmode::TypePosition,
                            "shape_node",
                        )
                        .in_declaration_body("shape"),
                    ),
            ],
        );
        parse_embedded_fixture(source, &keyword_map, &surface_map)
            .map_err(|errs| format!("vocab_scriptkit consumer example failed to parse: {errs:?}"))?;
        Ok(())
    }
}

//! Vocab-facing editor surfaces: scoped symbols from imported descriptors (completion limited to the active DSL scope,
//! hover metadata, definition at the activating `pub` import) and editor-facing ownership across an embedded fragment's
//! boundary (RFC 081, #1022).

use std::collections::HashMap;

use super::{
    active_scoped_symbol_completions, embedded_dsl_ownership_at_offset, embedded_dsl_ownership_hover_markdown,
    find_pub_library_import_span, scoped_symbol_at_offset, scoped_symbol_hover_markdown,
};
use incan_frontend::ast::Program;
use incan_frontend::{lexer, parser};

fn scoped_symbol_fixture() -> (
    String,
    Program,
    parser::ImportedLibraryDslSurfaces,
    parser::ImportedLibraryVocab,
) {
    let source = "import pub::analytics\n\ndef configure() -> None:\n  query:\n    sum(amount)\n\nconst outside = 1\n";
    let mut keyword_map = HashMap::new();
    keyword_map.insert(
        "analytics".to_string(),
        vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "analytics.query".to_string(),
            },
            keywords: vec![incan_vocab::KeywordSpec {
                name: "query".to_string(),
                surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                compound_tokens: Vec::new(),
                placement: incan_vocab::KeywordPlacement::TopLevel,
            }],
            valid_decorators: Vec::new(),
        }],
    );
    let mut surface_map = HashMap::new();
    surface_map.insert(
        "analytics".to_string(),
        vec![
            incan_vocab::DslSurface::on_import("analytics.query")
                .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                .with_scoped_symbol(
                    incan_vocab::ScopedSymbolDescriptor::aggregate("query.sum", "sum")
                        .with_role(incan_vocab::ScopedSymbolRoleMetadata::new("aggregate.total").with_label("Total"))
                        .with_misuse_scope(incan_vocab::ScopedSymbolMisuseScope::ActiveDsl)
                        .in_declaration_body("query"),
                ),
        ],
    );
    let tokens = lexer::lex(source).expect("fixture should lex");
    let ast = parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
        .expect("fixture should parse");
    (source.to_string(), ast, surface_map, keyword_map)
}

#[test]
fn scoped_symbol_completion_is_limited_to_active_dsl_scope() {
    let (source, ast, surface_map, _keyword_map) = scoped_symbol_fixture();
    let scoped_offset = source.find("sum(amount)").expect("sum call should exist");
    let items = active_scoped_symbol_completions(&ast, &surface_map, scoped_offset);
    assert!(
        items.iter().any(|item| {
            item.dependency_key == "analytics" && item.descriptor.key == "query.sum" && item.descriptor.symbol == "sum"
        }),
        "expected sum completion inside query block, got {items:?}"
    );

    let outside_offset = source.find("const outside").expect("outside binding should exist");
    let outside_items = active_scoped_symbol_completions(&ast, &surface_map, outside_offset);
    assert!(
        outside_items.is_empty(),
        "scoped symbol completions must not leak outside the owning DSL scope: {outside_items:?}"
    );
}

#[test]
fn scoped_symbol_hover_resolves_imported_descriptor_metadata() {
    let (source, ast, surface_map, _keyword_map) = scoped_symbol_fixture();
    let offset = source.find("sum(amount)").expect("sum call should exist") + 1;
    let occurrence =
        scoped_symbol_at_offset(&ast, &source, &surface_map, offset).expect("sum should resolve as scoped symbol");
    let markdown = scoped_symbol_hover_markdown(occurrence.dependency_key, occurrence.descriptor);

    assert_eq!(occurrence.dependency_key, "analytics");
    assert_eq!(occurrence.descriptor.key, "query.sum");
    assert_eq!(&source[occurrence.symbol_span.start..occurrence.symbol_span.end], "sum");
    assert!(
        markdown.contains("scoped DSL symbol")
            && markdown.contains("`pub::analytics`")
            && markdown.contains("Descriptor: `query.sum`")
            && markdown.contains("Family: `aggregate-like`"),
        "hover markdown should expose descriptor metadata, got:\n{markdown}"
    );
}

#[test]
fn scoped_symbol_definition_points_to_activating_pub_import() {
    let (source, ast, surface_map, _keyword_map) = scoped_symbol_fixture();
    let offset = source.find("sum(amount)").expect("sum call should exist") + 1;
    let occurrence =
        scoped_symbol_at_offset(&ast, &source, &surface_map, offset).expect("sum should resolve as scoped symbol");
    let import_span = find_pub_library_import_span(&ast, occurrence.dependency_key, occurrence.symbol_span.start)
        .expect("activating import span should be available");

    assert_eq!(&source[import_span.start..import_span.end], "import pub::analytics");
}

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Parse a markup-fragment fixture through the only entrypoint that produces embedded fragments.
///
/// `parse_with_source` threads the original source to the submode tokenizer; the ordinary entrypoints are
/// fragment-blind and would hand back a program with no fragment in it, making every assertion below vacuous.
/// `lex_tolerant` matches it for the reason the parser fixtures document: fragment content routinely holds
/// bytes ordinary Incan tokenization rejects.
fn parse_markup_fixture(source: &str) -> Result<Program, String> {
    let mut keyword_map = HashMap::new();
    keyword_map.insert(
        "webkit".to_string(),
        vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "webkit.html".to_string(),
            },
            keywords: vec![incan_vocab::KeywordSpec::block("html")],
            valid_decorators: Vec::new(),
        }],
    );
    let mut surface_map = HashMap::new();
    surface_map.insert(
        "webkit".to_string(),
        vec![
            incan_vocab::DslSurface::on_import("webkit.html")
                .with_declaration(incan_vocab::DeclarationSurface::named("html"))
                .with_embedded_fragment(
                    incan_vocab::EmbeddedFragmentDescriptor::new(
                        "html.fragment",
                        incan_vocab::EmbeddedFragmentSubmode::Markup,
                        "fragment",
                    )
                    .in_declaration_body("html"),
                ),
        ],
    );

    let (tokens, _lex_errors) = lexer::lex_tolerant(source);
    parser::parse_with_source(&tokens, None, Some(&keyword_map), Some(&surface_map), source)
        .map_err(|errors| format!("parse errors: {errors:?}"))
}

/// A fragment whose tag name is also a real local binding in the same function.
///
/// The collision is the point: `section` is a perfectly good Incan identifier, so every source-text-driven
/// resolution path in this server would happily answer the tag name with the local's type unless ownership is
/// settled first.
const COLLIDING_FIXTURE: &str = "import pub::webkit\n\ndef render(title: str) -> None:\n    section = title\n    html:\n        <section class=\"card\">\n            <h1>{title}</h1>\n        </section>\n";

#[test]
fn a_tag_name_colliding_with_a_local_binding_is_still_dsl_owned() -> TestResult {
    let program = parse_markup_fixture(COLLIDING_FIXTURE)?;
    let tag_offset = COLLIDING_FIXTURE
        .find("<section")
        .ok_or("fixture should contain the opening tag")?
        + 1;

    let ownership =
        embedded_dsl_ownership_at_offset(&program, tag_offset).ok_or("the tag name must be claimed as DSL-owned")?;
    assert_eq!(ownership.submode, incan_vocab::EmbeddedFragmentSubmode::Markup);
    assert_eq!(ownership.descriptor_key, "html.fragment");
    assert_eq!(ownership.dependency_key, "webkit");
    Ok(())
}

#[test]
fn the_same_spelling_outside_the_fragment_is_ordinary_incan() -> TestResult {
    // The other half of the collision. `section = title` is an ordinary assignment and must keep every
    // resolution path it has; claiming it would trade one wrong answer for another.
    let program = parse_markup_fixture(COLLIDING_FIXTURE)?;
    let binding_offset = COLLIDING_FIXTURE
        .find("section = title")
        .ok_or("fixture should contain the local binding")?;

    assert!(
        embedded_dsl_ownership_at_offset(&program, binding_offset).is_none(),
        "ordinary Incan outside a fragment must not be claimed by one"
    );
    Ok(())
}

#[test]
fn an_expression_hole_falls_through_to_ordinary_resolution() -> TestResult {
    // A hole is ordinary Incan (RFC 081), so it must not be claimed as DSL-owned — that is what keeps hover,
    // signature help and go-to-definition working for code that merely happens to sit inside a fragment.
    let program = parse_markup_fixture(COLLIDING_FIXTURE)?;
    let hole_offset = COLLIDING_FIXTURE
        .find("{title}")
        .ok_or("fixture should contain the expression hole")?
        + 1;

    assert!(
        embedded_dsl_ownership_at_offset(&program, hole_offset).is_none(),
        "an expression hole must fall through to ordinary Incan resolution"
    );
    Ok(())
}

#[test]
fn the_ownership_hover_names_the_submode_descriptor_and_dependency() -> TestResult {
    // The hover has one job: tell the reader who owns this text and why nothing here resolves like Incan.
    let program = parse_markup_fixture(COLLIDING_FIXTURE)?;
    let tag_offset = COLLIDING_FIXTURE
        .find("<section")
        .ok_or("fixture should contain the opening tag")?
        + 1;
    let ownership =
        embedded_dsl_ownership_at_offset(&program, tag_offset).ok_or("the tag name must be claimed as DSL-owned")?;

    let markdown = embedded_dsl_ownership_hover_markdown(&ownership);
    assert!(
        markdown.contains("Markup"),
        "hover should name the submode:\n{markdown}"
    );
    assert!(
        markdown.contains("html.fragment"),
        "hover should name the claiming descriptor:\n{markdown}"
    );
    assert!(
        markdown.contains("pub::webkit"),
        "hover should name the dependency it came from:\n{markdown}"
    );
    assert!(
        markdown.contains("not ordinary Incan"),
        "hover should say plainly that this is not ordinary Incan:\n{markdown}"
    );
    Ok(())
}

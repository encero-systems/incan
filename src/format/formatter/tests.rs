//! Formatter tests for RFC 081 descriptor-gated embedded fragments (#1022).
//!
//! RFC 081 gives the formatter exactly two modes and forbids a third fallback state: render from the structured
//! artifact, or preserve source verbatim for a DSL that declares itself layout-sensitive. These tests pin both
//! modes and the idempotency the structural mode has to hold.

use std::collections::HashMap;

use incan_syntax::lexer;

use crate::format::FormatConfig;
use crate::format::formatter::Formatter;

type KeywordMap = HashMap<String, Vec<incan_vocab::KeywordRegistration>>;
type SurfaceMap = HashMap<String, Vec<incan_vocab::DslSurface>>;

/// Build keyword and surface maps activating one embedded-fragment descriptor on a declaration body.
fn fixture_maps(
    keyword: &str,
    provider: &str,
    submode: incan_vocab::EmbeddedFragmentSubmode,
    layout_sensitive: bool,
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

    let mut descriptor =
        incan_vocab::EmbeddedFragmentDescriptor::new(&format!("{keyword}.fragment"), submode, "fragment")
            .in_declaration_body(keyword);
    if layout_sensitive {
        descriptor = descriptor.layout_sensitive();
    }

    let mut surface_map = SurfaceMap::new();
    surface_map.insert(
        provider.to_string(),
        vec![
            incan_vocab::DslSurface::on_import(&namespace)
                .with_declaration(incan_vocab::DeclarationSurface::named(keyword))
                .with_embedded_fragment(descriptor),
        ],
    );
    (keyword_map, surface_map)
}

/// Parse through the one entrypoint that produces embedded fragments, then format the result.
///
/// `parse_with_source` is the only parser entrypoint that threads original source through, which is what an
/// embedded fragment's submode re-tokenization needs; the ordinary entrypoints are fragment-blind and would make
/// this test silently assert on a program that has no fragment in it at all.
fn format_fixture(source: &str, keyword_map: &KeywordMap, surface_map: &SurfaceMap) -> Result<String, String> {
    let (tokens, _lex_errors) = lexer::lex_tolerant(source);
    let program = incan_syntax::parser::parse_with_source(&tokens, None, Some(keyword_map), Some(surface_map), source)
        .map_err(|errs| format!("parse errors: {errs:?}"))?;
    Ok(Formatter::new(FormatConfig::default()).format(&program))
}

#[test]
fn a_layout_sensitive_descriptor_keeps_its_fragment_verbatim() -> Result<(), String> {
    // A DSL that declares itself layout-sensitive owns its own whitespace, so the formatter must not restructure
    // it. This is one of RFC 081's two modes, and the descriptor is what selects it.
    let (keyword_map, surface_map) =
        fixture_maps("raw", "textkit", incan_vocab::EmbeddedFragmentSubmode::RawText, true);
    let source =
        "import pub::textkit\n\ndef banner() -> None:\n    raw:\n        line one\n           deliberately indented\n";
    let formatted = format_fixture(source, &keyword_map, &surface_map)?;

    assert!(
        formatted.contains("           deliberately indented"),
        "layout-sensitive source must survive untouched:\n{formatted}"
    );
    Ok(())
}

#[test]
fn a_structural_markup_fragment_is_formatted_from_its_nodes() -> Result<(), String> {
    // The other mode: a descriptor that does not declare layout sensitivity gets rendered from the structural
    // tree. Ragged source indentation is layout, so it is re-derived rather than reproduced.
    let (keyword_map, surface_map) =
        fixture_maps("html", "webkit", incan_vocab::EmbeddedFragmentSubmode::Markup, false);
    let source = "import pub::webkit\n\ndef render(title: str) -> None:\n    html:\n        <section class=\"card\">\n              <h1>{title}</h1>\n        </section>\n";
    let formatted = format_fixture(source, &keyword_map, &surface_map)?;

    assert!(
        formatted.contains("<section class=\"card\">"),
        "the element and its literal attribute should render structurally:\n{formatted}"
    );
    assert!(
        formatted.contains("<h1>{title}</h1>"),
        "an inline child keeps its text and hole on one line:\n{formatted}"
    );
    assert!(
        !formatted.contains("              <h1>"),
        "the source's ragged indentation is layout and should not survive:\n{formatted}"
    );
    Ok(())
}

#[test]
fn structural_fragment_formatting_is_idempotent() -> Result<(), String> {
    // Formatting has to be a fixed point: whitespace the first pass introduces becomes text nodes on re-parse, so
    // a renderer that reproduced them would drift further on every pass. This is the property that makes the
    // structural mode safe to run in `incan fmt --check`.
    let (keyword_map, surface_map) =
        fixture_maps("html", "webkit", incan_vocab::EmbeddedFragmentSubmode::Markup, false);
    let source = "import pub::webkit\n\ndef render(title: str, body: str) -> None:\n    html:\n        <section>\n          <h1>{title}</h1>\n          <p>Body: {body}</p>\n        </section>\n";

    let once = format_fixture(source, &keyword_map, &surface_map)?;
    let twice = format_fixture(&once, &keyword_map, &surface_map)?;
    assert_eq!(
        once, twice,
        "second pass changed the output:\n--- once ---\n{once}\n--- twice ---\n{twice}"
    );
    Ok(())
}

#[test]
fn a_structural_style_fragment_renders_rules_and_declarations() -> Result<(), String> {
    // The style submode nests declarations under a rule rather than children under an element, so it exercises a
    // different branch of the structural renderer than markup does.
    let (keyword_map, surface_map) =
        fixture_maps("css", "styleforge", incan_vocab::EmbeddedFragmentSubmode::Style, false);
    let source = "import pub::styleforge\n\ndef theme(accent: str) -> None:\n    css:\n        .card {\n            color: {accent};\n        }\n";

    let once = format_fixture(source, &keyword_map, &surface_map)?;
    assert!(
        once.contains(".card {"),
        "the selector and block opener should render:\n{once}"
    );
    assert!(
        once.contains("color: {accent};"),
        "a declaration with a hole value should render with its semicolon:\n{once}"
    );

    let twice = format_fixture(&once, &keyword_map, &surface_map)?;
    assert_eq!(once, twice, "style formatting should be a fixed point:\n{once}");
    Ok(())
}

#[test]
fn a_regex_template_fragment_survives_a_formatting_round_trip() -> Result<(), String> {
    // A template string's backticks and `${...}` delimiters are consumed while parsing and never stored as nodes,
    // so rendering the node tree the way other submodes render theirs emitted `hello {name}!` -- a fragment this
    // submode rejects outright. `incan fmt` therefore rewrote a valid file into one that no longer parses, which
    // is why the round trip is the assertion here rather than the output text alone.
    let (keyword_map, surface_map) = fixture_maps(
        "script",
        "scriptkit",
        incan_vocab::EmbeddedFragmentSubmode::RegexTemplate,
        false,
    );
    let source = "import pub::scriptkit\n\ndef render(name: str) -> None:\n    script:\n        `hello ${name}!`\n";

    let once = format_fixture(source, &keyword_map, &surface_map)?;
    assert!(
        once.contains("`hello ${name}!`"),
        "the template's own delimiters must be reconstructed:\n{once}"
    );
    let twice = format_fixture(&once, &keyword_map, &surface_map)?;
    assert_eq!(once, twice, "template formatting should be a fixed point:\n{once}");
    Ok(())
}

#[test]
fn a_regex_literal_fragment_survives_a_formatting_round_trip() -> Result<(), String> {
    // The submode's other accepted form. It keeps its own node, so it only has to prove it is not swept into the
    // template-reconstruction path added for the case above.
    let (keyword_map, surface_map) = fixture_maps(
        "script",
        "scriptkit",
        incan_vocab::EmbeddedFragmentSubmode::RegexTemplate,
        false,
    );
    let source = "import pub::scriptkit\n\ndef render() -> None:\n    script:\n        /^[a-z]+$/gi\n";

    let once = format_fixture(source, &keyword_map, &surface_map)?;
    assert!(
        once.contains("/^[a-z]+$/gi"),
        "a regex literal must render as itself, not as a template:\n{once}"
    );
    let twice = format_fixture(&once, &keyword_map, &surface_map)?;
    assert_eq!(once, twice, "regex formatting should be a fixed point:\n{once}");
    Ok(())
}

#[test]
fn preserved_fragment_text_does_not_grow_a_blank_line_per_pass() -> Result<(), String> {
    // Both modes that write preserved text verbatim -- a layout-sensitive descriptor, and `RawText`'s content
    // runs -- used to emit the newline that terminates the fragment's last line, which the enclosing statement
    // writer emits as well. Every `incan fmt` pass added one more blank line, so a formatted file was never
    // stable. Two passes is enough to catch it: the growth is one line per pass.
    for layout_sensitive in [false, true] {
        let (keyword_map, surface_map) = fixture_maps(
            "note",
            "textkit",
            incan_vocab::EmbeddedFragmentSubmode::RawText,
            layout_sensitive,
        );
        let source =
            "import pub::textkit\n\ndef banner(who: str) -> None:\n    note:\n        TODO({who}): still to do\n";

        let once = format_fixture(source, &keyword_map, &surface_map)?;
        let twice = format_fixture(&once, &keyword_map, &surface_map)?;
        assert_eq!(
            once, twice,
            "layout_sensitive={layout_sensitive}: preserved text must be a fixed point:\n--- once ---\n{once}\n--- \
             twice ---\n{twice}"
        );
        assert!(
            once.contains("TODO({who}): still to do"),
            "layout_sensitive={layout_sensitive}: preserved content must survive:\n{once}"
        );
    }
    Ok(())
}

// ---- Declaration and literal round trips (#1401) ----

/// Format `source` through the public entrypoint, or fail with the formatter's own message.
fn formatted(source: &str) -> Result<String, String> {
    crate::format::format_source(source).map_err(|error| error.to_string())
}

#[test]
fn an_enum_keeps_its_trait_adoption() -> Result<(), String> {
    // `enum Level with Display` and `enum Level` are different types. The writer omitted `en.traits` entirely, so
    // formatting silently rewrote the first into the second — a meaning change reported as success.
    let source = "from std.traits import Display

enum Level with Display:
    Low
    High
";
    let once = formatted(source)?;
    assert!(
        once.contains("enum Level with Display:"),
        "the adoption clause was dropped:
{once}"
    );
    assert_eq!(
        once,
        formatted(&once)?,
        "formatting is not a fixed point:
{once}"
    );
    Ok(())
}

#[test]
fn a_byte_literal_keeps_its_escapes() -> Result<(), String> {
    // A quote or backslash byte sits in the printable range, so writing it raw closed the literal early or began a
    // bogus escape. An escaped quote came back unescaped, producing source that no longer parses.
    let source = r#"def main() -> None:
    quoted = b"a\"b"
    slashed = b"x\\y"
    println(f"{len(quoted)}{len(slashed)}")
"#;
    let once = formatted(source)?;
    assert!(once.contains(r#"b"a\"b""#), "the escaped quote was lost:\n{once}");
    assert!(once.contains(r#"b"x\\y""#), "the escaped backslash was lost:\n{once}");
    assert_eq!(once, formatted(&once)?, "formatting is not a fixed point:\n{once}");
    Ok(())
}

#[test]
fn a_guarded_match_arm_keeps_its_guard_and_still_parses() -> Result<(), String> {
    // `incan fmt` used to write the guard before an arrow -- `_ if n <= 0 => ...` -- which the parser then rejected,
    // so `examples/simple/fib.incn` was reformatted into a syntax error. The guard is no longer the arrow form's
    // problem: both spellings share one arm grammar, so it survives formatting either way (#1401).
    let source = "def fib(n: int) -> int:
    match n:
        case _ if n <= 0:
            return 0
        case _:
            return 1
";
    let once = formatted(source)?;
    assert!(
        once.contains("if n <= 0"),
        "the guard was dropped:
{once}"
    );
    // `formatted` parses its input, so a second pass succeeding is also proof the first pass stayed parseable.
    assert_eq!(
        once,
        formatted(&once)?,
        "formatting is not a fixed point:
{once}"
    );
    Ok(())
}

#[test]
fn an_arrow_arm_written_with_a_guard_round_trips() -> Result<(), String> {
    // The arrow spelling carries a guard of its own now, so a file written that way must survive formatting without
    // being rewritten into the other spelling.
    let source = "def classify(n: int) -> str:
    return match n:
        x if x < 0 => \"negative\"
        0 => \"zero\"
        _ => \"positive\"
";
    let once = formatted(source)?;
    assert!(
        once.contains("x if x < 0 =>"),
        "the guarded arrow arm was rewritten into another spelling:
{once}"
    );
    assert_eq!(
        once,
        formatted(&once)?,
        "formatting is not a fixed point:
{once}"
    );
    Ok(())
}

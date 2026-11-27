//! Formatter tests for RFC 081 descriptor-gated embedded fragments (#1022).
//!
//! RFC 081 gives the formatter exactly two modes and forbids a third fallback state: render from the structured
//! artifact, or preserve source verbatim for a DSL that declares itself layout-sensitive. These tests pin both
//! modes and the idempotency the structural mode has to hold.

use std::collections::HashMap;

use incan_syntax::lexer;

use crate::FormatConfig;
use crate::formatter::Formatter;

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
/// One embedded-fragment row: the descriptor that activates the fragment, a source, and its formatted output's checks.
struct FragmentCase {
    name: &'static str,
    keyword: &'static str,
    provider: &'static str,
    submode: incan_vocab::EmbeddedFragmentSubmode,
    layout_sensitive: bool,
    source: &'static str,
    /// Texts the formatted output contains.
    has: &'static [&'static str],
    /// Texts the formatted output does not contain.
    lacks: &'static [&'static str],
    /// Whether formatting the output again leaves it unchanged.
    stable: bool,
}

impl FragmentCase {
    fn run(&self) -> Result<(), String> {
        let (keyword_map, surface_map) = fixture_maps(self.keyword, self.provider, self.submode, self.layout_sensitive);
        let once = format_fixture(self.source, &keyword_map, &surface_map)?;
        check_output(&once, self.has, self.lacks)?;
        if self.stable {
            let twice = format_fixture(&once, &keyword_map, &surface_map)?;
            if once != twice {
                return Err(format!(
                    "second pass changed the output:\n--- once ---\n{once}\n--- twice ---\n{twice}"
                ));
            }
        }
        Ok(())
    }
}

fn check_output(output: &str, has: &[&str], lacks: &[&str]) -> Result<(), String> {
    if let Some(missing) = has.iter().find(|text| !output.contains(*text)) {
        return Err(format!("expected {missing:?} in:\n{output}"));
    }
    if let Some(present) = lacks.iter().find(|text| output.contains(*text)) {
        return Err(format!("expected no {present:?} in:\n{output}"));
    }
    Ok(())
}

fn report(kind: &str, failures: Vec<String>) {
    assert!(
        failures.is_empty(),
        "{} {kind} row(s) failed:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

use incan_vocab::EmbeddedFragmentSubmode::{Markup, RawText, RegexTemplate, Style};

const FRAGMENT_CASES: &[FragmentCase] = &[
    // A DSL that declares itself layout-sensitive owns its own whitespace, so the formatter must not restructure
    // it. This is one of RFC 081's two modes, and the descriptor is what selects it.
    FragmentCase {
        name: "a_layout_sensitive_descriptor_keeps_its_fragment_verbatim",
        keyword: "raw",
        provider: "textkit",
        submode: RawText,
        layout_sensitive: true,
        source: "import pub::textkit\n\ndef banner() -> None:\n    raw:\n        line one\n           deliberately indented\n",
        has: &["           deliberately indented"],
        lacks: &[],
        stable: false,
    },
    // The other mode: a descriptor that does not declare layout sensitivity gets rendered from the structural
    // tree. Ragged source indentation is layout, so it is re-derived rather than reproduced.
    FragmentCase {
        name: "a_structural_markup_fragment_is_formatted_from_its_nodes",
        keyword: "html",
        provider: "webkit",
        submode: Markup,
        layout_sensitive: false,
        source: "import pub::webkit\n\ndef render(title: str) -> None:\n    html:\n        <section class=\"card\">\n              <h1>{title}</h1>\n        </section>\n",
        has: &["<section class=\"card\">", "<h1>{title}</h1>"],
        lacks: &["              <h1>"],
        stable: false,
    },
    // Formatting has to be a fixed point: whitespace the first pass introduces becomes text nodes on re-parse, so
    // a renderer that reproduced them would drift further on every pass. This is the property that makes the
    // structural mode safe to run in `incan fmt --check`.
    FragmentCase {
        name: "structural_fragment_formatting_is_idempotent",
        keyword: "html",
        provider: "webkit",
        submode: Markup,
        layout_sensitive: false,
        source: "import pub::webkit\n\ndef render(title: str, body: str) -> None:\n    html:\n        <section>\n          <h1>{title}</h1>\n          <p>Body: {body}</p>\n        </section>\n",
        has: &[],
        lacks: &[],
        stable: true,
    },
    // Preserve holes and custom-property references while rendering style declarations to a parseable fixed point.
    // The style submode nests declarations under a rule rather than children under an element.
    FragmentCase {
        name: "a_structural_style_fragment_renders_rules_and_declarations",
        keyword: "css",
        provider: "styleforge",
        submode: Style,
        layout_sensitive: false,
        source: r"import pub::styleforge

def theme(accent: str) -> None:
    css:
        .card {
            color: {accent};
            background: var(--accent-color);
        }
",
        has: &[".card {", "color: {accent};", "background: var(--accent-color);"],
        lacks: &[],
        stable: true,
    },
    // The template's own delimiters must be reconstructed.
    FragmentCase {
        name: "a_regex_template_fragment_survives_a_formatting_round_trip",
        keyword: "script",
        provider: "scriptkit",
        submode: RegexTemplate,
        layout_sensitive: false,
        source: "import pub::scriptkit\n\ndef render(name: str) -> None:\n    script:\n        `hello ${name}!`\n",
        has: &["`hello ${name}!`"],
        lacks: &[],
        stable: true,
    },
    // A regex literal must render as itself, not as a template.
    FragmentCase {
        name: "a_regex_literal_fragment_survives_a_formatting_round_trip",
        keyword: "script",
        provider: "scriptkit",
        submode: RegexTemplate,
        layout_sensitive: false,
        source: "import pub::scriptkit\n\ndef render() -> None:\n    script:\n        /^[a-z]+$/gi\n",
        has: &["/^[a-z]+$/gi"],
        lacks: &[],
        stable: true,
    },
    // Preserved text must be a fixed point and survive, with and without layout sensitivity (this row and the next).
    FragmentCase {
        name: "preserved_fragment_text_does_not_grow_a_blank_line_per_pass",
        keyword: "note",
        provider: "textkit",
        submode: RawText,
        layout_sensitive: false,
        source: "import pub::textkit\n\ndef banner(who: str) -> None:\n    note:\n        TODO({who}): still to do\n",
        has: &["TODO({who}): still to do"],
        lacks: &[],
        stable: true,
    },
    FragmentCase {
        name: "preserved_fragment_text_does_not_grow_a_blank_line_per_pass_layout_sensitive",
        keyword: "note",
        provider: "textkit",
        submode: RawText,
        layout_sensitive: true,
        source: "import pub::textkit\n\ndef banner(who: str) -> None:\n    note:\n        TODO({who}): still to do\n",
        has: &["TODO({who}): still to do"],
        lacks: &[],
        stable: true,
    },
];

#[test]
fn embedded_fragment_cases() {
    report(
        "fragment",
        FRAGMENT_CASES
            .iter()
            .filter_map(|case| case.run().err().map(|problem| format!("{}: {problem}", case.name)))
            .collect(),
    );
}

/// A source formatted with the default configuration: texts its output keeps, and formatting it again is a fixed
/// point.
const PLAIN_CASES: &[(&str, &str, &[&str])] = &[
    (
        "an_enum_keeps_its_trait_adoption",
        "from std.traits import Display

enum Level with Display:
    Low
    High
",
        &["enum Level with Display:"],
    ),
    (
        "a_byte_literal_keeps_its_escapes",
        r#"def main() -> None:
    quoted = b"a\"b"
    slashed = b"x\\y"
    println(f"{len(quoted)}{len(slashed)}")
"#,
        &[r#"b"a\"b""#, r#"b"x\\y""#],
    ),
    (
        "a_string_literal_keeps_its_escapes_between_double_quotes",
        r#"def main() -> None:
    escape = "\u{1b}"
    letter = "\u{41}"
    quoted = 'it\'s "\u{1b}"'
    println(escape + letter + quoted + '\u{9}')
"#,
        &[
            r#"escape = "\u{1b}""#,
            r#"letter = "\u{41}""#,
            r#"quoted = "it's \"\u{1b}\"""#,
            r#"quoted + "\u{9}")"#,
        ],
    ),
    (
        "a_guarded_match_arm_keeps_its_guard_and_still_parses",
        "def fib(n: int) -> int:
    match n:
        case _ if n <= 0:
            return 0
        case _:
            return 1
",
        &["if n <= 0"],
    ),
    // The guarded arrow arm is not rewritten into another spelling.
    (
        "an_arrow_arm_written_with_a_guard_round_trips",
        "def classify(n: int) -> str:
    return match n:
        x if x < 0 => \"negative\"
        0 => \"zero\"
        _ => \"positive\"
",
        &["x if x < 0 =>"],
    ),
];

fn formatted(source: &str) -> Result<String, String> {
    crate::format_source(source).map_err(|error| error.to_string())
}

fn run_plain(source: &str, has: &[&str]) -> Result<(), String> {
    let once = formatted(source)?;
    check_output(&once, has, &[])?;
    if once != formatted(&once)? {
        return Err(format!("formatting is not a fixed point:\n{once}"));
    }
    Ok(())
}

#[test]
fn plain_source_cases() {
    report(
        "plain",
        PLAIN_CASES
            .iter()
            .filter_map(|(name, source, has)| run_plain(source, has).err().map(|problem| format!("{name}: {problem}")))
            .collect(),
    );
}

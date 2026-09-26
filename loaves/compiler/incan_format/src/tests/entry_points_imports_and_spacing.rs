//! The crate entry points and module-level layout: embedded fragments, invalid and empty input, the single trailing
//! newline, `format_source_with_config`, `check_formatted`, `format_diff`, parenthesized and `pub` import forms (#116,
//! #948), `rust::` imports with versions and their merging, top-level spacing between imports, consts, statics and
//! declarations, RFC 053 spacing under a custom config, and typed feature conditions.

use super::*;

/// Format one source file whose fragments are claimed by a `note:` raw-text descriptor.
///
/// The formatter has no descriptor map of its own, so an embedded fragment can only be formatted through a
/// parse that already carries one. This mirrors `format_source_with_query_vocab` for the RFC 081 surface.
fn format_source_with_note_fragment_vocab(source: &str) -> Result<String, FormatError> {
    let tokens = lexer::lex(source).map_err(|errs| {
        FormatError::SyntaxError(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n"))
    })?;
    let metadata = incan_vocab::VocabRegistration::new()
        .with_surface(
            incan_vocab::DslSurface::on_import("notekit")
                .with_declaration(incan_vocab::DeclarationSurface::named("note").with_statement_body())
                .with_embedded_fragment(
                    incan_vocab::EmbeddedFragmentDescriptor::new(
                        "note.fragment",
                        incan_vocab::EmbeddedFragmentSubmode::RawText,
                        "note_nodes",
                    )
                    .in_declaration_body("note"),
                ),
        )
        .metadata();
    let mut keyword_map = std::collections::HashMap::new();
    keyword_map.insert(
        "notekit".to_string(),
        vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "notekit".to_string(),
            },
            keywords: vec![incan_vocab::KeywordSpec::block("note")],
            valid_decorators: Vec::new(),
        }],
    );
    let mut surface_map = std::collections::HashMap::new();
    surface_map.insert("notekit".to_string(), metadata.dsl_surfaces);
    // `parse_with_source` is the only entrypoint that enables RFC 081 fragments: the parser needs the original
    // source to slice a claimed submode's raw byte range rather than reusing whatever the ordinary lexer made
    // of it. This mirrors what `incan fmt` does through `CompilationSession`.
    let ast =
        parser::parse_with_source(&tokens, None, Some(&keyword_map), Some(&surface_map), source).map_err(|errs| {
            let mut msg = String::new();
            for err in &errs {
                msg.push_str(&diagnostics::format_error("<input>", source, err));
            }
            FormatError::SyntaxError(msg)
        })?;
    format_parsed_source_with_config(source, &ast, FormatConfig::default())
}

/// An embedded fragment survives formatting byte-for-byte, and reformatting changes nothing further.
///
/// RFC 081 fragments are foreign syntax the formatter does not own, so its declared fallback is to reproduce
/// the preserved source text verbatim. Structural formatting for known fragments is #1022's work; until that
/// lands, the fallback is the contract, and it was previously untested. A regression here would silently
/// rewrite content the compiler explicitly refuses to interpret.
#[test]
fn embedded_fragment_formatting_preserves_its_source_and_is_idempotent() -> Result<(), FormatError> {
    let source = concat!(
        "import pub::notekit\n",
        "\n",
        "def deploy_note(owner: str) -> None:\n",
        "    note:\n",
        "        TODO({owner}): rotate   the  key before <<release>>\n",
    );

    let once = format_source_with_note_fragment_vocab(source)?;
    let twice = format_source_with_note_fragment_vocab(&once)?;
    assert_eq!(once, twice, "formatting an embedded fragment should be idempotent");

    // The irregular inner spacing is deliberate: it is exactly what a structural formatter would normalize,
    // and exactly what the verbatim fallback must not touch.
    assert!(
        once.contains("TODO({owner}): rotate   the  key before <<release>>"),
        "the fragment's source text must survive verbatim, got:\n{once}"
    );
    Ok(())
}

#[test]
fn test_format_source_invalid_syntax() {
    let source = "def foo(";
    let result = format_source(source);
    assert!(result.is_err());
}

#[test]
fn test_format_source_empty() {
    let source = "";
    let result = format_source(source);
    assert!(result.is_ok());
}

/// Regression (GitHub #189): declarations already end with a newline; an extra `newline()` at EOF produced `\n\n`.
#[test]
fn test_format_source_eof_has_single_trailing_newline_only() -> Result<(), FormatError> {
    let source = r#"def f() -> int:
    return 1


"#;
    let formatted = format_source(source)?;
    let trailing_nl = formatted.chars().rev().take_while(|c| *c == '\n').count();
    assert_eq!(
        trailing_nl, 1,
        "expected exactly one trailing newline at EOF; got {trailing_nl}: {formatted:?}"
    );
    Ok(())
}

#[test]
fn test_format_source_with_custom_config() {
    let source = r#"def foo() -> int:
  return 42
"#;
    let config = FormatConfig::new().with_indent_width(2);
    let result = format_source_with_config(source, config);
    assert!(result.is_ok());
}

#[test]
fn test_format_source_with_different_line_length() {
    let source = r#"def foo() -> int:
  return 42
"#;
    let config = FormatConfig::new().with_line_length(80);
    let result = format_source_with_config(source, config);
    assert!(result.is_ok());
}

#[test]
fn test_check_formatted_simple() {
    let source = r#"def foo() -> int:
    return 42
"#;
    let result = check_formatted(source);
    assert!(result.is_ok());
}

#[test]
fn test_check_formatted_invalid_syntax() {
    let source = "def foo(";
    let result = check_formatted(source);
    assert!(result.is_err());
}

#[test]
fn test_format_diff_no_changes() {
    let source = r#"def foo() -> int:
    return 42
"#;
    let result = format_diff(source);
    // May have no changes if already formatted, or may have changes
    assert!(result.is_ok());
}

#[test]
fn test_format_diff_invalid_syntax() {
    let source = "def foo(";
    let result = format_diff(source);
    assert!(result.is_err());
}

#[test]
fn test_format_diff_returns_diff() {
    // Improperly indented source
    let source = r#"def foo() -> int:
 return 42
"#;
    let result = format_diff(source);
    assert!(result.is_ok());
    // The diff may or may not be Some depending on formatter behavior
}

#[test]
fn test_format_diff_trailing_newline_only_is_actionable() -> Result<(), FormatError> {
    let source = "def foo() -> int:\n    return 42";
    let result = format_diff(source)?;
    let diff = result
        .ok_or_else(|| FormatError::SyntaxError("diff should be present for trailing-newline change".to_string()))?;
    assert!(
        diff.contains("trailing-newline"),
        "expected trailing newline hint in diff, got: {diff}"
    );
    Ok(())
}

/// A short import that fits on one line should be kept (or collapsed to) single-line form.
#[test]
fn test_format_import_short_stays_single_line() -> Result<(), FormatError> {
    let source = "from db import (CategoryId, TagId)\n";
    let config = FormatConfig::new().with_line_length(120);
    let result = format_source_with_config(source, config)?;
    assert_eq!(result.trim_end(), "from db import CategoryId, TagId");
    Ok(())
}

/// A comma-separated import that already fits on one line is unchanged.
#[test]
fn test_format_import_bare_short_unchanged() -> Result<(), FormatError> {
    let source = "from db import CategoryId, TagId\n";
    let config = FormatConfig::new().with_line_length(120);
    let result = format_source_with_config(source, config)?;
    assert_eq!(result.trim_end(), "from db import CategoryId, TagId");
    Ok(())
}

/// A long multi-item import that exceeds the line length should be wrapped.
#[test]
fn test_format_import_long_wraps_to_parens() -> Result<(), FormatError> {
    // Use a very short limit so the list definitely overflows.
    let source = "from db import CategoryId, TagId, OtherId\n";
    let config = FormatConfig::new().with_line_length(20).with_trailing_commas(true);
    let result = format_source_with_config(source, config)?;
    assert!(
        result.contains('('),
        "expected parenthesized output for long import; got: {result}"
    );
    assert!(
        result.contains("CategoryId,\n"),
        "expected each item on its own line; got: {result}"
    );
    Ok(())
}

/// A multi-line parenthesized import that fits on one line is collapsed to single-line.
#[test]
fn test_format_import_multiline_parens_collapses_when_fits() -> Result<(), FormatError> {
    let source = "from db import (\n    CategoryId,\n    TagId,\n)\n";
    let config = FormatConfig::new().with_line_length(120);
    let result = format_source_with_config(source, config)?;
    assert_eq!(result.trim_end(), "from db import CategoryId, TagId");
    Ok(())
}

/// Trailing comma in parenthesized output is controlled by the `trailing_commas` config.
#[test]
fn test_format_import_no_trailing_comma_when_disabled() -> Result<(), FormatError> {
    let source = "from db import CategoryId, TagId, OtherId\n";
    let config = FormatConfig::new().with_line_length(20).with_trailing_commas(false);
    let result = format_source_with_config(source, config)?;
    // Last item should not have a trailing comma.
    assert!(
        !result.contains("OtherId,\n"),
        "expected no trailing comma after last item; got: {result}"
    );
    assert!(
        result.contains("OtherId\n"),
        "expected last item without comma; got: {result}"
    );
    Ok(())
}

#[test]
fn test_format_pub_library_import_round_trip() -> Result<(), FormatError> {
    let source = "import pub::mylib as lib\n";
    let formatted = format_source(source)?;
    assert_eq!(formatted.trim_end(), source.trim_end());
    Ok(())
}

#[test]
fn test_format_pub_from_import_collapses_parenthesized_list() -> Result<(), FormatError> {
    let source = "from pub::mylib import (\n    Widget,\n    make_widget as build_widget,\n)\n";
    let config = FormatConfig::new().with_line_length(120);
    let formatted = format_source_with_config(source, config)?;
    assert_eq!(
        formatted.trim_end(),
        "from pub::mylib import Widget, make_widget as build_widget"
    );
    Ok(())
}

#[test]
fn test_format_nested_pub_import_paths_canonicalize_to_dots_issue948() -> Result<(), FormatError> {
    let source = "import pub::mylib::hyperquant::index as h\nfrom pub::mylib::hyperquant::search import find\n";
    let formatted = format_source(source)?;
    assert_eq!(
        formatted,
        "import pub::mylib.hyperquant.index as h\nfrom pub::mylib.hyperquant.search import find\n"
    );
    assert_eq!(format_source(&formatted)?, formatted);
    Ok(())
}

#[test]
fn test_format_top_level_spacing_imports_consts_and_function() -> Result<(), FormatError> {
    let source = r#"from rust::std::f64::consts import PI, E
from rust::std::f64 import INFINITY, NAN
const A: int = 1
const B: int = 2
def sum_constants() -> int:
  return A + B
"#;
    let result = format_source(source)?;

    let expected = r#"from rust::std::f64::consts import PI, E
from rust::std::f64 import INFINITY, NAN

const A: int = 1
const B: int = 2


def sum_constants() -> int:
    return A + B
"#;
    assert_eq!(result, expected);
    Ok(())
}

#[test]
fn test_format_top_level_spacing_single_line_alias_then_body_bearing_decl() -> Result<(), FormatError> {
    let source = r#"type UserId = str
model User:
  id: UserId

def load_user(id: UserId) -> User:
  pass
"#;
    let result = format_source(source)?;

    let expected = r#"type UserId = str


model User:
    id: UserId


def load_user(id: UserId) -> User:
    pass
"#;
    assert_eq!(result, expected);
    Ok(())
}

#[test]
fn test_format_top_level_spacing_static_then_function_uses_two_blank_lines() -> Result<(), FormatError> {
    let source = r#"static prism_store_node_counts: list[int] = []
pub def allocate_prism_store_id() -> int:
  return len(prism_store_node_counts)
"#;
    let result = format_source(source)?;

    let expected = r#"static prism_store_node_counts: list[int] = []


pub def allocate_prism_store_id() -> int:
    return len(prism_store_node_counts)
"#;
    assert_eq!(result, expected);
    Ok(())
}

#[test]
fn test_format_source_with_custom_config_keeps_rfc053_spacing() -> Result<(), FormatError> {
    let source = r#"trait User:
  def connect(self) -> None: ...
  def reset(self) -> None:
    pass

def build_user() -> User:
  pass
"#;
    let config = FormatConfig::new().with_indent_width(2);
    let formatted = format_source_with_config(source, config)?;

    let expected = r#"trait User:
  def connect(self) -> None

  def reset(self) -> None:
    pass


def build_user() -> User:
  pass
"#;
    assert_eq!(formatted, expected);
    Ok(())
}

#[test]
fn test_format_rust_from_import_with_version_wraps_black_style() -> Result<(), FormatError> {
    let source = r#"from rust::libm @ "0.2" import sqrt as rust_sqrt, fabs as rust_abs, floor as rust_floor, ceil as rust_ceil, pow as rust_pow, exp as rust_exp
"#;
    let config = FormatConfig::new().with_line_length(80).with_trailing_commas(true);
    let result = format_source_with_config(source, config)?;

    assert!(
        result.starts_with("from rust::libm @ \"0.2\" import (\n"),
        "expected parenthesized rust import list; got: {result}"
    );
    assert!(
        result.contains("sqrt as rust_sqrt,\n") && result.contains("pow as rust_pow,\n"),
        "expected one item per line with trailing commas; got: {result}"
    );
    Ok(())
}

#[test]
fn test_format_merges_adjacent_rust_from_imports_same_target() -> Result<(), FormatError> {
    let source = r#"from rust::libm @ "0.2" import sqrt as rust_sqrt, fabs as rust_abs
from rust::libm @ "0.2" import floor as rust_floor, ceil as rust_ceil
from rust::libm @ "0.2" import pow as rust_pow, exp as rust_exp
"#;
    let config = FormatConfig::new().with_line_length(80).with_trailing_commas(true);
    let result = format_source_with_config(source, config)?;

    let import_prefix = "from rust::libm @ \"0.2\" import";
    assert_eq!(
        result.matches(import_prefix).count(),
        1,
        "expected adjacent compatible rust imports to merge; got: {result}"
    );
    assert!(
        result.contains("sqrt as rust_sqrt,\n")
            && result.contains("floor as rust_floor,\n")
            && result.contains("pow as rust_pow,\n"),
        "expected all merged import items present in wrapped output; got: {result}"
    );
    Ok(())
}

#[test]
fn test_format_source_feature_conditions_are_typed_and_idempotent() -> Result<(), FormatError> {
    let source = r#"when feature("pretty") and feature("json"):
  from std.json import JsonValue
  pub def render(value: JsonValue) -> str:
    return "json"

pub def always() -> str:
  return "always"
"#;
    let expected = r#"when feature("json") and feature("pretty"):
    from std.json import JsonValue

    pub def render(value: JsonValue) -> str:
        return "json"


pub def always() -> str:
    return "always"
"#;

    let formatted = format_source(source)?;
    assert_eq!(formatted, expected);
    assert_eq!(format_source(&formatted)?, expected);
    Ok(())
}

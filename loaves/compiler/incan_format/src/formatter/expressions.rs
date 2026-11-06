//! Expression formatting: expressions, literals, operators, patterns, match arms, and types.

use incan_lang::lang::keywords;
use incan_semantics_core::SurfaceFeatureKey;
use incan_syntax::ast::*;

use super::Formatter;

mod calls_and_chains;
mod expression_forms;
mod literals_patterns_types;

struct MethodChainSegment<'a> {
    method: &'a str,
    type_args: &'a [Spanned<Type>],
    args: &'a [CallArg],
}

/// Escape special characters in a string.
fn escape_string(s: &str) -> String {
    let mut result = String::new();
    for c in s.chars() {
        match c {
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            '\\' => result.push_str("\\\\"),
            '"' => result.push_str("\\\""),
            c => result.push(c),
        }
    }
    result
}

/// Escape an f-string literal segment so formatter output stays parseable and semantically stable.
///
/// Unlike ordinary strings, `{` and `}` are control characters in f-strings and must be doubled when emitted as
/// literal text. We also preserve escaped control characters (`\n`, `\r`, `\t`) as textual escapes instead of
/// materializing physical whitespace in formatter output.
fn escape_fstring_literal(s: &str) -> String {
    let mut result = String::new();
    for c in s.chars() {
        match c {
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            '\\' => result.push_str("\\\\"),
            '"' => result.push_str("\\\""),
            '{' => result.push_str("{{"),
            '}' => result.push_str("}}"),
            c => result.push(c),
        }
    }
    result
}

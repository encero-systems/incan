//! Declaration formatting: imports, models, classes, traits, enums, newtypes, functions, methods, properties,
//! decorators, fields, params, and type params.

use incan_syntax::ast::*;

use super::{Formatter, RFC053_METHOD_BLANK_LINES};

mod callables;
mod top_level;
mod type_declarations;

/// Escape a value enum string literal for formatter round-tripping.
fn escape_value_enum_string(s: &str) -> String {
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

/// Remove at most the common docstring indentation while respecting UTF-8 boundaries.
fn strip_common_indent(line: &str, indent: usize) -> &str {
    let mut chars_to_strip = indent;
    let mut start = 0usize;

    for (idx, ch) in line.char_indices() {
        if chars_to_strip == 0 {
            start = idx;
            break;
        }

        if ch.is_whitespace() {
            chars_to_strip -= 1;
            start = idx + ch.len_utf8();
            continue;
        }

        start = idx;
        break;
    }

    &line[start..]
}

/// Return normalized docstring body lines.
fn normalized_docstring_lines(doc: &str) -> Vec<String> {
    let lines: Vec<&str> = doc.lines().collect();
    let first = lines.first().map(|line| line.trim()).unwrap_or_default();
    let rest = lines.get(1..).unwrap_or_default();
    let common_indent = rest
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.chars().take_while(|ch| ch.is_whitespace()).count())
        .min()
        .unwrap_or(0);

    let mut normalized = Vec::new();
    normalized.push(first.to_string());
    let mut previous_blank = false;
    for line in rest {
        if line.trim().is_empty() {
            if !previous_blank {
                normalized.push(String::new());
                previous_blank = true;
            }
        } else {
            normalized.push(strip_common_indent(line, common_indent).trim_end().to_string());
            previous_blank = false;
        }
    }
    normalized
}

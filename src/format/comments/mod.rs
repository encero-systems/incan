//! Formatter-internal comment preservation pipeline.

pub(super) mod buffer;
mod model;
mod reattach;
mod scanner;

/// Reattach scanned comments to the formatted syntax tree.
pub(super) fn reattach_comments(source: &str, formatted: &str) -> String {
    reattach::reattach_comments(source, formatted)
}

/// Count line comments in formatted source text.
pub(super) fn count_line_comments(source: &str) -> usize {
    scanner::count_line_comments(source)
}

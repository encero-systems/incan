//! Formatter-internal comment preservation pipeline.

pub(super) mod buffer;
mod model;
mod reattach;
mod scanner;

pub(super) fn reattach_comments(source: &str, formatted: &str) -> String {
    reattach::reattach_comments(source, formatted)
}

pub(super) fn count_line_comments(source: &str) -> usize {
    scanner::count_line_comments(source)
}

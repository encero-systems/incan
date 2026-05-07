//! Normalize blank-line runs during comment reattachment emission.

use super::scanner::{StringState, comment_start_index, reset_single_line_string_state};

/// Count leading indentation using source whitespace width rather than byte length.
pub(in crate::format) fn leading_indent_width(line: &str) -> usize {
    line.chars().take_while(|ch| ch.is_whitespace()).count()
}

/// Line sink that enforces formatter-wide blank-line normalization as lines are emitted.
///
/// Blank runs are capped by the indentation scope of the following code line: root-level code may have two blank
/// lines, while indented code may have only one. Blank lines inside triple-quoted strings are preserved verbatim. This
/// keeps newline policy in the comment reattachment output path instead of requiring a second cleanup pass afterward.
pub(in crate::format) struct NormalizedLineBuffer {
    lines: Vec<String>,
    state: StringState,
    pending_blank_lines: usize,
}

impl NormalizedLineBuffer {
    pub(in crate::format) fn new() -> Self {
        Self {
            lines: Vec::new(),
            state: StringState::None,
            pending_blank_lines: 0,
        }
    }

    /// Push one already-formatted line, dropping excess blank lines outside triple-quoted strings.
    pub(in crate::format) fn push_line(&mut self, line: String) {
        let inside_multiline_string = matches!(
            self.state,
            StringState::TripleSingleQuoted | StringState::TripleDoubleQuoted
        );
        let is_blank = line.trim().is_empty();

        if is_blank {
            if inside_multiline_string {
                self.lines.push(line);
            } else {
                self.pending_blank_lines += 1;
            }
            return;
        }

        if !inside_multiline_string {
            let max_blank_lines = if leading_indent_width(&line) == 0 { 2 } else { 1 };
            let blank_lines = self.pending_blank_lines.min(max_blank_lines);
            self.lines.extend(std::iter::repeat_with(String::new).take(blank_lines));
            self.pending_blank_lines = 0;
        }

        let _ = comment_start_index(&line, &mut self.state);
        reset_single_line_string_state(&mut self.state);
        self.lines.push(line);
    }

    /// Preserve a user-authored readability gap before an indented comment block.
    pub(in crate::format) fn ensure_blank_line_before(&mut self, indent: usize) {
        if indent > 0 && self.pending_blank_lines == 0 && self.lines.last().is_some_and(|line| !line.is_empty()) {
            self.pending_blank_lines = 1;
        }
    }

    pub(in crate::format) fn ends_with_nonblank_line(&self) -> bool {
        self.lines.last().is_some_and(|line| !line.is_empty())
    }

    /// Join buffered lines after removing trailing blank lines, then restore one final newline when requested.
    pub(in crate::format) fn finish(self, trailing_newline: bool) -> String {
        let mut lines = self.lines;
        while lines.last().is_some_and(|line| line.trim().is_empty()) {
            lines.pop();
        }

        let mut out = lines.join("\n");
        if trailing_newline {
            out.push('\n');
        }
        out
    }
}

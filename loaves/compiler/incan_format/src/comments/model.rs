//! Extract source comments into stable leading, trailing, inline, and EOF attachment buckets.

use std::collections::HashMap;

use super::buffer::leading_indent_width;
use super::scanner::{StringState, comment_start_index, reset_single_line_string_state};
use super::stream::normalize_code_for_stream;

/// Stand-alone comment lines that are anchored to a nearby formatted code line.
#[derive(Clone)]
pub(super) struct AnchoredStandaloneBlock {
    pub anchor: String,
    pub occurrence: usize,
    /// The anchor line's code in the normalized stream (see `stream`).
    pub code: StreamCode,
    pub indent: usize,
    pub lines: Vec<String>,
    /// Source gap before the block, capped when emitted at its indentation scope.
    pub blank_lines_before: usize,
    /// A root-level block following a suite opens the next top-level section and waits for the suite to end.
    pub after_root_suite: bool,
    /// Source gap after a leading block or a trailing root-level suite block.
    pub blank_lines_after: usize,
}

#[derive(Clone)]
pub(super) struct InlineComment {
    pub anchor: String,
    pub occurrence: usize,
    /// The code before the comment, in the normalized stream.
    pub code: StreamCode,
    pub text: String,
}

/// Where one source line's code sits in the normalized stream that formatting preserves.
#[derive(Clone, Copy)]
pub(super) struct StreamCode {
    /// Offset of the line's first stream byte.
    pub offset: usize,
    /// Number of stream bytes; zero for a line such as a lone `)`.
    pub len: usize,
}

impl StreamCode {
    /// Offset just past the line's last stream byte.
    pub fn end(self) -> usize {
        self.offset + self.len
    }
}

pub(super) struct ExtractedComments {
    pub leading_standalone: Vec<AnchoredStandaloneBlock>,
    pub trailing_standalone: Vec<AnchoredStandaloneBlock>,
    pub eof_standalone: Vec<String>,
    pub inline_comments: Vec<InlineComment>,
    /// The source's code, normalized as `stream::normalize_code_for_stream` does, in source order.
    pub stream: String,
}

/// A source code line that a comment block can attach to.
#[derive(Clone)]
struct CodeAnchor {
    text: String,
    occurrence: usize,
    indent: usize,
    code: StreamCode,
}

/// Stand-alone comment lines collected while scanning the source before their anchor is known.
struct PendingStandaloneBlock {
    indent: usize,
    lines: Vec<String>,
    saw_blank_after: bool,
    blank_lines_before: usize,
    blank_lines_after: usize,
}

impl PendingStandaloneBlock {
    /// Start a comment block with its source indentation and preceding readability gap.
    fn new(line: &str, indent: usize, blank_lines_before: usize) -> Self {
        Self {
            indent,
            lines: vec![line.trim_end().to_string()],
            saw_blank_after: false,
            blank_lines_before,
            blank_lines_after: 0,
        }
    }
}

/// The scanner's running state: comments sorted so far, the code stream, and the code anchors seen per indentation.
#[derive(Default)]
struct Extraction {
    leading_standalone: Vec<AnchoredStandaloneBlock>,
    trailing_standalone: Vec<AnchoredStandaloneBlock>,
    eof_standalone: Vec<String>,
    inline_comments: Vec<InlineComment>,
    stream: String,
    source_anchor_occurrences: HashMap<String, usize>,
    last_code_at_indent: HashMap<usize, CodeAnchor>,
    /// Separated comment blocks still waiting for their first same-scope code anchor.
    unanchored_standalone: Vec<PendingStandaloneBlock>,
}

impl Extraction {
    /// Record one source code line and offer its anchor to preceding blocks that have no same-scope code yet.
    fn code_line(&mut self, line: &str, code: &str) -> CodeAnchor {
        let text = normalize_code_for_match(code);
        let occurrence = self.source_anchor_occurrences.get(&text).copied().unwrap_or(0) + 1;
        let normalized = normalize_code_for_stream(code);
        let offset = self.stream.len();
        self.stream.push_str(&normalized);
        let anchor = CodeAnchor {
            text,
            occurrence,
            indent: leading_indent_width(line),
            code: StreamCode {
                offset,
                len: normalized.len(),
            },
        };
        for block in std::mem::take(&mut self.unanchored_standalone) {
            finalize_pending_standalone_block(block, Some(&anchor), self);
        }
        anchor
    }

    /// Make `anchor` the latest code at its indentation, after any pending comment block has attached to it.
    fn remember(&mut self, anchor: CodeAnchor) {
        self.source_anchor_occurrences
            .insert(anchor.text.clone(), anchor.occurrence);
        self.last_code_at_indent.insert(anchor.indent, anchor);
    }

    /// Return the sorted comments and the code stream.
    fn finish(mut self) -> ExtractedComments {
        for block in self.unanchored_standalone {
            if block.blank_lines_before > 0 && !self.eof_standalone.is_empty() {
                self.eof_standalone.push(String::new());
            }
            self.eof_standalone.extend(block.lines);
        }
        ExtractedComments {
            leading_standalone: self.leading_standalone,
            trailing_standalone: self.trailing_standalone,
            eof_standalone: self.eof_standalone,
            inline_comments: self.inline_comments,
            stream: self.stream,
        }
    }
}

/// Normalize source text before matching comments back to code.
pub(super) fn normalize_code_for_match(code: &str) -> String {
    code.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Collect same-scope comment blocks and inline comments from `source` into attachment-friendly buckets.
pub(super) fn extract_comments(source: &str) -> ExtractedComments {
    let mut state = StringState::None;
    let mut pending_standalone: Option<PendingStandaloneBlock> = None;
    let mut extraction = Extraction::default();
    let mut preceding_blank_lines = 0usize;

    for line in source.lines() {
        let comment_idx = comment_start_index(line, &mut state);
        reset_single_line_string_state(&mut state);

        let Some(idx) = comment_idx else {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                if let Some(block) = &mut pending_standalone {
                    block.saw_blank_after = true;
                    block.blank_lines_after += 1;
                }
                preceding_blank_lines += 1;
                continue;
            }

            let anchor = extraction.code_line(line, trimmed);
            if let Some(block) = pending_standalone.take() {
                finalize_pending_standalone_block(block, Some(&anchor), &mut extraction);
            }
            extraction.remember(anchor);
            preceding_blank_lines = 0;
            continue;
        };

        let code_prefix = &line[..idx];
        let comment_text = line[idx..].trim_end().to_string();
        if code_prefix.trim().is_empty() {
            let indent = leading_indent_width(line);
            if let Some(block) = &mut pending_standalone {
                if !block.saw_blank_after && block.indent == indent {
                    block.lines.push(line.trim_end().to_string());
                } else {
                    let old_block =
                        std::mem::replace(block, PendingStandaloneBlock::new(line, indent, preceding_blank_lines));
                    finalize_pending_standalone_block(old_block, None, &mut extraction);
                }
            } else {
                pending_standalone = Some(PendingStandaloneBlock::new(line, indent, preceding_blank_lines));
            }
            preceding_blank_lines = 0;
            continue;
        }

        let anchor = extraction.code_line(line, code_prefix.trim_end());
        if let Some(block) = pending_standalone.take() {
            finalize_pending_standalone_block(block, Some(&anchor), &mut extraction);
        }

        extraction.inline_comments.push(InlineComment {
            anchor: anchor.text.clone(),
            occurrence: anchor.occurrence,
            code: anchor.code,
            text: comment_text,
        });
        extraction.remember(anchor);
        preceding_blank_lines = 0;
    }

    if let Some(block) = pending_standalone.take() {
        finalize_pending_standalone_block(block, None, &mut extraction);
    }

    extraction.finish()
}

/// Attach a pending source comment block to its nearest stable formatted location.
///
/// Blocks immediately followed by same-indent code attach forward, as do separated blocks with no previous same-indent
/// code. Other separated blocks attach to the previous same-indent code line, with EOF as the fallback when no stable
/// anchor exists. A trailing root-level suite block retains both source gaps and must wait for the suite to end, rather
/// than treating its internal blank lines as attachment boundaries.
/// Blocks without any same-indent anchor wait for later code and fall back to EOF only when scanning ends.
fn finalize_pending_standalone_block(
    block: PendingStandaloneBlock,
    next_anchor: Option<&CodeAnchor>,
    extraction: &mut Extraction,
) {
    let lines = trim_trailing_blank_comment_lines(&block.lines);
    match next_anchor {
        Some(next)
            if next.indent == block.indent
                && (!block.saw_blank_after || !extraction.last_code_at_indent.contains_key(&block.indent)) =>
        {
            let after_root_suite = block.indent == 0
                && extraction
                    .last_code_at_indent
                    .get(&0)
                    .is_some_and(|previous| previous.text.ends_with(':'));
            extraction.leading_standalone.push(AnchoredStandaloneBlock {
                anchor: next.text.clone(),
                occurrence: next.occurrence,
                code: next.code,
                indent: block.indent,
                lines,
                blank_lines_before: block.blank_lines_before,
                after_root_suite,
                blank_lines_after: block.blank_lines_after,
            });
        }
        _ => {
            if let Some(previous) = extraction.last_code_at_indent.get(&block.indent) {
                let after_root_suite = block.indent == 0 && previous.text.ends_with(':');
                let block = AnchoredStandaloneBlock {
                    anchor: previous.text.clone(),
                    occurrence: previous.occurrence,
                    code: previous.code,
                    indent: block.indent,
                    lines,
                    blank_lines_before: block.blank_lines_before,
                    after_root_suite,
                    blank_lines_after: if after_root_suite { block.blank_lines_after } else { 0 },
                };
                extraction.trailing_standalone.push(block);
            } else {
                extraction.unanchored_standalone.push(block);
            }
        }
    }
}

/// Trim blank comment lines from the end of a comment block.
fn trim_trailing_blank_comment_lines(lines: &[String]) -> Vec<String> {
    let mut out = lines.to_vec();
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out
}

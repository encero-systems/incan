//! Extract source comments into stable leading, trailing, inline, and EOF attachment buckets.

use std::collections::HashMap;

use super::buffer::leading_indent_width;
use super::scanner::{StringState, comment_start_index, reset_single_line_string_state};

/// Stand-alone comment lines that are anchored to a nearby formatted code line.
#[derive(Clone)]
pub(super) struct AnchoredStandaloneBlock {
    pub anchor: String,
    pub occurrence: usize,
    pub indent: usize,
    pub lines: Vec<String>,
    pub blank_line_before: bool,
}

#[derive(Clone)]
pub(super) struct InlineComment {
    pub anchor: String,
    pub occurrence: usize,
    pub text: String,
}

pub(super) struct ExtractedComments {
    pub leading_standalone: Vec<AnchoredStandaloneBlock>,
    pub trailing_standalone: Vec<AnchoredStandaloneBlock>,
    pub eof_standalone: Vec<String>,
    pub inline_comments: Vec<InlineComment>,
}

/// Stand-alone comment lines collected while scanning the source before their anchor is known.
struct PendingStandaloneBlock {
    indent: usize,
    lines: Vec<String>,
    saw_blank_after: bool,
    saw_blank_before: bool,
}

/// Normalize source text before matching comments back to code.
pub(super) fn normalize_code_for_match(code: &str) -> String {
    code.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Collect same-scope comment blocks and inline comments from `source` into attachment-friendly buckets.
pub(super) fn extract_comments(source: &str) -> ExtractedComments {
    let mut state = StringState::None;
    let mut pending_standalone: Option<PendingStandaloneBlock> = None;
    let mut leading_standalone: Vec<AnchoredStandaloneBlock> = Vec::new();
    let mut trailing_standalone: Vec<AnchoredStandaloneBlock> = Vec::new();
    let mut eof_standalone: Vec<String> = Vec::new();
    let mut inline_comments: Vec<InlineComment> = Vec::new();
    let mut source_anchor_occurrences: HashMap<String, usize> = HashMap::new();
    let mut last_code_at_indent: HashMap<usize, (String, usize)> = HashMap::new();
    let mut last_source_line_was_blank = false;

    for line in source.lines() {
        let comment_idx = comment_start_index(line, &mut state);
        reset_single_line_string_state(&mut state);

        let Some(idx) = comment_idx else {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                if let Some(block) = &mut pending_standalone {
                    block.saw_blank_after = true;
                }
                last_source_line_was_blank = true;
                continue;
            }

            let anchor = normalize_code_for_match(trimmed);
            let occurrence = source_anchor_occurrences.get(&anchor).copied().unwrap_or(0) + 1;
            let indent = leading_indent_width(line);
            if let Some(block) = pending_standalone.take() {
                finalize_pending_standalone_block(
                    block,
                    Some((anchor.clone(), occurrence, indent)),
                    &last_code_at_indent,
                    &mut leading_standalone,
                    &mut trailing_standalone,
                    &mut eof_standalone,
                );
            }
            last_code_at_indent.insert(indent, (anchor.clone(), occurrence));
            source_anchor_occurrences.insert(anchor, occurrence);
            last_source_line_was_blank = false;
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
                    let old_block = std::mem::replace(
                        block,
                        PendingStandaloneBlock {
                            indent,
                            lines: vec![line.trim_end().to_string()],
                            saw_blank_after: false,
                            saw_blank_before: last_source_line_was_blank,
                        },
                    );
                    finalize_pending_standalone_block(
                        old_block,
                        None,
                        &last_code_at_indent,
                        &mut leading_standalone,
                        &mut trailing_standalone,
                        &mut eof_standalone,
                    );
                }
            } else {
                pending_standalone = Some(PendingStandaloneBlock {
                    indent,
                    lines: vec![line.trim_end().to_string()],
                    saw_blank_after: false,
                    saw_blank_before: last_source_line_was_blank,
                });
            }
            last_source_line_was_blank = false;
            continue;
        }

        let anchor = normalize_code_for_match(code_prefix.trim_end());
        let occurrence = source_anchor_occurrences.get(&anchor).copied().unwrap_or(0) + 1;
        let indent = leading_indent_width(line);
        if let Some(block) = pending_standalone.take() {
            finalize_pending_standalone_block(
                block,
                Some((anchor.clone(), occurrence, indent)),
                &last_code_at_indent,
                &mut leading_standalone,
                &mut trailing_standalone,
                &mut eof_standalone,
            );
        }

        inline_comments.push(InlineComment {
            anchor: anchor.clone(),
            occurrence,
            text: comment_text,
        });
        last_code_at_indent.insert(indent, (anchor.clone(), occurrence));
        source_anchor_occurrences.insert(anchor, occurrence);
        last_source_line_was_blank = false;
    }

    if let Some(block) = pending_standalone.take() {
        finalize_pending_standalone_block(
            block,
            None,
            &last_code_at_indent,
            &mut leading_standalone,
            &mut trailing_standalone,
            &mut eof_standalone,
        );
    }

    ExtractedComments {
        leading_standalone,
        trailing_standalone,
        eof_standalone,
        inline_comments,
    }
}

/// Attach a pending source comment block to its nearest stable formatted location.
///
/// Blocks that are immediately followed by same-indent code become leading comments for that next code line. Blocks
/// separated by a blank line, or followed by different indentation, attach to the previous code line at the same
/// indentation. Blocks without a stable leading or trailing anchor use the EOF fallback path.
fn finalize_pending_standalone_block(
    block: PendingStandaloneBlock,
    next_anchor: Option<(String, usize, usize)>,
    last_code_at_indent: &HashMap<usize, (String, usize)>,
    leading_standalone: &mut Vec<AnchoredStandaloneBlock>,
    trailing_standalone: &mut Vec<AnchoredStandaloneBlock>,
    eof_standalone: &mut Vec<String>,
) {
    let lines = trim_trailing_blank_comment_lines(&block.lines);
    match next_anchor {
        Some((anchor, occurrence, next_indent)) if !block.saw_blank_after && next_indent == block.indent => {
            leading_standalone.push(AnchoredStandaloneBlock {
                anchor,
                occurrence,
                indent: block.indent,
                lines,
                blank_line_before: block.saw_blank_before,
            });
        }
        _ => {
            if let Some((prev_anchor, prev_occurrence)) = last_code_at_indent.get(&block.indent) {
                trailing_standalone.push(AnchoredStandaloneBlock {
                    anchor: prev_anchor.clone(),
                    occurrence: *prev_occurrence,
                    indent: block.indent,
                    lines,
                    blank_line_before: block.saw_blank_before,
                });
            } else {
                eof_standalone.extend(lines);
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

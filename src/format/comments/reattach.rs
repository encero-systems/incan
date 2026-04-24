//! Reattach extracted comment bundles onto the formatter's AST-shaped output.

use std::collections::{HashMap, VecDeque};

use super::buffer::{NormalizedLineBuffer, leading_indent_width};
use super::model::{AnchoredStandaloneBlock, InlineComment, extract_comments, normalize_code_for_match};
use super::scanner::{StringState, comment_start_index, reset_single_line_string_state};

/// Reattach preserved source comments onto the formatter's AST-shaped output.
///
/// The formatter itself only emits comments that are represented in the AST. This pass preserves stand-alone and
/// inline `# ...` comments from the original source by anchoring them to nearby formatted code lines while respecting
/// indentation scope, blank-line separation, and string-literal boundaries.
pub(super) fn reattach_comments(source: &str, formatted: &str) -> String {
    let extracted = extract_comments(source);
    let mut out_lines = NormalizedLineBuffer::new();
    let mut leading_idx = 0usize;
    let mut trailing_idx = 0usize;
    let mut inline_idx = 0usize;
    let mut formatted_state = StringState::None;
    let mut formatted_anchor_occurrences: HashMap<String, usize> = HashMap::new();
    let mut pending_trailing: VecDeque<AnchoredStandaloneBlock> = VecDeque::new();

    for line in formatted.lines() {
        let line_trimmed = line.trim();
        let current_indent = leading_indent_width(line);
        flush_ready_trailing_blocks(&mut out_lines, &mut pending_trailing, line_trimmed, current_indent);
        let normalized = if line_trimmed.is_empty() {
            None
        } else {
            Some(normalize_code_for_match(line_trimmed))
        };
        let occurrence = normalized.as_ref().map(|n| {
            let next = formatted_anchor_occurrences.get(n).copied().unwrap_or(0) + 1;
            formatted_anchor_occurrences.insert(n.clone(), next);
            next
        });
        let mut line_expanded_from_inline_comment = false;

        if let Some(normalized) = &normalized {
            while leading_idx < extracted.leading_standalone.len() {
                let Some(match_kind) = leading_block_match_kind(
                    &extracted.leading_standalone[leading_idx],
                    normalized,
                    occurrence,
                    current_indent,
                ) else {
                    break;
                };

                if match_kind == LeadingBlockMatch::InlineSuffix
                    && expand_inline_match_arm_with_leading_block(
                        &mut out_lines,
                        line,
                        &extracted.leading_standalone[leading_idx],
                    )
                {
                    line_expanded_from_inline_comment = true;
                } else {
                    emit_anchored_block(&mut out_lines, &extracted.leading_standalone[leading_idx]);
                }
                leading_idx += 1;

                if line_expanded_from_inline_comment {
                    break;
                }
            }
        }

        if line_expanded_from_inline_comment {
            queue_trailing_blocks(
                &extracted.trailing_standalone,
                &mut trailing_idx,
                &mut pending_trailing,
                normalized.as_ref(),
                occurrence,
            );
            continue;
        }

        let mut out_line = line.to_string();
        let has_existing_comment = comment_start_index(line, &mut formatted_state).is_some();
        reset_single_line_string_state(&mut formatted_state);

        if !has_existing_comment
            && let Some(normalized) = &normalized
            && let Some(inline_comment) = extracted.inline_comments.get(inline_idx)
            && inline_comment_matches(inline_comment, normalized, occurrence)
        {
            out_line.push_str("  ");
            out_line.push_str(&inline_comment.text);
            inline_idx += 1;
        }

        out_lines.push_line(out_line);
        queue_trailing_blocks(
            &extracted.trailing_standalone,
            &mut trailing_idx,
            &mut pending_trailing,
            normalized.as_ref(),
            occurrence,
        );
    }

    while leading_idx < extracted.leading_standalone.len() {
        emit_anchored_block(&mut out_lines, &extracted.leading_standalone[leading_idx]);
        leading_idx += 1;
    }

    while trailing_idx < extracted.trailing_standalone.len() {
        pending_trailing.push_back(extracted.trailing_standalone[trailing_idx].clone());
        trailing_idx += 1;
    }

    flush_ready_trailing_blocks(&mut out_lines, &mut pending_trailing, "", 0);

    if !extracted.eof_standalone.is_empty() {
        if out_lines.ends_with_nonblank_line() {
            out_lines.push_line(String::new());
        }
        for line in extracted.eof_standalone {
            out_lines.push_line(line);
        }
    }

    out_lines.finish(formatted.ends_with('\n') || source.ends_with('\n'))
}

/// Emit a source comment block at its formatted anchor, including its preserved leading readability gap.
fn emit_anchored_block(out_lines: &mut NormalizedLineBuffer, block: &AnchoredStandaloneBlock) {
    if block.blank_line_before {
        out_lines.ensure_blank_line_before(block.indent);
    }
    for line in &block.lines {
        out_lines.push_line(line.clone());
    }
}

/// Flush trailing comment blocks once the formatted stream has moved out of their indentation scope.
fn flush_ready_trailing_blocks(
    out_lines: &mut NormalizedLineBuffer,
    pending_trailing: &mut VecDeque<AnchoredStandaloneBlock>,
    line_trimmed: &str,
    current_indent: usize,
) {
    while let Some(block) = pending_trailing.front() {
        let ready = line_trimmed.is_empty() || (!line_trimmed.is_empty() && current_indent <= block.indent);
        if !ready {
            break;
        }
        if let Some(block) = pending_trailing.pop_front() {
            emit_anchored_block(out_lines, &block);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LeadingBlockMatch {
    Exact,
    WrappedPrefix,
    InlineSuffix,
}

/// Return whether a source-anchored leading comment block belongs before the current formatted line.
///
/// Exact matches cover the ordinary no-wrap path. Prefix matches cover statements that are wrapped by formatting:
/// a source anchor such as `scenario=SubstraitConformanceScenario(...)` becomes a first formatted line like
/// `scenario=SubstraitConformanceScenario(`, so the original full-line anchor will no longer appear verbatim.
fn leading_block_match_kind(
    block: &AnchoredStandaloneBlock,
    normalized: &str,
    occurrence: Option<usize>,
    current_indent: usize,
) -> Option<LeadingBlockMatch> {
    if block.anchor == normalized && occurrence.is_some_and(|occ| occ == block.occurrence) {
        return Some(LeadingBlockMatch::Exact);
    }

    let suffix_idx = normalized.len().saturating_sub(block.anchor.len());
    let inline_suffix_match = normalized.len() > block.anchor.len()
        && block.indent > current_indent
        && normalized.ends_with(&block.anchor)
        && normalized
            .get(..suffix_idx)
            .and_then(|prefix| prefix.chars().last())
            .is_some_and(|ch| !ch.is_alphanumeric() && ch != '_');

    if inline_suffix_match {
        return Some(LeadingBlockMatch::InlineSuffix);
    }

    if block.indent == current_indent
        && matches!(normalized.chars().last(), Some('(' | '[' | '{'))
        && block.anchor.starts_with(normalized)
    {
        return Some(LeadingBlockMatch::WrappedPrefix);
    }

    None
}

/// Expand an inline `match` arm body back into block form so a preserved leading comment can stay attached to it.
fn expand_inline_match_arm_with_leading_block(
    out_lines: &mut NormalizedLineBuffer,
    line: &str,
    block: &AnchoredStandaloneBlock,
) -> bool {
    let Some((prefix, body)) = line.rsplit_once(" => ") else {
        return false;
    };

    if block.blank_line_before {
        out_lines.ensure_blank_line_before(block.indent);
    }
    out_lines.push_line(format!("{prefix} =>"));
    for comment_line in &block.lines {
        out_lines.push_line(comment_line.clone());
    }
    out_lines.push_line(format!("{}{}", " ".repeat(block.indent), body.trim_start()));
    true
}

fn inline_comment_matches(inline_comment: &InlineComment, normalized: &str, occurrence: Option<usize>) -> bool {
    inline_comment.anchor == normalized && occurrence.is_some_and(|occ| occ == inline_comment.occurrence)
}

fn queue_trailing_blocks(
    trailing_standalone: &[AnchoredStandaloneBlock],
    trailing_idx: &mut usize,
    pending_trailing: &mut VecDeque<AnchoredStandaloneBlock>,
    normalized: Option<&String>,
    occurrence: Option<usize>,
) {
    while *trailing_idx < trailing_standalone.len()
        && normalized.is_some_and(|n| n == &trailing_standalone[*trailing_idx].anchor)
        && occurrence.is_some_and(|occ| occ == trailing_standalone[*trailing_idx].occurrence)
    {
        pending_trailing.push_back(trailing_standalone[*trailing_idx].clone());
        *trailing_idx += 1;
    }
}

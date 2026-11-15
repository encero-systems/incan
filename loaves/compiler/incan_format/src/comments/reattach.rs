//! Reattach extracted comment bundles onto the formatter's AST-shaped output.
//!
//! Each comment is placed by its code's offset in the normalized stream that formatting preserves (`stream`), so a
//! comment stays beside its code when formatting wraps, joins or reindents that code. A comment whose code the stream
//! cannot locate falls back to matching its anchor line's text.

use std::collections::{HashMap, VecDeque};

use super::buffer::{NormalizedLineBuffer, leading_indent_width};
use super::model::{
    AnchoredStandaloneBlock, ExtractedComments, InlineComment, StreamCode, extract_comments, normalize_code_for_match,
};
use super::scanner::{StringState, comment_start_index, reset_single_line_string_state};
use super::stream::{AlignedLines, LineRange};

/// Reattach preserved source comments onto the formatter's AST-shaped output.
///
/// The formatter itself only emits comments that are represented in the AST. This pass preserves stand-alone and
/// inline `# ...` comments from the original source by anchoring them to nearby formatted code lines while respecting
/// indentation scope, blank-line separation, and string-literal boundaries.
///
/// 1. Extract the source's comments and align the formatted lines with the source's code stream.
/// 2. Emit each formatted line with the comments that belong before, on and after it.
/// 3. Emit whatever no line claimed, then the comments that follow all code.
pub(super) fn reattach_comments(source: &str, formatted: &str) -> String {
    // 1. Comments, and the formatted line each one belongs at.
    let extracted = extract_comments(source);
    let lines: Vec<&str> = formatted.lines().collect();
    let targets = StreamTargets::new(&extracted, &lines);
    let mut reattachment = Reattachment::new(&extracted, &targets);
    // 2. Every formatted line, with its comments.
    for (index, line) in lines.iter().enumerate() {
        reattachment.line(index, line);
    }
    // 3. The rest.
    reattachment.finish(formatted.ends_with('\n') || source.ends_with('\n'))
}

/// The formatted line each comment belongs at, found through the normalized stream; `None` falls back to text anchors.
struct StreamTargets {
    aligned: AlignedLines,
    leading: Vec<Option<usize>>,
    trailing: Vec<Option<usize>>,
    inline: Vec<Option<usize>>,
}

impl StreamTargets {
    /// Locate every extracted comment among the formatted `lines`; trivia before all source code stays before all
    /// formatted code even when import coalescing changes the first stream anchor.
    fn new(extracted: &ExtractedComments, lines: &[&str]) -> Self {
        let aligned = AlignedLines::new(&extracted.stream, lines);
        let first_code_line = lines.iter().position(|line| !line.trim().is_empty());
        let leading = extracted
            .leading_standalone
            .iter()
            .map(|block| {
                if block.code.offset == 0 {
                    first_code_line
                } else {
                    leading_target(&aligned, block.code)
                }
            })
            .collect();
        let trailing = extracted
            .trailing_standalone
            .iter()
            .map(|block| line_ending(&aligned, block.code))
            .collect();
        let inline = extracted
            .inline_comments
            .iter()
            .map(|comment| line_ending(&aligned, comment.code))
            .collect();
        Self {
            aligned,
            leading,
            trailing,
            inline,
        }
    }
}

/// The line a leading block belongs before: the first whose code ends after the anchor starts, or, for an anchor that
/// is only a closing bracket, that bracket's own line.
fn leading_target(aligned: &AlignedLines, code: StreamCode) -> Option<usize> {
    if code.len == 0
        && let Some(line) = aligned.empty_at(code.offset)
    {
        return Some(line);
    }
    aligned.ending_after(code.offset)
}

/// The line whose code ends with the anchor's code: the line an inline comment follows, and the line after which a
/// trailing block waits for its scope to end, even when formatting wrapped the anchor over several lines.
fn line_ending(aligned: &AlignedLines, code: StreamCode) -> Option<usize> {
    if code.len == 0 {
        return aligned.empty_at(code.offset).or_else(|| aligned.ending_at(code.offset));
    }
    let last = code.end() - 1;
    aligned
        .ending_after(last)
        .filter(|line| aligned.range(*line).is_some_and(|range| range.start <= last))
}

/// The output being built and the comments not yet placed, in source order.
struct Reattachment<'a> {
    extracted: &'a ExtractedComments,
    targets: &'a StreamTargets,
    out_lines: NormalizedLineBuffer,
    leading_idx: usize,
    trailing_idx: usize,
    inline_idx: usize,
    formatted_state: StringState,
    formatted_anchor_occurrences: HashMap<String, usize>,
    pending_trailing: VecDeque<AnchoredStandaloneBlock>,
}

/// One formatted line as the attachment rules read it.
struct FormattedLine<'l> {
    index: usize,
    text: &'l str,
    indent: usize,
    normalized: Option<String>,
    occurrence: Option<usize>,
    range: Option<LineRange>,
}

impl<'a> Reattachment<'a> {
    /// Start with an empty output.
    fn new(extracted: &'a ExtractedComments, targets: &'a StreamTargets) -> Self {
        Self {
            extracted,
            targets,
            out_lines: NormalizedLineBuffer::new(),
            leading_idx: 0,
            trailing_idx: 0,
            inline_idx: 0,
            formatted_state: StringState::None,
            formatted_anchor_occurrences: HashMap::new(),
            pending_trailing: VecDeque::new(),
        }
    }

    /// Emit formatted line `index` with its comments.
    ///
    /// 1. Flush trailing blocks whose scope this line leaves.
    /// 2. Emit the leading blocks that belong before it, expanding an inline arm whose body one of them precedes.
    /// 3. Emit the line with its inline comment, then queue the trailing blocks anchored on it.
    fn line(&mut self, index: usize, text: &str) {
        let line = self.formatted_line(index, text);
        // 1. Trailing blocks of a scope this line has left.
        if !matches!(
            self.formatted_state,
            StringState::TripleSingleQuoted | StringState::TripleDoubleQuoted
        ) {
            flush_ready_trailing_blocks(
                &mut self.out_lines,
                &mut self.pending_trailing,
                text.trim(),
                line.indent,
            );
        }
        // 2. Leading blocks; an expanded arm already wrote this line.
        if self.leading_blocks(&line) {
            self.queue_trailing_blocks(&line);
            return;
        }
        // 3. The line itself.
        let mut out_line = text.to_string();
        let has_existing_comment = comment_start_index(text, &mut self.formatted_state).is_some();
        reset_single_line_string_state(&mut self.formatted_state);
        if !has_existing_comment && let Some(comment) = self.inline_comment_for(&line) {
            out_line.push_str("  ");
            out_line.push_str(&comment.text);
            self.inline_idx += 1;
        }
        self.out_lines.push_line(out_line);
        self.queue_trailing_blocks(&line);
    }

    /// Read one formatted line's indentation, text anchor, occurrence and stream range.
    fn formatted_line<'l>(&mut self, index: usize, text: &'l str) -> FormattedLine<'l> {
        let trimmed = text.trim();
        let normalized = (!trimmed.is_empty()).then(|| normalize_code_for_match(trimmed));
        let occurrence = normalized.as_ref().map(|normalized| {
            let next = self.formatted_anchor_occurrences.get(normalized).copied().unwrap_or(0) + 1;
            self.formatted_anchor_occurrences.insert(normalized.clone(), next);
            next
        });
        FormattedLine {
            index,
            text,
            indent: leading_indent_width(text),
            normalized,
            occurrence,
            range: self.targets.aligned.range(index),
        }
    }

    /// Emit the leading blocks that belong before `line`; return whether one of them expanded `line` as an inline arm.
    fn leading_blocks(&mut self, line: &FormattedLine) -> bool {
        let Some(normalized) = &line.normalized else {
            return false;
        };
        while let Some(block) = self.extracted.leading_standalone.get(self.leading_idx) {
            let Some(match_kind) = self.leading_match(block, line, normalized) else {
                return false;
            };
            self.leading_idx += 1;
            if match_kind == LeadingBlockMatch::InlineSuffix
                && expand_inline_match_arm_with_leading_block(&mut self.out_lines, line.text, block)
            {
                return true;
            }
            let indent = if match_kind == LeadingBlockMatch::Stream {
                line.indent
            } else {
                block.indent
            };
            emit_anchored_block(&mut self.out_lines, block, indent, false);
        }
        false
    }

    /// Whether `block` belongs before `line`: by its stream target when it has one, otherwise by its anchor's text.
    fn leading_match(
        &self,
        block: &AnchoredStandaloneBlock,
        line: &FormattedLine,
        normalized: &str,
    ) -> Option<LeadingBlockMatch> {
        let Some(target) = self.targets.leading[self.leading_idx] else {
            return leading_block_match_kind(block, normalized, line.occurrence, line.indent);
        };
        if target > line.index {
            return None;
        }
        if line
            .range
            .is_some_and(|range| range.arm_body == Some(block.code.offset))
        {
            return Some(LeadingBlockMatch::InlineSuffix);
        }
        Some(LeadingBlockMatch::Stream)
    }

    /// The next inline comment, when it belongs on `line`.
    fn inline_comment_for(&self, line: &FormattedLine) -> Option<&'a InlineComment> {
        let comment = self.extracted.inline_comments.get(self.inline_idx)?;
        let belongs = match self.targets.inline[self.inline_idx] {
            Some(target) => target == line.index,
            None => line
                .normalized
                .as_ref()
                .is_some_and(|normalized| inline_comment_matches(comment, normalized, line.occurrence)),
        };
        belongs.then_some(comment)
    }

    /// Queue the trailing blocks anchored on `line` or on code before it.
    fn queue_trailing_blocks(&mut self, line: &FormattedLine) {
        while let Some(block) = self.extracted.trailing_standalone.get(self.trailing_idx) {
            let anchored = match self.targets.trailing[self.trailing_idx] {
                Some(target) => target <= line.index,
                None => {
                    line.normalized
                        .as_ref()
                        .is_some_and(|normalized| normalized == &block.anchor)
                        && line.occurrence == Some(block.occurrence)
                }
            };
            if !anchored {
                return;
            }
            self.pending_trailing.push_back(block.clone());
            self.trailing_idx += 1;
        }
    }

    /// Emit the comments no line claimed and the comments after all code, then join the output.
    fn finish(mut self, trailing_newline: bool) -> String {
        for block in &self.extracted.leading_standalone[self.leading_idx..] {
            emit_anchored_block(&mut self.out_lines, block, block.indent, true);
        }
        self.pending_trailing
            .extend(self.extracted.trailing_standalone[self.trailing_idx..].iter().cloned());
        while let Some(block) = self.pending_trailing.pop_front() {
            emit_anchored_block(&mut self.out_lines, &block, block.indent, true);
        }
        if !self.extracted.eof_standalone.is_empty() {
            if self.out_lines.ends_with_nonblank_line() {
                self.out_lines.push_line(String::new());
            } else {
                self.out_lines.set_blank_lines(0);
            }
            for line in &self.extracted.eof_standalone {
                self.out_lines.push_line(line.clone());
            }
        }
        self.out_lines.finish(trailing_newline)
    }
}

/// Emit a source comment block at `indent`, including its preserved leading readability gap.
///
/// A root-level block after a suite takes declaration spacing when another construct follows it; at EOF it keeps its
/// source gap. A block placed through the stream takes its formatted anchor's indentation, which formatting may have
/// changed. A leading block also keeps its source gap before code.
fn emit_anchored_block(
    out_lines: &mut NormalizedLineBuffer,
    block: &AnchoredStandaloneBlock,
    indent: usize,
    at_eof: bool,
) {
    if block.after_root_suite {
        out_lines.set_blank_lines(if at_eof { block.blank_lines_before } else { 2 });
    } else if block.blank_lines_before > 0 {
        out_lines.ensure_blank_line_before(indent);
    }
    for line in &block.lines {
        if indent == block.indent {
            out_lines.push_line(line.clone());
        } else {
            out_lines.push_line(format!("{}{}", " ".repeat(indent), line.trim_start()));
        }
    }
    if block.after_root_suite || block.blank_lines_after > 0 {
        out_lines.set_blank_lines(block.blank_lines_after);
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
        let ready = if line_trimmed.is_empty() {
            !block.after_root_suite
        } else {
            current_indent <= block.indent
        };
        if !ready {
            break;
        }
        if let Some(block) = pending_trailing.pop_front() {
            emit_anchored_block(out_lines, &block, block.indent, false);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LeadingBlockMatch {
    /// The stream places the block before this line, at the line's indentation.
    Stream,
    Exact,
    WrappedPrefix,
    InlineSuffix,
}

/// Return whether a source-anchored leading comment block belongs before the current formatted line, by text.
///
/// This is the fallback for a block whose code the stream could not locate. Exact matches cover the ordinary no-wrap
/// path. Prefix matches cover statements that are wrapped by formatting: a source anchor such as
/// `scenario=SubstraitConformanceScenario(...)` becomes a first formatted line like
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

    if block.indent == current_indent && wrapped_statement_prefix_matches(&block.anchor, normalized) {
        return Some(LeadingBlockMatch::WrappedPrefix);
    }

    None
}

/// Return whether a formatted line is the first line of a wrapped source statement.
///
/// Constructor/call wrapping keeps the opening delimiter on the first formatted line. Fluent method-chain wrapping
/// splits after the receiver, so the formatted line is a same-indent prefix and the remaining source anchor starts
/// with the next method segment.
fn wrapped_statement_prefix_matches(source_anchor: &str, formatted_anchor: &str) -> bool {
    if source_anchor == formatted_anchor {
        return false;
    }

    if let Some(stripped) = source_anchor.strip_prefix(formatted_anchor) {
        if matches!(formatted_anchor.chars().last(), Some('(' | '[' | '{')) {
            return true;
        }

        return stripped.starts_with('.');
    }

    formatted_anchor.starts_with(source_anchor) && matches!(source_anchor.chars().last(), Some('(' | '[' | '{'))
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

    if block.blank_lines_before > 0 {
        out_lines.ensure_blank_line_before(block.indent);
    }
    out_lines.push_line(format!("{prefix} =>"));
    for comment_line in &block.lines {
        out_lines.push_line(comment_line.clone());
    }
    out_lines.push_line(format!("{}{}", " ".repeat(block.indent), body.trim_start()));
    true
}

/// Return whether an inline comment still belongs to a formatted node, by text.
fn inline_comment_matches(inline_comment: &InlineComment, normalized: &str, occurrence: Option<usize>) -> bool {
    inline_comment.anchor == normalized && occurrence.is_some_and(|occ| occ == inline_comment.occurrence)
}

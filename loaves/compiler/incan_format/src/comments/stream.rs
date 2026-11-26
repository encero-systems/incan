//! Locate code in the normalized stream that formatting preserves, so comments follow their code through reflow.
//!
//! Formatting changes whitespace, line breaks, grouping parentheses, trailing commas, line continuations and quote
//! characters, and keeps every other character of the code in order. Removing those from the source and from the
//! formatted text leaves one stream; a source comment's offset in it names the formatted line it belongs beside,
//! however formatting wrapped, joined or reindented the code around it.

use super::scanner::{StringState, comment_start_index, reset_single_line_string_state};

/// The shortest needle, in stream bytes, that finds a formatted line again after the two streams diverge.
const RESYNC_NEEDLE: usize = 48;

/// How far past the cursor, in stream bytes, a formatted line is searched for after the streams diverge.
const RESYNC_WINDOW: usize = 1 << 16;

/// The stream bytes one formatted line's code covers.
#[derive(Clone, Copy)]
pub(super) struct LineRange {
    /// Offset of the line's first stream byte.
    pub start: usize,
    /// Offset just past the line's last stream byte; equal to `start` for a line such as a lone `)`.
    pub end: usize,
    /// Offset where an inline `match` arm's body starts, after its `=>`.
    pub arm_body: Option<usize>,
}

/// The formatted lines that were found in the source stream, in order, for locating comment offsets.
pub(super) struct AlignedLines {
    lines: Vec<(usize, LineRange)>,
}

impl AlignedLines {
    /// Align each line of `formatted` with the source `stream`.
    ///
    /// 1. Strip each line's comment and normalize its code.
    /// 2. Continue at the cursor when the stream continues with the line.
    /// 3. Otherwise find the line, followed by the lines after it, ahead of the cursor; a line that formatting
    ///    introduced or respelled is found nowhere and stays unaligned.
    pub fn new(stream: &str, formatted: &[&str]) -> Self {
        // 1. Normalized code per line; blank lines have none.
        let codes = normalized_line_codes(formatted);
        let mut cursor = 0usize;
        let mut lines = Vec::new();
        for (index, code) in codes.iter().enumerate() {
            let Some(code) = code else {
                continue;
            };
            // 2. The stream continues with this line, or 3. it is found again after a divergence.
            let start = if stream[cursor..].starts_with(code.as_str()) {
                Some(cursor)
            } else {
                resynchronized_start(stream, cursor, &codes[index..])
            };
            let Some(start) = start else {
                continue;
            };
            cursor = start + code.len();
            let arm_body = inline_arm_body(formatted[index], start);
            lines.push((
                index,
                LineRange {
                    start,
                    end: cursor,
                    arm_body,
                },
            ));
        }
        Self { lines }
    }

    /// The range of formatted line `index`, when that line was aligned.
    pub fn range(&self, index: usize) -> Option<LineRange> {
        let position = self.lines.partition_point(|(line, _)| *line < index);
        self.lines
            .get(position)
            .filter(|(line, _)| *line == index)
            .map(|(_, range)| *range)
    }

    /// The first formatted line whose code ends after `offset`.
    pub fn ending_after(&self, offset: usize) -> Option<usize> {
        let position = self.lines.partition_point(|(_, range)| range.end <= offset);
        self.lines.get(position).map(|(line, _)| *line)
    }

    /// The first formatted line with no stream code that sits exactly at `offset`, such as a closing bracket.
    pub fn empty_at(&self, offset: usize) -> Option<usize> {
        let position = self.lines.partition_point(|(_, range)| range.end < offset);
        self.lines[position..]
            .iter()
            .take_while(|(_, range)| range.end == offset)
            .find(|(_, range)| range.start == offset)
            .map(|(line, _)| *line)
    }

    /// The last formatted line whose code ends exactly at `offset`, when that line holds code.
    pub fn ending_at(&self, offset: usize) -> Option<usize> {
        let position = self.lines.partition_point(|(_, range)| range.end <= offset);
        position
            .checked_sub(1)
            .and_then(|previous| self.lines.get(previous))
            .filter(|(_, range)| range.end == offset && range.start < offset)
            .map(|(line, _)| *line)
    }
}

/// Normalize code into the stream formatting preserves: no whitespace, parentheses, commas or line continuations, and
/// one quote character.
pub(super) fn normalize_code_for_stream(code: &str) -> String {
    code.chars()
        .filter(|ch| !ch.is_whitespace() && !matches!(ch, '(' | ')' | ',' | '\\'))
        .map(|ch| if ch == '\'' { '"' } else { ch })
        .collect()
}

/// Normalize the code of each line, skipping comments and string contents' `#`; a blank line has no code.
fn normalized_line_codes(lines: &[&str]) -> Vec<Option<String>> {
    let mut state = StringState::None;
    lines
        .iter()
        .map(|line| {
            let comment = comment_start_index(line, &mut state);
            reset_single_line_string_state(&mut state);
            let code = comment.map_or(*line, |index| &line[..index]);
            (!code.trim().is_empty()).then(|| normalize_code_for_stream(code))
        })
        .collect()
}

/// Find where the first of `codes` starts in the stream ahead of `cursor`, matching it together with the lines after it
/// so that a short line is not found at an unrelated place.
fn resynchronized_start(stream: &str, cursor: usize, codes: &[Option<String>]) -> Option<usize> {
    let mut needle = String::new();
    for code in codes.iter().flatten() {
        needle.push_str(code);
        if needle.len() >= RESYNC_NEEDLE {
            break;
        }
    }
    if needle.is_empty() {
        return None;
    }
    let mut limit = (cursor + RESYNC_WINDOW).min(stream.len());
    while !stream.is_char_boundary(limit) {
        limit += 1;
    }
    stream[cursor..limit].find(&needle).map(|found| cursor + found)
}

/// The stream offset of an inline `match` arm's body on a formatted line that starts at `start`, split where comment
/// reattachment expands the arm.
fn inline_arm_body(line: &str, start: usize) -> Option<usize> {
    let (pattern, _body) = line.rsplit_once(" => ")?;
    Some(start + normalize_code_for_stream(pattern).len() + "=>".len())
}

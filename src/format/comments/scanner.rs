//! Scan formatter input for `#` comments while respecting string-literal boundaries.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StringState {
    None,
    SingleQuoted,
    DoubleQuoted,
    TripleSingleQuoted,
    TripleDoubleQuoted,
}

/// Count `#...` comments outside string literals.
///
/// This supports a strict safety check for formatter output:
/// if formatting would reduce comment count, we refuse to rewrite.
pub(super) fn count_line_comments(source: &str) -> usize {
    let mut state = StringState::None;
    let mut count = 0usize;

    for line in source.lines() {
        if comment_start_index(line, &mut state).is_some() {
            count += 1;
        }
        // Single-quoted strings are line-local; triple-quoted strings can span lines.
        if matches!(state, StringState::SingleQuoted | StringState::DoubleQuoted) {
            state = StringState::None;
        }
    }

    count
}

pub(super) fn comment_start_index(line: &str, state: &mut StringState) -> Option<usize> {
    let mut i = 0usize;
    while i < line.len() {
        let rest = &line[i..];
        let mut chars = rest.chars();
        let ch = chars.next()?;
        let ch_len = ch.len_utf8();

        match state {
            StringState::None => {
                if rest.starts_with("'''") {
                    *state = StringState::TripleSingleQuoted;
                    i += 3;
                    continue;
                }
                if rest.starts_with("\"\"\"") {
                    *state = StringState::TripleDoubleQuoted;
                    i += 3;
                    continue;
                }
                if ch == '\'' {
                    *state = StringState::SingleQuoted;
                    i += ch_len;
                    continue;
                }
                if ch == '"' {
                    *state = StringState::DoubleQuoted;
                    i += ch_len;
                    continue;
                }
                if ch == '#' {
                    return Some(i);
                }
                i += ch_len;
            }
            StringState::SingleQuoted => {
                if ch == '\\' {
                    if let Some(next) = chars.next() {
                        i += ch_len + next.len_utf8();
                    } else {
                        i += ch_len;
                    }
                    continue;
                }
                if ch == '\'' {
                    *state = StringState::None;
                }
                i += ch_len;
            }
            StringState::DoubleQuoted => {
                if ch == '\\' {
                    if let Some(next) = chars.next() {
                        i += ch_len + next.len_utf8();
                    } else {
                        i += ch_len;
                    }
                    continue;
                }
                if ch == '"' {
                    *state = StringState::None;
                }
                i += ch_len;
            }
            StringState::TripleSingleQuoted => {
                if rest.starts_with("'''") {
                    *state = StringState::None;
                    i += 3;
                } else {
                    i += ch_len;
                }
            }
            StringState::TripleDoubleQuoted => {
                if rest.starts_with("\"\"\"") {
                    *state = StringState::None;
                    i += 3;
                } else {
                    i += ch_len;
                }
            }
        }
    }

    None
}

/// Leave triple-quoted string state intact while clearing one-line quote state at line boundaries.
pub(super) fn reset_single_line_string_state(state: &mut StringState) {
    if matches!(state, StringState::SingleQuoted | StringState::DoubleQuoted) {
        *state = StringState::None;
    }
}

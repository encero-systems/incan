//! Read a string by ascending Unicode-scalar index in linear total time (#3070).
//!
//! A `str` is UTF-8, so finding scalar `index` walks the string from its start, and a loop that reads `s[i]` or
//! `s[i:i + k]` for each `i` in turn would walk from the start on every read. The compiler gives such a loop a cursor
//! for each string it reads by index and never changes inside the loop: the scalar index of the last boundary a read
//! found, that boundary's byte offset, and the string's scalar count once `len` has counted it (`-1` before). A read at
//! or after the cursor walks only the scalars between the two, so reading each scalar once, in ascending order, walks
//! the string once. A read before the cursor walks from the start, as every read did before. Negative indices and
//! slices with a step other than one keep the ordinary helpers. Results and errors are those of
//! [`crate::strings::str_index`], [`crate::strings::str_slice`] and [`crate::strings::str_len`].
//!
//! A cursor starts at `(0, 0, -1)` and is valid only for the string it is used with, unchanged.

use crate::errors::raise;
use crate::strings::{str_index, str_len, str_slice};
use incan_lang::errors::IncanError;

/// A string's read position: the scalar index of a boundary, its byte offset, and the scalar count or `-1`.
pub type TextCursor = (i64, i64, i64);

/// Find the byte offset of scalar `index`, or `s.len()` when `s` has fewer scalars, walking from the cursor when
/// `index` is at or after it. Returns the offset and the number of scalars walked, and leaves the cursor there.
fn seek(s: &str, index: usize, cursor: &mut TextCursor) -> (usize, usize) {
    let mut scalar = usize::try_from(cursor.0).unwrap_or(0);
    let mut byte = usize::try_from(cursor.1).unwrap_or(0);
    if index < scalar || byte > s.len() || !s.is_char_boundary(byte) {
        scalar = 0;
        byte = 0;
    }
    let mut walked = 0;
    for character in s[byte..].chars() {
        if scalar == index {
            break;
        }
        byte += character.len_utf8();
        scalar += 1;
        walked += 1;
    }
    cursor.0 = i64::try_from(scalar).unwrap_or(i64::MAX);
    cursor.1 = i64::try_from(byte).unwrap_or(i64::MAX);
    (byte, walked)
}

/// Index a string by Unicode scalar index, as [`str_index`] does, seeking from the loop's cursor.
///
/// ## Panics
///
/// - If `idx` is out of range: with `IndexError: string index out of range`.
pub fn str_index_from<S: AsRef<str>>(s: S, idx: i64, cursor: &mut TextCursor) -> String {
    let s = s.as_ref();
    let Ok(index) = usize::try_from(idx) else {
        return str_index(s, idx);
    };
    let (byte, _) = seek(s, index, cursor);
    match s[byte..].chars().next() {
        Some(character) => character.to_string(),
        None => raise(IncanError::string_index_out_of_range()),
    }
}

/// Slice a string over Unicode scalars, as [`str_slice`] does, seeking its start from the loop's cursor.
///
/// Each bound is present when its flag is set. A forward slice with non-negative bounds seeks its start from the cursor
/// and walks its length from there; any other slice is [`str_slice`]'s.
///
/// ## Panics
///
/// - If `step == 0`: with `ValueError: slice step cannot be zero`.
#[allow(clippy::too_many_arguments)]
pub fn str_slice_from<S: AsRef<str>>(
    s: S,
    has_start: bool,
    start: i64,
    has_end: bool,
    end: i64,
    has_step: bool,
    step: i64,
    cursor: &mut TextCursor,
) -> String {
    let s = s.as_ref();
    let start = has_start.then_some(start);
    let end = has_end.then_some(end);
    let step = has_step.then_some(step);
    let forward = matches!(step, None | Some(1));
    let (Ok(first), true) = (usize::try_from(start.unwrap_or(0)), forward) else {
        return str_slice(s, start, end, step);
    };
    let Some(end) = end else {
        let (start_byte, _) = seek(s, first, cursor);
        return s[start_byte..].to_string();
    };
    let Ok(last) = usize::try_from(end) else {
        return str_slice(s, start, Some(end), step);
    };
    let (start_byte, _) = seek(s, first, cursor);
    let mut end_byte = start_byte;
    for character in s[start_byte..].chars().take(last.saturating_sub(first)) {
        end_byte += character.len_utf8();
    }
    s[start_byte..end_byte].to_string()
}

/// Count a string's Unicode scalars, as [`str_len`] does, once per cursor.
pub fn str_len_from<S: AsRef<str>>(s: S, cursor: &mut TextCursor) -> i64 {
    if cursor.2 < 0 {
        cursor.2 = str_len(s);
    }
    cursor.2
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXTS: [&str; 4] = ["", "plain ascii text", "héllo wörld ünïcode", "a€b😀c\u{10ffff}d"];

    fn fresh() -> TextCursor {
        (0, 0, -1)
    }

    fn panics<T>(read: impl FnOnce() -> T + std::panic::UnwindSafe) -> bool {
        std::panic::catch_unwind(read).is_err()
    }

    /// The timing-free measure of #3070: an ascending read of every scalar walks each scalar once, for ASCII and
    /// non-ASCII text alike, where a walk from the start would cost `n * (n - 1) / 2` steps.
    #[test]
    fn an_ascending_read_of_every_scalar_walks_the_string_once() {
        for text in ["a".repeat(2_000), "é€😀".repeat(700)] {
            let count = text.chars().count();
            let mut cursor = fresh();
            let mut walked = 0;
            for index in 0..count {
                walked += seek(&text, index, &mut cursor).1;
            }
            assert_eq!(walked, count - 1, "{} scalars", count);
            let mut cursor = fresh();
            let mut sliced = 0;
            for index in 0..count {
                sliced += seek(&text, index, &mut cursor).1;
                let _ = str_slice_from(&text, true, index as i64, true, index as i64 + 3, false, 0, &mut cursor);
            }
            assert_eq!(sliced, count - 1, "{} scalars, sliced", count);
        }
    }

    #[test]
    fn every_read_equals_the_ordinary_helper_in_any_order() {
        for text in TEXTS {
            let count = text.chars().count() as i64;
            let mut cursor = fresh();
            let order: Vec<i64> = (0..count)
                .chain((0..count).rev())
                .chain([0, count / 2, 1, count - 1])
                .collect();
            for index in order.into_iter().filter(|index| *index >= 0) {
                assert_eq!(
                    str_index_from(text, index, &mut cursor),
                    str_index(text, index),
                    "{text:?}[{index}]"
                );
                for (start, end) in [
                    (index, index + 1),
                    (index, index + 3),
                    (index, count + 5),
                    (index + 2, index),
                ] {
                    assert_eq!(
                        str_slice_from(text, true, start, true, end, false, 0, &mut cursor),
                        str_slice(text, Some(start), Some(end), None),
                        "{text:?}[{start}:{end}]"
                    );
                }
                assert_eq!(
                    str_slice_from(text, true, index, false, 0, false, 0, &mut cursor),
                    str_slice(text, Some(index), None, None)
                );
            }
            for index in [-1, -count] {
                if count > 0 {
                    assert_eq!(str_index_from(text, index, &mut cursor), str_index(text, index));
                }
            }
            for (start, end, step) in [(-3, -1, None), (0, -1, None), (1, 9, Some(2)), (9, 1, Some(-1))] {
                assert_eq!(
                    str_slice_from(
                        text,
                        true,
                        start,
                        true,
                        end,
                        step.is_some(),
                        step.unwrap_or(0),
                        &mut cursor
                    ),
                    str_slice(text, Some(start), Some(end), step)
                );
            }
            assert_eq!(str_slice_from(text, false, 0, false, 0, false, 0, &mut cursor), text);
            assert_eq!(str_len_from(text, &mut cursor), count);
        }
    }

    #[test]
    fn reads_past_either_end_fail_as_the_ordinary_helpers_do() {
        let text = "aé";
        let mut cursor = fresh();
        assert!(panics(move || str_index_from(text, 2, &mut cursor)));
        let mut cursor = fresh();
        assert!(panics(move || str_index_from(text, -3, &mut cursor)));
        let mut cursor = fresh();
        assert!(panics(move || str_slice_from(
            text,
            true,
            0,
            true,
            2,
            true,
            0,
            &mut cursor
        )));
        let mut cursor = fresh();
        assert_eq!(str_slice_from(text, true, 5, true, 9, false, 0, &mut cursor), "");
    }

    #[test]
    fn the_count_is_taken_once_per_cursor() {
        let mut cursor = fresh();
        assert_eq!(str_len_from("héllo", &mut cursor), 5);
        assert_eq!(cursor.2, 5);
        assert_eq!(str_len_from("héllo", &mut cursor), 5);
    }
}

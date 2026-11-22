//! Zip list snapshots, enumerate lists, bytes and text, and materialize Unicode scalar strings.

use incan_std_primitives::iter::{Generator, nonnegative_count};
use incan_std_primitives::strings::{str_len, str_split};

/// Independently clonable zip cursors with the legacy polling order: left before right.
#[derive(Clone, Debug)]
pub struct ZipLists<T, U> {
    pub left: Vec<T>,
    pub right: Vec<U>,
    pub left_index: i64,
    pub right_index: i64,
}

/// Retain both source snapshots with their cursors at the beginning, polling neither.
pub fn zip_lists<T: Clone, U: Clone>(left: Vec<T>, right: Vec<U>) -> ZipLists<T, U> {
    ZipLists {
        left,
        right,
        left_index: 0,
        right_index: 0,
    }
}

/// Clone both snapshots and cursors without polling either source.
pub fn clone_zip<T: Clone, U: Clone>(values: &ZipLists<T, U>) -> ZipLists<T, U> {
    values.clone()
}

/// Poll one pair: advance the left cursor, then the right one, and append both owned items only when both exist.
pub fn poll_zip<T: Clone, U: Clone>(values: &mut ZipLists<T, U>, output: &mut Vec<(T, U)>) -> bool {
    let Some(first) = values.left.get(nonnegative_count(values.left_index)).cloned() else {
        return false;
    };
    values.left_index += 1;
    let Some(second) = values.right.get(nonnegative_count(values.right_index)).cloned() else {
        return false;
    };
    values.right_index += 1;
    output.push((first, second));
    true
}

/// Defer the remaining pairs, moving the snapshots out at their cursors without drawing an item now.
pub fn zip_lists_generator<T: Clone + 'static, U: Clone + 'static>(values: ZipLists<T, U>) -> Generator<(T, U)> {
    let left = values.left.into_iter().skip(nonnegative_count(values.left_index));
    let right = values.right.into_iter().skip(nonnegative_count(values.right_index));
    Generator::new(left.zip(right))
}

/// The materialized `(index, item)` pairs of a borrowed list.
pub fn enumerate_list<T: Clone>(values: &Vec<T>) -> Vec<(i64, T)> {
    values
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, item)| (index as i64, item))
        .collect()
}

/// Byte enumeration items as source integers.
pub fn enumerate_bytes(values: &Vec<u8>) -> Vec<(i64, i64)> {
    values
        .iter()
        .enumerate()
        .map(|(index, byte)| (index as i64, i64::from(*byte)))
        .collect()
}

/// Unicode scalar strings with their zero-based positions.
pub fn enumerate_text(values: &String) -> Vec<(i64, String)> {
    text_items(values)
        .into_iter()
        .enumerate()
        .map(|(index, item)| (index as i64, item))
        .collect()
}

/// Materialize Unicode scalars by splitting at native boundaries and keeping the nonempty pieces.
pub fn text_items(values: &String) -> Vec<String> {
    str_split(values, Some(""))
        .into_iter()
        .filter(|part| str_len(part) != 0)
        .collect()
}

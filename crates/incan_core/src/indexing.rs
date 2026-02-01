//! Indexing and slicing helpers shared by compiler/runtime.
//!
//! These helpers normalize Python-like slice bounds (negative indices and clamping),
//! keeping behavior consistent across core string semantics and stdlib collections.

/// Normalize slice bounds using Python-like semantics.
///
/// - `len`: collection length.
/// - `start`/`end`: optional bounds (may be negative).
/// - `step`: step size (must be non-zero; checked by the caller).
///
/// Returns `(start_idx, end_idx)` after applying default values, negative index normalization,
/// and clamping for the given step direction.
pub fn normalize_slice_bounds(len: i64, start: Option<i64>, end: Option<i64>, step: i64) -> (i64, i64) {
    let default_start = if step > 0 { 0 } else { len - 1 };
    let default_end = if step > 0 { len } else { -1 };

    let mut start_idx = start.unwrap_or(default_start);
    let mut end_idx = end.unwrap_or(default_end);

    if start_idx < 0 {
        start_idx += len;
    }
    // Important: for negative steps, `end = None` uses the sentinel `-1` (not `len-1`).
    // Python's slice normalization keeps this sentinel as-is.
    if end.is_some() && end_idx < 0 {
        end_idx += len;
    }

    if step > 0 {
        start_idx = start_idx.clamp(0, len);
        end_idx = end_idx.clamp(0, len);
    } else {
        start_idx = start_idx.clamp(-1, len - 1);
        end_idx = end_idx.clamp(-1, len - 1);
    }

    (start_idx, end_idx)
}

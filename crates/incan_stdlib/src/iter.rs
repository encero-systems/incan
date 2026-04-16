//! Iteration helpers for Incan-generated Rust code.
//!
//! This module provides small iterator utilities with Python-like behavior.

use crate::errors::raise;
use incan_core::errors::IncanError;

/// A Python-like `range(start, end, step)` iterator over `i64`.
#[derive(Debug, Clone)]
pub struct PyRange {
    cur: i64,
    end: i64,
    step: i64,
}

impl Iterator for PyRange {
    type Item = i64;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.step > 0 {
            if self.cur >= self.end {
                return None;
            }
        } else if self.cur <= self.end {
            return None;
        }
        let out = self.cur;
        self.cur += self.step;
        Some(out)
    }
}

/// Create a Python-like `range(start, end, step)`.
///
/// - End is **exclusive**.
/// - Supports negative steps.
///
/// TODO(perf): Extend lowering/codegen specialization beyond literal `step == 1`(for example, constant-folded
///             expressions that evaluate to `1`) so more loops can use native Rust ranges where semantics are
/// identical.
///
/// ## Panics
/// - `ValueError: range() arg 3 must not be zero` if `step == 0`.
#[inline]
pub fn range(start: i64, end: i64, step: i64) -> PyRange {
    if step == 0 {
        raise(IncanError::range_step_zero());
    }
    PyRange { cur: start, end, step }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_positive_step() {
        let xs: Vec<i64> = range(0, 5, 1).collect();
        assert_eq!(xs, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn range_negative_step() {
        let xs: Vec<i64> = range(5, 0, -2).collect();
        assert_eq!(xs, vec![5, 3, 1]);
    }

    #[test]
    #[should_panic(expected = "ValueError: range() arg 3 must not be zero")]
    fn range_zero_step_panics_with_value_error() {
        let _ = range(0, 5, 0);
    }
}

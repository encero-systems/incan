//! Testing helpers for Incan-generated Rust code.
//!
//! These are intended to be imported from Incan via:
//! - `from std.testing import assert, assert_eq, assert_ne, assert_true, assert_false, fail`

use std::fmt::Debug;

/// Assert that a condition is true.
///
/// # Panics
///
/// Panics if `condition` is false.
pub fn assert(condition: bool) {
    if !condition {
        panic!("assertion failed");
    }
}

/// Assert that two values are equal.
///
/// # Panics
///
/// Panics if `left != right`.
pub fn assert_eq<T: PartialEq + Debug>(left: T, right: T) {
    if left != right {
        panic!(
            "assertion failed: left != right\n  left:  {:?}\n  right: {:?}",
            left, right
        );
    }
}

/// Assert that two values are not equal.
///
/// # Panics
///
/// Panics if `left == right`.
pub fn assert_ne<T: PartialEq + Debug>(left: T, right: T) {
    if left == right {
        panic!(
            "assertion failed: left == right\n  left:  {:?}\n  right: {:?}",
            left, right
        );
    }
}

/// Assert that a condition is true.
///
/// # Panics
///
/// Panics if `condition` is false.
pub fn assert_true(condition: bool) {
    assert(condition);
}

/// Assert that a condition is false.
///
/// # Panics
///
/// Panics if `condition` is true.
pub fn assert_false(condition: bool) {
    assert(!condition);
}

/// Explicitly fail a test with a message.
///
/// # Panics
///
/// Always panics with the provided `msg`.
pub fn fail(msg: String) {
    panic!("{}", msg);
}

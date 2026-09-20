//! The `harness` area of the behaviour-fixture family: the harness proving itself through the compiler suite.
//!
//! These fixtures retire nothing. Each exercises one shape of the format a twin lane will rely on -- a refused
//! program, a non-zero exit code, contained rather than exact stdout lines, a module directory with an import, a
//! project directory with its own manifest -- so a change to the runner or to the route underneath it is caught here
//! before it is caught in a twin. See the family's `README.md` for the format.

use incan_test_support::behavior_fixtures::assert_area_green;

/// Run every harness fixture and fail with the full per-fixture report when any of them does not hold.
#[test]
fn behavior_harness_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("harness")
}

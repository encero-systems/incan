//! The `harness` area of the behaviour-fixture family: the harness proving itself through the compiler suite.
//!
//! These fixtures retire nothing. Each exercises one shape of the format a twin lane will rely on -- a refused
//! program (one code, and two codes from one check), a non-zero exit code, an exit code as the only observable, an
//! empty stdout, contained rather than exact stdout lines, a module directory with an import, a project directory
//! with its own manifest -- so a change to the runner or to the route underneath it is caught here before it is
//! caught in a twin. The header refusals (a directive after the header ended, an empty contains block, and the rest)
//! are unit tests of `parse_header` in `incan_test_support`, not fixtures: an area fixture must pass. See the
//! family's `README.md` for the format.
//!
//! A root file holds one `#[test]` per leaf area, each calling `assert_area_green`; an area is one libtest case and
//! holds at most `MAX_FIXTURES_PER_AREA` fixtures, which the runner enforces.

use incan_test_support::behavior_fixtures::assert_area_green;

/// Run every harness fixture and fail with the full per-fixture report when any of them does not hold.
#[test]
fn behavior_harness_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("harness")
}

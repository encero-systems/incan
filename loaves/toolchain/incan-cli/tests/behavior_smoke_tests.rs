//! The `smoke` area of the behaviour-fixture family, run through the compiler suite.
//!
//! Every fixture under `loaves/compiler/incan_test_support/fixtures/behavior/smoke/` is an Incan program whose header
//! declares what a run must show; `incan_test_support::behavior_fixtures` discovers, runs and compares them and this
//! root only names the area. One libtest case runs the whole area and its failure message lists every fixture that
//! did not show what it declared, with the fixture's path and its expected-versus-actual, so a failure is attributed
//! to a fixture without one Rust function per fixture. The format is described in the family's `README.md`.

use incan_test_support::behavior_fixtures::assert_area_green;

/// Run every smoke fixture and fail with the full per-fixture report when any of them does not hold.
#[test]
fn behavior_smoke_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("smoke")
}

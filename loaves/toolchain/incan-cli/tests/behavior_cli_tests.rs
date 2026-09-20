//! The `cli` area of the behaviour-fixture family: twins of the retire-class tests under `loaves/toolchain/incan-cli`.
//!
//! Each fixture under `loaves/compiler/incan_test_support/fixtures/behavior/cli/` is an Incan program whose header
//! names the retired test it stands in for and what a run must show. The retired tests proved their point by reading
//! generated Rust after a build or a codegen call; these fixtures prove the same program's observable behaviour by
//! running it, so they hold on whatever route slice 7 puts underneath. `incan_test_support::behavior_fixtures`
//! discovers, runs and compares them; this root only names the area, and one libtest case reports every fixture that
//! did not show what it declared. The format is described in the family's `README.md`.

use incan_test_support::behavior_fixtures::assert_area_green;

/// Run every CLI-area fixture and fail with the full per-fixture report when any of them does not hold.
#[test]
fn behavior_cli_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("cli")
}

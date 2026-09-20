//! The `cli_dependencies` area of the behaviour-fixture family: project fixtures with in-fixture path dependencies,
//! run through the compiler suite.
//!
//! Every fixture under `loaves/compiler/incan_test_support/fixtures/behavior/cli_dependencies/` is a project whose
//! `loaf.toml` declares `[dependencies]` path entries under the fixture directory (`deps/<name>`), and whose program
//! reaches them through `pub::<name>`; each is the route-agnostic twin of a retire-class test whose surviving
//! observable is what a consumer prints, or the diagnostic that refuses it, across a package boundary. The runner
//! bakes every provider before the run, in dependency order, because a consumer refuses to run until its providers
//! have published a package Loaf. The bake carries no Cargo authority: this root is deliberately not registered in
//! `OvenCompilerSuiteTargetCapabilities`, so under the suite a provider the Oven serves from the sealed stdlib Loaf
//! (an Incan library with no dependencies of its own) bakes without Cargo, and a provider that would need Cargo
//! fails its fixture at the scheduler's guard. The format is described in the family's `README.md`.

use incan_test_support::behavior_fixtures::assert_area_green;

/// Run every dependency fixture, baking its providers first, and fail with the full per-fixture report when any of
/// them does not hold.
#[test]
fn behavior_cli_dependencies_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("cli_dependencies")
}

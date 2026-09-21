//! The `snapshots_*` areas of the behavior-fixture family: twins of the retire-class codegen snapshot tests in
//! `loaves/compiler/incan_emit/tests/codegen_snapshot_tests.rs`.
//!
//! Every fixture under `loaves/compiler/incan_test_support/fixtures/behavior/snapshots_<topic>/` is one of the
//! programs those tests generated Rust from (a `codegen_snapshots/*.incn` input or a source inlined in the test),
//! given a `main` that prints what the retired test's assertions were indirectly about, and a header naming the
//! retired tests and the run's expected observables. The snapshot and `contains(...)` assertions on the Rust die
//! with the route; the program's behavior does not, so the fixtures hold on whatever route slice 7 puts underneath.
//! `incan_test_support::behavior_fixtures` discovers, runs and compares them; this root only names the areas, one
//! libtest case per leaf area (the family's `README.md` caps a leaf area at 60 fixtures), each reporting every
//! fixture that did not show what it declared.

use incan_test_support::behavior_fixtures::assert_area_green;

/// Run the lists, dicts, sets, comprehensions, iterators, strings and builtins fixtures.
#[test]
fn behavior_snapshots_collections_and_strings_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("snapshots_collections_and_strings")
}

/// Run the newtypes, validation, JSON and serde trait fixtures.
#[test]
fn behavior_snapshots_newtypes_and_serde_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("snapshots_newtypes_and_serde")
}

/// Run the stdlib module surface fixtures (math, fs, tempfile, testing, async, derives, registry, graph, uuid, regex,
/// compression, traits, web).
#[test]
fn behavior_snapshots_stdlib_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("snapshots_stdlib")
}

/// Run the traits, supertraits, bounds, generics, protocol hooks and fallible iteration fixtures.
#[test]
fn behavior_snapshots_traits_and_generics_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("snapshots_traits_and_generics")
}

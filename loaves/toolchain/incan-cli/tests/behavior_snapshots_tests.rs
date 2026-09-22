//! The `snapshots_*` areas of the behaviour-fixture family: twins of the retire-class codegen snapshot tests in
//! `loaves/compiler/incan_emit/tests/codegen_snapshot_tests.rs`, and the behaviour proofs of the pattern fixes
//! those twins surfaced (#1707, #1708, #1714).
//!
//! Every fixture under `loaves/compiler/incan_test_support/fixtures/behavior/snapshots_<topic>/` is a program with
//! a `main` that prints what its retired tests were indirectly about, and a header naming the retired tests and the
//! run's expected observables. `incan_test_support::behavior_fixtures` discovers, runs and compares them; this root
//! only names the areas, one libtest case per leaf area (the family's `README.md` caps a leaf area at 60 fixtures),
//! each reporting every fixture that did not show what it declared.

use incan_test_support::behavior_fixtures::assert_area_green;

/// Run the models, classes, fields, constructors and properties fixtures.
#[test]
fn behavior_snapshots_models_and_classes_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("snapshots_models_and_classes")
}

/// Run the enums, patterns, match, unions and `isinstance` fixtures.
#[test]
fn behavior_snapshots_enums_and_matching_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("snapshots_enums_and_matching")
}

/// Run the lists, dicts, sets, comprehensions, iterators, strings and builtins fixtures.
#[test]
fn behavior_snapshots_collections_and_strings_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("snapshots_collections_and_strings")
}

/// Run the stdlib module surface fixtures (math, fs, tempfile, testing, async, derives, registry, graph, uuid, regex,
/// compression, traits, web).
#[test]
fn behavior_snapshots_stdlib_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("snapshots_stdlib")
}

//! The `cli_*` areas of the behavior-fixture family, one libtest case per leaf area; `cli_dependencies` has a root of
//! its own.
//!
//! Each fixture under `loaves/compiler/incan_test_support/fixtures/behavior/cli_<area>/` is an Incan program whose
//! header declares what a run or a check must show and names the retired tests it stands in for. The retired tests
//! proved their point by reading generated Rust after a build or a codegen call; these fixtures prove the same
//! program's observable behavior by running it, or by checking it when the observable is a refusal, so they hold on
//! whatever route slice 7 puts underneath. `incan_test_support::behavior_fixtures` discovers, runs and compares them;
//! this root only names the areas, and each case reports every fixture of its area that did not show what it declared.
//! The format is described in the family's `README.md`.

use incan_test_support::behavior_fixtures::assert_area_green;

/// Programs refused at check time, each with the diagnostic code its header declares.
#[test]
fn behavior_cli_refusals_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("cli_refusals")
}

/// Programs refused at check time for a change through a place that does not permit it: a binding declared without
/// `mut`, a parameter not marked `mut`, or the items of such a place that a `for` loop changes.
#[test]
fn behavior_cli_refusals_mutation_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("cli_refusals_mutation")
}

/// Literals, collections, assignments, patterns, numerics, mut parameters, and the task handles, channel senders and
/// locks a loop passes on.
#[test]
fn behavior_cli_values_and_calls_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("cli_values_and_calls")
}

/// Modules, facades and imports across files, models, methods, traits and their defaults, generics, decorators and web
/// types.
#[test]
fn behavior_cli_modules_and_declarations_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("cli_modules_and_declarations")
}

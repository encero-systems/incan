//! The `lowering_*` areas of the behaviour-fixture family: twins of the retire-class unit tests of the legacy
//! backend's lowering under `loaves/compiler/incan_ir/**`.
//!
//! Every fixture under `loaves/compiler/incan_test_support/fixtures/behavior/lowering_<topic>/` is a program whose
//! behaviour the retired test's lowering decision was about: the shape of a call, which declaration an alias or a
//! dispatched method resolves to, what a pattern or an assertion binds, which constructor form a partial or a
//! decorated surface reaches. The retired tests asserted the intermediate representation those decisions produced;
//! the IR dies with its crate, the program's meaning does not, so the fixtures hold on whatever route slice 7 puts
//! underneath. `incan_test_support::behavior_fixtures` discovers, runs and compares them; this root only names the
//! areas, one libtest case per leaf area (the family's `README.md` caps a leaf area at 60 fixtures), each reporting
//! every fixture that did not show what it declared.

use incan_test_support::behavior_fixtures::assert_area_green;

/// Run the call, method-dispatch, iterator and Result surface, decorator, partial and trait-dispatch fixtures.
#[test]
fn behavior_lowering_expressions_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("lowering_expressions")
}

/// Run the declaration fixtures: models, enums, generics, frozen collections, numeric widths, assertions, serde
/// derives, module-scoped traits and the check-time refusals.
#[test]
fn behavior_lowering_declarations_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("lowering_declarations")
}

/// Run the project fixtures whose programs cross a package boundary: each declares in-fixture providers that the
/// runner bakes first, with no Cargo authority, before the consumer runs.
#[test]
fn behavior_lowering_dependencies_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("lowering_dependencies")
}

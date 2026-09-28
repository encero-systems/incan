//! The `emit_*` areas of the behavior-fixture family: twins of the retire-class unit tests inside the frozen
//! emitter tree `loaves/compiler/incan_emit/src/emit/**`.
//!
//! Every fixture under `loaves/compiler/incan_test_support/fixtures/behavior/emit_<topic>/` is an Incan program
//! whose header names the emitter tests it retires and the run's expected observables. Those tests built IR by hand
//! and asserted a token, a helper call, a borrow shape or a type spelling in the generated Rust; each fixture
//! instead exercises the source construct that shape stood for and prints what it produces, so the fixture holds on
//! whatever route slice 7 puts underneath. `incan_test_support::behavior_fixtures` discovers, runs and compares
//! them; this root only names the areas, one libtest case per leaf area (the family's `README.md` caps a leaf area
//! at 60 fixtures), each reporting every fixture that did not show what it declared.

use incan_test_support::behavior_fixtures::assert_area_green;

/// Run the calls, builtins, std.testing assertions, dict and list methods, iterator adapters and stdlib import
/// fixtures.
#[test]
fn behavior_emit_calls_and_builtins_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("emit_calls_and_builtins")
}

/// Run the value enum, trait, static and local binding, constructor surface, module binding and refused-program
/// fixtures.
#[test]
fn behavior_emit_declarations_and_modules_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("emit_declarations_and_modules")
}

/// Run the project fixtures whose in-fixture dependencies publish unions, constructor surfaces and every declaration
/// kind across a package boundary.
#[test]
fn behavior_emit_dependency_unions_and_surfaces_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("emit_dependency_unions_and_surfaces")
}

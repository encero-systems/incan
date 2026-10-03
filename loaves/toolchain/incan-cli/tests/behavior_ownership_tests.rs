//! The `ownership_*` areas of the behavior-fixture family: twins of the retire-class tests of the emitter's ownership
//! planner and conversion policy (`incan_emit/src/ownership.rs`, `src/conversions.rs`, `src/trait_bound_inference.rs`
//! and `tests/implicit_borrowing_codegen_tests.rs`).
//!
//! Those tests handed the planner a synthetic expression and asserted the plan it chose: clone, move, lend, convert
//! at a boundary, infer a bound. Each fixture under `loaves/compiler/incan_test_support/fixtures/behavior/ownership_*/`
//! is the Incan program such a plan exists for, run to completion with output that proves the program's meaning: the
//! value a caller still holds after a call, the text a string boundary produced, the items a loop or comprehension
//! visited, a generic function running for a concrete type. They hold on whatever route slice 7 puts underneath.
//! `incan_test_support::behavior_fixtures` discovers, runs and compares them; this root only names the areas, one
//! libtest case each, and every case reports each fixture that did not show what it declared. The format is described
//! in the family's `README.md`.

use incan_test_support::behavior_fixtures::assert_area_green;

/// Strings crossing call, assignment, return, field and collection boundaries: literals, variables, consts, statics
/// and frozen strings, plus the primitives that need no conversion at all.
#[test]
fn behavior_ownership_string_boundaries_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("ownership_string_boundaries")
}

/// Values passed and used again, fields read out of owners, loop bindings stored into results, generator sources
/// consumed or reused, and a field read through a web extractor wrapper.
#[test]
fn behavior_ownership_values_and_fields_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("ownership_values_and_fields")
}

/// Parameters the callee mutates, dict probes through such parameters, loops over strings, bytes and enum lists,
/// comprehension and `list()` sources, a bounded read through a buffer, self returned as a value, and channel ends.
#[test]
fn behavior_ownership_parameters_and_receivers_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("ownership_parameters_and_receivers")
}

/// Exact-width float arithmetic and its finiteness check, and generic functions and models whose inferred bounds are
/// satisfied by concrete types: formatting, cloning, reflection, fallible iteration, same-named functions.
#[test]
fn behavior_ownership_numerics_and_bounds_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("ownership_numerics_and_bounds")
}

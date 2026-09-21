//! The `codegen_*` areas of the behaviour-fixture family, run through the compiler suite.
//!
//! Every fixture under `loaves/compiler/incan_test_support/fixtures/behavior/codegen_<area>/` is an Incan program whose
//! header declares what a run must show; each is the route-agnostic twin of a retire-class test of `incan_emit` outside
//! its frozen `src/emit/` tree, whose surviving observable is a program's output or a check-time diagnostic rather than
//! the generated Rust it inspected. `incan_test_support::behavior_fixtures` discovers, runs and compares them and this
//! root only names the areas: one libtest case per leaf area, so the suite's two-thread root budget runs two areas at a
//! time and a failure message lists every fixture of the area that did not show what it declared, with the fixture's
//! path and its expected-versus-actual. The format is described in the family's `README.md`.

use incan_test_support::behavior_fixtures::assert_area_green;

/// Run every fixture of the functions-and-aliases area (partials, aliases, statics, decorators, explicit type
/// arguments, closures, magic methods) and fail with the full per-fixture report when any of them does not hold.
#[test]
fn behavior_codegen_functions_and_aliases_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("codegen_functions_and_aliases")
}

/// Run every fixture of the modules-and-imports area (nested modules, import spellings, field aliases across modules,
/// facades, std module imports and derive bundles, qualified type annotations) and fail with the full report when any
/// of them does not hold.
#[test]
fn behavior_codegen_modules_and_imports_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("codegen_modules_and_imports")
}

/// Run every fixture of the models-and-collections area (reflection, membership, serialization, empty collections in
/// generics, value aliases, match-arm ownership, string helpers, entrypoints, stdlib surfaces) and fail with the full
/// report when any of them does not hold.
#[test]
fn behavior_codegen_models_and_collections_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("codegen_models_and_collections")
}

/// Run every fixture of the dependencies area (projects with an in-fixture `pub::` provider the runner bakes first)
/// and fail with the full report when any of them does not hold.
#[test]
fn behavior_codegen_dependencies_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("codegen_dependencies")
}

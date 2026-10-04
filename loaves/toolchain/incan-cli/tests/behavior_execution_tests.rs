//! The `execution` family of the behavior-fixture family, run through the compiler suite.
//!
//! These fixtures carry over the programs the Body IR interpreter's tests ran, each with the result those tests
//! asserted, before the interpreter is removed (#1337 stage 2). A program whose entry returned a value prints it from
//! `main`; one that stopped on a checked failure declares its stdout and exit code. As fixtures they hold for whatever
//! route runs Incan programs, so the interpreter's coverage outlives it. `incan_test_support::behavior_fixtures`
//! discovers, runs and compares them; this root only names the areas, one libtest case each.

use incan_test_support::behavior_fixtures::assert_area_green;

/// Programs of calls, callables, generators, nominal types, enums, `isinstance` and async tasks.
#[test]
fn behavior_execution_calls_types_and_modules_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("execution_calls_types_and_modules")
}

/// Programs over collections: `len`, sorting, hashed containers, `enumerate` and `zip`.
#[test]
fn behavior_execution_collections_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("execution_collections")
}

/// Programs over typed numerics and scalar conversions.
#[test]
fn behavior_execution_numerics_and_conversions_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("execution_numerics_and_conversions")
}

/// Programs over scalars, bindings and bodies, including checked arithmetic that stops the program.
#[test]
fn behavior_execution_scalars_and_bodies_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("execution_scalars_and_bodies")
}

/// Programs over strings, printing and program output.
#[test]
fn behavior_execution_strings_and_output_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("execution_strings_and_output")
}

/// Second area of calls, modules and types: programs the interpreter refused, with the legacy route's result.
#[test]
fn behavior_execution_calls_types_and_modules_2_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("execution_calls_types_and_modules_2")
}

/// Second area of collections.
#[test]
fn behavior_execution_collections_2_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("execution_collections_2")
}

/// Second area of numerics and conversions.
#[test]
fn behavior_execution_numerics_and_conversions_2_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("execution_numerics_and_conversions_2")
}

/// Second area of scalars and bodies: programs the interpreter's source profile refused.
#[test]
fn behavior_execution_scalars_and_bodies_2_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("execution_scalars_and_bodies_2")
}

/// Second area of strings and output.
#[test]
fn behavior_execution_strings_and_output_2_fixtures_hold() -> Result<(), Box<dyn std::error::Error>> {
    assert_area_green("execution_strings_and_output_2")
}

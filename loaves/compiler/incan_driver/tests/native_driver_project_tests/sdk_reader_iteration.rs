//! Structural and fallible hook execution needed by the reader's checked polling path.

use super::{corpus, driver_fixture};

/// Structural iteration evaluates its source once, polls the selected cursor and shares the comprehension path.
#[test]
fn structural_iteration_hooks_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("structural-iteration-hooks")?,
        &fixture.sysroot,
        &fixture.formatting,
        include_str!("../../../incan_test_support/fixtures/native_iteration/structural.incn"),
    )
}

/// Fallible polling propagates its first error and distinguishes exhaustion from a successfully yielded item.
#[test]
fn fallible_iteration_hooks_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("fallible-iteration-hooks")?,
        &fixture.sysroot,
        &fixture.formatting,
        include_str!("../../../incan_test_support/fixtures/native_iteration/fallible.incn"),
    )
}

/// A distinct legacy-buildable statement-loop witness preserves the full failed supplemental control above unchanged.
#[test]
fn structural_statement_iteration_hooks_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("structural-statement-iteration-hooks")?,
        &fixture.sysroot,
        &fixture.formatting,
        include_str!("../../../incan_test_support/fixtures/native_iteration/structural_statements.incn"),
    )
}

//! Assignment-three parity for generator set conversion and immutable descriptor constants.

use super::{corpus, driver_fixture};

/// Set construction consumes the original generators and keeps the same distinct values as legacy.
#[test]
fn generator_set_original_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let graph = driver_fixture()?;
    corpus::check_builtin_source(
        &graph.driver_binary("debug"),
        &graph.scratch("calls3-generator-set-original")?,
        &graph.sysroot,
        &graph.formatting,
        include_str!(
            "../../../incan_test_support/fixtures/behavior/cli_values_and_calls/set_constructor_over_generator.incn"
        ),
    )
}

/// Nested model constants preserve empty frozen fields, comparisons, and absent optional values.
#[test]
fn descriptor_constant_original_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let graph = driver_fixture()?;
    corpus::check_builtin_source(
        &graph.driver_binary("debug"),
        &graph.scratch("calls3-descriptor-constant-original")?,
        &graph.sysroot,
        &graph.formatting,
        include_str!(
            "../../../incan_test_support/fixtures/behavior/snapshots_values_and_control_flow/descriptor_const_with_empty_frozen_list.incn"
        ),
    )
}

//! Assignment-three parity for source slice hooks, including the generic adopter in the complete original.

use super::{corpus, driver_fixture};

/// Explicit and omitted slice parts keep the original's generic source hook and builtin slice output.
#[test]
fn original_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let graph = driver_fixture()?;
    corpus::check_builtin_source(
        &graph.driver_binary("debug"),
        &graph.scratch("calls3-slice-hook-original")?,
        &graph.sysroot,
        &graph.formatting,
        include_str!(
            "../../../incan_test_support/fixtures/behavior/emit_calls_and_builtins/slice_syntax_calls_the_getslice_hook.incn"
        ),
    )
}

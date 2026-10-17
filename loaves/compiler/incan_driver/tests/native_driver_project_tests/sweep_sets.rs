//! Live legacy parity for canonical set conversions from checked iterable sources.

use super::{corpus, driver_fixture};

/// A generator's yielded items are collected once into the set legacy constructs.
#[test]
fn set_generator_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let graph = driver_fixture()?;
    let root = graph.scratch("sweep-set-generator")?;
    corpus::check_builtin_source(
        &graph.driver_binary("debug"),
        &root,
        &graph.sysroot,
        &graph.formatting,
        include_str!(
            "../../../incan_test_support/fixtures/behavior/cli_values_and_calls/set_constructor_over_generator.incn"
        ),
    )
}

/// The original frozen source and optional-element constructor matrix keeps its legacy bytes.
#[test]
fn set_constructor_matrix_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let graph = driver_fixture()?;
    let root = graph.scratch("sweep-set-matrix")?;
    corpus::check_builtin_source(
        &graph.driver_binary("debug"),
        &root,
        &graph.sysroot,
        &graph.formatting,
        include_str!(
            "../../../incan_test_support/fixtures/behavior/snapshots_collections_and_strings/set_constructor.incn"
        ),
    )
}

/// Frozen source set conversion keeps live legacy parity independently of subsequent indexed operations.
#[test]
fn frozen_collection_conversion_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let graph = driver_fixture()?;
    let root = graph.scratch("sweep-frozen-set")?;
    corpus::check_builtin_source(
        &graph.driver_binary("debug"),
        &root,
        &graph.sysroot,
        &graph.formatting,
        "const NAMES: FrozenList[str] = [\"a\", \"b\", \"a\"]\n\ndef main() -> None:\n    unique: Set[str] = set(NAMES)\n    println(len(unique))\n    println(\"a\" in unique)\n",
    )
}

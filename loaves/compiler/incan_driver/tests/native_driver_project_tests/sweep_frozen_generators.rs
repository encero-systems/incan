//! Live legacy parity for generator expressions whose first source is a frozen collection.

use super::{corpus, driver_fixture};

/// The complete original retains owned keys and elements through frozen-source comprehension and generator reads.
#[test]
fn frozen_collection_original_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let graph = driver_fixture()?;
    corpus::check_builtin_source(
        &graph.driver_binary("debug"),
        &graph.scratch("sweep-frozen-generator-original")?,
        &graph.sysroot,
        &graph.formatting,
        include_str!(
            "../../../incan_test_support/fixtures/behavior/snapshots_collections_and_strings/const_frozen_collection_membership_and_iteration.incn"
        ),
    )
}

/// Evaluating a frozen first-source snapshot does not run the generator's element expression before consumption.
#[test]
fn frozen_source_keeps_generator_body_deferred() -> Result<(), Box<dyn std::error::Error>> {
    let graph = driver_fixture()?;
    corpus::check_builtin_source(
        &graph.driver_binary("debug"),
        &graph.scratch("sweep-frozen-generator-deferred")?,
        &graph.sysroot,
        &graph.formatting,
        r#"const NUMBERS: FrozenList[int] = [3, 1]

def observe(value: int) -> int:
    println(f"visit {value}")
    return value * 2

def main() -> None:
    deferred = (observe(value) for value in NUMBERS)
    println("built")
    println(list(deferred))
"#,
    )
}

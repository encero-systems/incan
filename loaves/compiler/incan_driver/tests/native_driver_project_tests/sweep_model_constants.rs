//! Live parity for immutable model constants with nested frozen values.

use super::{corpus, driver_fixture};

/// Frozen equality compares the real baked wrappers, including sequence order and unchanged field owners.
#[test]
fn frozen_collection_equality_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let graph = driver_fixture()?;
    corpus::check_builtin_source(
        &graph.driver_binary("debug"),
        &graph.scratch("sweep-frozen-equality")?,
        &graph.sysroot,
        &graph.formatting,
        r#"const FIRST: FrozenList[int] = [1, 2]
const SAME: FrozenList[int] = [1, 2]
const REVERSED: FrozenList[int] = [2, 1]
const KEYS: FrozenSet[int] = {1, 2}
const KEYS_AGAIN: FrozenSet[int] = {1, 2}
const TABLE: FrozenDict[int, int] = {1: 2}
const TABLE_AGAIN: FrozenDict[int, int] = {1: 2}

model Snapshot:
    items: FrozenList[int]

def main() -> None:
    println(FIRST == SAME)
    println(FIRST != REVERSED)
    println(KEYS == KEYS_AGAIN)
    println(TABLE != TABLE_AGAIN)
    value = Snapshot(items=FIRST)
    println(value.items == SAME)
    println(value.items.len())
"#,
    )
}

/// The untouched original preserves nested model constants, an empty frozen collection, and an Option field.
#[test]
fn descriptor_model_constant_original_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let graph = driver_fixture()?;
    corpus::check_builtin_source(
        &graph.driver_binary("debug"),
        &graph.scratch("sweep-descriptor-const")?,
        &graph.sysroot,
        &graph.formatting,
        include_str!(
            "../../../incan_test_support/fixtures/behavior/snapshots_values_and_control_flow/descriptor_const_with_empty_frozen_list.incn"
        ),
    )
}

/// Repeated reads are independent values and a constant alias retains the initial fields.
#[test]
fn model_constant_value_reads_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let graph = driver_fixture()?;
    corpus::check_builtin_source(
        &graph.driver_binary("debug"),
        &graph.scratch("sweep-model-const-values")?,
        &graph.sysroot,
        &graph.formatting,
        r#"@derive(Clone, Eq, Descriptor)
model Version:
    major: int
    minor: int

const INITIAL: Version = Version(major=0, minor=1)
const ALIAS: Version = INITIAL

def main() -> None:
    mut changed = INITIAL
    changed.major = 9
    println(changed.major)
    println(INITIAL.major)
    println(ALIAS.minor)
"#,
    )
}

//! Live legacy parity for indexed values in scalar operations and tag comparisons.

use super::{check_project_modules_case, corpus, driver_fixture};

/// The frozen original's indexed reads retain live legacy parity independently of later generator operations.
#[test]
fn frozen_index_operations_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let graph = driver_fixture()?;
    let original = include_str!(
        "../../../incan_test_support/fixtures/behavior/snapshots_collections_and_strings/const_frozen_collection_membership_and_iteration.incn"
    );
    let (indexed_reads, _) = original
        .split_once("    println(\" \".join(sorted([key.upper() for key in TABLE])))\n")
        .ok_or("frozen original lost its generator boundary")?;
    corpus::check_builtin_source(
        &graph.driver_binary("debug"),
        &graph.scratch("sweep-frozen-projections")?,
        &graph.sysroot,
        &graph.formatting,
        indexed_reads,
    )
}

/// Sorted indexed enums and Options compare the materialized element's tag exactly once.
#[test]
fn sorted_index_operations_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let graph = driver_fixture()?;
    corpus::check_builtin_source(
        &graph.driver_binary("debug"),
        &graph.scratch("sweep-sorted-projections")?,
        &graph.sysroot,
        &graph.formatting,
        include_str!(
            "../../../incan_test_support/fixtures/behavior/snapshots_traits_and_generics/sorted_list_of_ordered_types.incn"
        ),
    )
}

/// A list reached through a generic model field supplies the indexed scalar used in arithmetic.
#[test]
fn module_field_index_operations_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_modules_case(
        "sweep-module-field-projections",
        include_str!(
            "../../../incan_test_support/fixtures/behavior/emit_declarations_and_modules/constructor_arguments_name_field_types_the_module_does_not_import/main.incn"
        ),
        &[
            (
                "ids.incn",
                include_str!(
                    "../../../incan_test_support/fixtures/behavior/emit_declarations_and_modules/constructor_arguments_name_field_types_the_module_does_not_import/ids.incn"
                ),
            ),
            (
                "records.incn",
                include_str!(
                    "../../../incan_test_support/fixtures/behavior/emit_declarations_and_modules/constructor_arguments_name_field_types_the_module_does_not_import/records.incn"
                ),
            ),
        ],
        b"0\ntrue\n1\ntrue\n1\n0\nnone\n1\np\n0\n3\n",
    )
}

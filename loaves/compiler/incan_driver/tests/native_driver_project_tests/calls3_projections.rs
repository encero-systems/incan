//! Assignment-three parity for indexed model fields and sorted nominal values.

use super::{check_project_modules_case, corpus, driver_fixture};

/// The complete original keeps ordered enum and Option values while reading indexed elements.
#[test]
fn sorted_original_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let graph = driver_fixture()?;
    corpus::check_builtin_source(
        &graph.driver_binary("debug"),
        &graph.scratch("calls3-sorted-original")?,
        &graph.sysroot,
        &graph.formatting,
        include_str!(
            "../../../incan_test_support/fixtures/behavior/snapshots_traits_and_generics/sorted_list_of_ordered_types.incn"
        ),
    )
}

/// Module-owned field types keep the full original's indexed reads and legacy output.
#[test]
fn module_fields_original_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_modules_case(
        "calls3-module-fields-original",
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

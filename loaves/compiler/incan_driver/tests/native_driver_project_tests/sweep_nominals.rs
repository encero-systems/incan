//! Live legacy parity for source carrier identities across same-spelled module declarations.

use super::{census, check_project_modules_case};

/// Same-spelled source owners keep their declaring modules through omitted argument defaults.
#[test]
fn same_named_defaults_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_modules_case(
        "sweep-same-named-defaults",
        include_str!(
            "../../../incan_test_support/fixtures/behavior/lowering_declarations/default_type_constructors_keep_module_identity/main.incn"
        ),
        &[
            (
                "a.incn",
                include_str!(
                    "../../../incan_test_support/fixtures/behavior/lowering_declarations/default_type_constructors_keep_module_identity/a.incn"
                ),
            ),
            (
                "b.incn",
                include_str!(
                    "../../../incan_test_support/fixtures/behavior/lowering_declarations/default_type_constructors_keep_module_identity/b.incn"
                ),
            ),
        ],
        b"1\n2\n",
    )?;
    census::require_named_refusal(
        "cli_dependencies",
        "dependency_same_named_declarations_in_two_modules",
        "unsupported Body IR nominal type Product",
    )?;
    census::require_named_refusal(
        "lowering_dependencies",
        "provider_nominal_types_and_unions_issue892",
        "unsupported Body IR union member conversion",
    )
}

/// Private source defaults and same-spelled classes keep exact storage owners through facade imports.
#[test]
fn same_named_vaults_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_modules_case(
        "sweep-same-named-vaults",
        include_str!(
            "../../../incan_test_support/fixtures/behavior/emit_declarations_and_modules/private_default_fields_across_modules/main.incn"
        ),
        &[
            (
                "text_vaults.incn",
                include_str!(
                    "../../../incan_test_support/fixtures/behavior/emit_declarations_and_modules/private_default_fields_across_modules/text_vaults.incn"
                ),
            ),
            (
                "number_vaults.incn",
                include_str!(
                    "../../../incan_test_support/fixtures/behavior/emit_declarations_and_modules/private_default_fields_across_modules/number_vaults.incn"
                ),
            ),
            (
                "public_api.incn",
                include_str!(
                    "../../../incan_test_support/fixtures/behavior/emit_declarations_and_modules/private_default_fields_across_modules/public_api.incn"
                ),
            ),
            (
                "vault_facade.incn",
                include_str!(
                    "../../../incan_test_support/fixtures/behavior/emit_declarations_and_modules/private_default_fields_across_modules/vault_facade.incn"
                ),
            ),
            (
                "sealed_models.incn",
                include_str!(
                    "../../../incan_test_support/fixtures/behavior/emit_declarations_and_modules/private_default_fields_across_modules/sealed_models.incn"
                ),
            ),
        ],
        b"public sealed r1\nopen 2\ntext r7 6\n5 provider-default 42\nexported r7 3\n",
    )
}

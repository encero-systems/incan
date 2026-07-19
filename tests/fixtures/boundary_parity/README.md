# Boundary Parity Fixtures

These fixtures keep boundary-identity regressions compact. They model failure families from the 0.3 RC cycle onward with small synthetic packages instead of adding one downstream-shaped regression for every historical bug.

- `boundary_parity_preserves_dependency_owned_union_helpers_through_facade` covers provider-owned union wrappers through facades, aliases, list arguments, methods, and generated Rust ownership.
- `boundary_parity_preserves_decorated_alias_partial_identity_through_facade` covers decorated callable identity, aliases, partial presets, direct source imports, facade source imports, source test batches, and provider/facade/consumer package boundaries.
- `boundary_parity_preserves_enum_method_defaults_through_facade` covers dependency-owned enum methods and materialized default arguments through provider/facade/consumer package boundaries.
- `boundary_parity_preserves_absolute_crate_public_types_issue882` covers absolute sibling-module imports, public model fields, enum variants, direct check/build, library build/re-export, and test-batch parity.
- `boundary_parity_activates_dependency_vocab_across_check_fmt_and_test` covers dependency-provided vocab activation through `--check`, `fmt --check`, and `incan test`.
- `test_qualified_partial_constructor_presets_cross_package_const_metadata_issue699` covers source-qualified constructor partials whose generated const-safe metadata depends on provider-owned model fields.
- Existing synthetic Rust callback tests in `cli_integration` cover Rust metadata/callback planning without adding heavyweight downstream crates to Incan's regression lane.

When adding boundary coverage, extend these fixture families before adding another one-off downstream-shaped test. The goal is fewer tests with stronger semantic coverage, not a larger slow suite.

# Boundary Parity Fixtures

These fixtures keep the 0.4 boundary-identity work compact. They model the failure families from the 0.3 RC cycle with small synthetic packages instead of adding one downstream-shaped regression for every historical bug.

- `boundary_parity_preserves_dependency_owned_union_helpers_through_facade` covers provider-owned union wrappers through facades, aliases, list arguments, methods, and generated Rust ownership.
- `boundary_parity_preserves_decorated_alias_partial_identity_through_facade` covers decorated callable identity, aliases, partial presets, and provider/facade/consumer package boundaries.
- `boundary_parity_activates_dependency_vocab_across_check_fmt_and_test` covers dependency-provided vocab activation through `--check`, `fmt --check`, and `incan test`.
- Existing synthetic Rust callback tests in `cli_integration` cover Rust metadata/callback planning without adding heavyweight downstream crates to Incan's regression lane.

When adding boundary coverage, extend these fixture families before adding another one-off downstream-shaped test. The goal is fewer tests with stronger semantic coverage, not a larger slow suite.

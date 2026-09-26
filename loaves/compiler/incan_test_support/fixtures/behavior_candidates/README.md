# Behavior-fixture candidates

A candidate is a behavior fixture, written in the format of `../behavior/README.md`, that cannot be admitted to a
`fixtures/behavior/<area>/` yet. Nothing runs it: the runner (`incan_test_support::behavior_fixtures::discover`)
reads only `fixtures/behavior/<area>/` for the areas declared in `scripts/test_inventory/dispositions.json`
(`fixture_roots`), and the inventory collector reads the same areas, so a file here is neither discovered, checked,
nor counted. The inventory rows that refer to a candidate (`open` rows in `dispositions.json`) name its path here.

A candidate lives here rather than in a scratch folder so that a reviewer, and the lane that finally admits it, can
read the program and its expected observables in the tree; each is verified standalone (`incan oven bake --project .`,
then `incan run src/main.incn`) at the time it was written. Admitting one means moving it into an area, pointing its
rows' `twin` at the new path, and running the area's root.

## Why each group waits

- `snapshots/` (twins of `loaves/compiler/incan_emit/tests/codegen_snapshot_tests.rs` retire tests, #1701):
  - programs importing through `rust::` (`comprehension_over_rust_iterator`, `list_constructor_sources`,
    `rust_associated_call_in_elif`, `rust_derive_passthrough`, `rust_duration_coercion`,
    `rust_interop_associated_functions`, `rust_interop_field_access`, `rust_path_type_return`,
    `rust_result_match_scrutinee`, `rust_std_imports`, `rust_supertrait_imported`,
    `rust_trait_import_without_metadata_unused`, `titlecase_var_not_type`) pass standalone and wait for a Cargo-free
    bake under the compiler suite: a `rust::` program needs source-current project inspection authority, which only an
    explicit bake of its project records, and that bake runs Rust inspection through Cargo (see *Provider bakes and
    Cargo* in `../behavior/README.md`).
  - project fixtures whose provider itself needs the compatibility publisher (`compiled_provider_fallible_stream/`,
    `pub_import_widgets/`) pass after a manual provider bake and wait for the same Cargo-free bake.
  - halves of a twinned program that the route refuses (`model_alias_constructor_pattern`): the rest of the program is twinned in an area; the row names the half held here.
  - `str_over_arithmetic_expression`: no retire row; it pins a route bug met while twinning. `str(a + b)` is accepted
    by `incan check` and the generated Rust fails to compile (E0277 `cannot add String to {integer}`: the conversion is
    applied to the right operand only). Admitted to an area when the route renders the whole expression.
- `cli/` (twins of `loaves/toolchain/incan-cli/tests/integration_tests.rs` retire tests, #1695): three `rust::`
  programs (`decorated_rust_generic_mutable_reference_args`, `rust_f32_argument_coercion`,
  `rust_generic_mutable_reference_args`) that pass standalone and wait for the Cargo-free bake; the rows say so. The
  fourth of that lane's candidates, `dependency_helper_defaults_and_unions/`, was admitted by #1699 and is a live
  fixture in `../behavior/cli_dependencies/`.
- `dependencies/` (#1699): `rust_std_path_queries.incn`, a `rust::std::path::Path` program with no retire row (it was
  the probe for a `rust::` area that was withdrawn), and `transitive_provider_chain/`, a project whose provider has a
  provider of its own; under the suite that provider's bake goes through the compatibility publisher's test-dependency
  envelope and reaches Cargo. Both wait for the Cargo-free bake. That lane also held byte-identical copies of the three
  `cli/` programs; they are kept once, under `cli/`.

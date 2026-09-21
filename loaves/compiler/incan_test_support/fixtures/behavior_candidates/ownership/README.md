# Parked candidates: ownership twins

Programs written as twins of retire-class tests in `loaves/compiler/incan_emit/src/ownership.rs`,
`loaves/compiler/incan_emit/src/conversions.rs`, `loaves/compiler/incan_emit/src/trait_bound_inference.rs` and
`loaves/compiler/incan_emit/tests/implicit_borrowing_codegen_tests.rs` that cannot run under the compiler suite today. They are not under `fixtures/behavior/` and no root discovers
them; each header says which rows it is for and why it waits. The rows in `scripts/test_inventory/dispositions.json` are `open` and name the candidate.

- `implicit_borrowing_clone_counter/`, `rust_into_bound_string_variables/`,
  `rust_list_elements_converted_into_a_vec/`, `rust_string_parameters_by_value_and_in_a_field/`: project fixtures with
  an in-fixture Rust crate (`[rust-dependencies]`). `rust_box_as_ref_argument.incn`, `rust_duration_fields_to_rust_parameters.incn`,
  `rust_string_concatenation.incn`, `rust_value_assigned_to_a_new_name.incn`: single files importing through `rust::`.
  A `rust::` program needs source-current inspection authority, which only an explicit bake records, and that bake
  runs Rust inspection through Cargo (54 launches were counted on the guard for one file). They were not executed:
  Cargo is ruled out on the machine that wrote them, so the expected lines are reasoned from the programs (the
  clone counts in `implicit_borrowing_clone_counter/` are the retired test's own consumer assertions). They land when
  a Cargo-free bake serves `rust::` under the suite.
- `bounds_from_a_generic_model_implementation.incn`: an `incan check`-accepted program the route refuses (a declared
  model bound and an impl-inferred bound are not carried to a generic caller; rustc E0277/E0599). Lands when that is
  fixed.

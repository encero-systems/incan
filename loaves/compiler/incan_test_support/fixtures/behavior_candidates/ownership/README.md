# Parked candidates: ownership twins

Programs written as twins of retire-class tests in `loaves/compiler/incan_emit/src/ownership.rs`,
`loaves/compiler/incan_emit/src/conversions.rs`, `loaves/compiler/incan_emit/src/trait_bound_inference.rs` and
`loaves/compiler/incan_emit/tests/implicit_borrowing_codegen_tests.rs` that cannot run under the compiler suite today.
They are not under `fixtures/behavior/` and no root discovers them; each header says which rows it is for and why it
waits. The rows in `scripts/test_inventory/dispositions.json` are `open` and name the candidate.

None of these programs was executed. Their expected lines are reasoned from the program, the crate beside it and the
compiler source, and each header says so.

- `implicit_borrowing_clone_counter/`, `rust_callback_parameter_forwarded_to_a_mut_parameter/`,
  `rust_into_bound_string_variables/`, `rust_list_elements_converted_into_a_vec/`,
  `rust_string_parameters_by_value_and_in_a_field/`, `rust_trait_object_view_to_a_reference_parameter/`: project
  fixtures with an in-fixture Rust crate (`[rust-dependencies]`). `generic_functions_over_a_non_clone_rust_value.incn`,
  `rust_box_as_ref_argument.incn`, `rust_duration_fields_to_rust_parameters.incn`, `rust_string_concatenation.incn`,
  `rust_value_assigned_to_a_new_name.incn`: single files importing through `rust::`. A `rust::` program needs
  source-current inspection authority, which only an explicit bake records, and that bake runs Rust inspection through
  Cargo (54 launches were counted on the guard for one file). They land when a Cargo-free bake serves `rust::` under
  the suite.
- `bounds_from_a_generic_model_implementation.incn` and
  `fallible_iteration_over_reader_chunks_from_a_bare_generic.incn`: `incan check`-accepted programs the route refuses,
  the #1280 family (a bound declared on a model, inferred for its implementation or carried by a stdlib implementation
  is not carried to a generic caller that names the parameter bare; rustc E0277/E0599). They land when that is fixed.

## `implicit_borrowing_clone_counter/`: how the counts are derived

`borrow_fixture::Item` counts every call of `Clone::clone` on a node, and the clone of a `Table` clones its map, so a
copy costs one count per node copied: `document()` (`Table{project: Table{count: Integer(7)}}`) costs 3, its `project`
child costs 2, an `Integer` leaf costs 1. The retired `inferred_traversal_compiles_and_runs_without_any_tree_clones`
consumer asserts the first three of these (`observe` 0, `Reader.read` 1, `Holder.snapshot` +3); the rest follow the
same arithmetic and the inference in `loaves/compiler/incan_ir/src/borrow_inference.rs`, whose `SharedUse` proof
accepts only shared receiver contracts and same-root cursor reassignments, and whose `Escapes` pass keeps the owned ABI
for a function named outside a direct call or called with overlapping arguments.

| Line | Program step | Copies | Why |
|---|---|---|---|
| `observe` | `observe` -> `traverse`, a cursor over `get` children | 0 | `get(&self) -> Option<&Item>` is a shared receiver borrow; `item` and `current` are lent |
| `is_table` | `Holder.observe` -> private `is_table(self.item)` | 0 | `is_table` is only ever a direct callee, so it takes `&Item`; the field is lent |
| `inspect` | `inspect` -> `classify` -> `is_table` | 0 | the private helpers take `&Item` and the public entry lends its parameter |
| `ref local` | `Holder.view`: `current: &Item = self.item` | 0 | a reference-typed local is a borrow, never a materialization |
| `walk` | `Holder.walk`: cursor started from `self.item` | 0 | the cursor is proven shared and the owner is not read again in the body |
| `selected` | `Reader.read` -> `select`, ending in `current.clone()` | +1 = 1 | the explicit clone copies the `Integer(7)` leaf; the walk to it lends |
| `owned child` | `take_child`: `current = child` from `child()` | +2 = 3 | `child()` returns an owned `Item` (`get(key).cloned()`), copying the `project` subtree; the cursor stays owned |
| `snapshot` | `Holder.snapshot`: `current = self.item`, then the field is replaced | +3 = 6 | the owner is written after the read, so the read materializes a copy of the whole tree |
| `ref param` | `is_table_ref(item: &Item)` directly and through `check_ref` | 0 | a declared reference parameter borrows its argument, in a return too |
| `ref traverse` | `traverse_ref(item: &Item, ...)` reassigning its cursor | 0 | `current = child` keeps the borrowed shape |
| `callback` | `callback()` returns `is_table_callback`, called on a fresh tree | 0 | the escaping classifier keeps the owned ABI, so the temporary is moved in |
| `pair` | `observe_pair` -> `classify_pair(item, item)` | +3 = 9 | overlapping arguments keep `classify_pair` owned; the first `item` is not its last use and is copied whole, the second is moved |
| `select` | `select_owned`: the cursor is returned | 0 | a returned cursor is an escape, so `item` and `current` stay owned and move |
| `clear` | `clear`: `current.clear()`, then `child_count()` is 0 | 0 | `clear(&mut self)` is not a shared contract, so the parameter stays owned and moves |
| `trait callback` | `Factory.callback` default returns `is_table_callback`, called once | 0 | the same escaping classifier, reached through a trait default |

`is_table` and `is_table_callback` are two functions on purpose: a classifier that escapes as a callable value keeps the
owned ABI, and one that only a direct call reaches is lent its argument. One function cannot show both.

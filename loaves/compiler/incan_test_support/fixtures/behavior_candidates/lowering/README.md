# Lowering candidates

Candidates written while disposing of the `loaves/compiler/incan_ir/**` retire rows (#1561). The format is that of
`../../behavior/README.md`; nothing here is discovered, run or counted by the harness or the inventory. Each `open`
row in `scripts/test_inventory/dispositions.json` that refers to one of these names its path.

## Programs the checker accepts and the route refuses

Each of these passes `incan check` and fails in the generated Rust (reproduced standalone on 2026-09-21, direct
`incan oven bake --project .` with no Cargo). They are admitted to an area once the route compiles them.

- `clone_bound_with_std_import.incn`: `from std.derives.copying import Clone` and a generic bound `T with Clone` over a
  `@derive(Clone)` model (E0277, `copying::Clone` is not implemented; the #1727 family). The bound-free half is live
  as `../../behavior/lowering_expressions/clone_dispatch_with_std_import.incn`.
- `generic_list_index_without_clone_bound.incn`: `def first[K](items: list[K]) -> K: return items[0]` (E0308,
  `expected K, found &K`). With `K with Clone` the same program runs; the live twin
  `../../behavior/lowering_declarations/generic_nominal_arguments.incn` carries that bound.
- `frozen_dict_lookup_and_frozen_list_join.incn`: indexing a const `FrozenDict` (E0608), `contains_key` on it
  (E0277), and `str.join` over a comprehension of a const `FrozenList[str]` (E0308).
- `set_of_underived_enum.incn`: a model field `set[Tag]` over an enum with no derives (E0277, `Tag: Eq` and
  `Tag: Hash`). The live twin `../../behavior/lowering_declarations/serde_derive_propagation.incn` derives them.
- `trait_default_names_its_own_module_types/`: an imported trait's default method returns a type of its own module
  that the adopter never imports (E0425 in the adopter's expansion). The stdlib flavor of the same fact is live as
  `../../behavior/lowering_declarations/stdlib_trait_default_names_its_own_module_type.incn`.
- `trait_default_constructs_module_type_across_modules/`: an imported trait's default method constructs a model of its
  own module by named fields, expanded into an adopter in another module that imports that model (E0423, the
  expansion spells a tuple-struct call). The local-binding half of that default body is live as
  `../../behavior/lowering_declarations/trait_default_local_shadows_module_type/`.
- `imported_partial_keeps_target_defaults/`: an imported top-level partial called without its target's defaulted
  residual argument, from the package root and from a nested namespace (E0061). The live twin
  `../../behavior/lowering_dependencies/parent_namespace_and_qualified_partial_issue948/` passes that argument.
- `fallible_iterator_chain_without_trait_import/`: `map(...).collect()` on a `FallibleIterator` adopter returned by a
  dependency, in a consumer that never imports the trait (E0599, trait not in scope). The live twin
  `../../behavior/lowering_dependencies/fallible_iterator_through_public_library/` imports it.

## Programs importing through `rust::`

Not verified: a `rust::` program needs source-current project inspection authority, which only an explicit bake of
its project records through Cargo, and Cargo was not available where these were written; `incan check` refuses them
too before that bake. They wait for a Cargo-free bake under the suite (see *Provider bakes and Cargo* in
`../../behavior/README.md`) and are read, not run, until then.

- `rust_std_time_receivers.incn`: type-like receivers (`Duration.from_secs`, nested in a method argument) and a
  constant receiver (`UNIX_EPOCH`).
- `rust_box_generic_receiver.incn`: `Box[Node[T]].as_ref()` keeps the generic argument.
- `rust_std_collections_argument_coercions.incn`: a borrowed `str` argument, a lookup argument that keeps its shape,
  and an argument taken by mutable reference.
- `rust_named_field_constructor.incn`: a Rust struct constructed by named fields.
- `rusttype_interop_edges.incn`: `into str via` and `from str via` edges on a `rusttype` over `String`.

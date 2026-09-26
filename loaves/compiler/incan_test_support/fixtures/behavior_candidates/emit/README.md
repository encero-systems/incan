# Behavior-fixture candidates: twins of the emitter unit tests

A candidate is a behavior fixture, written in the format of `../../behavior/README.md`, that cannot be admitted to a
`fixtures/behavior/emit_<topic>/` area yet. Nothing runs it: the runner and the inventory collector read only the
areas declared in `scripts/test_inventory/dispositions.json` (`fixture_roots`), so a file here is neither discovered,
checked, nor counted. The `open` rows of the retire-class tests under `loaves/compiler/incan_emit/src/emit/**` that
refer to a candidate name its path here.

These candidates were written from the language reference; none of the candidates was executed, and the suite cannot
run any of them today. Verify one standalone (`incan oven bake --project .`, then `incan run` on its entry point)
before admitting it; expect the expected lines to need a correction or two.

## Why each waits

- `rust_*.incn`: programs importing through `rust::std`. Several are `rust::std` analogs of a shape the retired
  test pinned over a third-party crate (prost, tokenizers, polars, bevy, an external builder); the row says which.
  They wait for a Cargo-free bake under the compiler suite: a `rust::` program needs source-current project
  inspection authority, which only an explicit bake of its project records, and that bake runs Rust inspection
  through Cargo (see *Provider bakes and Cargo* in `../../behavior/README.md`).
- `transitive_dependency_nominal_routes/`: a project whose provider has a provider of its own (the consumer also
  declares that inner package directly, to name its model). Under the suite the provider's bake goes through the
  compatibility publisher's test-dependency envelope, which runs Cargo.
- `trait_default_method_mutable_parameter.incn`: a route bug (filed separately). A `mut` parameter is passed so the
  caller sees the change (`incan_ir` `Mutability::Mutable`; a free function's `mut items: list[int]` works that way),
  so the expected lines follow that contract: the appended item is visible to the caller. `incan check` accepts the
  trait default method; the trait-default emission path passes the parameter by value with no `mut`, so the generated
  Rust is refused (E0596). Admitted once the route keeps the contract on trait default method parameters.
- `checked_c_bindings_over_libc.incn`: a route bug and a runner gap. `incan check` accepts a `binding` program;
  `incan run`, `incan build` and `incan oven bake` refuse it with `syntax error: Expected declaration, found
  Ident("binding")`, with and without an `[interop.c]` manifest section. The CLI's own C test reaches a run through
  `incan lock` and `incan oven interop bake`, which the behavior runner does not perform.

## Route bugs found when the areas first ran

These programs were fixtures, or parts of fixtures, that the first replay of the `emit_*` areas refused. Each twin
kept what its retired test asserted where the failure lay outside it; where it did not, the program moved here and its
rows are `open`.

- `dependency_same_named_declarations_in_two_modules/`: moved from `emit_dependency_unions_and_surfaces/`; the
  provider's bake stops at "emitted union ... binds Product to two canonical identities" when two of its modules
  declare `Product` and `Answer = Product | int` at the same source positions. Two rows.
- `dependency_method_returning_a_union/`: the `Box.answer()` part of
  `dependency_unions_across_the_package_boundary`; a dependency model method's `int | str` result keeps the provider's
  union type at a consumer's own `int | str` parameter (E0308), while a provider function's union result is accepted
  there. One row.
- `static_dict_get_with_a_reused_str_parameter.incn`: a str parameter read again after `get` on a static dict is moved
  into the lookup (E0382). `static_collection_method_mutations.incn` keeps its twin by calling `get` from a helper
  whose parameter is at its last use. No row.
- `str_const_returned_as_frozen_str.incn`: a const declared `str` returned at a `FrozenStr | int` or
  `Option[FrozenStr]` destination is accepted by `incan check` and refused by the run (E0308).
  `isinstance_str_over_const_union_and_option.incn` declares its const `FrozenStr`, the storage its retired test
  names. No row.
- `model_named_like_a_std_web_type.incn`: a model named `Response` is an unknown symbol in its own static method's
  return annotation (the stdlib surface-type check does not find it); `static_method_with_str_argument.incn` names its
  model `Page`. No row.

# Behaviour-fixture candidates: twins of the emitter unit tests

A candidate is a behaviour fixture, written in the format of `../../behavior/README.md`, that cannot be admitted to a
`fixtures/behavior/emit_<topic>/` area yet. Nothing runs it: the runner and the inventory collector read only the
areas declared in `scripts/test_inventory/dispositions.json` (`fixture_roots`), so a file here is neither discovered,
checked, nor counted. The `open` rows of the retire-class tests under `loaves/compiler/incan_emit/src/emit/**` that
refer to a candidate name its path here.

These candidates were written from the language reference and were **not executed**: the lane that wrote them ran
no compiler binary outside the compiler suite, and the suite cannot run any of them today. Verify one standalone
(`incan oven bake --project .`, then `incan run` on its entry point) before admitting it; expect the expected lines to
need a correction or two.

## Why each waits

- `rust_*.incn`: programs importing through `rust::std`. Several are `rust::std` analogues of a shape the retired
  test pinned over a third-party crate (prost, tokenizers, polars, bevy, an external builder); the row says which.
  They wait for a Cargo-free bake under the compiler suite: a `rust::` program needs source-current project
  inspection authority, which only an explicit bake of its project records, and that bake runs Rust inspection
  through Cargo (see *Provider bakes and Cargo* in `../../behavior/README.md`).
- `transitive_dependency_nominal_routes/`: a project whose provider has a provider of its own. Under the suite that
  provider's bake goes through the compatibility publisher's test-dependency envelope, which runs Cargo.
- `trait_default_method_mutable_parameter.incn`: a route bug. `incan check` accepts a trait default method with a
  `mut items: list[int]` parameter that appends to it; the generated Rust drops the parameter's `mut` (E0596).
  Admitted once the route keeps it.
- `checked_c_bindings_over_libc.incn`: a route bug and a runner gap. `incan check` accepts a `binding` program;
  `incan run`, `incan build` and `incan oven bake` refuse it with `syntax error: Expected declaration, found
  Ident("binding")`, with and without an `[interop.c]` manifest section. The CLI's own C test reaches a run through
  `incan lock` and `incan oven interop bake`, which the behaviour runner does not perform.

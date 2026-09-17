# Oven control plane

The Incan-authored half of Oven: the decisions a bake makes before any compiler runs. The Rust host under `src/oven/` owns the store, leases, direct `rustc` execution and the receipts; this project owns the reading of `loaf.toml`, the selection of a native plan, and the activation of Rust dependency features, and it is written in Incan so the build system's own rules are expressed in the language it builds. Generated Rust is never edited.

## What it decides

- **Intake** (`intake.incn`, `manifest_errors.incn`, `source_inventory.incn`): reads a local `loaf.toml` closure into a topology of authored aliases and canonical roots. Manifest bytes and their SHA-256 are source evidence only, never an effective action key; a parsed request excludes comments and formatting. A selected package inventory records build-unit presence only to produce one inert-script warning, never an execution candidate. Every refusal is an `IntakeError` naming the manifest, the field path, the TOML location where one exists, a category and a detail.
- **Selection** (`plan.incn`, `plan_json.incn`, `source_unit.incn`): `select_native` is a pure, partial selector over intact closures. It compares explicit runtime, target, toolchain, profile and feature facts and checked provider semantics; a candidate that cannot be proven compatible is refused, never guessed, and equal ranks are reported as ambiguous. `source_unit` binds a batch of source units to the foundations that offer them and calls the selector once per batch.
- **Native compilation** (`native_compilation.incn`): projects the checked native inputs of one compilation — command sections, once-only options, input digests — while keeping physical admission and compilation provenance the host's; it discovers nothing and holds no graph.
- **Rust dependency activation** (`rust_source.incn`, `rust_activation.incn`, `rust_graph.incn`): reads the dependency catalog, evaluates `cfg` predicates and version requirements per unit, and propagates feature demands across the graph to a fixed point, keeping host and target activation separate.
- **Invocation** (`invocation.incn`): turns the CLI flags and environment observations a command was given into the set of restrictions it runs under (locked, frozen, offline and their implications). It reads nothing itself, and a decision admits no lock, plan, network action or publication.

`lib.incn` is the public facade; everything a host calls is re-exported there.

## Wire schemas

The host and this project exchange JSON documents. The schemas this source currently reads and writes:

| Schema | Owner | Purpose |
| --- | --- | --- |
| `incan.oven.selection/1`, `/2` | `plan_json.incn` | one native plan selection; `/2` requires the request's `build_unit_identity` and exact-unit evidence per candidate |
| `incan.oven.source-unit-batch/1`, `/2`, `/3` | `plan_json.incn`, `source_unit.incn` | one selection serving a batch of source units; `/3` adds compiler-runtime needs and grants |
| `incan.oven.native-compilation/2` | `native_compilation.incn` | one direct-`rustc` invocation to validate |
| `incan.oven.rust-policy-exchange/1` | `rust_policy_exchange.incn` | validate one authenticated selected-Rust-graph projection and return exhaustive activations, source inventories and inert-script warnings |

Every wire field is required; an unknown or absent field is a refusal, not a default.

## Layout

```text
loaf.toml               project manifest: two scripts, two Rust dependencies (cfg-expr, semver)
src/*.incn              the modules above, plus lib.incn
src/test_*.incn         one test module per source module
src/acceptance.incn     runs the ten local-intake contracts as one baked program
src/plan_json_main.incn the sealed `core_engine` script: REQUEST RESPONSE file paths, strict UTF-8 exchange
tests/fixtures/         manifests the intake tests read: a cycle, a collision, a plan missing its features, a Rust source tree
```

The intake tests also read the committed `workbench/app`, `workbench/pricing` and `workbench/catalog` projects.

## Running it

From the repository root, with a compiler built from this tree:

```bash
incan check workspaces/oven/src/lib.incn
incan test workspaces/oven
incan test workspaces/oven/src/test_rust_graph.incn
incan test workspaces/oven/src/test_rust_policy_exchange.incn
incan oven bake --project workspaces/oven
```

The bake produces both declared scripts. `core_engine` takes exactly two arguments, `REQUEST RESPONSE`, and performs one selection exchange between those files.
The two focused test commands exercise the Rust graph policy and its strict host-exchange envelope directly. The baked
`acceptance` script runs only the ten local-intake contracts imported by `src/acceptance.incn`; it is not an aggregate
runner for every `test_*.incn` module.

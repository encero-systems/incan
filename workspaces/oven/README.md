# Oven control plane

The Incan-authored half of Oven: the decisions a bake makes before any compiler runs. The Rust crates under `loaves/oven/` own the store, leases, direct `rustc` execution and receipts; this project owns reading `loaf.toml`, selecting a native plan, and activating Rust dependency features. It is written in Incan so the build system's own rules are expressed in the language it builds. Generated Rust is never edited.

## What it decides

- **Intake** (`intake.incn`, `manifest_errors.incn`, `source_inventory.incn`): reads a local `loaf.toml` closure into a topology of authored aliases and canonical roots. Manifest bytes and their SHA-256 are source evidence only, never an effective action key; a parsed request excludes comments and formatting. A selected package inventory records build-unit presence only to produce one inert-script warning, never an execution candidate. Every refusal is an `IntakeError` naming the manifest, the field path, the TOML location where one exists, a category and a detail.
- **Selection** (`plan.incn`, `plan_json.incn`, `source_unit.incn`): `select_native` is a pure, partial selector over intact closures. It compares explicit runtime, target, toolchain, profile and feature facts and checked provider semantics; a candidate that cannot be proven compatible is refused, never guessed, and equal ranks are reported as ambiguous. `source_unit` binds a batch of source units to the foundations that offer them and calls the selector once per batch.
- **Native compilation** (`native_compilation.incn`): projects the checked native inputs of one compilation — command sections, once-only options, input digests — while keeping physical admission and compilation provenance the host's; it discovers nothing and holds no graph.
- **Rust dependency activation** (`rust_source.incn`, `rust_activation.incn`, `rust_graph.incn`): reads dependency catalogs, evaluates `cfg` predicates and version requirements per unit, and propagates feature demands across the graph to a fixed point, keeping host and target activation separate. Authenticated roots carry the requested features, default-feature choice and declaration owner; physical units carry only their observed effective features, and source ownership remains a separate host-validated fact.
- **Invocation** (`invocation.incn`): turns the CLI flags and environment observations a command was given into the set of restrictions it runs under (locked, frozen, offline and their implications). It reads nothing itself, and a decision admits no lock, plan, network action or publication.

`lib.incn` is the public facade and re-exports the host-facing functions.

## Wire schemas

The host and this project exchange JSON documents. The schemas this source currently reads and writes:

| Schema | Owner | Purpose |
| --- | --- | --- |
| `incan.oven.selection/1`, `/2` | `plan_json.incn` | one native plan selection; `/2` requires the request's `build_unit_identity` and exact-unit evidence per candidate |
| `incan.oven.source-unit-batch/1`, `/2`, `/3` | `plan_json.incn`, `source_unit.incn` | one selection serving a batch of source units; `/3` adds compiler-runtime needs and grants |
| `incan.oven.native-compilation/2` | `native_compilation.incn` | one direct-`rustc` invocation to validate |
| `incan.oven.rust-policy-exchange/5` | `rust_policy_exchange.incn` | validate one authenticated selected-Rust-graph projection, including authority-bound root intent and toolchain-bound sysroot externs, typed environment, generated-member and linked-library closure evidence; return exhaustive activations, derived default-feature demands, source inventories and inert-script warnings |
| `incan.oven.loaf-resolution/1` | `loaf_resolve.incn` | source-selection projection: `units` in deterministic Loaf/domain order; each unit contains `loaf`, `version`, `archive_digest`, `domain` (`host` or `target`), sorted `features`, and `target_predicates` (declaration index, target expression, and boolean match). This projection is not a trust admission. |
| `incan.oven.loaf-closure-request/1` | `loaf_closure_exchange.incn` | source dispatch for a pinned snapshot: `pin`, `index_commit`, `names`, `cfg`, index-spelled `roots`, `target` and `host`; returns the shared standalone lock driver's resolution or a refusal with `status` and `reason`. Host staging must extract committed index bytes rather than copy the worktree. The CLI currently requires `--lock` because the engine cannot bake without sealed Rust inspection authority. |

Every wire field is required; an unknown or absent field is a refusal, not a default. A refusal carries a stable `kind` and `fields` path, and may add a `detail` naming the rule that refused; the detail is prose for the operator holding a retained exchange, never something a host branches on.

## Layout

```text
loaf.toml               project manifest: two scripts, no third-party Rust dependencies
src/*.incn              the modules above, plus lib.incn
src/semver_req.incn     pure Incan version parsing, precedence, total ordering and requirement matching
src/cfg_predicate.incn  canonical RFC 119 selectors over caller-supplied cfg facts
src/test_*.incn         one test module per source module
src/acceptance.incn     runs the ten local-intake contracts as one baked program
src/loaf_index.incn     strict pinned Loaf index and root-dependency JSON intake
src/loaf_resolve.incn   semver selection with shared activation and per-domain feature demand
src/loaf_closure_exchange.incn shared standalone/engine resolution over an explicitly pinned snapshot
src/loaf_facts.incn     recorded feature-table closure audit independent of Rust inspection
src/loaf_facts_main.incn pure Incan pin audit driver: PIN_DIRECTORY NAMES_FILE CFG_TRIPLE CFG_FILE
src/loaf_pin_check.incn pin audit and real-root smoke driver: PIN_DIRECTORY NAMES_FILE CFG_FILE
src/plan_json_main.incn the sealed `core_engine` script: REQUEST RESPONSE file paths, strict UTF-8 exchange and schema dispatch
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

The bake produces both declared scripts. `core_engine` takes exactly two arguments, `REQUEST RESPONSE`, and dispatches a Rust-policy request using the exchange schema listed above to its strict policy decoder; other requests retain the native selection decoder.

The two focused test commands exercise the Rust graph policy and its strict exchange envelope directly. The baked `acceptance` script runs only the ten local-intake contracts imported by `src/acceptance.incn`; it is not an aggregate runner for every `test_*.incn` module. The Rust host adapter that supplies authenticated production inputs remains separate from this Incan policy workspace.

## Resolver index pin and policy boundaries

`loaf_lock_main` accepts `PIN_DIRECTORY NAMES_FILE CFG_FILE ROOTS_FILE INDEX_COMMIT`. Its index pin is always caller-supplied, and the staged `revision` must match. SDK root documents use `deps` and optional local `host-projects`; closure root documents use `roots` and retain named refusals while resolving admitted roots together.

`loaf_pin.pinned_revision()` selects incan.pub revision `6ec35e0d7e2d202496e2f7a108bb5111b6a9ff87` from `index`. The pin audit drivers require a caller-owned immutable extraction whose `revision` file names that revision; the marker is a selection check, not an integrity attestation. `read_index` continues to accept explicit fixture directories for tests. Neither driver changes the registry checkout or reads upstream package metadata.

The pure feature audit takes the retained cfg output and its target triple explicitly. It refuses recorded facts for another target rather than guessing their cfg values. A non-weak `x/f` adds local feature `x` only when that feature exists and an optional declaration of `x` applies to the supplied target. `dep:x` and `x?/f` do not add `x`; defaults enter the audit only through recorded membership. The resolver applies the same target eligibility independently in host and target domains.

`semver_req` implements complete SemVer 2.0 versions and comma-conjoined requirements with caret, tilde, equality, ordered comparators, and trailing `*`, `x`, or `X` wildcards. A bare version means caret. Pre-releases need an explicit same-core pre-release comparator and must satisfy every comparator. Build metadata does not affect requirements or SemVer precedence; the resolver preserves the former semver crate's build-metadata tie order when choosing among equal-precedence candidates. Version components retain the unsigned 64-bit range without narrowing to an Incan integer. Requirements retain the replaced parser's 32-comparator limit and ASCII-space syntax.

`cfg_predicate` accepts exact target triples and canonical `cfg(...)` selectors with atoms, quoted key-value atoms, `all`, `any`, and single-argument `not`. It evaluates all branches from the supplied complete snapshot, including feature and debug-assertion facts; it never infers package features or queries the host. Canonical separators are `, ` and ` = `, with no other whitespace outside quoted values. Unsupported functions, invalid arity, malformed quoting, escapes and noncanonical spacing are refusals naming the construct.

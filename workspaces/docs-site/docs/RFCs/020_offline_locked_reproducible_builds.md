# RFC 020: offline / locked / reproducible builds (Cargo policy + generated project contract)

- **Status:** Draft
- **Created:** 2026-01-21
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 013 (dependency + lockfile direction), RFC 015 (project lifecycle CLI), RFC 019 (test runner + CLI)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/38
- **RFC PR:** —
- **Written against:** v0.1
- **Shipped in:** —

## Summary

Define a first-class, user-facing **Cargo policy contract** for Incan that supports enterprise/restricted environments:

- **Cargo policy flags** on `incan build`, `incan run`, and `incan test`:
    - `--offline` (no network)
    - `--locked` (must use an existing lockfile)
    - (optional) `--frozen` (implies offline + locked; mirrors Cargo)
- A **precedence model** for policy (CLI flags + CI-friendly env vars; project config is explicitly out of scope here).
- A **generated-project persistence contract** for `target/incan/**` and `target/incan_tests/**`:
    what is regenerated vs preserved, and where artifacts live.

This RFC intentionally avoids overlap with:

- **RFC 013** (dependency specification + `incan.lock`): this RFC does *not* define `incan.lock` or `incan lock/update`.
- **RFC 015** (project metadata + lifecycle): this RFC does *not* define `incan.toml` schema or project layout/init/new.
- **RFC 019** (test runner + CLI): this RFC does *not* define
test discovery/selection/reporting flags; it only adds Cargo policy flags that constrain the underlying Cargo subprocess invocations used by
  `incan test`.

## Motivation

In enterprise or restricted environments, adopting a compiler/toolchain depends on **predictable, enforceable policy**:

- no unexpected network access during CI or local builds
- deterministic dependency resolution (lockfile enforcement)
- a stable answer to “where are outputs” and “what does the tool overwrite”

Today, Incan uses Cargo under the hood and may trigger network activity (e.g. “Updating crates.io index”) during:

- `incan build` and `incan run` (via `cargo build/run` on a generated project under `target/incan/<name>`)
- `incan test` (via `cargo test` on a generated harness project under `target/incan_tests/...`)

Users can work around this by `cd`-ing into generated Cargo projects and running `cargo --offline --locked ...` themselves, but that undermines the promise of a coherent toolchain and makes CI policy inconsistent.

## Goals

- Provide an official way to run builds/tests with Cargo policies (`--offline`, `--locked`, optionally `--frozen`).
- Make CI reproducibility easy via explicit, composable flags and CI-friendly env vars.
- Specify the generated-project contract (default locations; what is overwritten vs preserved; where artifacts land).
- Keep this change **non-breaking by default** (policy is opt-in).

## Non-Goals (this RFC)

- Replacing Cargo’s resolver or implementing a new dependency solver.
- Designing the `incan.toml` schema for projects (RFC 015).
- Defining `incan.lock` format, `incan lock`, or `incan update` workflows (RFC 013).
- Providing a full vendoring/mirroring solution (but we sketch the direction).

## Terminology

- **Generated project**: the Cargo project emitted by Incan as an intermediate build artifact.
- **Workspace root**: the directory where the user runs `incan ...` (or the nearest project root per future RFC 015).
- **Policy**: constraints applied to underlying Cargo invocation(s) (offline/locked/frozen, plus advanced cargo args).
- **Lockfile**:
    - **Cargo lockfile**: `Cargo.lock` used by Cargo to pin transitive crates.
    - **Incan lockfile** (future): `incan.lock` (RFC 013 direction), a
      source-of-truth lock that can materialize Cargo state.

## Guide-level explanation (how users think about it)

### The mental model

Incan orchestrates Cargo. You can tell Incan to enforce the same policies you’d enforce with Cargo:

- **Offline mode** means: “If Cargo would need the network, fail instead.”
- **Locked mode** means: “Do not change the lockfile; if it’s missing or needs changes, fail instead.”

### CLI examples

Build without network:

```bash
incan build src/main.incn --offline
```

Build and require an existing lockfile:

```bash
incan build src/main.incn --locked
```

Strict CI mode (recommended):

```bash
incan build src/main.incn --frozen
incan test tests/ --frozen
```

Run without network (useful in locked-down dev shells):

```bash
incan run src/main.incn --offline
```

Advanced: pass extra Cargo flags (escape hatch):

```bash
incan build src/main.incn --cargo-args "--features" "my_feature" "--no-default-features"
```

### What happens to generated files

Incan generates a Cargo project under `target/` and reuses it across runs. In general:

- Incan **overwrites** generated source and manifests (`Cargo.toml`, `src/**`) to reflect current Incan sources.
- Cargo **manages** build outputs (`target/**` inside the generated project).
- Lockfiles are **preserved** across Incan runs so that `--locked/--frozen` are meaningful.

The precise contract is specified below.

## Reference-level explanation (precise rules)

### CLI surface (normative; additive)

Add Cargo policy flags to the following commands:

- `incan build <FILE> [OUTPUT_DIR]`
- `incan run <FILE>` and `incan run -c "<CODE>"`
- `incan test [PATH]` (in addition to the runner/selection surface defined by RFC 019)

Flags:

- `--offline`
    - Semantics: underlying Cargo invocations must not access the network.
    - Implementation requirement: pass Cargo’s offline semantics through
      (e.g. `cargo ... --offline` and/or `CARGO_NET_OFFLINE=true`).
- `--locked`
    - Semantics: builds/tests must use an existing lockfile and must not modify it.
    - Implementation requirement: pass Cargo’s `--locked` to Cargo invocations.
- `--frozen`
    - Semantics: equivalent to `--offline --locked` (mirrors Cargo’s “no network and no lockfile updates”).
    - If implemented, `--frozen` implies both `--offline` and `--locked` and they are treated as set.
- `--cargo-args <ARGS...>`
    - Semantics: additional arguments forwarded to the underlying Cargo
      invocation **after** policy flags, as an escape hatch.
    - Safety: `incan` should not attempt to validate arbitrary Cargo args beyond basic well-formedness.

### Configuration + precedence (normative)

Policy configuration sources, highest priority first:

1. **CLI flags** on the specific command invocation
2. **Environment variables** (for CI policy)
3. Defaults (no policy enforced)

Environment variables (recommended):

- `INCAN_OFFLINE=1` behaves like `--offline`
- `INCAN_LOCKED=1` behaves like `--locked`
- `INCAN_FROZEN=1` behaves like `--frozen`
- `INCAN_CARGO_ARGS="..."` optional; see parsing rules below

#### `--cargo-args` and `INCAN_CARGO_ARGS` parsing (normative)

**CLI flag** (`--cargo-args`):

The CLI flag accepts multiple arguments that are forwarded to Cargo. Use one of these forms:

```bash
# Multiple separate arguments
incan build src/main.incn --cargo-args "--features" "my_feature" "--no-default-features"

# Or use a separator (recommended for clarity)
incan build src/main.incn -- --features my_feature --no-default-features
```

If `--` is present, all arguments after it are treated as Cargo args (and `--cargo-args` is not needed).

**Environment variable** (`INCAN_CARGO_ARGS`):

The environment variable is split on whitespace. Quoting is **not** supported to avoid cross-platform shell escaping issues.

```bash
# Simple whitespace-separated args
INCAN_CARGO_ARGS="--features my_feature --no-default-features"

# NOT supported (quotes are literal, not parsed):
INCAN_CARGO_ARGS='--features "my feature"'  # broken: passes literal quote chars
```

For arguments containing spaces, use the CLI flag instead of the environment variable.

Notes:

- If `INCAN_FROZEN` is set, it implies offline + locked, regardless of the other two env vars.
- CLI flags override env vars (CI can still enforce by not letting users override flags; that is outside this RFC).

Out of scope (to avoid overlap with RFC 015):

- A project-level config key in `incan.toml` that sets default Cargo policy.
    If and when we add it, it must follow RFC 015’s `incan.toml` approach and
    must not conflict with the CLI/env precedence defined above.

### Generated project locations and persistence (normative)

#### Default output directories

- `incan build <file>` and `incan run <file>` generate under `target/incan/<name>/`.
- `incan test` generates one or more harness projects under `target/incan_tests/**`.

#### Generated project naming (normative)

The `<name>` for a generated project is computed as follows:

1. **Project mode** (when `incan.toml` exists):
   - Use the `[project].name` value from `incan.toml`.
   - Example: `target/incan/`**`my_app`**`/`

2. **Single-file mode** (no `incan.toml`):
   - Use the file's path relative to the current working directory, with path
     separators replaced by `_` and the `.incn` extension removed.
   - Examples (the `<name>` portion is shown in **bold**):
     - `incan run src/main.incn` → `target/incan/`**`src_main`**`/`
     - `incan run examples/main.incn` → `target/incan/`**`examples_main`**`/`
     - `incan run foo/bar/baz.incn` → `target/incan/`**`foo_bar_baz`**`/`
     - `incan run main.incn` → `target/incan/`**`main`**`/`

Rationale: this prevents collisions between files with the same basename in different directories, while keeping names readable and predictable.

#### Regeneration rules

Within a generated project directory, files are classified as:

- **Generated (owned by Incan)**: may be overwritten on each run.
    - `Cargo.toml`
    - `src/**` (generated backend code)
    - Any other files explicitly emitted by Incan in the future
- **Preserved (owned by Cargo / user tooling)**: Incan must not delete or overwrite them by default.
    - `Cargo.lock`
    - `target/**` (Cargo build artifacts)
    - `.cargo/**` (if present; e.g. registry overrides, config)

Rationale: preserving `Cargo.lock` is required for meaningful `--locked` behavior across invocations when `Cargo.lock` is the only lock artifact in play.

Future compatibility note (RFC 013):

- RFC 013 introduces a project-root `incan.lock` as the source-of-truth lockfile, embedding a Cargo lock payload.
- If and when RFC 013 is implemented, `Cargo.lock` files inside generated
project directories become **derived artifacts** materialized from
  `incan.lock` and may be overwritten deterministically.
- In that world, the “preserved `Cargo.lock`” rule is superseded by RFC 013’s lock materialization contract.

#### Lockfile requirements for policy modes

When `--locked` or `--frozen` is set:

- **Project mode** (when `incan.toml` exists per RFC 015):
    - The authoritative lock artifact is the project-root `incan.lock` (RFC 013).
    - If `incan.lock` is missing (or out of date per RFC 013), the command must fail with a clear diagnostic:
        - explain that `incan.lock` is required for strict modes
        - instruct the user to run `incan lock`
        - point to the project root (`incan.toml` location)
    - Incan must materialize the embedded Cargo lock payload from `incan.lock`
      into `Cargo.lock` inside the generated project directory before invoking
      Cargo.

- **Single-file mode** (no `incan.toml`):
    - The authoritative lock artifact is `Cargo.lock` inside the generated project directory.
    - If the relevant generated project has **no `Cargo.lock`**, the command must fail with a clear diagnostic:
        - explain that a lockfile is required
        - recommend running once without `--locked` to generate it
        - point to the generated project path

When `--offline` or `--frozen` is set:

- If Cargo would require network access (registry index, git fetch, etc.), the
command must fail and surface Cargo’s error.
- Incan should add a short prefix that clarifies that this failure was expected under offline policy.

### Relationship to RFC 013 (`incan.lock`) (informative, non-normative)

RFC 013 proposes an Incan lockfile (`incan.lock`) as a reproducible source-of-truth for Rust dependencies.

This RFC does **not** define `incan.lock`. It only defines:

- how `incan` forwards Cargo policy flags (`--offline/--locked/--frozen`)
- how generated Cargo projects are treated (notably: `Cargo.lock` handling
differs by project vs single-file mode; see above)

If and when RFC 013 is implemented, its `incan.lock` workflow must compose with this RFC’s guarantees. In particular, policy flags still constrain Cargo subprocesses, and generated-project persistence rules remain true unless superseded by a newer RFC.

In practice, that means:

- The same CLI flags (`--offline/--locked/--frozen`) constrain Cargo subprocesses exactly as specified here.
- RFC 013 additionally defines Incan-level lock freshness requirements for `incan.lock` under strict modes.
- Generated-project lockfile handling may change from “preserve `Cargo.lock`”
to “materialize `Cargo.lock` from `incan.lock`” once RFC 013 is implemented.

## Design details

### Policy application: which Cargo invocations are affected

The policy flags apply to all Cargo subprocesses invoked by `incan` for that command, including:

- `incan build`: `cargo build ...`
- `incan run`: `cargo run ...`
- `incan test`: `cargo test ...`

### Policy does not mean “no Rust dependencies”

Offline/locked policy constrains resolution and fetching, not whether crates are used. If a project depends on crates not already available in Cargo’s local cache (or a vendor directory), offline builds will fail. That is intended and is part of the contract.

### “Escape hatch” stability

`--cargo-args` is intentionally a thin pass-through and is not guaranteed stable across major versions beyond:

- existence of the flag
- “args are forwarded to Cargo”

The stable, recommended surface is `--offline/--locked/--frozen`.

## Compatibility / migration

- This RFC is **additive**: existing invocations continue to work without changes.
- Users who want determinism can start adding `--locked` (or `--frozen`) in CI immediately.
- Users may need an initial “priming” run (without `--offline`) to populate Cargo caches and generate `Cargo.lock`.

## Alternatives considered

- **Do nothing**: users run `cargo --offline/--locked` manually in generated directories.
    - Rejected: inconsistent CI policy, brittle, undermines toolchain contract.
- **Environment variables only**:
    - Rejected: discoverability is worse; flags are the primary, user-facing contract (env vars remain useful for CI).
- **Make offline/locked the default**:
    - Rejected (for now): would be breaking for new users and for first-time
      builds; revisit once `incan lock` and vendoring exist.

## Drawbacks

- More CLI surface area and more combinations to test.
- Offline mode can be confusing without a vendoring/mirroring story (addressed via phased plan and clear diagnostics).
- The generated-project contract commits us to a persistence model that later RFCs must respect.

## Layers affected

- **CLI / tooling**: must expose and propagate Cargo policy flags consistently
across `incan build`, `incan run`, and `incan test`.
- **Build orchestration**: must thread policy through all Cargo subprocess
invocation paths rather than only selected commands.
- **Generated project management**: must preserve the documented persistence
and overwrite behavior for generated Cargo projects under `target/incan/**` and `target/incan_tests/**`.
- **Docs / examples**: must explain offline, locked, and frozen behavior
clearly enough that CI and restricted-environment users can reason about failure modes.

## Implementation architecture

- Tooling changes:
    - Add `--offline/--locked/--frozen/--cargo-args` to `build`, `run`, `test`
    - Thread a `CargoPolicy` (or equivalent) from CLI parsing to all Cargo subprocess calls
- Backend changes:
    - Apply policy consistently to every Cargo subprocess path used by `incan build`, `incan run`, and `incan test`
- Tests to add:
    - CLI parsing tests for new flags
    - A unit/integration test that verifies we pass `--offline` / `--locked` through to Cargo command construction

### Follow-ups (explicitly out of scope here)

- Project-level defaults in `incan.toml` (RFC 015)
- Incan-level lockfile workflows (`incan.lock`, `incan lock`, `incan update`) (RFC 013)
- Vendoring/mirroring strategy to make offline-from-clean-machine reliable (likely a follow-up RFC)

## Unresolved questions

1. Should `--frozen` be implemented immediately, or deferred until `incan.lock` exists (RFC 013)?
2. For `incan test`, what is the preferred harness caching strategy under `target/incan_tests/**` (per-test vs per-run)?
3. Do we want an `incan doctor` check that validates offline readiness
(Cargo cache present, vendor dir present, etc.) as part of RFC 015, or as a follow-up RFC?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->

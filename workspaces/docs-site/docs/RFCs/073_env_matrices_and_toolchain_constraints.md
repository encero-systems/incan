# RFC 073: environment matrices and toolchain constraints

- **Status:** Planned
- **Created:** 2026-04-25
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 013 (Rust crate dependencies)
    - RFC 015 (hatch-like tooling and project lifecycle CLI)
    - RFC 018 (language primitives for testing)
    - RFC 019 (test runner, CLI, and ecosystem)
    - RFC 020 (Cargo offline and locked policy)
    - RFC 117 (`loaf.toml` and Oven's language-neutral project model)
    - RFC 118 (Incan and Oven command-line surfaces)
- **Issue:** https://github.com/encero-systems/incan/issues/401
- **RFC PR:** —
- **Written against:** ~~v0.3~~ v0.5
- **Shipped in:** —

## Summary

This RFC defines two parts of the Oven-managed environment model: enforceable Incan toolchain constraints and matrix-expanded environments. `loaf.toml` projects and named environments may declare `requires-incan` constraints that the active toolchain must satisfy before project-aware execution begins, and environments may define matrices that expand one logical env into multiple concrete env instances. Environment and matrix constraints only narrow inherited compatibility; they do not loosen it. Matrix execution stays at the Oven lifecycle layer: it repeats env scripts across resolved env cells, but it does not define test-runner semantics, parallel scheduling policy, or toolchain installation.

## Core model

Read this RFC as four foundations plus three mechanisms:

1. **Foundation:** `requires-incan` is not documentation; it is an executable constraint that project-aware commands must enforce.
2. **Foundation:** The project root defines a baseline toolchain contract, and each environment or matrix cell may only narrow that contract for its own workflows.
3. **Foundation:** A matrix env is still one named env in the manifest, but it expands into multiple concrete env instances at execution time.
4. **Foundation:** Matrix orchestration belongs to Oven, not to the test runner. RFC 019 remains the owner of test collection, parametrization, fixtures, reporting, and parallel test execution semantics.
5. **Mechanism A:** Execution commands compute an effective `requires-incan` constraint and select an installed toolchain that satisfies it, failing early when none does, while inspection commands surface compatibility without being blocked by it.
6. **Mechanism B:** `[[envs.<name>.matrix]]` in `loaf.toml` declares one or more axes that generate concrete env instances by Cartesian product.
7. **Mechanism C:** Selecting a concrete env instance runs one resolved env; selecting a matrix root env runs the target script across all generated env instances in deterministic order.

## Motivation

RFC 015 deliberately stopped before two capabilities that matter in real projects. First, `requires-incan` exists in the manifest shape, but without enforcement it is only an annotation. That is not enough for CI, compatibility validation, or contributor onboarding. A project that claims support for `>=0.3,<0.4` must not be built with `0.4`, and must not fail later in confusing ways.

Second, named envs without matrices solve only the single-configuration case. Hatch's value is not just that it names environments; it also lets one logical workflow expand across versions and variants without repo-local shell scripts. Incan needs the same capability for compatibility checks, release validation, feature toggles, and runner work built on top of RFC 019.

The key boundary is that this RFC should not re-specify the test runner. A matrix env may run `oven test`, but the semantics of how tests are discovered, filtered, reported, or parallelized remain owned by RFC 019. This RFC is about orchestrating repeated command contexts, not about changing what a test run means.

## Goals

- Make `[project].requires-incan` and env-level `requires-incan` enforceable for project-aware execution commands.
- Allow named envs to expand into multiple concrete env instances through declarative matrices.
- Keep RFC 117's standard environments while allowing matrices on explicit envs.
- Make matrix execution deterministic and scriptable for CI and local compatibility testing.
- Ensure env-specific constraints can narrow a project contract but cannot silently loosen it.
- Keep the lifecycle CLI surface simple enough that users can understand what will run before it runs.
- Select among installed toolchains, so one project can be tested against several Incan versions on one machine.

## Non-Goals

- Defining how toolchains are installed; RFC 118 owns that. A run never downloads a toolchain by itself.
- Replacing RFC 019's ownership of test-runner behavior such as `--jobs`, discovery, or report formats.
- Adding Python-style virtual environment creation or shell activation semantics.
- Designing a general templating language for scripts or manifest interpolation.
- Making matrices inherit across env templates; that adds surprising expansion behavior.

## Guide-level explanation

### Project-wide toolchain constraints

Projects may declare which Incan versions they support:

```toml
[project]
name = "foo_bar"
version = "0.3.0"
requires-incan = ">=0.3,<0.4"
```

From that point on, project-aware execution commands must select a toolchain that satisfies it before doing real work:

```text
oven run
oven bake
oven test
oven lock
oven env run dev test
```

Several Incan versions can be installed side by side. If the active toolchain fits, it is used. Otherwise Oven uses the newest installed toolchain that fits, and the receipt records which one ran. If none fits, the command fails immediately and says how to get one:

```text
no installed toolchain satisfies >=0.3,<0.4 (active: 0.4.2)
help: incan toolchain install 0.3
```

Inspection commands such as `oven env show` should still be able to report the effective constraint and which toolchain would be selected.

Single-file commands remain separate. `incan run script.incn` without a project root does not consult `requires-incan` because there is no project manifest to read.

### Env-specific toolchain constraints

An environment may narrow the project-wide contract for a particular workflow:

```toml
[project]
name = "foo_bar"
version = "0.3.0"
requires-incan = ">=0.3,<0.5"

[envs.release]
requires-incan = ">=0.4,<0.5"

[envs.release.scripts]
build = ["oven", "bake", "--locked"]
```

Now `oven env run release build` requires a toolchain compatible with the intersection of the project and env constraints. In this example that intersection is still `>=0.4,<0.5`, because the env narrows the broader project contract.

### Matrix environments

A matrix environment defines one logical workflow that expands into multiple concrete envs:

```toml
[envs.compat]
scripts.test = ["oven", "test"]

[[envs.compat.matrix]]
incan = [
  { name = "0.3", requires-incan = ">=0.3,<0.4" },
  { name = "0.4", requires-incan = ">=0.4,<0.5" },
]
profile = [
  { name = "debug", env-vars = { INCAN_PROFILE = "debug" } },
  { name = "release", env-vars = { INCAN_PROFILE = "release" } },
]
```

This one env expands into four concrete env instances:

```text
compat[incan=0.3,profile=debug]
compat[incan=0.3,profile=release]
compat[incan=0.4,profile=debug]
compat[incan=0.4,profile=release]
```

`oven env show compat` should show the root env plus the generated cells. `oven env show 'compat[incan=0.4,profile=release]'` should show the resolved concrete env after all overlays have been applied.

### Running a matrix env

Running the matrix root executes the chosen script once for each generated cell:

```text
oven env run compat test
```

This is equivalent to running:

```text
oven env run 'compat[incan=0.3,profile=debug]' test
oven env run 'compat[incan=0.3,profile=release]' test
oven env run 'compat[incan=0.4,profile=debug]' test
oven env run 'compat[incan=0.4,profile=release]' test
```

Each cell selects its own toolchain, so the `0.3` cells run on an installed `0.3` and the `0.4` cells on an installed `0.4`. A cell with no fitting toolchain fails rather than being skipped, because a green result must mean the cell ran. The CLI should make that expansion visible in dry-run and show output so users can see what is going to execute.

### Relationship to RFC 019

If an env script happens to be `["oven", "test"]`, the matrix only repeats the test command in multiple env contexts. It does not change the selected test semantics inside any one run. Discovery, fixtures, markers, output formats, and test parallelism remain defined by RFC 019.

## Reference-level explanation

### `requires-incan` and `requires-oven` grammar and validation

Every `requires-incan` and `requires-oven` value must use the same SemVer requirement grammar accepted elsewhere in the manifest. Invalid requirement syntax makes the manifest invalid and must produce a targeted diagnostic. `requires-oven` (RFC 117) is enforced with exactly the rules this section states for `requires-incan`: it intersects through the same layers, it is checked against the active Oven version before project-aware execution, and both requirements are published to the registry index (RFC 125) so a resolver can skip versions the installed toolchain cannot build. A Rust-only Loaf may declare `requires-oven` alone.

### Execution versus inspection commands

The following commands must resolve the nearest project root and enforce an effective `requires-incan` constraint before beginning project-aware execution:

- `oven run`, `oven build`, `oven bake`, `oven check`, `oven test`
- `oven lock`, `oven update`, `oven add`, `oven publish`
- `oven env run`, `oven action run`
- any other Oven operation that compiles, executes, locks, publishes, or mutates a `loaf.toml` project (RFC 118)

The following commands must resolve and report the effective constraint, but they must not require the active toolchain to satisfy it merely to inspect configuration:

- `oven env list`, `oven env show`, `oven plan`, `oven inspect`
- `incan check`, `incan version`
- other read-only or manifest-maintenance commands that do not compile, execute, or lock project code

If `[project].requires-incan` is absent, the project-level baseline constraint is unconstrained.

If the command resolves an environment, the environment may contribute its own `requires-incan`. Environment and matrix constraints must intersect with inherited constraints; they do not replace them. An env that declares a broader range than the project baseline does not widen the effective constraint. An env or matrix cell whose declared constraint is disjoint from its inherited constraint makes that env definition invalid.

### Toolchain selection

Oven must select a toolchain for each concrete env from the toolchains installed in the Incan home, under RFC 118. If the active toolchain satisfies the effective constraint, it must be selected. Otherwise the newest installed toolchain that satisfies it must be selected. The selected toolchain must carry out the whole operation, and the receipt must record it. Selection must be deterministic for a given set of installed toolchains, and must never download anything.

If no installed toolchain satisfies the effective constraint, the command must fail before running scripts, reading lock policy, compiling user code, or mutating project files.

The diagnostic must include:

- the active toolchain version and the installed versions
- the effective `requires-incan` constraint
- the source layers that contributed to the effective constraint
- the command that installs a fitting toolchain

For inspection commands, the CLI should surface the same effective constraint and should indicate which toolchain would be selected.

Commands that do not resolve a project root must not attempt to infer or enforce `requires-incan`.

### Environment schema additions

This RFC adds the following optional fields to `[envs.<name>]` in `loaf.toml`:

- `requires-incan: str`
- `matrix: List[Table]`

Each `[[envs.<name>.matrix]]` table defines one matrix declaration. Multiple matrix tables are allowed; their generated cells are concatenated in declaration order rather than merged.

Within a matrix table, each key is an axis name. Each axis value must be a non-empty list. Axis names must be unique within that matrix table.

Axis entries may be either:

- a bare string, which is shorthand for `{ name = "<value>" }`
- a table with a required `name: str` and optional overlay fields

Supported overlay fields for a matrix value are:

- `requires-incan: str`
- `cwd: str`
- `env-vars: Table[str, str]`
- `dependencies: Table`
- `dev-dependencies: Table`

An implementation may reject unsupported overlay fields with a clear diagnostic rather than silently ignoring them.

### Matrix expansion

For one matrix table, the CLI must generate the Cartesian product of all axis values in axis declaration order.

Concrete env names must use this normalized form:

```text
<env>[<axis1>=<value1>,<axis2>=<value2>,...]
```

The displayed axis order must follow the axis declaration order from the matrix table.

Concrete env selection on the CLI must accept axis assignments in any order, but the tool must normalize them in output and diagnostics.

Matrices must not be inherited through `extends` or the standard `dev` env. Only the matrix declarations physically present on the selected env generate cells. This keeps expansion local and predictable.

If a matrix table would generate duplicate concrete env names, the manifest is invalid and the CLI must fail with a diagnostic naming the collision.

### Effective env resolution

For a concrete matrix env, the effective configuration is computed in this order:

1. project base config
2. the standard `dev` env, unless the selected root env is detached
3. envs named in `extends`, in declaration order
4. selected root env
5. matrix value overlays, in matrix-axis declaration order for the chosen cell

Later layers replace or merge earlier layers using the same rules as RFC 015 env resolution.

The effective toolchain constraint for a concrete env is the intersection of every contributing `requires-incan` layer in the resolution chain. If no contributing layer declares `requires-incan`, the concrete env is unconstrained. If the intersection is empty, the env definition is invalid and the CLI must fail with a diagnostic that names the conflicting layers and constraints.

### CLI behavior

`oven env list` should show root envs by default. It may provide `--expanded` to include generated concrete envs.

`oven env show` with no env argument should show an overview of root envs and indicate which envs are matrix roots.

`oven env show <env>` where `<env>` is a matrix root should show the root env summary plus the generated concrete env instances.

`oven env show <concrete-env>` must show the fully resolved concrete env configuration.

`oven env run <env> <script>` behaves as follows:

- if `<env>` is a non-matrix env, run the script once
- if `<env>` is a concrete matrix env, run the script once using that concrete env
- if `<env>` is a matrix root env, run the script once for each generated concrete env in deterministic order

When running a matrix root env, the CLI must print which concrete env is currently executing.

The default matrix execution policy is sequential. Concurrent matrix execution is not part of this design.

The aggregate exit status for matrix-root execution must be non-zero if any concrete env run fails. The CLI may stop at the first failure or continue through all cells if an explicit flag requests that behavior, but the default policy must be documented and consistent across commands.

### Dry-run and diagnostics

When `--dry-run` is supported for `oven env run`, it must show:

- the concrete env instances that would run
- the effective `requires-incan` and selected toolchain for each instance
- the final argv, cwd, and env-var additions for each instance

If a concrete env cannot be resolved because of an invalid matrix assignment, unknown axis, missing script, or no fitting toolchain, the diagnostic must name the concrete env string that failed.

## Design details

### Why `requires-incan` is an executable contract

RFC 015 already established `requires-incan` as part of project metadata. Leaving it unenforced weakens the whole lifecycle surface because users cannot rely on project manifests to describe supported toolchains. This RFC makes the field operational: if a project or env declares a requirement, the CLI must treat it as a contract.

Enforcement alone would make the most useful matrix impossible, because a single active toolchain could only ever pass one `incan` axis value. So Oven selects among the toolchains already installed. Installing one stays an explicit step, since a test run must not quietly download a compiler.

### Why `requires-incan` intersects instead of replacing

Replacement is too loose. If the project says `>=0.3,<0.5` and an env says `>=0.2,<0.3`, replacement would let one env claim support outside the project's declared contract. That weakens the meaning of the project baseline and makes compatibility harder to audit.

Intersection preserves the right asymmetry: the project defines the broad contract, and envs may narrow it for more specific workflows. If a project says `>=0.3,<0.5` and an env says `>=0.4,<0.5`, the effective constraint is the tighter env range. If the env says `>=0.2,<0.3`, the configuration is invalid because the env contradicts the project contract instead of refining it.

### Why matrices do not inherit

Inherited matrices create surprising combinatorics. One base env with a toolchain matrix and one child env with a profile matrix could silently explode into a larger cross product than the author intended. Hatch avoids this by not inheriting matrices, and Incan should copy that constraint.

The author who wants a matrix must declare it on the env that owns it. That keeps reviewable configuration local and avoids debugging invisible expansion from templates.

### Why matrix values use overlays instead of manifest interpolation

Hatch supports context formatting, but that is broader than Incan needs here. The immediate need is to vary a few env properties, especially `requires-incan` and environment variables, across a Cartesian product. Overlay values accomplish that without introducing a string templating language into scripts or dependency specifications.

This RFC deliberately keeps the matrix mechanism concrete and explicit.

### Compatibility and migration

This RFC is additive at the manifest-schema level, but enforcement changes behavior:

- projects that set `[project].requires-incan` get a fitting installed toolchain, or an early failure when none is installed
- envs may add `requires-incan` and matrices without changing non-matrix env behavior
- envs that try to broaden or contradict the project baseline fail validation instead of being interpreted permissively

Enforcement makes previously ignored metadata meaningful, so a project that newly fails has a configuration issue rather than a compiler regression. The diagnostics must make that clear.

## Alternatives considered

### Keep `requires-incan` as documentation only

Rejected because it fails the main reason to have the field at all. Projects and CI need early, actionable validation, not passive notes.

### Add only project-level enforcement and defer env-level constraints

Rejected because it leaves the most useful Hatch-like workflow unsupported: one repo validating different toolchain contracts for different named workflows.

### Add matrix execution only for `oven test`

Rejected because it would couple environment orchestration to the testing surface and duplicate concepts that belong in the lifecycle layer. A matrix env should be able to run build, test, docs, release, or any future named script.

### Check only the active toolchain

Rejected because a matrix over Incan versions could then never pass on one machine.

### Download a missing toolchain automatically

Rejected because a test run should not quietly fetch a compiler, which is awkward in CI and governed runs. The failure names the install command instead.

### Skip cells whose toolchain does not fit

Rejected because the result would be green without having tested what it claims.

### Make matrices inherit through `extends`

Rejected because it makes expansion harder to predict and review. Explicit local matrix declarations are easier to reason about.

## Drawbacks

- Enforced `requires-incan` will break some projects that currently carry stale metadata without consequences.
- Matrix configuration introduces another layer of manifest complexity and can become noisy if overused.
- Oven has to run a toolchain other than the one that was invoked, and a user may be surprised which one ran unless the output says so.
- Intersected constraints are stricter, which means some env configurations that look plausible at first glance will be rejected as contradictory.
- Sequential matrix execution may be slower than users expect.

## Implementation architecture

The recommended implementation shape is to treat toolchain validation and matrix resolution as two separate phases before command execution.

First, resolve the project root and manifest-backed env model exactly once. Then resolve the requested env into either one concrete env or an ordered list of concrete envs. Finally, validate `requires-incan` per concrete env before launching any child process or compiler work for that env.

This separation matters because the same concrete-env resolution should power `list`, `show`, `run`, and machine-readable output. Matrix expansion should therefore be a reusable lifecycle-layer capability rather than logic embedded only inside one command path.

## Layers affected

- **Manifest schema / configuration validation:** the schema must admit env-level `requires-incan` and matrix declarations, validate axis shapes, and reject duplicate concrete env names or invalid overlay keys.
- **CLI / tooling:** Oven execution commands must enforce effective toolchain constraints; inspection commands must surface effective constraints and compatibility state without blocking basic visibility; `oven env list`, `show`, and `run` must understand root envs versus concrete matrix envs and present them clearly. RFC 118 may expose downward-delegating Incan conveniences, but it must not duplicate Oven environment resolution.
- **Lifecycle env resolution:** env resolution must expand matrices deterministically, normalize concrete env names, and apply overlay merge rules in the specified order.
- **Testing surface integration:** `oven env run ... test` must remain only an orchestration wrapper around RFC 019 semantics, not a second test-runner implementation.
- **Documentation:** user docs and reference docs must explain the difference between project constraints, env constraints, root envs, and concrete matrix env instances.

## Design Decisions

- **Oven selects among installed toolchains.** The active toolchain is used if it fits, otherwise the newest installed one that fits, and the receipt records it. No fitting toolchain is a failure that names the install command, and a run never downloads anything. Cells are never skipped.
- **Matrix-root execution stops at the first failing cell by default.** An explicit `--keep-going` continues through every cell and prints a per-cell summary at the end. The aggregate exit status is non-zero if any cell failed in either mode, the chosen mode is recorded in the receipt, and every Oven command that expands a matrix uses the same default. Stopping early is the conservative default for a tool whose other operations fail closed; the flag exists for CI.
- **No `--toolchain` override.** Toolchain selection is a plan fact under RFC 117 and RFC 119, derived from the installed toolchains and the effective `requires-incan` and `requires-oven` constraints, and recorded in the receipt. A command-line override would let one invocation bypass a constraint the project declared, which is the failure this RFC exists to prevent. Explicit toolchain selection as a plan input is not part of this design; toolchain selection is never a constraint bypass. The enforced constraint is always one the project's own manifest declares, so the escape is always available to the author: edit `requires-incan` or `requires-oven` in `loaf.toml`. A dependency's published requirement (RFC 117) only lets the resolver skip versions the installed toolchain cannot build; it never triggers this refusal, so no failure is ever raised against a Loaf the author does not control.
- **`oven env list` always shows root environments; `--expanded` is always explicit.** When a project has only matrix environments, the default listing shows each root with its cell count and the exact `--expanded` invocation, so the expanded view is one keystroke away without a mode that changes shape depending on manifest contents.

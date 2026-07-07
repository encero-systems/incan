# RFC 073: environment matrices and toolchain constraints

- **Status:** Draft
- **Created:** 2026-04-25
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 013 (Rust crate dependencies)
    - RFC 015 (hatch-like tooling and project lifecycle CLI)
    - RFC 018 (language primitives for testing)
    - RFC 019 (test runner, CLI, and ecosystem)
    - RFC 020 (Cargo offline and locked policy)
- **Issue:** https://github.com/encero-systems/incan/issues/401
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC extends RFC 015 with two missing parts of the Hatch-like environment model: enforceable Incan toolchain constraints and matrix-expanded environments. Projects and named environments may declare `requires-incan` constraints that the active toolchain must satisfy before project-aware execution begins, and environments may define matrices that expand one logical env into multiple concrete env instances. Environment and matrix constraints only narrow inherited compatibility; they do not loosen it. Matrix execution stays at the lifecycle layer: it repeats env scripts across resolved env cells, but it does not define test-runner semantics, parallel scheduling policy, or toolchain installation.

## Core model

Read this RFC as four foundations plus three mechanisms:

1. **Foundation:** `requires-incan` is not documentation; it is an executable constraint that project-aware commands must enforce.
2. **Foundation:** The project root defines a baseline toolchain contract, and each environment or matrix cell may only narrow that contract for its own workflows.
3. **Foundation:** A matrix env is still one named env in the manifest, but it expands into multiple concrete env instances at execution time.
4. **Foundation:** Matrix orchestration belongs to the lifecycle CLI, not to the test runner. RFC 019 remains the owner of test collection, parametrization, fixtures, reporting, and parallel test execution semantics.
5. **Mechanism A:** Execution commands compute an effective `requires-incan` constraint and fail early when the active toolchain does not satisfy it, while inspection commands surface compatibility without being blocked by it.
6. **Mechanism B:** `[[tool.incan.envs.<name>.matrix]]` declares one or more axes that generate concrete env instances by Cartesian product.
7. **Mechanism C:** Selecting a concrete env instance runs one resolved env; selecting a matrix root env runs the target script across all generated env instances in deterministic order.

## Motivation

RFC 015 deliberately stopped before two capabilities that matter in real projects. First, `requires-incan` exists in the manifest shape, but without enforcement it is only an annotation. That is not enough for CI, compatibility validation, or contributor onboarding. A project that claims support for `>=0.3,<0.4` needs the toolchain to reject `0.4` immediately rather than failing later in confusing ways.

Second, named envs without matrices solve only the single-configuration case. Hatch's value is not just that it names environments; it also lets one logical workflow expand across versions and variants without repo-local shell scripts. Incan needs the same capability for compatibility checks, release validation, feature toggles, and future runner work built on top of RFC 019.

The key boundary is that this RFC should not re-specify the test runner. A matrix env may run `incan test`, but the semantics of how tests are discovered, filtered, reported, or parallelized remain owned by RFC 019. This RFC is about orchestrating repeated command contexts, not about changing what a test run means.

## Goals

- Make `[project].requires-incan` and env-level `requires-incan` enforceable for project-aware execution commands.
- Allow named envs to expand into multiple concrete env instances through declarative matrices.
- Keep the ambient `default` env model from RFC 015 while allowing matrices on explicit envs.
- Make matrix execution deterministic and scriptable for CI and local compatibility testing.
- Ensure env-specific constraints can narrow a project contract but cannot silently loosen it.
- Keep the lifecycle CLI surface simple enough that users can understand what will run before it runs.
- Leave room for future toolchain installation and discovery work without blocking constraint enforcement now.

## Non-Goals

- Defining a toolchain installer, downloader, or registry client.
- Replacing RFC 019's ownership of test-runner behavior such as `--jobs`, discovery, or report formats.
- Adding Python-style virtual environment creation or shell activation semantics.
- Designing a general templating language for scripts or manifest interpolation.
- Making matrices inherit across env templates; that adds surprising expansion behavior and is not required for the first version.

## Guide-level explanation

### Project-wide toolchain constraints

Projects may declare which Incan versions they support:

```toml
[project]
name = "foo_bar"
version = "0.3.0"
requires-incan = ">=0.3,<0.4"
```

From that point on, project-aware execution commands must validate the active toolchain before doing real work:

```text
incan run
incan build
incan test
incan lock
incan env run default test
```

If the active toolchain does not satisfy the constraint, the command fails immediately with a diagnostic that names the active version and the required constraint.

Inspection commands such as `incan env show` should still be able to report the effective constraint and whether the current toolchain satisfies it.

Single-file commands remain separate. `incan run script.incn` without a project root does not consult `requires-incan` because there is no project manifest to read.

### Env-specific toolchain constraints

An environment may narrow the project-wide contract for a particular workflow:

```toml
[project]
name = "foo_bar"
version = "0.3.0"
requires-incan = ">=0.3,<0.5"

[tool.incan.envs.release]
requires-incan = ">=0.4,<0.5"

[tool.incan.envs.release.scripts]
build = ["incan", "build", "--locked"]
```

Now `incan env run release build` requires an active toolchain compatible with the intersection of the project and env constraints. In this example that intersection is still `>=0.4,<0.5`, because the env narrows the broader project contract.

### Matrix environments

A matrix environment defines one logical workflow that expands into multiple concrete envs:

```toml
[tool.incan.envs.compat]
scripts.test = ["incan", "test"]

[[tool.incan.envs.compat.matrix]]
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

`incan env show compat` should show the root env plus the generated cells. `incan env show 'compat[incan=0.4,profile=release]'` should show the resolved concrete env after all overlays have been applied.

### Running a matrix env

Running the matrix root executes the chosen script once for each generated cell:

```text
incan env run compat test
```

This is equivalent to running:

```text
incan env run 'compat[incan=0.3,profile=debug]' test
incan env run 'compat[incan=0.3,profile=release]' test
incan env run 'compat[incan=0.4,profile=debug]' test
incan env run 'compat[incan=0.4,profile=release]' test
```

The CLI should make that expansion visible in dry-run and show output so users can see what is going to execute.

### Relationship to RFC 019

If an env script happens to be `["incan", "test"]`, the matrix only repeats the test command in multiple env contexts. It does not change how `incan test` behaves inside any one run. Discovery, fixtures, markers, output formats, and test parallelism remain defined by RFC 019.

## Reference-level explanation

### `requires-incan` grammar and validation

Every `requires-incan` value must use the same SemVer requirement grammar accepted elsewhere in the manifest. Invalid requirement syntax makes the manifest invalid and must produce a targeted diagnostic.

### Execution versus inspection commands

The following commands must resolve the nearest project root and enforce an effective `requires-incan` constraint before beginning project-aware execution:

- `incan run` in project mode
- `incan build` in project mode
- `incan test`
- `incan lock`
- `incan env run`
- any future project-aware subcommand that consumes `incan.toml`

The following commands must resolve and report the effective constraint, but they must not require the active toolchain to satisfy it merely to inspect configuration:

- `incan env list`
- `incan env show`
- `incan version`
- future read-only or manifest-maintenance commands that do not compile, execute, or lock project code

If `[project].requires-incan` is absent, the project-level baseline constraint is unconstrained.

If the command resolves an environment, the environment may contribute its own `requires-incan`. Environment and matrix constraints must intersect with inherited constraints; they do not replace them. An env that declares a broader range than the project baseline does not widen the effective constraint. An env or matrix cell whose declared constraint is disjoint from its inherited constraint makes that env definition invalid.

If the active Incan toolchain version does not satisfy the effective constraint, the command must fail before running scripts, reading lock policy, compiling user code, or mutating project files.

The diagnostic must include:

- the active toolchain version
- the effective `requires-incan` constraint
- the source layers that contributed to the effective constraint

For inspection commands, the CLI should surface the same effective constraint and should indicate whether the active toolchain satisfies it.

Commands that do not resolve a project root must not attempt to infer or enforce `requires-incan`.

### Environment schema additions

This RFC adds the following optional fields to `[tool.incan.envs.<name>]`:

- `requires-incan: str`
- `matrix: List[Table]`

Each `[[tool.incan.envs.<name>.matrix]]` table defines one matrix declaration. Multiple matrix tables are allowed; their generated cells are concatenated in declaration order rather than merged.

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

Matrices must not be inherited through `extends` or the ambient `default` env. Only the matrix declarations physically present on the selected env generate cells. This keeps expansion local and predictable.

If a matrix table would generate duplicate concrete env names, the manifest is invalid and the CLI must fail with a diagnostic naming the collision.

### Effective env resolution

For a concrete matrix env, the effective configuration is computed in this order:

1. project base config
2. ambient `default` env, unless the selected root env is detached
3. envs named in `extends`, in declaration order
4. selected root env
5. matrix value overlays, in matrix-axis declaration order for the chosen cell

Later layers replace or merge earlier layers using the same rules as RFC 015 env resolution.

The effective toolchain constraint for a concrete env is the intersection of every contributing `requires-incan` layer in the resolution chain. If no contributing layer declares `requires-incan`, the concrete env is unconstrained. If the intersection is empty, the env definition is invalid and the CLI must fail with a diagnostic that names the conflicting layers and constraints.

### CLI behavior

`incan env list` should show root envs by default. It may provide `--expanded` to include generated concrete envs.

`incan env show` with no env argument should show an overview of root envs and indicate which envs are matrix roots.

`incan env show <env>` where `<env>` is a matrix root should show the root env summary plus the generated concrete env instances.

`incan env show <concrete-env>` must show the fully resolved concrete env configuration.

`incan env run <env> <script>` behaves as follows:

- if `<env>` is a non-matrix env, run the script once
- if `<env>` is a concrete matrix env, run the script once using that concrete env
- if `<env>` is a matrix root env, run the script once for each generated concrete env in deterministic order

When running a matrix root env, the CLI must print which concrete env is currently executing.

The default matrix execution policy is sequential. Future RFCs may add concurrent matrix execution, but this RFC does not require it.

The aggregate exit status for matrix-root execution must be non-zero if any concrete env run fails. The CLI may stop at the first failure or continue through all cells if an explicit flag requests that behavior, but the default policy must be documented and consistent across commands.

### Dry-run and diagnostics

When `--dry-run` is supported for `incan env run`, it must show:

- the concrete env instances that would run
- the effective `requires-incan` for each instance
- the final argv, cwd, and env-var additions for each instance

If a concrete env cannot be resolved because of an invalid matrix assignment, unknown axis, missing script, or unsatisfied `requires-incan`, the diagnostic must name the concrete env string that failed.

## Design details

### Why `requires-incan` is an executable contract

RFC 015 already established `requires-incan` as part of project metadata. Leaving it unenforced weakens the whole lifecycle surface because users cannot rely on project manifests to describe supported toolchains. This RFC makes the field operational: if a project or env declares a requirement, the CLI must treat it as a contract.

This remains a constraint story, not a toolchain installation story. The CLI validates the active toolchain it is running under. A future RFC may define discovery or auto-selection of other installed Incan versions, but that is not required for useful enforcement.

### Why `requires-incan` intersects instead of replacing

Replacement is too loose. If the project says `>=0.3,<0.5` and an env says `>=0.2,<0.3`, replacement would let one env claim support outside the project's declared contract. That weakens the meaning of the project baseline and makes compatibility harder to audit.

Intersection preserves the right asymmetry: the project defines the broad contract, and envs may narrow it for more specific workflows. If a project says `>=0.3,<0.5` and an env says `>=0.4,<0.5`, the effective constraint is the tighter env range. If the env says `>=0.2,<0.3`, the configuration is invalid because the env contradicts the project contract instead of refining it.

### Why matrices do not inherit

Inherited matrices create surprising combinatorics. One base env with a toolchain matrix and one child env with a profile matrix could silently explode into a larger cross product than the author intended. Hatch avoids this by not inheriting matrices, and Incan should copy that constraint.

The author who wants a matrix must declare it on the env that owns it. That keeps reviewable configuration local and avoids debugging invisible expansion from templates.

### Why matrix values use overlays instead of manifest interpolation

Hatch supports context formatting, but that is broader than Incan needs here. The immediate need is to vary a few env properties, especially `requires-incan` and environment variables, across a Cartesian product. Overlay values accomplish that without introducing a string templating language into scripts or dependency specifications.

If a future RFC needs interpolation, it can add it later. This RFC deliberately keeps the matrix mechanism concrete and explicit.

### Compatibility and migration

This RFC is additive at the manifest-schema level, but enforcement changes behavior:

- projects that already set `[project].requires-incan` will start seeing early failures on incompatible toolchains for execution commands
- envs may add `requires-incan` and matrices without changing non-matrix env behavior
- envs that try to broaden or contradict the project baseline will now fail validation instead of being interpreted permissively
- existing RFC 015 env definitions remain valid

Because enforcement makes previously ignored metadata meaningful, the rollout should treat newly failing projects as configuration issues rather than compiler regressions. The diagnostics must make that clear.

## Alternatives considered

### Keep `requires-incan` as documentation only

Rejected because it fails the main reason to have the field at all. Projects and CI need early, actionable validation, not passive notes.

### Add only project-level enforcement and defer env-level constraints

Rejected because it leaves the most useful Hatch-like workflow unsupported: one repo validating different toolchain contracts for different named workflows.

### Add matrix execution only for `incan test`

Rejected because it would couple environment orchestration to the testing surface and duplicate concepts that belong in the lifecycle layer. A matrix env should be able to run build, test, docs, release, or any future named script.

### Add toolchain installation and discovery in the same RFC

Rejected because that turns one coherent lifecycle extension into an installer design. Constraint enforcement is useful on its own and should not wait on a full toolchain-management story.

### Make matrices inherit through `extends`

Rejected because it makes expansion harder to predict and review. Explicit local matrix declarations are easier to reason about.

## Drawbacks

- Enforced `requires-incan` will break some projects that currently carry stale metadata without consequences.
- Matrix configuration introduces another layer of manifest complexity and can become noisy if overused.
- Without a separate toolchain installer story, some users will still need to manage multiple Incan versions manually.
- Intersected constraints are stricter, which means some env configurations that look plausible at first glance will be rejected as contradictory.
- Sequential matrix execution may be slower than users expect until there is a later concurrency story.

## Implementation architecture

The recommended implementation shape is to treat toolchain validation and matrix resolution as two separate phases before command execution.

First, resolve the project root and manifest-backed env model exactly once. Then resolve the requested env into either one concrete env or an ordered list of concrete envs. Finally, validate `requires-incan` per concrete env before launching any child process or compiler work for that env.

This separation matters because the same concrete-env resolution should power `list`, `show`, `run`, and future machine-readable output. Matrix expansion should therefore be a reusable lifecycle-layer capability rather than logic embedded only inside one command path.

## Layers affected

- **Manifest schema / configuration validation:** the schema must admit env-level `requires-incan` and matrix declarations, validate axis shapes, and reject duplicate concrete env names or invalid overlay keys.
- **CLI / tooling:** execution commands must enforce effective toolchain constraints; inspection commands must surface effective constraints and compatibility state without blocking basic visibility; `incan env list`, `show`, and `run` must understand root envs versus concrete matrix envs and present them clearly.
- **Lifecycle env resolution:** env resolution must expand matrices deterministically, normalize concrete env names, and apply overlay merge rules in the specified order.
- **Testing surface integration:** `incan env run ... test` must remain only an orchestration wrapper around RFC 019 semantics, not a second test-runner implementation.
- **Documentation:** user docs and reference docs must explain the difference between project constraints, env constraints, root envs, and concrete matrix env instances.

## Unresolved questions

- Should matrix-root execution stop at the first failing concrete env by default, or should it continue through all cells and summarize failures at the end?
- Should the lifecycle CLI eventually grow an explicit `--toolchain <version-or-path>` override, or should toolchain selection remain entirely external while this RFC remains a constraint-and-validation layer only?
- Should `incan env list` eventually default to an expanded view when a project has only matrix envs, or is explicit `--expanded` always the better UX?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->

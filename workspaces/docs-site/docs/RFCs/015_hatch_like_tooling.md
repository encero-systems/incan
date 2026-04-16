# RFC 015: hatch-like tooling (project lifecycle CLI)

- **Status:** In Progress
- **Created:** 2025-12-23
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 013 (Rust crate dependencies)
    - RFC 020 (Cargo offline/locked policy)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/73
- **RFC PR:** —
- **Written against:** v0.1
- **Shipped in:** —

## Summary

Introduce a first-class, batteries-included project lifecycle CLI — similar in spirit to Python’s Hatch — for:

- **Versioning**: `incan version <major|minor|patch|alpha|beta|rc|dev>` (with optional `--dry-run`)
- **Project scaffolding**: `incan init` (in-place) and `incan new <name>` (new directory)
- **Environments**: `incan env ...` for repeatable, named command execution (CI-friendly) without implicit “magic”
- (future) **Matrix testing** (tox/nox-style): a follow-up RFC may add a matrix/env runner once RFC 019 stabilizes
- Additional “hatch-like” ergonomics where it fits Incan’s workflow (format/lint/release/build/publish).

This RFC defines the CLI surface, the project metadata format, and the implementation boundaries so we don’t bake policy into ad-hoc scripts.

## Motivation

Incan is a compiler + runtime ecosystem, but day-to-day developer experience is heavily shaped by tooling:

- Starting a new project should be **one command**.
- Bumping versions should be **correct and consistent** across project metadata, derived artifacts, and any package metadata.
- Running tests should support **repeatable environments** and **matrix execution**, without forcing users to learn Cargo
internals.
- Release workflows should be **scriptable** and **standard** across projects.

Python’s Hatch demonstrates that a single tool can cover the project lifecycle. This RFC adapts the useful parts to Incan.

## Goals

- Provide an ergonomic, consistent, and scriptable CLI for common workflows:
    - `init`, `new`, `version`, `test`, `env`
    - (future) `fmt`, `lint`, `build`, `publish`
- Define a single source of truth for project metadata (name, version, toolchain constraints, entrypoints, dependencies).
- Keep builds deterministic and reproducible (align with RFC 013 + RFC 020).
- Avoid “magic”: scaffolded project files are explicit and readable.

## Non-Goals

- Implement a public package registry client (publish/install) in this RFC (can be a follow-up RFC).
- Replace Cargo for Rust-level dependency resolution (we can orchestrate Cargo, not reinvent it).
- Provide virtualenv-style isolation identical to Python (we’ll use explicit env configs and reproducible commands instead).

## Terminology

- **Project**: An Incan repository containing Incan sources and metadata.
- **Environment**: A named configuration overlay for repeatable command execution (`cwd`, `env-vars`, scripts,
dependency overlays).
- **Matrix**: Running an environment set across multiple dimensions (e.g., debug/release, features on/off).

## Project Metadata

Add `incan.toml` at repo root (similar to `pyproject.toml`), as the canonical metadata source.

### Minimal example (bin-style project)

```toml title="incan.toml"
[project]
name = "hello_incan"
version = "0.1.0"

# Entry points for project-aware execution (future: `incan run <script>`)
[project.scripts]
main = "src/main.incn"

[dependencies]
# Cargo/Rust crate dependencies for `rust::...` (RFC 013)
rand = "0.8"
serde = { version = "1.0", features = ["derive"] }
```

Notes:

- `[tool.incan]` may contain additional tool-specific configuration (e.g., formatter settings, test timeouts).
These are defined by their respective RFCs (e.g., RFC 019 for test configuration) and are not specified here.
- `version` is SemVer-compatible with pre-release tags.
- Rust dependencies integrate with RFC 013 rules.
- `incan.toml` is the **project metadata** and is intended to be edited.
- Generated build artifacts under `target/` are readable for debugging, but are **not** intended for manual editing
(RFC 020).

### `[project]` schema (normative)

The `[project]` table is the canonical, toolchain-owned metadata for an Incan project.

Required keys:

- **`name: str`**: the project name.
- **`version: str`**: the project version (SemVer; pre-release tags allowed).

Optional keys (all `str` unless noted):

- **`description`**: short human-readable description.
- **`authors: List[str]`**: list of author strings (recommended format: `"Name <email@example.com>"`).
- **`maintainers: List[str]`**: list of maintainer strings (same format as `authors`).
- **`license`**: SPDX license identifier or SPDX expression.
- **`license-files: List[str]`**: paths to license files (relative to project root; future-facing).
- **`readme`**: path to a readme file (relative to project root).
- **`homepage`**: project homepage URL.
- **`repository`**: source repository URL.
- **`documentation`**: documentation URL.
- **`issues`**: issue tracker URL.
- **`keywords: List[str]`**: keywords/tags (used for search/discovery; future-facing).
- **`classifiers: List[str]`**: trove-like classifiers (future-facing; useful for packaging/indexes).
- **`requires-incan`**: SemVer requirement for the minimum supported Incan toolchain version.
- **`private: bool`**: if true, the project must not be publishable (future: enforced by `incan publish`).

Validation rules:

- `name` must be non-empty and should be stable over the project’s lifetime.
    - Recommended (non-normative) name pattern: `^[a-zA-Z][a-zA-Z0-9_-]*$`.
- `version` must be SemVer-compatible (including pre-release tags like `-alpha.1`).
- Paths like `readme` must be relative to project root (unless absolute); if present they must not escape the project root.
    - The same rule applies to `license-files` and `[project.scripts]` entrypoint paths.

Unknown keys:

- Unknown keys under `[project]` should produce a warning (to catch typos) and are validated by `incan check-config` (future).

Full example (metadata-rich):

```toml title="incan.toml"
[project]
name = "my_app"
version = "0.1.0-alpha.1"
description = "An example Incan application"
authors = ["Arthur Dent <arthur.dent@example.com>", "Tricia McMillan <trillian@example.com>"]
maintainers = ["Build Team <build-team@example.com>"]
license = "MIT"
readme = "README.md"
homepage = "https://example.com/my_app"
repository = "https://github.com/example/my_app"
documentation = "https://docs.example.com/my_app"
issues = "https://github.com/example/my_app/issues"
keywords = ["incan", "cli", "example"]

# Minimum Incan version required to build/test this project
requires-incan = ">=0.2.0"

[project.scripts]
main = "src/main.incn"

[dependencies]
reqwest = "0.12"
serde = { version = "1.0", features = ["derive"] }
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }

# [tool.incan] may include formatter, test, or other tool-specific settings (see respective RFCs)
```

### `[project.scripts]` schema (normative)

`[project.scripts]` maps script names to Incan entrypoint files (paths relative to project root).

- Values are `str` paths to `.incn` files.
- A script name should be a simple identifier (recommended snake_case).
- `incan new --bin` must create a `main` script by default: `main = "src/main.incn"`.

This RFC does not define project-aware `incan run` behavior for scripts yet. It only defines the configuration shape so it can be used by follow-up tooling RFCs without changing `incan.toml`.

Note: `[project.scripts]` maps script names to `.incn` entrypoint paths. This is distinct from
`[tool.incan.envs.<name>.scripts]` (defined below), which maps script names to shell command argv lists for env execution.

---

## Project root discovery (normative)

Most `incan` subcommands operate on a project.

Project root resolution:

- Starting from the current working directory, walk upward to find the nearest directory containing `incan.toml`.
- The first `incan.toml` found determines the project root.
- If no `incan.toml` is found, the command must fail with a clear diagnostic suggesting `incan init` or `incan new`.

Monorepos / nested projects:

- Nested `incan.toml` files are allowed; the nearest one wins.
- A future extension may add explicit workspace support; this RFC’s behavior is intentionally simple and deterministic.

Override:

- Commands may accept `--project <path>` to explicitly target a project root (path to a directory containing `incan.toml`).

### When `incan.toml` is required (normative)

`incan.toml` is **mandatory** for project-aware commands:

- `incan test`, `incan version`, `incan env ...`

`incan.toml` is **not required** for single-file execution:

- `incan run <file>` and `incan run -c "<code>"` work without a project context.
- In this mode, project-level features (dependencies, envs, versioning) are not available.
- If a single-file script needs Rust dependencies, it must use inline annotations (RFC 013) or be part of a project.

`incan build <file>` may operate in either mode:

- In **project mode**, project-level dependencies and strict lock semantics apply (RFC 013/020).
- In **single-file mode**, dependency configuration is limited to inline annotations and known-good defaults (RFC 013),
and strict flags operate on the generated project’s `Cargo.lock` (RFC 020).

## CLI Design

### `incan new <name>`

Create a new directory containing a minimal Incan project scaffold:

- `incan.toml`
- `src/main.incn` (hello world)
- `README.md`
- `.gitignore`

The generated `incan.toml` must include:

- `[project]` with `name` and `version`
- `[project.scripts]` with `main = "src/main.incn"`

Reproducibility (normative):

- `incan new` creates a project with `incan.toml` at the project root.
- On the first `incan build` or `incan test`, the toolchain generates `incan.lock` at the project root (per RFC 013).
- Projects are recommended to commit `incan.lock` for reproducible builds (especially in CI).

Behavior:

- By default, `incan new` is **interactive**: it prompts for project metadata (description, author, license, etc.).
- Use `-y` / `--yes` to skip prompts and use defaults (non-interactive mode for scripting/CI).

Flags:

- `--bin` (default; creates `src/main.incn`)
- `--dir <path>` (default: `./<name>`)
- `--force` (overwrite existing directory)
- `-y` / `--yes` (non-interactive; use defaults without prompting)

Note: `--lib` is intentionally deferred until there is a packaging/distribution story for Incan libraries.

### `incan init`

Initialize `incan.toml` (and `src/main.incn`) in the current directory.

Behavior:

- By default, `incan init` is **interactive**: it prompts for project metadata.
- Use `-y` / `--yes` to skip prompts and use defaults (non-interactive mode for scripting/CI).

Flags:

- `--force` (overwrite existing metadata)
- `--detect` (attempt to infer: scripts/entrypoints, existing version strings, etc.)
- `-y` / `--yes` (non-interactive; use defaults without prompting)

### `incan version <bump>`

Update the project version in `incan.toml` and any derived files that must match.

Scope (normative):

- `incan version` updates the **project version** only.
- Updating the Incan compiler/toolchain crate versions is a maintainer workflow and out of scope for this RFC.

Bumps:

- `major`, `minor`, `patch`
- `alpha`, `beta`, `rc`, `dev`

Rules:

- `major/minor/patch` operate on the release core and clear pre-release unless `--keep-prerelease`.
- `alpha/beta/rc/dev`:
    - If no prerelease exists, append `-<tag>.1`
    - If same prerelease exists, increment numeric suffix
    - If different prerelease exists, switch tag and reset to `.1`

Flags:

- `--dry-run`
- `--set <version>` (explicit override)
- `--keep-prerelease`
- `-m` / `--message <msg>` (for future integration with changelog/commit tooling)

Output should print:

- old version
- new version
- modified files

### `incan test`

Default test runner entrypoint.

Behavior:

- `incan test` runs the Incan test runner as specified by RFC 019 (project-neutral behavior).
- Cargo policy flags (`--offline/--locked/--frozen`) must be propagated consistently to any Cargo subprocesses,
as per RFC 020.

This RFC intentionally does **not** define repo-maintainer workflows for the Incan compiler repository (e.g. “run all workspace Rust tests”); those are out of scope for user-facing tooling semantics.

Flags:

- The test runner’s flags and semantics are specified by RFC 019.

### `incan env`

`incan env` provides a small “task/env runner” layer for repeatable commands, without changing the semantics of core
commands like `incan test`.

Core command stability (normative):

- Core commands like `incan test` are **not configurable** via `incan.toml`. Configuration is applied only when the user
explicitly uses `incan env run ...` or `incan env show ...`.

Core shape:

- `incan env list [--format text|json]` (list configured envs; outputs env names, one per line in text mode)
- `incan env show <env> [--format text|json]` (show the fully-resolved env after inheritance/merging)
- `incan env run <env> <script> [--dry-run] [-- <args...>]` (run a configured script in an env)

Configuration (normative):

```toml title="incan.toml"
[tool.incan.envs.default]
# The `default` env is included by other envs unless they set `detached = true`.
env-vars = { INCAN_NO_BANNER = "1" }

[tool.incan.envs.default.scripts]
test = ["incan", "test"]

[tool.incan.envs.unit]
# `default` is included implicitly for `unit` (unless `detached = true` is set).
env-vars = { INCAN_FANCY_ERRORS = "1" }

[tool.incan.envs.unit.scripts]
test = ["incan", "test"]

[tool.incan.envs.docs]
# Demonstrates env inheritance: `docs` includes `default` and also extends `unit`.
extends = ["unit"]
cwd = "workspaces/docs-site"

[tool.incan.envs.docs.scripts]
docs_build = ["python3", "-m", "mkdocs", "build", "-q"]
```

Normative behavior:

- `incan env list` must output all configured env names. In `text` mode, one env name per line. In `json` mode, a JSON
array of env names.
- `incan env show <env>` must resolve env inheritance and merging using the same rules as `incan env run` and then print:
    - resolved overlay chain (base → default? → extends… → env)
    - resolved `cwd`
    - resolved `env-vars`
    - resolved scripts (and the final argv for each script)
    - resolved dependency overlays (base + env additions/overrides)
- `--format` controls output format; if omitted, `text` is used.
- `incan env run ...` executes the configured script **without** any further env selection/indirection. In particular,
invoking `incan test` inside an env script must run the test runner directly and must not “re-enter” env resolution.
- Implementations must prevent accidental recursive self-invocation (e.g. an env script calling `incan env run ...` in a
way that would re-resolve the same env). If recursion is detected, the command must fail with a clear diagnostic.
- `--` separates `incan env run` arguments from additional user arguments passed through to the underlying command.
- There are no implicit lifecycle hooks (e.g. no automatic `pre*`/`post*` script execution). Only the explicitly-invoked
  `<script>` is run.
- `--dry-run` must print the resolved command (`cwd`, `env-vars`, argv) and exit successfully without executing it.

Example (`--dry-run` output):

```bash
$ incan env run unit test --dry-run -- -k "addition"
env: unit
cwd: /home/user/my_project
env-vars:
  INCAN_NO_BANNER=1
  INCAN_FANCY_ERRORS=1
command: incan test -k addition
```

> Note: `env-vars` shows the merged result after inheritance — `INCAN_NO_BANNER` comes from `default`,
> `INCAN_FANCY_ERRORS` comes from `unit`.

Example:

```bash
# Run the "test" script in the "unit" env, passing "-k addition" to `incan test`
incan env run unit test -- -k "addition"
```

`env` (Environment) context:

- Environments may define:
    - **`extends`**: an optional list of other env names to include before this env (non-circular)
    - **`detached`**: if `true`, do not include the `default` env (defaults to `false`)
    - **`cwd`**: working directory to run scripts from (relative to project root unless absolute)
    - **`env-vars`**: environment variables injected into the process environment
    - **`scripts`**: a mapping of script name to argv (`List[str]`)
    - **additional Rust dependencies** to be merged into the project dependency set, using RFC 013 schema:
        - `[tool.incan.envs.<name>.dependencies]` (merged into `[dependencies]`)
        - `[tool.incan.envs.<name>.dev-dependencies]` (merged into `[dev-dependencies]`)

Environment inheritance (normative; Hatch-like):

- There is a special env named `default`. If it exists, it is included automatically for every other env **unless**
  `detached = true` is set for that env.
- An env may additionally declare `extends = ["env_a", "env_b", ...]`. These envs are included (in order) before the env
itself.
- Duplicate inclusion is an error: if an env would appear more than once in the resolved overlay chain, `incan env show/run`
must fail with a clear diagnostic. (Rationale: duplicates usually indicate a misconfigured graph and can create surprising override behavior.)
- Cycles are forbidden. If inheritance is circular, `incan env show/run` must fail with a clear diagnostic.
- Inheritance is a configuration overlay mechanism, not isolation: it does not create virtualenv-style sandboxes.
- All overlays are applied deterministically in this order:
project base → `default` (if included) → extended envs (in order) → target env.
- Merge behavior for common env fields:
    - **scripts**: merged by name; later overlays override earlier ones on conflicts
    - **env-vars**: merged by key; later overlays override earlier ones on conflicts (unsetting is out of scope)
    - **cwd**: the last overlay that defines `cwd` wins

Dependency merge semantics:

- Dependencies are **additive** (no removals).
- If the same crate key is specified in both base and env dependencies at any point in the chain:
    - **Version/source**: the env entry **replaces** the base entry.
    - **Features**: the env entry's features are **unioned** with the base entry's features.
- It is an error for an env to define both canonical and alias dependency tables (same rule as RFC 013), applied within
the env scope.
- Envs cannot **remove** base dependencies; they can only add or override.

## Additional Commands (future; non-normative)

These exist today in Makefiles across many repos, and this RFC leans toward **CLI-native** equivalents so projects do not need Make as a dependency.

However, they are intentionally deferred: these commands should be specified in follow-up RFCs once the core semantics of
`incan test` (RFC 019) and policy propagation (RFC 020) are settled.

- `incan fmt` / `incan fmt --check`
- `incan lint` (clippy-like checks for compiler + emitted code)
- `incan smoke-test` (build + tests + examples + benchmark smoke-check, mirroring current repo conventions)
- `incan doctor` (environment diagnostics: toolchain version, cargo, PATH, permissions)
- `incan check-config` (validate `incan.toml` for correctness and conflicts; may be folded into `incan doctor`)
    - Example validations: required keys in `[project]`, path safety, env inheritance cycles, unknown keys/typos

Extensibility (future; non-normative):

- Cargo-style third-party subcommands may be supported (similar to `cargo-foo` → `cargo foo`):
if `incan <cmd>` is not built-in, the CLI may attempt to execute `incan-<cmd>` from `PATH`.

## Layers affected

- **CLI surface** — `incan new`, `incan init`, `incan version`, and `incan env` are new top-level commands introduced by this RFC. Existing project-aware commands (`build`, `run`, `test`, `lock`) must consult `incan.toml` and derive project metadata from it.
- **Project manifest model** — `incan.toml` must be parsed, validated against the `[project]`, `[project.scripts]`, and `[tool.incan.envs.*]` schemas, and diagnosed precisely when keys are missing or invalid.
- **Project root discovery** — commands must walk upward from the current directory to locate `incan.toml`, with a `--project <path>` override, and all project-aware commands must agree on the resolved root.
- **Project generation** — generated project files such as `Cargo.toml` and entrypoint wiring must derive from `incan.toml` metadata rather than hardcoded defaults.
- **Documentation** — a project configuration reference and a "your first Incan project" guide are expected deliverables of this RFC.

## Implementation architecture

*(Non-normative.)* A practical rollout has four broad pieces:

1. **Metadata and scaffolding**: support `incan.toml` parsing and validation, project-root discovery, and the `incan new` / `incan init` scaffolding path.
2. **Version management**: support SemVer-aware version bumping and explicit version setting against the project metadata source of truth.
3. **Environment runner**: support `incan env list`, `show`, and `run`, including inheritance, deterministic overlay rules, dry-run output, and recursion or cycle diagnostics.
4. **Documentation and polish**: provide guide-level docs and examples that make the project lifecycle workflow discoverable for ordinary Incan users rather than repo maintainers only.

Implementation sequencing is not part of the public contract. The design claim is the project-lifecycle CLI surface and `incan.toml` model defined by this RFC, not any one internal rollout order.

## Alternatives considered

- **Rely solely on Makefile targets**: simple but inconsistent across repos, hard to compose and introspect;
    also adds an extra tool dependency we don’t need.
- **Embed everything in Cargo**: good for Rust, but Incan’s source-of-truth isn’t Cargo.toml;
    also doesn’t cover project scaffolding or Incan-centric metadata.
- **Adopt an existing tool (justfile, cargo-make)**: helps execution but doesn’t solve metadata/version semantics.

## Design Decisions

1. Development versions support both `-dev.N` and `-dev+<sha>` forms.
   - `-dev.N` is used for sequential dev releases such as `0.2.0-dev.1`.
   - `-dev+<sha>` is used for CI-oriented build metadata where commit traceability matters.

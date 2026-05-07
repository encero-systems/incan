# Project lifecycle reference

This page is the language-facing reference for Incan project lifecycle concepts: project roots, `incan.toml` metadata, version bumps, and named environments. For the CLI flag reference, see [CLI reference](../../tooling/reference/cli_reference.md).

## Project root

An Incan project root is the nearest ancestor directory containing `incan.toml`.

```text
greeter/
|-- incan.toml
|-- src/
|   |-- main.incn
|   `-- greet.incn
`-- tests/
    `-- test_main.incn
```

Project-aware commands use the project root for metadata, dependencies, source-root resolution, lock files, and lifecycle configuration. Nested projects are allowed; the nearest `incan.toml` wins.

Single-file commands can still run without a project:

```bash
incan run hello.incn
incan run -c "import this"
```

Project-level features such as manifest dependencies, version management, and named environments require `incan.toml`.

## `incan.toml`

`incan.toml` is the project manifest. It is intended to be edited and committed.

Common sections:

| Section                    | Purpose                                                                                       |
| -------------------------- | --------------------------------------------------------------------------------------------- |
| `[project]`                | Project metadata: name, version, description, authors, license, readme, toolchain requirement |
| `[project.scripts]`        | Named Incan entry points such as `main = "src/main.incn"`                                     |
| `[build]`                  | Build settings such as `source-root`                                                          |
| `[dependencies]`           | Incan library dependencies                                                                    |
| `[rust-dependencies]`      | Rust crate dependencies available to production code                                          |
| `[rust-dev-dependencies]`  | Rust crate dependencies available only to tests                                               |
| `[tool.incan.envs.<name>]` | Named lifecycle environments for `incan env`                                                  |

Minimal application manifest:

```toml title="incan.toml"
[project]
name = "greeter"
version = "0.1.0"
requires-incan = ">=0.2.0"

[project.scripts]
main = "src/main.incn"
```

## Project scaffolding

`incan new` creates a new project directory. With no positional name, it prompts interactively when stdin is a terminal:

```bash
incan new
```

For scripted use, pass a name or `--dir` and use `--yes`:

```bash
incan new greeter --yes
incan new --dir apps/greeter --yes
```

Both `incan new` and `incan init` accept metadata flags:

| Flag                    | Meaning                                      |
| ----------------------- | -------------------------------------------- |
| `--description <text>`  | Write `[project].description`                |
| `--author <author>`     | Add one `[project].authors` entry            |
| `--license <license>`   | Write `[project].license`                    |
| `--name <name>`         | Override the project name for `incan init`   |
| `--version <version>`   | Override the initial version for `incan init` |
| `--yes` / `-y`          | Skip prompts and use defaults/flag values    |

`incan new` derives the project name from `NAME`, then from `--dir`, then from an interactive prompt. In non-interactive mode it requires either `NAME` or `--dir`.

## `[project]`

`[project]` is the canonical metadata table.

| Key              | Type            | Notes                                             |
| ---------------- | --------------- | ------------------------------------------------- |
| `name`           | string          | Stable project name                               |
| `version`        | string          | SemVer-compatible project version                 |
| `description`    | string          | Short human-readable description                  |
| `authors`        | list of strings | Author names, optionally with email addresses     |
| `maintainers`    | list of strings | Maintainer names, optionally with email addresses |
| `license`        | string          | SPDX identifier or expression                     |
| `license-files`  | list of strings | License file paths, relative to the project root  |
| `readme`         | string          | Path to README, relative to project root          |
| `homepage`       | string          | Project homepage URL                              |
| `repository`     | string          | Source repository URL                             |
| `documentation`  | string          | Documentation URL                                 |
| `issues`         | string          | Issue tracker URL                                 |
| `keywords`       | list of strings | Search/discovery keywords                         |
| `classifiers`    | list of strings | Future-facing classifier strings                  |
| `requires-incan` | string          | SemVer requirement for the Incan toolchain        |
| `private`        | bool            | Marks a project as not intended for publishing    |

`incan init --name greeter --version 0.1.0` writes the core project keys, `readme = "README.md"`, and a default `main` script. Metadata flags or interactive answers populate optional fields such as `description`, `authors`, and `license`.

## `[project.scripts]`

`[project.scripts]` maps script names to Incan source files:

```toml
[project.scripts]
main = "src/main.incn"
migrate = "src/migrate.incn"
```

These are Incan entry points, not shell commands. Use `[tool.incan.envs.<name>.scripts]` for shell-style command argv lists.

## Source root

The source root controls how local imports resolve.

Resolution order:

1. Use `[build] source-root` when it is set.
2. Otherwise use `src/` when the project has that directory.
3. Otherwise use the project root.

```toml
[build]
source-root = "src"
```

Tests resolve imports against the same source root as production code, so `tests/test_main.incn` can import `src/greet.incn` as `from greet import greet`.

## `incan version`

`incan version` updates the project version in `incan.toml`.

```bash
incan version patch
incan version minor --dry-run
incan version --set 1.2.0
```

Supported bump names:

| Bump    | Result                                                    |
| ------- | --------------------------------------------------------- |
| `major` | Increment the major version and clear prerelease metadata |
| `minor` | Increment the minor version and clear prerelease metadata |
| `patch` | Increment the patch version and clear prerelease metadata |
| `alpha` | Add or advance an `-alpha.N` prerelease                   |
| `beta`  | Add or advance a `-beta.N` prerelease                     |
| `rc`    | Add or advance an `-rc.N` prerelease                      |
| `dev`   | Add or advance a development prerelease                   |

Useful flags:

| Flag                | Meaning                                                                        |
| ------------------- | ------------------------------------------------------------------------------ |
| `--dry-run`         | Print the old version, new version, and modified files without writing changes |
| `--set <version>`   | Set an explicit SemVer-compatible version                                      |
| `--keep-prerelease` | Keep prerelease metadata when applying a release-core bump                     |

This command changes the project version only. It does not update the compiler, Cargo package versions in the Incan repository, or the `requires-incan` toolchain requirement.

## `incan env`

`incan env` runs named scripts inside named project environments. The ambient `default` environment is always available, and other environments include it unless they set `detached = true`.

Mental model:

- An env is a named command context, not a Python-style virtualenv.
- Env scripts are explicit argv lists stored in `incan.toml`.
- `incan env` is for repeatable workflows such as local test commands, CI commands, docs builds, or release checks.
- Plain `incan run`, `incan test`, and `incan build` remain valid direct commands; envs are an overlay for named workflows, not a replacement for the base CLI.

Subcommands:

| Command                        | Purpose                                                      |
| ------------------------------ | ------------------------------------------------------------ |
| `incan env list`               | List available environment names                             |
| `incan env show [env]`         | Show an overview table or print one resolved environment     |
| `incan env run <env> <script>` | Run one configured script inside one environment             |

Example configuration:

```toml title="incan.toml"
[tool.incan.envs.default]
env-vars = { INCAN_NO_BANNER = "1" }

[tool.incan.envs.default.scripts]
run = ["incan", "run"]
test = ["incan", "test"]

[tool.incan.envs.unit]
env-vars = { INCAN_FANCY_ERRORS = "1" }

[tool.incan.envs.unit.scripts]
test = ["incan", "test", "tests/"]

[tool.incan.envs.ci]
extends = ["unit"]

[tool.incan.envs.ci.scripts]
test = ["incan", "test", "--locked", "tests/"]
build = ["incan", "build", "src/main.incn", "--locked"]

[tool.incan.envs.docs]
detached = true
cwd = "workspaces/docs-site"

[tool.incan.envs.docs.scripts]
build = ["python3", "-m", "mkdocs", "build", "--strict"]
```

Example commands:

```bash
incan env list
incan env show
incan env show default
incan env show unit
incan env show ci
incan env show docs
incan env run default run
incan env run unit test -- -k "greet"
incan env run ci build
incan env run docs build
incan env run unit test --dry-run -- -k "greet"
```

Arguments after `--` are appended to the configured script argv.

`incan env show` with no env name prints a compact overview table, similar to Hatch. `incan env show default` works even when `[tool.incan.envs.default]` is not declared. In that case, `default` exposes the project base overlay with no extra overrides.

Typical pattern:

- `default` for shared baseline commands and environment variables
- a local developer env such as `unit`
- a stricter automation env such as `ci`
- a detached env for a separate subtree such as `docs`

## Environment fields

| Field      | Type                  | Meaning                                                                     |
| ---------- | --------------------- | --------------------------------------------------------------------------- |
| `extends`  | list of strings       | Other environments to merge before this one                                 |
| `detached` | bool                  | Do not include `default` automatically                                      |
| `cwd`      | string                | Working directory for scripts, relative to the project root unless absolute |
| `env-vars` | table                 | Environment variables to inject into the process                            |
| `scripts`  | table of string lists | Script names mapped to argv lists                                           |

Dependency overlay tables may also be used for environment-specific dependencies:

```toml
[tool.incan.envs.integration.rust-dev-dependencies]
testcontainers = "0.15"
```

## Environment merge rules

Overlay order:

```text
project base -> default -> extends entries -> target environment
```

Rules:

| Field        | Merge behavior                                                            |
| ------------ | ------------------------------------------------------------------------- |
| `scripts`    | Merge by name; later overlays replace earlier scripts with the same name  |
| `env-vars`   | Merge by key; later overlays replace earlier values with the same key     |
| `cwd`        | Last configured value wins                                                |
| Dependencies | Additive; same dependency key replaces version/source and unions features |

`default` is always present conceptually; declaring `[tool.incan.envs.default]` customizes it rather than creating it from nothing. Duplicate environment inclusion and inheritance cycles are errors. Use `incan env show <env>` to debug the resolved overlay chain.

Practical implications:

- Use `default` for shared baseline behavior such as `INCAN_NO_BANNER=1` or common `run` / `test` scripts.
- Use `extends` when one env is a stricter refinement of another, for example `ci` extending `unit`.
- Use `detached = true` when an env should ignore the default baseline entirely, such as a docs build rooted in another directory.
- Prefer shallow inheritance. If you need a diagram to explain your env graph, it is probably too complex.

## See also

- [Project lifecycle](../how-to/project_lifecycle.md)
- [Project configuration (`incan.toml`)](../../tooling/reference/project_configuration.md)
- [Managing dependencies](../../tooling/how-to/dependencies.md)
- [CLI reference](../../tooling/reference/cli_reference.md)

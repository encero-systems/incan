# Project lifecycle

This guide shows the practical project workflow: create a project, keep project metadata in `incan.toml`, bump the project version, and run repeatable commands through named environments.

```mermaid
flowchart LR
  A["new or init"] --> B["incan.toml + source + tests"]
  B --> C["run and test"]
  C --> D["lock and build"]
  D --> E["version bump"]
  E --> F["release workflow"]
  F -. "next change" .-> C
```

<p class="inc-diagram-caption">The manifest anchors a repeatable loop from scaffold through testing, locked builds, and release.</p>

## Create a project

Use `incan new` when you want the CLI to create a new project directory:

```bash
incan new
```

In interactive mode, `incan new` asks for the project name, version, description, author, and license before writing files. For CI or scripted setup, pass values as flags and skip prompts:

```bash
incan new greeter \
  --description "A small greeting command" \
  --author "Ada Example <ada@example.com>" \
  --license MIT \
  --yes
cd greeter
```

Use `incan init` when you already have a directory. It uses the same metadata prompts in interactive mode and the same metadata flags in non-interactive mode:

```bash
mkdir greeter
cd greeter
incan init \
  --name greeter \
  --version 0.1.0 \
  --description "A small greeting command" \
  --author "Ada Example <ada@example.com>" \
  --license MIT \
  --yes
```

The scaffold gives you:

```text
greeter/
|-- src/
|   `-- main.incn
|-- tests/
|   `-- test_main.incn
|-- README.md
|-- .gitignore
`-- incan.toml
```

Run the generated entry point and starter tests:

```bash
incan run
incan test
```

`incan.toml` marks the project root. Project-aware commands discover it by walking upward from your current directory. File and directory arguments are still shell paths, so write them relative to the directory you are running the command from.

## Fill in project metadata

Open `incan.toml` and treat `[project]` as the user-facing metadata for the project:

```toml title="incan.toml"
[project]
name = "greeter"
version = "0.1.0"
description = "A small greeting command"
authors = ["Ada Example <ada@example.com>"]
license = "MIT"
readme = "README.md"
requires-incan = ">=0.2.0"

[project.scripts]
main = "src/main.incn"
```

Keep this file in version control. It is the source of truth for project metadata, dependency declarations, source-root configuration, and lifecycle settings. Generated files under `target/` are build output; delete them if needed, but do not edit them as configuration.

## Use scripts for entry points

`[project.scripts]` maps names to Incan entry-point files. The `main` script is the default entry point used by project lifecycle commands that need one:

```toml
[project.scripts]
main = "src/main.incn"
migrate = "src/migrate.incn"
```

Use scripts for stable entry-point names, not shell automation. Shell-style lifecycle commands belong under `incan env` configuration.

## Bump the project version

Use `incan version` to update the project version in `incan.toml`:

```bash
incan version patch
```

Before changing files, check what would happen:

```bash
incan version minor --dry-run
```

Use an explicit version when release automation has already chosen one:

```bash
incan version --set 1.2.0
```

`incan version` changes the project version, not the compiler or toolchain version. Toolchain requirements stay in `requires-incan`, and compiler releases are maintained separately from your app or library.

## Set the required Incan toolchain

Use `[project].requires-incan` when a project needs a particular Incan release line:

```toml
[project]
name = "greeter"
version = "0.1.0"
requires-incan = ">=0.3,<0.4"
```

Project-aware execution commands enforce this before they do real work. If the active compiler is incompatible, `incan run`, `incan build`, `incan test`, `incan lock`, and `incan env run` fail with the active version and the requirement that rejected it. Plain single-file or inline commands outside a project do not use `requires-incan`.

You can make a named env stricter for release or CI workflows:

```toml
[tool.incan.envs.ci]
requires-incan = ">=0.3,<0.4"

[tool.incan.envs.ci.scripts]
test = ["incan", "test", "--locked"]
```

Inspect the effective requirement before running the script:

```bash
incan env show ci
incan env run ci test --dry-run
```

## Define repeatable environments

Use `incan env` when a project has commands that should always run with the same arguments, working directory, or environment variables.

The important mental model is:

- `[project.scripts]` is for Incan entry points such as `main = "src/main.incn"`
- `[tool.incan.envs.<name>.scripts]` is for named shell-style commands such as `["incan", "test", "--locked"]`
- `incan env` does not create a virtual environment or a shell session; it resolves a named command context and runs it explicitly

If all you want is "run the app" or "run the tests", plain `incan run` and `incan test` are still the normal commands. Reach for `incan env` when the project starts to accumulate named workflows such as local test runs, CI runs, docs builds, or release checks.

### A real-world env setup

This is a realistic small-project setup:

```toml title="incan.toml"
[tool.incan.envs.default]
env-vars = { INCAN_NO_BANNER = "1" }

[tool.incan.envs.default.scripts]
run = ["incan", "run"]
test = ["incan", "test"]
lock = ["incan", "lock"]

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
serve = ["python3", "-m", "mkdocs", "serve"]
```

That gives you four useful command contexts:

- `default`: the baseline command set for everyday local work
- `unit`: local test commands with extra developer-friendly settings
- `ci`: stricter test/build commands for automation
- `docs`: a detached docs workflow with its own working directory

### What you run

List the available envs:

```bash
incan env list
```

See the overview table:

```bash
incan env show
```

Inspect one resolved env before using it:

```bash
incan env show default
incan env show unit
incan env show ci
incan env show docs
```

Run the named scripts:

```bash
incan env run default run
incan env run unit test
incan env run ci build
incan env run docs build
```

Append extra arguments after `--`:

```bash
incan env run unit test -- -k "greet"
```

Use `--dry-run` while editing env configuration:

```bash
incan env run ci test --dry-run -- -k "greet"
```

### When `incan env` is worth it

`incan env` earns its keep when the same command needs to be remembered by multiple people or multiple systems.

Good uses:

- local developer shortcuts that should stay stable across the team
- CI commands that must always run with `--locked` or other policy flags
- docs or examples that live in another working directory
- release or verification commands with a stricter environment than day-to-day development

Poor uses:

- wrapping every ordinary command just because the feature exists
- building deep inheritance trees that are hard to inspect
- using envs as a replacement for `[project.scripts]`

## Use environment inheritance carefully

Every project has an ambient `default` env, even if you never declare `[tool.incan.envs.default]`. Declaring it customizes that baseline rather than creating a special extra env.

Other envs include `default` automatically unless they set `detached = true`.

Use `default` for shared baseline behavior:

```toml
[tool.incan.envs.default]
env-vars = { INCAN_NO_BANNER = "1" }

[tool.incan.envs.default.scripts]
test = ["incan", "test"]
```

Use `extends` when one env is a stricter or more specialized version of another:

```toml
[tool.incan.envs.ci]
extends = ["unit"]

[tool.incan.envs.ci.scripts]
test = ["incan", "test", "--locked", "tests/"]
```

Use `detached = true` when an env should stand on its own:

```toml
[tool.incan.envs.docs]
detached = true
cwd = "workspaces/docs-site"
```

Avoid clever inheritance. Two or three layers are usually enough. Duplicate inclusion and cycles are configuration errors, and `incan env show <env>` is the fastest way to confirm what a script will actually run.

## Project workflow checklist

For a typical application:

1. Create the scaffold with `incan new` or `incan init`.
2. Fill in `[project]` metadata before sharing the repo.
3. Keep `[project.scripts].main` pointed at the default entry point.
4. Commit `incan.toml` and `incan.lock`.
5. Use `incan version --dry-run` before bumping releases.
6. Put repeatable local and CI commands under `incan env`, then inspect them with `incan env show` or `--dry-run`.

## See also

- [Project lifecycle reference](../reference/project_lifecycle.md)
- [Imports and modules](imports_and_modules.md)
- [Project configuration (`incan.toml`)](../../tooling/reference/project_configuration.md)
- [CLI reference](../../tooling/reference/cli_reference.md)

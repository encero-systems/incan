# Work with several projects

Give each project its own `loaf.toml`, source tree and package identity. Connect reusable libraries through explicit path dependencies, and declare dependencies several projects share once in a workspace root.

## Connect a consumer to a producer

```text
repository/
├── producer/
│   ├── loaf.toml
│   └── src/lib.incn
└── consumer/
    ├── loaf.toml
    └── src/main.incn
```

In the consumer manifest:

```toml
[project]
name = "consumer"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"

[dependencies]
pricing = { path = "../producer" }
```

Import the producer's public exports through `pub::pricing`. Run the consumer by its source path:

```bash
incan run consumer/src/main.incn
```

The source path selects the consumer's manifest even when invoked from the repository root. Oven prepares its path dependencies. There is no shared workspace lock or implicit fan-out.

Follow [Build and consume a library](../../tooling/tutorials/build_and_consume_library.md) for the complete source.

## Share dependencies across members

A workspace root lists its members and declares the dependencies they share:

```toml
[project]
name = "pricing"
version = "0.1.0"

[workspace]
members = ["consumer"]

[workspace.dependencies]
pricing = { path = "." }
json = { loaf = "crates-io/serde_json", version = "1" }
```

A member inherits an entry by name:

```toml
[dependencies]
pricing = { workspace = true }
json = { workspace = true, features = ["preserve_order"] }
```

The root's `path = "."` reaches the member as `path = ".."`: a local path is spelled from the root and rebased onto each member. See [`[workspace]`](../../tooling/reference/project_configuration.md#workspace).

## Select members

Name the members a command acts on with `--workspace`, `--member NAME_OR_PATH` (or `-p`) and `--exclude NAME_OR_PATH`:

```bash
oven check --workspace
oven test --member consumer
```

Without them, a command in a member's directory acts on that member. At the root it acts on `default-members` when the root declares them, otherwise on the root project, or on every member of a virtual workspace. `oven workspace inspect` shows which members a command would act on. See [Member selection](../../tooling/reference/cli_reference.md#member-selection).

## Share environments across members

A workspace root can declare environment fragments under `[workspace.envs.NAME]`:

```toml
[workspace.envs.ci]
env-vars = { CI = "1" }
scripts.lint = ["incan", "fmt", "--check", "."]
```

A member's environment takes a fragment by extending it as `workspace:NAME`; the fragment applies before the member's own layer, so the member can override a variable or script:

```toml
[envs.ci]
extends = ["workspace:ci"]
env-vars = { CI_MEMBER = "consumer" }
```

`oven env show ci` in the member shows the chain `project -> dev -> workspace:ci -> ci`. See [`[workspace]`](../../tooling/reference/project_configuration.md#workspace).

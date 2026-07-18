# Project configuration (`incan.toml`)

This is the reference for the `incan.toml` project manifest format. For a practical guide to managing dependencies, see: [Managing dependencies](../how-to/dependencies.md).

## Overview

`incan.toml` is an optional project manifest that lives at your project root. It declares project metadata, build configuration, Incan library dependencies, Rust crate dependencies, and optional vocab companion crate settings. Project-aware commands discover it by walking upward from the current working directory, and file-oriented commands may also resolve it from the provided source path.

```text
my_project/
├── src/
│   └── main.incn
├── tests/
│   └── test_main.incn
├── incan.toml            # Project manifest
└── incan.lock            # Generated lock file (commit to VCS)
```

You can scaffold a full new project (manifest, entry point, starter test, README, and `.gitignore`) with `incan new`. Use `incan init` when you already have a directory and want to add Incan project files there.

## `[project]`

Project metadata. All fields are optional.

```toml
[project]
name = "my_app"
version = "0.1.0"
description = "A short description of what this project does"
authors = ["Alice <alice@example.com>"]
license = "MIT"
readme = "README.md"
requires-incan = ">=0.2"
```

`requires-incan` is enforced by project-aware execution commands. If the active compiler does not satisfy the requirement, `incan run` in project mode, `incan build`, `incan test`, `incan lock`, and `incan env run` fail before compiling, locking, or launching scripts. Single-file or inline commands without a discovered project manifest do not infer a toolchain requirement.

### `[project.scripts]`

Named entry points for CLI commands:

```toml
[project.scripts]
main = "src/main.incn"        # This is the default entry point if no other is set
migrate = "src/migrate.incn"  # This is an example of a named entry point called "migrate"
```

`incan new` and `incan init` set `main = "src/main.incn"` by default. When `main` is set, `incan lock` can run without a `FILE` argument.

### `[project.features]`

Public Incan package features are additive, package-owned switches. A feature may include another local feature, activate an optional Incan dependency, request a public feature from an active Incan dependency, or require an SDK component that the project has already enabled. It cannot directly select Cargo features, remove API, change runtime configuration, or install an SDK component.

The compact form uses checked references:

```toml
[project.features]
default = ["json"]
json = ["dep:serializer", "serializer/json"]
full = ["json", "server"]
```

Reference forms:

| Form | Meaning |
| --- | --- |
| `json` | Include feature `json` from the same package. |
| `dep:serializer` | Activate optional Incan dependency `serializer`. |
| `serializer/json` | Request feature `json` from active Incan dependency `serializer`. |

The expanded form names each edge type explicitly:

```toml
[project.features.server]
includes = ["json"]
optional-dependencies = ["http_server"]
dependency-features = { http_server = ["tls"] }
requires-sdk-components = ["stdlib-web"]
```

Compact and expanded declarations normalize into the same graph. A feature declaration uses one form or the other, not both. `default` is an ordinary feature name selected automatically unless the command or dependency edge disables defaults.

Use `when feature("name"):` to attach a positive feature requirement to source declarations. See [Conditional compilation](../../language/reference/conditional_compilation.md) and [SDK components and package features](sdk_components_and_package_features.md).

## `[sdk]`

The SDK section selects which official SDK components are enabled for the project. Omitting it selects the active SDK release's `default` profile.

```toml
[sdk]
profile = "minimal"
components = ["stdlib-system", "stdlib-data"]
exclude-components = ["stdlib-web"]
```

| Field | Type | Description |
| --- | --- | --- |
| `profile` | string | Base component profile. Incan 0.5 SDKs define `minimal`, `default`, and `full`. |
| `components` | list of strings | Components added to the selected profile before dependency expansion. |
| `exclude-components` | list of strings | Components that must not remain in the expanded selection. |

`stdlib-core` is mandatory. Selected components bring their declared component dependencies. Excluding a mandatory component or a component still required by another enabled component is an error. Selection does not install anything: an enabled component can still be unavailable in the active SDK installation, and that is diagnosed separately.

The nine v0.5 standard-library components are `stdlib-core`, `stdlib-system`, `stdlib-codecs`, `stdlib-compression`, `stdlib-data`, `stdlib-async`, `stdlib-observability`, `stdlib-web`, and `stdlib-testing`. Component membership and dependencies are SDK-versioned; source imports remain stable `std.*` paths.

## `[build]`

Build configuration. All fields are optional.

```toml
[build]
rust-edition = "2021"       # Rust edition for the generated Cargo.toml (default: compiler-chosen)
profile = "release"         # Cargo build profile
target = "x86_64-unknown-linux-gnu"  # Cross-compilation target
source-root = "src"         # Source root for module resolution (default: convention-based)
```

### `source-root`

The directory where the compiler and test runner look for user modules. Resolution order:

1. **Explicit**: if `source-root` is set, that directory is used (relative to project root)
2. **Convention**: if a `src/` directory exists at the project root, it is used automatically
3. **Fallback**: the project root itself (flat layout)

Most projects use the conventional `src/` layout and don't need to set this field. It exists for projects that keep their source in a different directory (e.g. `lib/`).

## `[tool.incan.envs]`

Named project environments define reusable command contexts for `incan env`. They are useful when a project has different test, build, or release workflows that need different environment variables, working directories, or script arguments. The ambient `default` environment is always available; defining `[tool.incan.envs.default]` customizes it.

```toml
[tool.incan.envs.default]
env-vars = { INCAN_NO_BANNER = "1" }

[tool.incan.envs.default.scripts]
test = ["incan", "test", "tests/"]

[tool.incan.envs.release]
extends = ["default"]
requires-incan = ">=0.3,<0.4"
env-vars = { INCAN_FANCY_ERRORS = "1" }

[tool.incan.envs.release.scripts]
build = ["incan", "build", "src/main.incn", "--locked"]
```

Fields:

| Field      | Type                  | Description                                                                 |
| ---------- | --------------------- | --------------------------------------------------------------------------- |
| `extends`  | list of strings       | Other environments to merge before this one                                 |
| `requires-incan` | string          | Additional Incan toolchain requirement for this env                         |
| `detached` | bool                  | Do not include `default` automatically                                      |
| `cwd`      | string                | Working directory for scripts, relative to the project root unless absolute |
| `env-vars` | table                 | Environment variables to inject into the process                            |
| `scripts`  | table of string lists | Script names mapped to argv lists                                           |

Env-level `requires-incan` narrows the project requirement for that environment. `incan env show <env>` and `incan env run <env> <script> --dry-run` display the effective requirement and whether the active compiler satisfies it; actual `incan env run` execution rejects unsatisfied constraints before spawning the script.

Environment matrices from RFC 073 remain deferred beyond `0.3`; ordinary named envs may declare `requires-incan`, but matrix expansion is not available yet.

Use the environment with:

```bash
incan env list
incan env show release
incan env run release build
```

See [Project lifecycle reference](../../language/reference/project_lifecycle.md) for merge and resolution rules.

## `[tool.incan.metadata]`

Checked contract metadata settings for project-declared model bundles.

```toml
[tool.incan.metadata]
model-bundles = ["contracts/order_summary.json"]
```

Fields:

| Field           | Type            | Description                                                                                                                                                                     |
| --------------- | --------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `model-bundles` | list of strings | Canonical model bundle JSON files to validate, materialize during build/run, embed into `.incnlib` artifacts when publishable, and expose through `incan tools metadata model`. |

Bundle paths are resolved relative to the project root unless absolute. See [Checked contract metadata](contract_metadata.md) for the bundle schema and tooling commands.

## `[vocab]`

Optional companion crate configuration for library-defined DSL metadata.

```toml
[vocab]
crate = "vocab_companion"
```

Use this only for library projects that export vocab entries. Projects without custom library DSLs can omit the section entirely.

### Fields

| Field   | Type   | Description                                                                           |
| ------- | ------ | ------------------------------------------------------------------------------------- |
| `crate` | string | Path to the vocab companion crate directory, relative to project root unless absolute |

During `incan build --lib`, the compiler:

1. resolves `[vocab].crate`
2. validates that the directory contains `Cargo.toml` and `src/lib.rs`
3. runs `cargo build` for that companion crate
4. derives the vocab payload from the companion crate's `library_vocab()` registration
5. packages the resulting metadata into the built `.incnlib` artifact

If the companion crate registers a desugarer via `incan_vocab::DesugarerRegistration`, `incan build --lib` also packages the matching Wasm artifact from the companion crate's build output. Any intermediate serialized metadata is a tooling concern (rather than part of the author-facing contract).

## `[dependencies]`

Incan library dependencies available in all contexts. (Note: for Rust crates, see `[rust-dependencies]`).

```toml
[dependencies]
mylib = { path = "../mylib" }
reporting = { path = "../reporting", default-features = false, features = ["json"] }
serializer = { path = "../serializer", optional = true }
```

Incan dependency table fields:

| Field | Type | Description |
| --- | --- | --- |
| `path` | string | Local library project path, relative to `incan.toml`. |
| `optional` | bool | Keep the dependency edge inactive until a package feature selects `dep:<name>`. |
| `default-features` | bool | Select the dependency's `default` feature; defaults to `true`. |
| `features` | list of strings | Public Incan features requested from the dependency. |

Feature requests are unified additively across every active dependency edge. They are Incan package features, not Cargo features; Rust crates continue to use the explicitly separate `[rust-dependencies]` and `--cargo-*` surfaces.

## `[rust-dependencies]`

Rust crate dependencies available in all contexts (build, run, test).

### String shorthand

For simple registry dependencies with just a version:

```toml
[rust-dependencies]
serde = "1.0"
rand = "0.8"
```

### Table form

For dependencies that need features, sources, or other options:

```toml
[rust-dependencies]
tokio = { version = "1.35", features = ["full"] }
serde = { version = "1.0", features = ["derive"], default-features = true }
```

### All fields

| Field              | Type   | Description                                              |
| ------------------ | ------ | -------------------------------------------------------- |
| `version`          | string | Cargo SemVer version requirement (required for registry) |
| `features`         | list   | Cargo features to enable                                 |
| `default-features` | bool   | Whether to include default features (default: `true`)    |
| `optional`         | bool   | Mark as optional (see below)                             |
| `package`          | string | The actual crate name if renaming (e.g. `serde-json`)    |
| `git`              | string | Git repository URL (mutually exclusive with `path`)      |
| `branch`           | string | Git branch (requires `git`)                              |
| `tag`              | string | Git tag (requires `git`)                                 |
| `rev`              | string | Git commit hash (requires `git`)                         |
| `path`             | string | Local path, relative to `incan.toml` location            |

## `[rust-dev-dependencies]`

Dependencies available only in test contexts (`tests/` directory). Same syntax as `[rust-dependencies]`.

```toml
[rust-dev-dependencies]
criterion = "0.5"
test_helpers = { path = "../test-helpers" }
```

Importing a dev-only crate from production code is a compile-time error:

```text
error: Rust crate `criterion` is dev-only and cannot be imported from production code
hint: Move the dependency to [rust-dependencies], or import it only from tests.
```

**Overlap rules**: If the same crate appears in both `[rust-dependencies]` and `[rust-dev-dependencies]`, the version, source, and default-features must match. Features are unioned, and the crate is treated as a normal dependency.

## `[rust-dependencies.optional]`

Syntactic sugar for declaring optional (rust) dependencies. Entries here are equivalent to setting `optional = true`:

```toml
[rust-dependencies.optional]
fancy_logging = "0.3"
metrics = { version = "1.0", features = ["prometheus"] }
```

is equivalent to:

```toml
[rust-dependencies]
fancy_logging = { version = "0.3", optional = true }
metrics = { version = "1.0", features = ["prometheus"], optional = true }
```

Optional dependencies generate a Cargo feature gate. Enable them at build time:

```bash
incan build src/main.incn --cargo-features fancy_logging
```

## Legacy alias tables

`[rust.dependencies]` and `[rust.dev-dependencies]` remain supported as backward-compatible aliases for `[rust-dependencies]` and `[rust-dev-dependencies]`. Prefer the non-nested table names in new manifests.

## Dependency sources

### Registry (default)

The default source is crates.io. Version is required:

```toml
[rust-dependencies]
serde = "1.0"
tokio = { version = "1.35", features = ["full"] }
```

### Git

Specify a git repository URL with exactly one of `branch`, `tag`, or `rev`:

```toml
[rust-dependencies]
my_internal_lib = { git = "https://github.com/company/lib.git", tag = "v1.0.0" }
bleeding_edge = { git = "https://github.com/company/lib.git", branch = "main" }
pinned = { git = "https://github.com/company/lib.git", rev = "abc1234" }
```

!!! warning "Strict mode and git branches"
    When building with `--locked` or `--frozen`, git dependencies using `branch = "..."` are rejected because branches are not reproducible. Use `tag` or `rev` instead.

### Path

Local path dependencies, relative to the `incan.toml` location:

```toml
[rust-dependencies]
shared_utils = { path = "../shared-utils" }
```

### Package renames

Use `package` to use a different crate name than the dependency key. For example:

```toml
[rust-dependencies]
json = { package = "serde_json", version = "1.0" }
```

This lets you `import rust::json` instead of `import rust::serde_json`.

## Complete example

```toml
[project]
name = "my_web_app"
version = "0.1.0"
description = "A web application built with Incan"
authors = ["Alice <alice@example.com>"]

[project.scripts]
main = "src/main.incn"

[build]
rust-edition = "2021"

[vocab]
crate = "vocab_companion"

[dependencies]
mylib = { path = "../mylib/target/lib" }

[rust-dependencies]
tokio = { version = "1.35", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
sqlx = { version = "0.7", features = ["runtime-tokio", "postgres"] }

[rust-dependencies.optional]
fancy_logging = "0.3"

[rust-dev-dependencies]
criterion = "0.5"

[tool.incan.envs.default]
env-vars = { INCAN_NO_BANNER = "1" }

[tool.incan.envs.default.scripts]
test = ["incan", "test", "tests/"]
```

## See also

- [Managing dependencies](../how-to/dependencies.md) - Practical guide
- [Rust interop](../../language/how-to/rust_interop.md) - Inline version annotations
- [CLI reference](cli_reference.md) - `incan new`, `incan init`, `incan version`, `incan env`, and dependency flags
- [Project lifecycle reference](../../language/reference/project_lifecycle.md) - Version bump and environment semantics
- [Author library DSLs with `incan_vocab`](../../contributing/how-to/authoring_vocab_crates.md) - Companion crate workflow

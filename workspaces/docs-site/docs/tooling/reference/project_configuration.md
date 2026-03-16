# Project configuration (`incan.toml`)

This is the reference for the `incan.toml` project manifest format. For a practical guide to managing dependencies, see:
[Managing dependencies](../how-to/dependencies.md).

## Overview

`incan.toml` is an optional project manifest that lives at your project root. It declares project metadata, build
configuration, Incan library dependencies, Rust crate dependencies, and optional vocab companion crate settings. The
compiler discovers it by walking upward from the source file's directory.

```text
my_project/
├── src/
│   └── main.incn
├── tests/
│   └── test_main.incn
├── incan.toml            # Project manifest
└── incan.lock            # Generated lock file (commit to VCS)
```

You can scaffold a full project (manifest, entry point, and starter test) with `incan init`.

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

### `[project.scripts]`

Named entry points for CLI commands:

```toml
[project.scripts]
main = "src/main.incn"        # This is the default entry point if no other is set
migrate = "src/migrate.incn"  # This is a example of a named entry point called "migrate"
```

`incan init` sets `main = "src/main.incn"` by default. When `main` is set, `incan lock` can run without
a `FILE` argument.

### `[project.features]`

Optional feature flags that can enable optional dependencies:

```toml
[project.features]
full = ["fancy_logging", "metrics"]
```

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

Most projects use the conventional `src/` layout and don't need to set this field. It exists for projects that keep
their source in a different directory (e.g. `lib/`).

## `[vocab]`

Optional companion crate configuration for library-defined DSL metadata.

```toml
[vocab]
crate = "vocab_companion"
```

Use this only for library projects that export vocab entries. Projects without custom library DSLs can omit the section entirely.

### Fields

|  Field  |  Type  |                                      Description                                      |
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
```

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

|       Field        |  Type  |                       Description                        |
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

`[rust.dependencies]` and `[rust.dev-dependencies]` remain supported as backward-compatible aliases for `[rust-dependencies]` and `[rust-dev-dependencies]`.
Prefer the non-nested table names in new manifests.

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
    When building with `--locked` or `--frozen`, git dependencies using `branch = "..."` are rejected
    because branches are not reproducible. Use `tag` or `rev` instead.

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
```

## See also

- [Managing dependencies](../how-to/dependencies.md) - Practical guide
- [Rust interop](../../language/how-to/rust_interop.md) - Inline version annotations
- [CLI reference](cli_reference.md) - `incan init`, `incan lock`, and dependency flags
- [Author library DSLs with `incan_vocab`](../../contributing/how-to/authoring_vocab_crates.md) - Companion crate workflow

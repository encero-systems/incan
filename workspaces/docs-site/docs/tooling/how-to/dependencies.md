# Managing dependencies

This guide covers how to add, configure, and lock Rust crate dependencies in Incan projects.

For the full manifest format, see: [Project configuration reference](../reference/project_configuration.md).  
For inline import syntax, see: [Rust interop](../../language/how-to/rust_interop.md).

## Adding a Rust crate (quick start)

The simplest way to use a Rust crate is with an inline version annotation:

```incan
import rust::my_crate @ "1.0"
```

This works in any `.incn` file, no configuration files needed. The compiler adds the dependency to the generated
`Cargo.toml` automatically.

For common crates (serde, tokio, reqwest, etc.), you don't even need a version — the compiler has tested defaults:

```incan
import rust::serde_json as json    # Uses known-good default: serde_json 1.0
import rust::tokio                 # Uses known-good default: tokio 1 with common features
```

## Using `incan.toml` for project dependencies

For projects with more than a handful of dependencies, create an `incan.toml` manifest:

```bash
incan init
```

This creates a starter `incan.toml`. Then declare your dependencies:

```toml
[project]
name = "my_app"

[rust-dependencies]
tokio = { version = "1.35", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
```

Once a crate is in `incan.toml`, the manifest is the single source of truth. Inline `@ "version"`
annotations for that crate are not allowed — use bare imports instead:

```incan
# Good: bare import, version comes from incan.toml
import rust::tokio

# Error: inline annotation conflicts with incan.toml
import rust::tokio @ "2.0"
```

## Specifying features

### Inline

```incan
import rust::tokio @ "1.0" with ["full"]
import rust::serde @ "1.0" with ["derive", "rc"]
```

When multiple files import the same crate, features are unioned automatically.

### In `incan.toml`

```toml
[rust-dependencies]
tokio = { version = "1.35", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
```

To disable default features:

```toml
[rust-dependencies]
serde = { version = "1.0", default-features = false, features = ["derive"] }
```

## Dev-only dependencies

Use `[rust-dev-dependencies]` for crates needed only during testing:

```toml
[rust-dev-dependencies]
criterion = "0.5"
test_helpers = { path = "../test-helpers" }
```

Dev dependencies are only available in test contexts (files under `tests/`). Importing a dev-only crate from production
code produces a compile-time error.

## Locking dependencies

### Generating the lock file

Run `incan lock` to resolve all dependencies and create `incan.lock`:

```bash
incan lock src/main.incn
```

Or, if your `incan.toml` has `[project.scripts].main` set:

```bash
incan lock
```

`incan.lock` embeds the resolved `Cargo.lock` and a fingerprint of your dependency inputs. **Commit it to version control**
for reproducible builds.

### Automatic lock generation

If `incan.lock` doesn't exist and you run `incan build` or `incan test` without strict flags, the lock file is created
automatically on first build.

### Strict mode for CI

Use `--locked` or `--frozen` to enforce that the lock file exists and is up to date:

```bash
# Requires incan.lock to exist and match current deps
incan build src/main.incn --locked

# Same as --locked, plus Cargo runs in offline/frozen mode
incan build src/main.incn --frozen
```

If the lock file is missing or stale, the command fails with a clear message:

```text
error: incan.lock is out of date; run `incan lock`
```

## Resolution rules

When the compiler resolves a dependency, it follows this precedence:

| Priority | Source                  | Example                                   |
| -------- | ----------------------- | ----------------------------------------- |
| 1 (high) | `incan.toml`            | `[dependencies] tokio = "1.35"`           |
| 2        | Inline annotation       | `import rust::tokio @ "1.35"`             |
| 3        | Known-good default      | `import rust::tokio` (compiler default)   |
| 4 (low)  | Error                   | `import rust::unknown_crate` (no version) |

Key rules:

- If a crate is in `incan.toml`, inline annotations for that crate are forbidden.
- If the same crate is imported inline in multiple files, the version must match exactly; features are unioned automatically.
- Known-good defaults only apply when there is no `incan.toml` entry and no inline annotation.

## Cargo feature flags

You can pass Cargo feature flags through the Incan CLI:

```bash
# Enable specific features
incan build src/main.incn --cargo-features fancy_logging,metrics

# Disable default features
incan build src/main.incn --cargo-no-default-features

# Enable all features
incan build src/main.incn --cargo-all-features
```

These flags affect dependency resolution and are included in the lock file fingerprint.

## Common errors and fixes

### Unknown crate without version

```text
error: unknown Rust crate `my_crate`: no version specified
```

**Fix**: Add `@ "version"` to the import, or add the crate to `incan.toml`.

### Inline annotation conflicts with manifest

```text
error: inline Rust dependency annotation for `tokio` is not allowed because it is configured in incan.toml
```

**Fix**: Remove the `@ "..."` and `with [...]` from the import. Use `incan.toml` to control the version.

### Version conflict across files

```text
error: conflicting inline dependency specifications for `uuid`
```

**Fix**: Make all inline version annotations match, or centralize the dependency in `incan.toml`.

### Dev-only crate in production code

```text
error: Rust crate `criterion` is dev-only and cannot be imported from production code
```

**Fix**: Move the crate to `[dependencies]`, or move the import to a test file.

### Optional dependency not enabled

```text
error: Rust crate `fancy_logging` is optional but not enabled for this build
```

**Fix**: Enable it with `--cargo-features fancy_logging`, or remove the `optional` flag.

### Stale lock file

```text
error: incan.lock is out of date; run `incan lock`
```

**Fix**: Run `incan lock` to regenerate the lock file after changing dependencies.

## See also

- [Project configuration reference](../reference/project_configuration.md) - Full `incan.toml` format
- [Rust interop](../../language/how-to/rust_interop.md) - Inline version/feature syntax
- [CLI reference](../reference/cli_reference.md) - `incan init`, `incan lock`, and flags
- [CI & automation](ci_and_automation.md) - Locked builds in CI

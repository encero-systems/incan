# Managing dependencies

This guide covers how to add, configure, and lock Rust crate dependencies in Incan projects.

For the full manifest format, see: [Project configuration reference](../reference/project_configuration.md). For inline import syntax, see: [Rust interop](../../language/how-to/rust_interop.md).

## Adding a Rust crate (quick start)

The simplest way to use a Rust crate is with an inline version annotation:

```incan
import rust::my_crate @ "1.0"
```

This works in any `.incn` file, no configuration files needed. The compiler adds the dependency to the generated `Cargo.toml` automatically.

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

Once a crate is in `incan.toml`, the manifest is the single source of truth. Inline `@ "version"` annotations for that crate are not allowed — use bare imports instead:

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

Dev dependencies are only available in test contexts (files under `tests/`). Importing a dev-only crate from production code produces a compile-time error.

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

`incan.lock` embeds the resolved `Cargo.lock` and a fingerprint of your dependency inputs. **Commit it to version control** for reproducible builds.

For compiled SDK providers, the fingerprint identifies checked provider contracts, dependency and feature choices, and authored Incan inputs. Native Rust output and host-derived ABI metadata remain covered by each installed provider artifact's exact integrity digest, but do not make an otherwise equivalent macOS and Linux SDK selection semantically different. User-authored path dependencies remain part of the semantic fingerprint.

### Default build/test behavior

If `incan.lock` doesn't exist and you run `incan build` or `incan test` without strict flags, the lock file is created automatically on first build.

If `incan.lock` already exists but is stale, default `incan build` and `incan test` warn and leave the file untouched, but do not use its embedded `Cargo.lock` as dependency authority. Run `incan lock` when you intentionally want to refresh the committed lock file. Strict `--locked` and `--frozen` commands reject the stale lock.

For `incan test`, a generated or changed lock can make the generated Rust harness stale. The runner preheats stale harnesses before executing tests so later test commands can reuse the compiled Cargo state. When lock generation sees Rust dependency inputs or stdlib feature requirements, it also preheats those dependencies with `cargo test --no-run` into the generated test target domain before writing `incan.lock`; unchanged relocks reuse a dependency preheat fingerprint, while cold preheats stream Cargo's own progress instead of leaving the terminal silent.

Generated Cargo output from check, build, run, test, library, lock-preheat, Rust-metadata tooling, and LSP paths is shared by default below `$INCAN_HOME/cache/generated-cargo/v1`, or `~/.incan/cache/generated-cargo/v1` when `INCAN_HOME` is unset. The compiler selects a compatibility domain from the Incan version, selected `rustc` command and verbose host/version output, selected Cargo executable and version, Rust/Cargo target and profile environment selectors, profile, canonical Cargo lock payload, Cargo features, and Cargo arguments that can affect compiled artifacts. Execution-only policy such as offline, locked, frozen, timing, verbosity, and color output does not split otherwise compatible domains. Cargo still fingerprints its own remaining compiler and configuration inputs inside that domain. Matching projects and clean worktrees therefore reuse compiled dependency artifacts without moving generated Incan source, published final binaries, or rust-inspect metadata workspaces out of the project. Cargo `--target-dir` and target/build-directory `--config` passthrough are rejected because they could escape the directory protected and reported by Incan; use `--generated-cargo-target-dir` for an explicit caller-owned target.

Before acquiring a domain, Incan prunes least-recently-used idle domains toward a 20 GiB default soft limit. The domain being acquired and every concurrently active domain remain protected, so active domains can temporarily keep total logical usage above that limit. Each domain is measured when its last Cargo lease ends, then the now-idle set is pruned toward the aggregate limit; if its rebuildable output exceeds the separate 20 GiB default safety limit, Incan discards that domain's Cargo target before it can remain as idle cache state. Change the safety limit with `INCAN_GENERATED_CACHE_MAX_ENTRY_BYTES` or use an explicit target when another system owns the lifecycle. Inspect logical file bytes with `incan cache inspect`, preview cleanup with `incan cache prune --dry-run`, prune toward the limit with `incan cache prune`, or remove an exact idle identity with `incan cache prune --identity <SHA256>`. Active builds retain a shared lease and are skipped by cleanup.

See [Generated-build storage model](../explanation/generated_build_storage.md) for ownership categories, the reproducible audit method, v0.5 measurements, and the Cargo-owned costs that remain.

For `incan build --lib`, the compiler also preheats the generated lock workspace into the same release-profile Cargo target directory used by the generated library build. This matters for packages with stable but expensive Rust dependency graphs: a warmed lock workspace should make the following library build reuse the dependency artifacts instead of compiling the same graph in a different target/profile domain. Use `--generated-cargo-target-dir <PATH>` or `INCAN_GENERATED_CARGO_TARGET_DIR` only when a caller such as CI needs to own the target lifecycle explicitly. Set `INCAN_GENERATED_CACHE_MAX_BYTES` to change the managed total limit, `INCAN_GENERATED_CACHE_MAX_ENTRY_BYTES` to change the managed per-domain safety limit, or `INCAN_GENERATED_CACHE=0` to restore project-local Cargo targets. Cold generated-library preheats print the target/profile domain before invoking Cargo and stream Cargo's progress until the preheat completes. Set `INCAN_LOCK_PREHEAT=0` only when you deliberately want to disable both lock/test dependency preheat and generated-library dependency preheat while debugging.

### Strict mode for CI

Use `--locked` or `--frozen` to enforce that the lock file exists and is up to date. Use `--offline` when Cargo must fail instead of touching the network:

```bash
# Requires incan.lock to exist and match current deps
incan build src/main.incn --locked

# Disallow network access during Cargo subprocesses
incan build src/main.incn --offline

# Same as --offline plus --locked
incan build src/main.incn --frozen
```

Before relying on `--frozen` in a restricted or offline environment, run:

```bash
incan tools doctor
```

`incan tools doctor` is the supported preflight path for local offline-readiness diagnostics. Its report is advisory, not a guarantee: `--frozen` still asks Cargo to use offline/locked policy, so any crate source that is missing from Cargo's local inputs can still make the build fail.

If the lock file is missing or stale, the command fails with a clear message:

```text
error: incan.lock is out of date; run `incan lock`
```

CI can set the same policy with environment variables:

```bash
INCAN_LOCKED=1 incan build src/main.incn
INCAN_FROZEN=1 incan test tests/
```

`INCAN_FROZEN=1` implies both offline and locked policy. Use `--no-offline`, `--no-locked`, or `--no-frozen` to disable matching environment defaults for a single command.

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

For advanced Cargo-only flags, use `--cargo-args` or put Cargo arguments after `--`:

```bash
incan build src/main.incn --cargo-args "--timings"
incan test tests/ -- --timings
```

`INCAN_CARGO_ARGS` is also supported for simple whitespace-separated CI defaults. Quoting is not parsed inside the environment variable; use the CLI form for arguments containing spaces.

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

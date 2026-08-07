# Managing dependencies

This guide covers how to declare and lock Rust crate requirements in Incan projects. In the Oven Alpha envelope,
normal `build`, `run`, and `test` commands consume only dependencies already sealed in a compatible toolchain Loaf;
they do not ask Cargo to resolve a new requirement or silently fall back to Cargo.

For the full manifest format, see: [Project configuration reference](../reference/project_configuration.md). For inline import syntax, see: [Rust interop](../../language/how-to/rust_interop.md).

## Adding a Rust crate (quick start)

The simplest way to use a Rust crate is with an inline version annotation:

```incan
import rust::my_crate @ "1.0"
```

This records a compatibility requirement without needing a project manifest. It succeeds in a normal Oven command
only when the selected Loaf already authorizes that crate, version, and feature closure. Otherwise Oven reports an
unsupported envelope and tells you to install a compatible toolchain; it does not resolve the crate during the
command.

For common crates, the compiler can supply a known requirement when the version is omitted:

```incan
import rust::serde_json as json    # Uses known-good default: serde_json 1.0
import rust::tokio                 # Uses known-good default: tokio 1 with common features
```

These defaults define dependency intent; they are not a promise that every published Alpha Loaf contains every
listed crate. See [Oven Alpha](../explanation/oven_alpha.md#current-alpha-boundary) for the supported envelope.

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

`incan.lock` records normalized semantic dependency, feature, provider, and implementation-facet inputs. **Commit it
to version control** so normal commands can validate that the project still matches the receipt-compatible Loaf
selection. The lock is not permission for a normal command to resolve missing crates with Cargo.

For compiled SDK providers, the fingerprint identifies checked provider contracts, dependency and feature choices, and authored Incan inputs. Native Rust output and host-derived ABI metadata remain covered by each installed provider artifact's exact integrity digest, but do not make an otherwise equivalent macOS and Linux SDK selection semantically different. User-authored path dependencies remain part of the semantic fingerprint.

### Legacy generated-Cargo behavior (pre-Oven Alpha)

The generated-Cargo cache, preheat controls, Cargo policy flags, and target-directory overrides describe the former
0.5 execution backend. They are not controls for ordinary Oven Alpha commands. See
[Generated-build storage model](../explanation/generated_build_storage.md) only when auditing that historical backend
or the explicit compatibility-publisher boundary.

### CI and offline use

Normal Oven Alpha `build`, `run`, and `test` do not launch Cargo or access a registry, so Cargo's `--offline`,
`--locked`, and `--frozen` policies are not normal-command controls. Commit `incan.lock`, install the required
Oven-enabled toolchain before entering the restricted environment, and let receipt/lock validation fail closed if
the project no longer matches the sealed Loaf. Maintainer publication can separately constrain the explicit
`legacy_cargo` baker with Cargo policy; that does not change the consumer contract.

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

## Rust dependency feature boundary

Declare Rust dependency features in the inline import or `[rust-dependencies]` entry before running `incan lock`.
The resulting requirement participates in semantic lock and receipt identity. A normal Oven command accepts it only
when a sealed Loaf authorizes the exact dependency/feature closure.

Cargo feature and argument passthrough options are not accepted by normal Oven Alpha `build`, `run`, or `test`.
They remain only on explicit compatibility surfaces such as lock preparation and the hidden maintainer baker. If a
required closure is absent, change the declaration and install or bake a matching Loaf instead of trying to mutate
Cargo resolution during the consumer command.

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

**Fix**: Enable it through the owning manifest or Incan package feature, regenerate `incan.lock`, and use a toolchain
whose Loaf authorizes the resulting closure. Otherwise remove the optional dependency.

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

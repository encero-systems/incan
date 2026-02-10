# RFC 013: Rust Crate Dependencies

**Status:** Implemented  
**Created:** 2025-12-16  
**Author(s):** Danny Meijer (@danny-meijer)  
**Supersedes:** Parts of RFC 005 (Cargo integration section)  
**Related:** RFC 015 (project lifecycle + `incan.toml`), RFC 020 (Cargo offline/locked policy)

## Summary

Define a comprehensive system for specifying Rust crate dependencies in Incan, including inline version annotations,
project configuration (`incan.toml`), and lock files for reproducibility.

## Motivation

Incan compiles to Rust, meaning access to the Rust ecosystem is a core value proposition.
The current implementation (v0.1) has limitations:

1. **Known-good crates work** - common crates like `serde`, `tokio` have curated defaults
2. **Unknown crates error** - no user-facing way to specify version/features
3. **Workaround is manual** - edit generated `Cargo.toml` (clunky, not reproducible)

This RFC introduces a proper dependency management system that is:

- **Easy by default** - common crates "just work"
- **Flexible when needed** - any crate can be used with explicit config
- **Reproducible** - builds are deterministic via lock files
- **Pythonic** - `incan.toml` feels like `pyproject.toml`

## Goals

- Allow any Rust crate with explicit version specification
- Maintain known-good defaults as convenient fallbacks
- Support features, git sources, and path dependencies
- Provide a familiar project configuration format
- Enable reproducible builds via lock file

## Non-Goals (this RFC)

- Automatic version resolution/upgrade (future: `incan update`)
- Private registry support
- Workspace/multi-project configuration
- Native FFI bindings beyond Rust

---

## 1. Inline Version Annotations

### 1.1 Basic Version

```incan
# Specify a version requirement
import rust::my_crate @ "1.0"
from rust::obscure_lib @ "0.5" import Widget

# Cargo SemVer requirement strings (same semantics as Cargo.toml `version = "..."`)
import rust::some_crate @ "^1.2"   # >=1.2.0, <2.0.0
import rust::other_crate @ "~1.2"  # >=1.2.0, <1.3.0
import rust::range_crate @ ">=1.30, <2.0"
import rust::exact_crate @ "=1.2.3" # exactly 1.2.3
```

#### 1.1.1 Version requirement syntax (normative)

All version requirement strings in this RFC use **Cargo’s SemVer requirement syntax** (the same syntax accepted by Cargo
in `Cargo.toml`).

- Shorthand `"1.2.3"` is equivalent to caret `"^1.2.3"`.
- Comma-separated constraints (e.g. `">=1.30, <2.0"`) are allowed.

Not supported (out of scope):

- Python/PEP 440 specifiers (e.g. `~=1.2`, `==1.2.*`, `!=1.3`).

### 1.2 Version with Features

```incan
# Enable features
import rust::tokio @ "1.0" with ["full"]
import rust::serde @ "1.0" with ["derive", "rc"]

# Import from crate with features
from rust::sqlx @ "0.7" with ["runtime-tokio", "postgres"] import Pool
```

### 1.3 Grammar Extension

```ebnf
(* Extended import syntax for rust:: crates *)
rust_import     = "import" "rust" "::" crate_path [ version_spec ] [ "as" IDENT ]
                | "from" "rust" "::" crate_path [ version_spec ] "import" import_list ;

crate_path      = IDENT { "::" IDENT } ;
version_spec    = "@" version_string [ "with" feature_list ] ;
version_string  = STRING ;  (* Cargo SemVer requirement string, e.g. "1.0", "^1.2", "~0.5", ">=1.30, <2.0", "=1.2.3" *)
feature_list    = "[" STRING { "," STRING } "]" ;
import_list     = IDENT { "," IDENT } ;
```

Normative note (interop alignment):

- A `rust::...` import may include a module path (e.g. `rust::chrono::naive::date`), but dependency resolution is always
  keyed by the **crate segment** (the first identifier after `rust::`), per RFC 005’s crate/path decomposition rules.
- Therefore, `@ "..."` / `with [...]` always apply to that crate segment, not to the module path.

---

## 2. Project Configuration (`incan.toml`)

We propose to add a central configuration file for Incan projects called `incan.toml`. The `incan.toml` format is heavily
inspired by Python's `pyproject.toml` - familiar, readable, and declarative.

Schema note (normative):

- In `incan.toml`, Cargo/Rust crate dependencies use Cargo-style tables:
    - **`[dependencies]`** for normal dependencies
    - **`[dev-dependencies]`** for test-only dependencies (bench tooling is a future extension)
- Compatibility/aliasing (for this RFC):
    - `[rust.dependencies]` / `[rust.dev-dependencies]` are accepted as aliases for `[dependencies]` / `[dev-dependencies]`.
    - It is a configuration error to specify both the canonical and alias tables for the same kind (e.g. both
      `[dependencies]` and `[rust.dependencies]`).
- RFC 015 defines the broader project lifecycle and treats `incan.toml` as the project metadata root; it must not define
  a separate, conflicting table for dependency policy.

Clarity note (future-proofing):

- In this RFC, `[dependencies]` / `[dev-dependencies]` refer specifically to **Cargo/Rust crates** used by `rust::...`
  imports.
- Incan does not yet define a separate “Incan package registry” concept. If a future packaging story is added, it must
  not overload these tables in a way that breaks Rust dependency determinism.

Recommended practice (strongly suggested):

- Prefer centralized Rust dependency policy in `incan.toml` for real projects.
- Reserve inline annotations (`@ "..."` / `with [...]`) for small scripts, prototypes, and one-off examples.

### 2.1 Minimal Example

```toml
[project]
name = "my_app"
version = "0.1.0"
```

### 2.2 Full Example

```toml
[project]
name = "my_app"
version = "0.1.0"
description = "An example Incan application"
authors = ["Alice <alice@example.com>"]
license = "MIT"
readme = "README.md"

# Minimum Incan version required
requires-incan = ">=0.2.0"

# Entry point for `incan run`
[project.scripts]
main = "src/main.incn"

# Dependencies (Cargo crates used by `rust::...` imports)
[dependencies]
# Simple version string
reqwest = "0.12"
rand = "0.8"

# Version with features
sqlx = { version = "0.7", features = ["runtime-tokio", "postgres", "mysql"] }
tokio = { version = "1.35", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }

# Git dependency
my_internal_crate = { git = "https://github.com/company/crate", tag = "v1.0.0" }

# Path dependency (for local development)
local_lib = { path = "../my-local-lib" }

# Optional dependencies (enabled via features)
[dependencies.optional]
fancy_logging = { version = "0.3", optional = true }

# Development dependencies (not included in release builds)
[dev-dependencies]
criterion = "0.5"
proptest = "1.0"

# Build configuration
[build]
# Rust edition for generated code (advanced)
# Allowed values (for this RFC): "2021", "2024" (default: "2021")
rust-edition = "2021"

# Optimization level: "debug", "release", "release-lto"
profile = "release"

# Target triple (optional, defaults to host)
# target = "x86_64-unknown-linux-gnu"

# Feature flags for the Incan project itself
[project.features]
default = ["json"]
json = []  # Enables JSON serialization support
full = ["json", "fancy_logging"]
```

Note:

- `[project.features]` is part of the general Incan project model (RFC 015). It is included here for context only.
- This RFC does **not** specify an automatic mapping between `[project.features]` and Rust crate features/optional
  dependencies. Any such mapping should be specified by RFC 015 (or a dedicated follow-up), so that Cargo feature
  semantics are not implicitly re-invented in multiple places.
- `[build]` is also part of the general Incan project model (RFC 015). This RFC references it only for completeness in
  the “full example”; dependency resolution and `incan.lock` do not depend on build settings like `rust-edition`.

### 2.3 Section Reference

| Section                   | Description                                     |
| ------------------------- | ----------------------------------------------- |
| `[project]`               | Project metadata (name, version, authors, etc.) |
| `[project.scripts]`       | Entry points for CLI commands                   |
| `[project.features]`      | Optional feature flags                          |
| `[dependencies]`          | Cargo/Rust crate dependencies                   |
| `[dev-dependencies]`      | Development-only crate dependencies             |
| `[build]`                 | Build configuration options                     |

### 2.4 Dependency Specification Formats

```toml
[dependencies]
# String shorthand - just version
crate_a = "1.0"

# Table form - version + features
crate_b = { version = "1.0", features = ["foo", "bar"] }

# Table form - git source
crate_c = { git = "https://github.com/...", branch = "main" }
crate_d = { git = "https://github.com/...", tag = "v1.0.0" }
crate_e = { git = "https://github.com/...", rev = "abc1234" }

# Table form - path source
crate_f = { path = "../local-crate" }

# Optional dependency
crate_g = { version = "1.0", optional = true }

# Default features disabled
crate_h = { version = "1.0", default-features = false, features = ["only-this"] }
```

### 2.4.1 Dependency renames (`package = "..."`) (normative)

Cargo supports depending on a package whose crates.io/package name differs from the identifier you want to use in code via
dependency renaming (`package = "..."`).

In Incan, `rust::...` imports refer to the **dependency key** (the Rust identifier form), which is also the name used in
generated Rust code.

Supported (for this RFC):

- In `incan.toml` dependency tables, users may specify a dependency rename using:

```toml
[dependencies]
# `rust::serde_json` in code, but crates.io package is `serde-json`
serde_json = { package = "serde-json", version = "1.0" }
```

- The `rust::` import must use the dependency key (`rust::serde_json` in the example above).
- If `package` is specified, it must be a non-empty string.
- It is a configuration error to specify two dependencies that would resolve to the same `(source, package)` pair with
  different `crate_name` keys.

Note:

- For the common case where the crates.io package name differs only by `-` vs `_`, Cargo/crates.io name normalization is
  typically sufficient. Using `package = "..."` is still allowed (and may improve clarity), and is required for
  non-trivial mismatches beyond `-`/`_` normalization.

Out of scope (for this RFC):

- Automatically inferring `package = "..."` from imports.
- Supporting non-trivial “one package provides multiple crate names” patterns beyond Cargo’s standard rename mechanism.

### 2.4.2 Optional dependencies (normative)

Incan supports Cargo-style optional dependencies via the standard Cargo fields:

- `optional = true` on a dependency entry

Enablement model (for this RFC):

- This RFC does **not** define an Incan-native feature system that automatically toggles Cargo optional dependencies.
- Optional dependency enablement is controlled by **Cargo features** (Cargo semantics), surfaced to users via RFC 020’s
  `--cargo-args` escape hatch (e.g. `--cargo-args "--features" "fancy_logging"`), and/or a future RFC 015 mapping.

`[dependencies.optional]` table:

- `[dependencies.optional]` is syntactic sugar for grouping optional dependency entries.
- It does not introduce any new enablement behavior; it only implies `optional = true` for entries in that table.

Cargo project generation (normative):

- Optional dependencies in `incan.toml` must be emitted as Cargo optional dependencies in the generated `Cargo.toml`
  (`optional = true` on the dependency entry).
- The generated Cargo package must expose a Cargo feature with the **same name as the dependency key** that enables the
  optional dependency (using Cargo’s namespaced dependency feature form):

```toml
[dependencies]
fancy_logging = { version = "0.3", optional = true }

[features]
fancy_logging = ["dep:fancy_logging"]
```

This avoids relying on “implicit optional-dependency features,” which are edition/toolchain-sensitive and can create
confusing behavior.

Locking note (normative):

- Because Cargo feature selection can change the resolved dependency graph, strict builds (`--locked` / `--frozen`) must
  ensure that the **feature selection used for the build/test** matches the one that was used to generate `incan.lock`.

Diagnostics expectation (normative):

- If user code imports an optional dependency but the required Cargo feature(s) are not enabled for the build/test, the
  toolchain must produce a targeted diagnostic explaining:
    - the crate is optional,
    - which Cargo feature name is expected (typically the dependency key),
    - and how to enable it (e.g. via RFC 020’s `--cargo-args "--features" "<name>"`, or via a future RFC 015 mapping).

### 2.5 Development dependencies (`[dev-dependencies]`) (normative)

Incan supports Rust **development dependencies** for test tooling that must not become part of release builds.

#### 2.5.1 What they are

- `[dev-dependencies]` has the **same specification formats** as `[dependencies]` (version string shorthand or table form
  with `version`, `features`, `git`, `path`, `default-features`, etc.).
- A crate listed in `[dev-dependencies]` is **dev-only**: it is available only in test contexts and must not be
  used by production code.

#### 2.5.2 Where dev-dependencies may be used (import gating)

Crates that are only present in `[dev-dependencies]` may be imported via `rust::...` **only** from test contexts as
defined by the testing RFCs:

- test files under `tests/` (`test_*.incn` / `*_test.incn`) (RFC 019), and
- inline `module tests:` blocks inside production source files (RFC 018/019).

If production code imports a dev-only crate, it is a **compile-time error** with a targeted diagnostic:

- explain the crate is dev-only (`[dev-dependencies]`)
- suggest moving it to `[dependencies]` if it is required at runtime.

#### 2.5.3 How dev-dependencies affect Cargo project generation

Because Incan generates Cargo projects to compile and test:

- `incan build` / `incan run`:
    - generated `Cargo.toml` must include Cargo `[dependencies]` entries from `incan.toml` `[dependencies]`
    - it must **not** include crates that are only in `[dev-dependencies]`
- `incan test`:
    - the generated test harness project must include:
        - Cargo `[dependencies]` entries from `incan.toml` `[dependencies]` (tests exercise production code), and
        - Cargo `[dev-dependencies]` entries from `incan.toml` `[dev-dependencies]`

#### 2.5.4 Locking (relationship to `incan.lock`)

`incan.lock` is the project-root, committed lockfile and the source of truth for “locked” builds (see §3.0–§3.3).

Normative requirement:

- `incan.lock` must cover the resolved dependency graph for **both**:
    - `[dependencies]`, and
    - `[dev-dependencies]` (so `incan test --locked` is meaningful and reproducible).

#### 2.5.5 Overlap rules (same crate in both tables)

If the same crate name appears in both `[dependencies]` and `[dev-dependencies]`:

- the crate is treated as a normal dependency (it is available in production code), and
- the specs must be compatible:
    - version/source/default-features must match (for this RFC), otherwise it is an error
    - features are unioned (Cargo-like behavior).

---

## 3. Lock File (`incan.lock`)

For reproducible builds, Incan uses a project-root lockfile named `incan.lock`.

Design goal:

- Keep the user-facing artifact **Incan-first** (`incan.lock`), while still leveraging Cargo’s proven locking semantics.
- Avoid unstable Cargo features like `--lockfile-path` (nightly-only as of 2026).
- Enable strict modes with clear “what to do next” errors, aligned with RFC 020’s Cargo policy flags.

### 3.0 Trust model and location (normative)

- `incan.lock` lives at the **project root** and is intended to be **committed** to version control.
- `incan.lock` is the **source of truth** for what “locked” means in Incan.
- In locked modes, tooling must never silently change dependency resolution; any drift must be a hard error with a clear
  instruction to run `incan lock`.

#### 3.0.1 Project mode vs single-file mode (normative)

This RFC defines `incan.lock` as a **project artifact**.

Definitions:

- **Project mode**: a project root exists (a directory containing `incan.toml`, discovered per RFC 015).
- **Single-file mode**: no project root exists (`incan.toml` is not found), and the toolchain is compiling a single file.

Rules:

- In **project mode**, strict flags (`--locked` / `--frozen`, RFC 020) are enforced against `incan.lock`:
    - `incan.lock` must exist and be up to date (fingerprint matches), otherwise error with "run `incan lock`".
    - Cargo is invoked with the corresponding Cargo policy flags (RFC 020), and `Cargo.lock` is materialized from
      `incan.lock` into the generated project directory.
- In **single-file mode**, there is no `incan.lock`:
    - Rust dependencies may only be specified via inline annotations (`@ "..."` / `with [...]`) and/or known-good defaults.
    - Strict flags (`--locked` / `--frozen`) are interpreted at the Cargo layer only (RFC 020) using the generated
      project’s `Cargo.lock` as the lock artifact.

### 3.1 Format

`incan.lock` is a small container file that embeds a Cargo lockfile payload.

Rationale:

- Cargo expects a file named `Cargo.lock` adjacent to the `Cargo.toml` it is building.
- Stable Cargo does not support redirecting the lockfile path.
- Therefore, Incan materializes the embedded payload to `Cargo.lock` in the generated build/test project directory.

Format (normative; TOML container):

```toml
# Auto-generated by Incan - do not edit manually
# Regenerate with: incan lock

[incan]
format = 1
incan-version = "0.2.0"
generated = "2026-01-21T12:00:00Z"

# Cargo feature selection for the generated Rust package at lock time.
# This matters because Cargo feature selection can change the resolved dependency graph.
cargo-features = []
cargo-no-default-features = false
cargo-all-features = false

# Fingerprint of the effective dependency inputs (incan.toml dependency tables + inline annotations + known-good defaults)
# This intentionally excludes non-dependency settings (e.g. `[build].rust-edition`).
# Used for strict-mode validation (`--locked` / `--frozen`) (uv-style: error if lock is out of date).
deps-fingerprint = "sha256:..."

[cargo]
# Verbatim Cargo.lock payload.
# Incan writes this string out as `Cargo.lock` next to the generated `Cargo.toml` when invoking Cargo.
lock = """
# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 4

[[package]]
name = "serde"
version = "1.0.195"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "..."

# ... more Cargo.lock content ...
"""
```

Notes:

- The embedded Cargo lock is stored as a verbatim payload; Incan does not define a second dependency graph format here.
- The `deps-fingerprint` is an Incan-level affordance so `--locked` can fail fast and explain “run `incan lock`.”

### 3.1.1 Canonicalization and `deps-fingerprint` (normative)

The canonicalization rules for the embedded Cargo lock payload, and the full `deps-fingerprint` algorithm, are specified
in **Appendix A**.

### 3.2 CLI Commands

| Command                | Description                                       |
| ---------------------- | ------------------------------------------------- |
| `incan build`          | Build; uses lock file if present                  |
| `incan lock`           | Regenerate lock file from current dependencies    |

Note (normative):

- `incan lock` is a project command and requires a project root (`incan.toml`) per RFC 015.

Default behavior (normative; uv-inspired):

- If `incan.lock` is missing and no strict policy flag is set, `incan build` / `incan test` may resolve dependencies and
  create `incan.lock` automatically (first-run convenience).
- If a strict policy flag is set (`--locked` or `--frozen` per RFC 020), `incan.lock` must already exist; otherwise the
  command must fail with a targeted diagnostic instructing the user to run `incan lock`.

> Note: Cargo-level offline/locked enforcement flags (`--offline/--locked/--frozen`) are specified by RFC 020. This RFC
> defines the Incan-level dependency and lockfile model; Cargo policy is a separate concern.

#### 3.2.1 Strict-mode flags: combined meaning (normative)

These flags have a **combined** effect:

- they constrain Incan’s own dependency/lock behavior (`incan.lock`), and
- they are forwarded to the underlying Cargo invocations (RFC 020).

Rules:

- `incan build/test --locked`:
    - **Incan lock layer**: `incan.lock` must exist and be **up to date** (its `deps-fingerprint` matches the current
      effective dependency inputs). If not, fail with “run `incan lock`”.
    - **Cargo policy layer**: Incan must invoke Cargo with `--locked`.
- `incan build/test --frozen`:
    - **Incan lock layer**: same as `--locked` (lock must exist and be up to date).
    - **Cargo policy layer**: Incan must invoke Cargo with `--frozen` (equivalent to Cargo `--offline --locked`, per RFC
      020).

Rationale:

- `--frozen` is the **strictest** mode and should not allow stale locks.
- The difference between `--locked` and `--frozen` is the **offline** constraint at the Cargo layer, not whether lock
  freshness is checked.

#### 3.2.2 `incan lock` and restricted environments (normative direction)

`incan lock` is the mechanism for producing/refreshing `incan.lock`.

Normative requirements:

- Strict flags `--locked/--frozen` are not meaningful for `incan lock` and should be rejected with a clear diagnostic
  (including when set via CI environment variables).
- `incan lock` must accept the same Cargo policy sources as other commands (RFC 020 precedence rules):
    - CLI flags: `--offline` and `--cargo-args ...`
    - Environment variables: `INCAN_OFFLINE=1` and `INCAN_CARGO_ARGS="..."`
  
- `incan lock` must forward the relevant policy to the underlying Cargo operations it uses for resolution (e.g. it must
  honor `--offline` when set).
- In offline mode, lock generation must fail if the required dependency sources are not already available locally (Cargo
  cache, mirror, or vendor directory). This is expected behavior and should surface Cargo’s error with a short prefix
  clarifying that offline policy caused the failure.

Recommended workflows (informative; enterprise-friendly):

- **Cache priming**: run `incan lock` once without offline policy in a controlled environment to populate registries/git
  caches, then use `--frozen` in CI.
- **Mirrors**: use Cargo registry mirrors / sparse index configuration via `.cargo/config.toml` (generated-project
  compatible) to avoid public network dependencies.
- **Vendoring**: prefer a `cargo vendor`-style workflow for fully air-gapped builds (future extension; RFC 020).

### 3.3 Relationship to `Cargo.lock` (normative)

Cargo uses `Cargo.lock` as its lockfile.

Rules:

- `incan.lock` is the project’s committed lockfile and contains an embedded `Cargo.lock` payload.
- `Cargo.lock` files written into generated Rust output directories are **derived artifacts** produced by extracting the
  embedded payload from `incan.lock`.
- In locked modes, Incan must ensure the embedded lock matches the current dependency inputs:
    - `--locked`: lock must exist and be **up to date** (fingerprint matches); otherwise fail and instruct `incan lock`.
    - `--frozen`: same as `--locked` for freshness; additionally enforce Cargo frozen/offline policy (RFC 020).
- Cargo policy flags still apply to the underlying Cargo invocation (RFC 020), but the authoritative lock artifact is
  `incan.lock` at the project root.

### 3.4 Design inspiration (informative): Hatch vs uv

For design inspiration and background context (Hatch vs uv), see **Appendix B**.

---

## 4. Resolution Rules

When resolving a Rust crate dependency, the following precedence applies (highest to lowest):

```text
1. incan.toml [dependencies] / [dev-dependencies] → explicit project config wins
2. Inline version annotation       → import rust::foo @ "1.0"
3. Known-good defaults             → curated list in compiler
4. Error                           → unknown crate without version
```

Note (normative):

- `[dependencies]` applies to all commands (`build`/`run`/`test`).
- `[dev-dependencies]` is only considered for test contexts (see §2.5).

### 4.1 Known-Good Defaults

The compiler maintains a curated list of common crates with tested version/feature combinations.
These serve as **convenient defaults**, not restrictions:

```rust
// In compiler (simplified)
static KNOWN_GOOD: &[(&str, &str)] = &[
    ("serde", r#"{ version = "1.0", features = ["derive"] }"#),
    ("tokio", r#"{ version = "1", features = ["rt-multi-thread", "macros"] }"#),
    ("reqwest", r#"{ version = "0.11", features = ["json"] }"#),
    // ... etc
];
```

Users can **override** these defaults via `incan.toml` or inline annotations.

Normative governance:

- The known-good list is versioned with the Incan toolchain (compiler/runtime) and is an onboarding convenience only.
- Known-good defaults apply **only** when a crate has **no explicit spec** in `incan.toml` and no inline annotation.
- Known-good defaults must never “win” over explicit user configuration.

User-facing contract (strongly recommended):

- The toolchain must publish a documented list of “known-good crates” (and their default version/features) per release
  (e.g. in release notes and/or a dedicated docs page).
- Changes to known-good defaults are allowed, but they must be treated as compatibility-relevant:
    - security updates may require updating defaults,
    - and toolchain upgrades may require re-locking for projects that rely on defaults (because defaults participate in
      `deps-fingerprint`).
- If a project wants stability across toolchain upgrades, it should pin versions/features explicitly in `incan.toml`.

### 4.2 Conflict Resolution

```incan
# This is an error - conflicting versions
import rust::tokio @ "1.0"   # Inline says 1.0
# incan.toml says tokio = "2.0"
```

Error message:

```bash
error: conflicting versions for `tokio`

  --> src/main.incn:3
    import rust::tokio @ "1.0"

  --> incan.toml:12
    tokio = "2.0"

Remove the inline version to use incan.toml, or update incan.toml to match.
```

### 4.3 Merging rules (normative)

The resolver collects all crate requirements across a project before invoking Cargo.

Definitions:

- A **crate spec** is the resolved dependency request for a single Rust crate (version requirement, source, features,
  and `default-features` policy).

Rules:

- **Single source of truth when `incan.toml` exists**:
    - If a crate is specified in `incan.toml` (either `[dependencies]` or `[dev-dependencies]`, including their `rust.*`
      aliases), inline annotations for that crate are a **compile-time error**.
    - Rationale: avoid scattering dependency policy throughout code; keep audits and reviews centralized.
- **Multiple inline sites (no `incan.toml` entry)**:
    - Version requirement strings must match exactly across all inline sites for a given crate (for this RFC).
    - Source (registry vs git vs path) must match; mismatches are an error.
    - Features are **unioned** across all sites.
    - `default-features` must be consistent across all sites; mismatches are an error.

Future extensions may loosen the “inline is error when `incan.toml` exists” rule (e.g. allow inline *feature adds* only),
but within the scope of this RFC, it should remain strict.

### 4.4 Source policy (security + reproducibility) (normative direction)

Git and path dependencies are supported, but reproducible builds require additional constraints:

- In locked/CI mode, git dependencies must resolve to an **exact commit** (e.g. `rev = "..."` or an immutable tag that is
  recorded as a commit in `incan.lock`). Floating `branch = "..."` is not reproducible and should be rejected in strict
  mode.
- Path dependencies are inherently machine-local unless vendored; locked/CI mode may reject them unless explicitly
  allowlisted by configuration (future RFC 020 / RFC 015 integration).

---

## 5. Error Messages

### 5.1 Unknown Crate Without Version

```bash
error: unknown Rust crate `my_obscure_lib`

  --> src/main.incn:5
    import rust::my_obscure_lib

This crate isn't in the known-good list. Specify a version:

    import rust::my_obscure_lib @ "1.0"

Or add it to incan.toml:

    [dependencies]
    my_obscure_lib = "1.0"

Tip: Check https://crates.io/crates/my_obscure_lib for available versions.
```

### 5.2 Feature Not Found

```bash
error: feature `nonexistent` not found in crate `tokio`

  --> src/main.incn:3
    import rust::tokio @ "1.0" with ["nonexistent"]

Available features: full, rt, rt-multi-thread, macros, time, sync, net, ...
```

### 5.3 Version Not Found

```bash
error: version `99.0` of `serde` does not exist

  --> src/main.incn:3
    import rust::serde @ "99.0"

Latest version: 1.0.195
Tip: Check https://crates.io/crates/serde/versions for available versions.
```

### 5.4 Lock Out of Date (`--locked` / `--frozen`)

```bash
error: incan.lock is out of date

  expected deps-fingerprint: sha256:aaaaaaaa...
    actual deps-fingerprint: sha256:bbbbbbbb...

This usually means your dependency inputs changed since the lock was generated:

- incan.toml dependency entries changed, and/or
- inline rust::... annotations changed, and/or
- toolchain known-good defaults changed (if you rely on defaults).
- Cargo feature selection changed (e.g. `--cargo-args "--features" ...`).

Fix:

    incan lock

Tip: If you want toolchain-upgrade stability, pin crate versions/features explicitly in incan.toml.
```

### 5.5 Optional Dependency Not Enabled

```bash
error: Rust crate `fancy_logging` is optional but not enabled for this build

  --> src/main.incn:3
    import rust::fancy_logging

This crate is declared as optional in incan.toml:

    [dependencies.optional]
    fancy_logging = { version = "0.3" }

Enable the Cargo feature for this dependency (usually the dependency key), for example:

    incan build --cargo-args "--features" "fancy_logging"

Alternatively, configure Cargo feature selection in your project configuration (once supported), so builds don’t rely on command-line flags.

Note: changing enabled features can change the resolved dependency graph; you may need to re-run `incan lock` before using
`--locked` / `--frozen`.
```

### 5.6 Inline Annotation Forbidden (crate is configured in `incan.toml`)

```bash
error: inline Rust dependency annotation for `tokio` is not allowed because it is configured in incan.toml

  --> src/main.incn:3
    import rust::tokio @ "1.0"

  --> incan.toml:12
    [dependencies]
    tokio = { version = "1.35", features = ["full"] }

This project uses incan.toml as the single source of truth for Rust dependencies.
Remove the inline annotation, or update incan.toml.
```

### 5.7 Conflicting Inline Sites (no `incan.toml` entry)

```bash
error: conflicting inline dependency specifications for `uuid`

  --> src/a.incn:2
    import rust::uuid @ "1.0"

  --> src/b.incn:7
    import rust::uuid @ "1.1"

Inline specs must match exactly within a project (for this RFC).
Fix: centralize `uuid` in incan.toml under [dependencies], or make the inline specs identical.
```

### 5.8 Dev-Dependency Imported from Production Code

```bash
error: Rust crate `criterion` is dev-only and cannot be imported from production code

  --> src/main.incn:5
    import rust::criterion

  --> incan.toml:30
    [dev-dependencies]
    criterion = "0.5"

Fix: move `criterion` to [dependencies], or import it only from tests (tests/ or module tests:).
```

### 5.9 Git Branch Not Allowed in Strict Mode

```bash
error: git dependency `my_internal_crate` uses `branch = "main"`, which is not reproducible in strict mode

  --> incan.toml:18
    my_internal_crate = { git = "https://github.com/company/crate", branch = "main" }

Fix:
  - pin to an exact commit: rev = "<sha>"
  - or use an immutable tag and ensure the resolved commit is recorded in incan.lock
```

### 5.10 Dependency Rename Collision

```bash
error: dependency keys collide: both resolve to the same package `serde-json` from crates.io

  --> incan.toml:12
    serde_json = { package = "serde-json", version = "1.0" }

  --> incan.toml:13
    json = { package = "serde-json", version = "1.0" }

Fix: keep only one key for this package/source, or use a different package/source.
```

---

## 6. Implementation Phases

### Phase 1: Inline Versions (initial scope)

- Add `@ "version"` syntax to parser (implemented in `crates/incan_syntax`)
- Pass version to `ProjectGenerator`
- Default behavior for unknown crates without an explicit version is a hard error (no wildcard fallback).

Optional escape hatch (non-default; not required by this RFC):

- If an “unpinned dependencies” mode exists for local prototyping, it must be **explicitly enabled** (e.g. via a CLI
  flag) and must produce a visibly non-reproducible build warning suitable for CI policy enforcement.

### Phase 2: Features Support

- Add `with ["feature"]` syntax (implemented in `crates/incan_syntax`)
- Update codegen to emit features in generated Cargo metadata (Cargo.toml)

### Phase 3: Project Configuration

- Parse `incan.toml` if present
- Merge with inline annotations per resolution rules
- Add `incan init` to the CLI to be able to create starter `incan.toml`

### Phase 4: Lock File

- Generate `incan.lock` on first build
- Use locked versions on subsequent builds
- Add `incan lock` command to CLI

### Phase 5: Advanced Sources

- Git dependencies (`git = "..."`)
- Path dependencies (`path = "..."`)
- Optional dependencies

---

## 7. Examples

### 7.1 Simple Usage (Phase 1+)

```incan
# Known-good crate - just works
import rust::serde

# Unknown crate - must specify version
import rust::obscure_parser @ "2.1"

# Known-good with different version
import rust::tokio @ "1.35"  # Override default

def main() -> None:
    print("Hello, Rust ecosystem!")
```

### 7.2 Full Project (Phase 3+)

**incan.toml:**

```toml
[project]
name = "web_service"
version = "0.1.0"

[dependencies]
axum = "0.7"
tokio = { version = "1", features = ["full"] }
sqlx = { version = "0.7", features = ["runtime-tokio", "postgres"] }
tracing = "0.1"
```

**src/main.incn:**

```incan
# No version needed - defined in incan.toml
from rust::axum import Router, Json
from rust::sqlx import PgPool
import rust::tracing as log

async def main() -> None:
    log.info("Starting server...")
    app = Router.new()
    # ...
```

### 7.3 Mixed Inline and Config

**incan.toml:**

```toml
[project]
name = "mixed_example"
version = "0.1.0"

[dependencies]
serde = { version = "1.0", features = ["derive"] }
```

**src/main.incn:**

```incan
# From incan.toml
import rust::serde

# Inline - not in incan.toml
import rust::uuid @ "1.0" with ["v4"]

# Override known-good default for one-off use
from rust::chrono @ "0.4" with ["serde", "clock"] import DateTime, Utc
```

---

## 8. Comparison with pyproject.toml

| pyproject.toml                    | incan.toml                     | Notes                             |
| --------------------------------- | ------------------------------ | --------------------------------- |
| `[project]`                       | `[project]`                    | Same structure                    |
| `dependencies = [...]`            | `[dependencies]`               | Cargo-style dependency tables     |
| `[project.optional-dependencies]` | `[dependencies.optional]`      | Similar (optional groups)         |
| `[tool.pytest]`                   | N/A                            | Tool-specific config not in scope |
| `requires-python`                 | `requires-incan`               | Minimum version                   |

---

## 9. Open Questions

1. **Workspace support**: Should `incan.toml` support workspaces with multiple packages?
2. **Private registries**: How to authenticate with private Cargo registries?
3. **Auto-update tooling**: Dependency update behavior (an `incan update` command) is intentionally out of scope for this
   RFC and should be specified in a dedicated future RFC (likely alongside project lifecycle tooling).

---

## 10. Checklist

### Implementing Phase 1: Inline Versions

- [x] Parser: `@ "version"` syntax on `rust::...` imports
- [x] Validation: version requirement strings must use **Cargo SemVer requirement syntax** (not PEP 440)
    - Early validation via `semver` crate in `dependency_resolver::validate_cargo_version_req()`
- [x] Codegen/resolution: pass the crate spec (crate name + version requirement) to project generation
- [x] Error: unknown crate without version (hard error)
- [x] Docs: update rust_interop.md

### Implementing Phase 2: Features

- [x] Parser: `with ["features"]` syntax
- [x] Resolution: features are unioned across sites per merging rules
- [x] Codegen: emit features in generated Cargo metadata
- [x] Error: unknown feature — **deferred to Cargo**: feature validation requires querying crate registry
  metadata, which Incan does not currently do. Invalid features are caught by Cargo during the build step.

### Implementing Phase 3: Project Configuration

- [x] Parser: `incan.toml` dependency tables (canonical + aliases):
    - [x] `[dependencies]`
    - [x] `[dev-dependencies]`
    - [x] Optional group: `[dependencies.optional]`
    - [x] Alias support: `[rust.dependencies]` and `[rust.dev-dependencies]`
    - [x] Error if both canonical and alias table are provided for the same kind
- [x] CLI: `incan init` command
- [x] Resolution: apply precedence rules (incan.toml > inline > known-good > error)
- [x] Known-good defaults: apply only when there is no `incan.toml` spec (canonical or alias) and no inline annotation
- [x] Rule: if a crate is specified in incan.toml (canonical or alias tables), inline annotations for that crate are a
  compile-time error
- [x] Dev-dependencies gating: crates that are only in `[dev-dependencies]` are only allowed in test contexts (RFC 018/019)
  and error in production code
- [x] Error: version/source/default-features conflicts across sites (per §4.2/§4.3)
- [x] Multiple inline sites (no incan.toml entry): enforce this RFC’s merge rules:
    - [x] version requirement strings must match exactly across inline sites
    - [x] source must match across inline sites (inline imports are always registry)
    - [x] `default-features` must match across inline sites (inline imports always use default)
    - [x] features are unioned across inline sites
- [x] Diagnostics: conflicting specs and resolution failures produce actionable errors that point to all relevant locations
  (e.g. inline import site(s) + `incan.toml`), and suggest the concrete fix

### Implementing Phase 4: Lock File

- [x] CLI: `incan lock` command (produce/refresh `incan.lock`)
- [x] `incan.lock` format (container + embedded Cargo.lock payload):
    - [x] `[incan]` metadata (`format`, `incan-version`, `generated`, `deps-fingerprint`, cargo feature selection)
    - [x] `[cargo].lock` verbatim embedded `Cargo.lock` payload
- [x] `deps-fingerprint` computation fingerprints dependency inputs and Cargo feature selection, and excludes non-dependency
  settings like `[build].rust-edition`
- [x] Default behavior: if `incan.lock` is missing and no strict policy flag is set, builds/tests may generate it
  (first-run convenience)
- [x] Strict behavior (uv-style; see RFC 020 for flags):
    - [x] `--locked`: `incan.lock` must exist and be up-to-date (fingerprint matches) or fail with “run incan lock”
    - [x] `--frozen`: `incan.lock` must exist and be up-to-date (fingerprint matches); use it and also enforce Cargo
      `--frozen` policy (offline + locked)
- [x] Diagnostics: missing/out-of-date lock failures are targeted and instruct the user to run `incan lock`
- [x] Materialization: embedded `Cargo.lock` is written as `Cargo.lock` into generated build/test Cargo project directories
- [x] Ensure dev-dependencies are represented so `incan test --locked` is meaningful

### Implementing Phase 5: Advanced Sources

- [x] Git dependencies
- [x] Path dependencies
- [x] Optional dependencies
- [x] Dev dependencies

### Error messages (section 5)

- [x] 5.1: Unknown crate without version
- [x] 5.2: Feature not found -- **deferred to Cargo** (requires registry queries; invalid features are caught by Cargo
  during the build step)
- [x] 5.3: Version not found -- **deferred to Cargo** (requires registry queries; invalid versions are caught by Cargo
  during the build step)
- [x] 5.4: Lock out of date (includes fingerprint details and explanation)
- [x] 5.5: Optional dependency not enabled
- [x] 5.6: Inline annotation forbidden (crate in incan.toml)
- [x] 5.7: Conflicting inline sites
- [x] 5.8: Dev-dependency imported from production code
- [x] 5.9: Git branch not allowed in strict mode
- [x] 5.10: Dependency rename collision

---

## Appendix A: Lockfile canonicalization and `deps-fingerprint` (normative)

### A.1 Embedded lock payload canonicalization

To avoid cross-platform churn and ensure deterministic `incan.lock` files:

- The embedded `[cargo].lock` payload must be stored as **UTF-8** text.
- Newlines must be normalized to `\n` (LF). `\r\n` must not appear in the embedded payload.
- The payload must end with a trailing newline.
- The payload content is otherwise treated as an opaque string (verbatim Cargo output); Incan does not attempt to
  reformat or “pretty print” it.

### A.2 `deps-fingerprint` algorithm

`deps-fingerprint` exists to answer: “Is the current lockfile still valid for the project’s dependency inputs?”

What is fingerprinted:

- The **effective crate specs** that would be used to generate the lock, after applying:
    - table alias normalization (`[rust.dependencies]` → `[dependencies]`, etc.),
    - precedence rules (incan.toml > inline > known-good),
    - merge rules defined by this RFC (feature union, conflict errors),
    - and dev-dependency gating decisions (the lock must cover both `[dependencies]` and `[dev-dependencies]`).
- The **Cargo feature selection** used for the generated Rust package at lock time:
    - `cargo-all-features`
    - `cargo-no-default-features`
    - `cargo-features`

Canonical representation:

- Build a canonical object:
    - `cargo_feature_selection`
    - `specs`

`cargo_feature_selection`:

- `cargo_all_features`: boolean
- `cargo_no_default_features`: boolean
- `cargo_features`: set of strings:
    - duplicates removed,
    - sorted lexicographically,
    - compared case-sensitively.

`specs`:

- Build a list of entries `Spec { crate_name, kind, source, version_req, default_features, features, optional, package }`
  where:
    - `crate_name` is the dependency key used by `rust::crate_name` imports (Rust identifier form).
    - `kind` is `"normal"` or `"dev"`.
    - `source` is one of:
        - `registry` (crates.io default; any custom registries are out of scope),
        - `git` (`git` URL plus exactly one of `rev` / `tag` / `branch`),
        - `path` (path relative to project root, normalized with `/` separators).
    - `version_req` is the Cargo SemVer requirement string with:
        - leading/trailing whitespace trimmed,
        - internal whitespace collapsed to single spaces,
        - otherwise preserved verbatim (for this RFC; no semantic re-serialization).
    - `default_features` is a boolean (missing defaults to `true`).
    - `features` is a set:
        - duplicates removed,
        - sorted lexicographically,
        - compared case-sensitively.
    - `optional` is a boolean (missing defaults to `false`).
    - `package` is either absent or a string (see §2.4.1 for renames).
- Sort the list by `(kind, crate_name)` where `kind` sorts `"normal"` before `"dev"`.
- Serialize the canonical object into canonical JSON (UTF-8, no whitespace, deterministic key order) and compute:
  `deps-fingerprint = "sha256:" + hex(sha256(json_bytes))`.

Stability note:

- Because the fingerprint is computed from the **effective** crate specs, toolchain-provided known-good defaults become
  part of the fingerprint for projects that rely on defaults. Changing the toolchain may therefore require re-locking,
  which is expected when you are not fully pinned by explicit `incan.toml` configuration.

---

## Appendix B: Design inspiration (informative): Hatch vs uv

This design is inspired by the developer experience of modern Python tooling:

- Tools like Hatch focus on project workflow and environment management; reproducible installs are often achieved via a
  separate lock mechanism or plugin ecosystem.
- Tools like Astral’s `uv` popularized a clear separation between:
    - “lock” (resolve and write a lockfile), and
    - “sync/run” (use the lockfile in strict modes).

Incan adopts the same mental model:

- `incan lock` produces/refreshes `incan.lock`
- `--locked` / `--frozen` (RFC 020) control the Cargo policy mode (`--locked` vs `--frozen`).
  In both cases, Incan requires a project-root `incan.lock` that is present and up to date (fingerprint matches).

RFC 015 provides more details on the project model and lifecycle.

# Manage dependencies

Dev.8 builds local Incan dependencies and links Rust dependencies from the installed toolchain's registry closure.

## Add a local Incan library

Point a dependency alias at its source project:

```toml
[project]
name = "pricing_app"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"

[dependencies]
pricing = { path = "../producer" }
```

The producer has its own `loaf.toml` and `src/lib.incn`. Import its public API through `pub::pricing`. Build and run the consumer with explicit paths:

```bash
incan build src/main.incn
incan run src/main.incn
```

Oven prepares the active path libraries before the consumer. Their artifacts live under the consumer root's `target/incan/loaves/`.

## Add a Rust requirement

```toml
[rust-dependencies]
regex = "1"
```

Use a versionless source import when the manifest declares the dependency:

```incan
from rust::regex import Regex
```

The declared requirement must match the installed registry's available unit, including its features. An absent unit is an explicit error; adding a version does not download source or invoke another package manager. For a single source, inline requirements use `@ "VERSION"` and `with ["FEATURE"]`; they do not provision missing artifacts.

## Select optional behavior

Use `optional = true` on an edge and activate it from a package feature with `dep:ALIAS`. Dependency feature requests use `ALIAS/FEATURE`. See [Select package features](sdk_components_and_package_features.md).

## Lock the selected graph

Generate the root project's selected dependency graph:

```bash
oven lock .
```

The generated `oven.lock` records exact path and compiled-unit edges. Commit it with the manifest. To choose another version-selection mode, run `oven lock --versions newest .` or `oven lock --versions lowest-direct .`; later lock commands keep that mode.

Require an unchanged committed graph in a build:

```bash
oven build --locked .
```

A missing or stale lock is refused and names `oven lock`. Ordinary run, build and test commands create or refresh the root lock silently.

## Inspect the selection

```bash
oven describe-unit src/main.incn
```

The unit description lists active features, Loaves, Rust crates and native artifacts. `incan lock` remains an unknown command.

The [manifest reference](../reference/project_configuration.md) lists accepted fields and defaults.

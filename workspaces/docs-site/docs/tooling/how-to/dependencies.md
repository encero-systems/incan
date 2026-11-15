# Manage dependencies

Dev.8 builds local Loaf dependencies and links registry Loaves from the installed toolchain's compiled closure.

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

```bash
oven add crates-io/regex
```

`oven add` writes the dependency line with the newest version the installed registry publishes, then locks the project. Name the Loaf in full. `oven add regex` doesn't pick one: it lists every published Loaf named `regex` for you to choose from. You can also write the line yourself:

```toml
[dependencies]
regex = { loaf = "crates-io/regex", version = "1" }
```

Use a versionless source import when the manifest declares the dependency:

```incan
from rust::regex import Regex
```

The dependency key `regex` is the `rust::regex` import name. The `loaf` field is the package identity; any syntactically valid scope is accepted at intake. The declared requirement must match the installed registry's available unit, including its features. An absent unit is an explicit error; adding a version does not download source or invoke another package manager. For a single source, inline requirements use `@ "VERSION"` and `with ["FEATURE"]`; they do not provision missing artifacts.

## Select optional behavior

Use `optional = true` on an edge and activate it from a package feature with `dep:ALIAS`. Dependency feature requests use `ALIAS/FEATURE`. See [Select package features](sdk_components_and_package_features.md).

## Lock the selected graph

Generate the root project's selected dependency graph:

```bash
oven lock .
```

The generated `oven.lock` records exact path and compiled-unit edges. Commit it with the manifest. To choose another version-selection mode, run `oven lock --versions newest .` or `oven lock --versions lowest-direct .`; later lock commands keep that mode.

An `untested lower bound` line means a requirement's lower bound names a version the registry does not publish, so the lock selected a later version and the bound itself was never tested. Raise the requirement's lower bound to the locked version when the project needs a tested bound.

Require an unchanged committed graph in a build:

```bash
oven build --locked .
```

A missing or stale lock is refused and names `oven lock`. Ordinary run, build and test commands create or refresh the root lock silently.

## Raise a requirement

A newly published version never reaches your project on its own. Raise a requirement when you want it:

```bash
oven update crates-io/regex
```

`oven update` moves the requirement to the newest published version it already admits, so a requirement of `1` can rise to a later `1.x.y` but never to `2`. Then it locks the project again. Every other dependency keeps its locked version. Without a name, `oven update` raises every registry requirement.

## Inspect the selection

```bash
oven describe-unit src/main.incn
```

The unit description lists active features, Loaves, Rust crates and native artifacts. `incan lock` remains an unknown command.

The [manifest reference](../reference/project_configuration.md) lists accepted fields and defaults.

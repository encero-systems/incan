# Project configuration (`loaf.toml`)

A dev.8 project manifest is a TOML file named `loaf.toml`. Source commands select the nearest manifest above the source file. Without one, the source is a standalone unit.

## Root tables

The accepted root tables are `project`, `dependencies`, `rust-dependencies`, `build`, `rust`, `vocab`, `tool` and `interop`. Unknown semantic tables or fields are refused. `sdk`, `workspace`, `envs` and `rust-dev-dependencies` are not accepted.

## `[project]`

`name` and `version` are required nonempty strings. Accepted metadata fields are `description`, `authors`, `license`, `license-files`, `readme`, `requires-incan` and `private`. Accepting a metadata field does not enforce a compiler-version requirement.

```toml
[project]
name = "hello"
version = "0.1.0"
```

### `[project.scripts]`

Maps script names to existing source-path strings. A project must have at least one script, a conventional `src/lib.incn`, or a `tests` directory. A script resolving to the conventional library is refused. Source commands still require an explicit path; a `main` script does not supply an omitted argument.

### `[project.features]`

Features are additive. `default` is selected unless `--no-default-features` is passed. `--features` adds named root features; `--all-features` selects all root features.

The compact form is a string array:

```toml
[project.features]
default = ["text"]
text = []
json = ["dep:serializer", "serializer/json"]
```

An unprefixed entry includes another local feature. `dep:ALIAS` activates an optional dependency. `ALIAS/FEATURE` requests a feature of an active dependency. The expanded form accepts `includes`, `optional-dependencies` and `dependency-features`:

```toml
[project.features.json]
includes = ["text"]
optional-dependencies = ["serializer"]
dependency-features = { serializer = ["json"] }
```

Unknown features, unknown dependency aliases, dependency-feature requests against inactive optional dependencies, and inclusion cycles are refused. Source declarations are conditioned by `when feature("NAME"):`.

## `[dependencies]`

A local Incan dependency names the source project, relative to this manifest:

```toml
[dependencies]
pricing = { path = "../producer" }
```

Accepted local-edge fields are `path`, `optional` (default `false`), `default-features` (default `true`) and `features` (default empty). A local edge with `loaf` names a sibling Rust facet. An Incan dependency's alias is its `pub::ALIAS` namespace.

A registry Rust dependency uses `loaf = "crates-io/PACKAGE"` and `version`. It also accepts `features`, `default-features`, `optional` and `target`.

## `[rust-dependencies]`

A string value is a version requirement. A table accepts `version`, `package`, `features`, `default-features`, `optional`, `path` and `target`. A version is required unless `path` is present. `package` supplies the upstream name when the key is an alias. Defaults are empty features, default features enabled, and nonoptional.

```toml
[rust-dependencies]
regex = "1"
json = { package = "serde_json", version = "1", features = ["preserve_order"] }
```

A requirement must match an available toolchain registry unit. Declaration does not download a crate. `target` accepts an exact target triple or a supported `cfg(...)` predicate. Git selectors and Cargo manifests are not dependency sources for this CLI.

## `[build]`

| Field | Type | Contract |
| --- | --- | --- |
| `source-root` | String | Project-relative source directory. Without it, `src` is selected when present, otherwise the project root. |
| `target` | String | Authored compilation-target intent. |
| `profile` | String | Authored profile intent; CLI unit commands default to `debug` and accept `--profile`. |
| `rust-edition` | String | Authored Rust edition intent. |

Declaring a target or profile does not install its artifacts.

## `[rust]`

Accepted fields are `name`, `type`, `edition` and `source`. `type` accepts `lib` or `proc-macro`. `edition` accepts `2015`, `2018`, `2021` or `2024`. `[rust.source]` accepts a required string `root`.

## `[vocab]`

Accepts `crate`. This table does not build a companion crate or a Wasm desugarer.

## `[tool.incan.metadata]`

`model-bundles` is an array of project-relative model-bundle paths supplied to the checker.

## `[tool.incan.codegraph]`

`sources` is an array of additional project paths for graph inspection. These paths do not add executable dependencies.

## `[interop.c]`

`[[interop.c.targets]]` entries can declare `headers`, an array of project-relative paths supplied to the checker. Header and binding contracts are specified in [`std.interop`](../../language/reference/stdlib/interop.md). Dev.8 exposes no interop bake, stage or deployment-plan command.

## Related contracts

- [CLI reference](cli_reference.md)
- [Package features and standard packages](sdk_components_and_package_features.md)
- [Project lifecycle](../../language/reference/project_lifecycle.md)

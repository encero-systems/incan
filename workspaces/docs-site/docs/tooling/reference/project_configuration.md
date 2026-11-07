# Project configuration (`loaf.toml`)

A project manifest is a TOML file named `loaf.toml`. Source commands select the nearest manifest above the source file. Without one, the source is a standalone unit.

## Root tables

The accepted root tables are `project`, `dependencies`, `build`, `rust`, `vocab`, `tool`, `interop` and `envs`. Unknown root tables are refused. `sdk`, `workspace`, `rust-dependencies` and `rust-dev-dependencies` are not accepted.

## `[project]`

`name` and `version` are required nonempty strings. Accepted metadata fields are `description`, `authors`, `license`, `license-files`, `readme`, `requires-incan` and `private`. Accepting a metadata field does not enforce a compiler-version requirement.

```toml
[project]
name = "hello"
version = "0.1.0"
```

### `[project.scripts]`

Maps script names to existing source-path strings. Project intake requires at least one script, an Incan `lib.incn` under the selected source root, a Rust `src/lib.rs`, or a `tests` directory. A script resolving to the conventional Incan library is refused. Source commands still require an explicit path; a `main` script does not supply an omitted argument.

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

Unknown features, unknown dependency aliases and dependency-feature requests against inactive optional dependencies are refused. Feature inclusion cycles are finite and accepted. Source declarations are conditioned by `when feature("NAME"):`.

## `[dependencies]`

Every project dependency is a table under `[dependencies]`. A local Incan dependency names the source project, relative to this manifest:

```toml
[dependencies]
pricing = { path = "../producer" }
```

Accepted local-edge fields are `path`, `optional` (default `false`), `default-features` (default `true`) and `features` (default empty). A local edge with `loaf` names a sibling Rust facet. An Incan dependency's alias is its `pub::ALIAS` namespace.

A registry dependency requires `loaf` and `version`. It also accepts `features` (default empty), `default-features` (default `true`), `optional` (default `false`) and `target`.

```toml
[dependencies]
regex = { loaf = "crates-io/regex", version = "1" }
json = { loaf = "crates-io/serde_json", version = "1", features = ["preserve_order"] }
```

The dependency key is the code-facing alias. `package`, `crate`, bare string values and the `[rust-dependencies]` table are refused with the corresponding `loaf` form. A requirement must match an available toolchain registry unit. Declaration does not download a Loaf. `target` accepts an exact target triple or a supported `cfg(...)` predicate. Git selectors and Cargo manifests are not dependency sources for this CLI.

`loaf` accepts an unscoped identity or one scope and name separated by `/`. Components contain 1 through 64 lowercase ASCII letters, digits or hyphens and begin with a letter or digit. Unscoped local identities and exact upstream names under `crates-io/` may also contain underscores. Capitals are refused and are never folded. The default Rust crate name is the identity's final component with hyphens replaced by underscores.

## `[build]`

| Field | Type | Contract |
| --- | --- | --- |
| `source-root` | String | Project-relative source directory. Without it, `src` is selected when present, otherwise the project root. |
| `target` | String | Authored compilation-target intent. |
| `profile` | String | Authored profile intent; CLI unit commands default to `debug` and accept `--profile`. |
| `rust-edition` | String | Authored Rust edition intent. |
| `unstable-features` | Array of distinct Rust identifiers | Unstable features of the pinned compiler that the Loaf's Incan units enable, such as `rustc_private`. Empty by default. |
| `toolchain-components` | Array of distinct component names | Components of the pinned toolchain that the Loaf's Incan units require, such as `rustc-dev`. A name uses ASCII letters, digits, `-` and `_`. Empty by default. |
| `sysroot-dependencies` | Array of distinct Rust identifiers | Libraries of the pinned toolchain that the Loaf's Incan units link, such as `rustc_driver`. Empty by default. |

Declaring a target or profile does not install its artifacts.

`registry-lock` is not an accepted `[build]` field. Exact selected producers are generated in [`oven.lock`](lock_file.md); the project manifest does not name a second producer record.

The Loaf's Incan units are its programs, its tests and its library. A unit enables only the unstable features its Loaf declares and links only the toolchain libraries its Loaf declares; a library the Loaf does not declare is refused when a `rust::` import names it. A declared component that the pinned toolchain does not have installed is refused, as is a declared sysroot dependency that does not name exactly one library in the toolchain's `lib/rustlib/HOST/lib`. A declared library is importable through `rust::`. An executable that links one loads it, with the standard library it was built with, from the pinned toolchain's sysroot, and never links shared development libraries. The declaration is part of each unit's identity: declaring, changing or removing it compiles the unit again.

```toml
[build]
unstable-features = ["rustc_private"]
toolchain-components = ["rustc-dev"]
sysroot-dependencies = ["rustc_driver"]
```

## `[envs.NAME]`

An environment accepts a `dependencies` table with the same edge fields as the root table. `rust-dependencies` is refused. `incan test` selects the root project's `test` environment in addition to its production dependencies, including for a Rust facet's unit-test harness. Ordinary builds do not select that overlay, and a dependency's own test environment does not propagate to consumers.

## `[rust]`

Accepted fields are `name`, `type`, `edition`, `source`, `bin`, `lib` and `build-script`. The `name` default is the project identity's final component after replacing each hyphen with an underscore. `type` accepts `lib` (the default) or `proc-macro`. `edition` accepts `2015`, `2018`, `2021` or `2024`, overrides `build.rust-edition` and defaults to `2024`. `build-script` is a boolean and defaults to `false`. Local intake refuses `true`, naming the Loaf identity and the absence of an exact recorded build fact.

`[rust.source]` accepts a required string `root`, relative to the project. The library source is `src/lib.rs` under that directory, or under the project when `root` is omitted. `rust.lib` does not override this discovery.

Each `[[rust.bin]]` requires string fields `name` and `path`; the source path is relative to the project. It also accepts `unstable_features`, `toolchain_components` and `sysroot_dependencies`, which take the values of the `[build]` keys spelled with hyphens. Other keys are refused. A nonempty `unstable_features` or `sysroot_dependencies` enables unstable-feature compilation or pinned-sysroot library linking, respectively, for the project's Rust binaries.

## `[vocab]`

Accepts a required nonempty string `entry`; `crate` and other keys are refused. [Vocabulary entries](vocabulary.md) specifies the entry contract.

## `[tool.incan.metadata]`

`model-bundles` is an array of project-relative model-bundle paths supplied to the checker.

## `[tool.incan.codegraph]`

`sources` is an array of additional project paths for graph inspection. These paths do not add executable dependencies.

## `[interop.c]`

`[[interop.c.targets]]` entries can declare `headers`, an array of project-relative paths supplied to the checker. Header and binding contracts are specified in [`std.interop`](../../language/reference/stdlib/interop.md).

## Related contracts

- [CLI reference](cli_reference.md)
- [Package features and standard packages](sdk_components_and_package_features.md)
- [Project lifecycle](../../language/reference/project_lifecycle.md)

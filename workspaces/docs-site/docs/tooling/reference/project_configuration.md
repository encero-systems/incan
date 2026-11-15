# Project configuration (`loaf.toml`)

A project manifest is a TOML file named `loaf.toml`. Source commands select the nearest manifest above the source file. Without one, the source is a standalone unit. A directory on the way that holds `incan.toml` and no `loaf.toml` stops the search with an error naming that file; `incan.toml` is never read.

## Root tables

The accepted root tables are `project`, `dependencies`, `build`, `rust`, `vocab`, `tool`, `interop`, `c`, `envs` and `workspace`. Unknown root tables are refused. `sdk`, `rust-dependencies` and `rust-dev-dependencies` are not accepted.

## `[project]`

`name` and `version` are required nonempty strings. Accepted metadata fields are `description`, `authors`, `maintainers`, `license`, `license-files`, `readme`, `homepage`, `repository`, `documentation`, `issues`, `keywords`, `classifiers`, `requires-incan` and `private`. `requires-incan` is a version requirement that the active toolchain must satisfy before `run`, `build`, `test`, `lint`, `oven check`, `oven lock`, `oven add`, `oven dependency` and `oven update` compile, run or lock anything; a toolchain it excludes is refused with `COMMAND refused: REASON`, exit `1`. `incan check` does not enforce it, and neither does `incan run --direct`. A prerelease toolchain satisfies it by its release version: `0.6.0-dev.8` meets `>=0.5` and misses `>=0.5.0,<0.6.0`, as `0.6.0` does, unless the requirement names a prerelease of that release, such as `>=0.6.0-dev.5`.

```toml
[project]
name = "hello"
version = "0.1.0"
```

### `registry-id`

Optional string containing `sha256:` followed by exactly 64 lowercase hexadecimal digits. It declares the identity of the package's registry root key. Oven validates the spelling without contacting a registry or authenticating the declaration. A malformed or non-string value is refused at `[project].registry-id`.

Oven carries this value with the package's own `name` to the compiler and includes it in the compilation key. A compiled library retains the producer's pair; a consumer's dependency alias does not replace it. A persistent record namespace requires a scoped name such as `example/catalog` as well as a registry identity. A bare project name or standalone source has no persistent record namespace.

Omitting the key preserves the existing unit key and records no explicit registry identity. No default registry identity is synthesized when the key is absent. These retained package facts do not by themselves provide generated record walking or stored-type selectors.

### `[project.scripts]`

Maps script names to existing source-path strings. Project intake requires at least one script, an Incan `lib.incn` under the selected source root, a Rust `src/lib.rs`, or a `tests` directory. A script resolving to the conventional Incan library, such as `library = "src/lib.incn"`, is accepted as 0.5.1 accepted it and ignored: the library is built as a library, never as an executable, and `run`, `check`, `build` and `test` print one warning naming the script line. A script does not select what `run`, `check` and `build` compile: without `PATH`, they select the Loaf's `lib.incn` or `main.incn`, as [`oven build`](cli_reference.md#oven-build) states.

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

A provider dependency is a `provider`-kind requirement: a target-specific library, SDK, toolchain, runtime or deployment requirement that an Oven-managed provider satisfies. It requires `provider`, which follows the `loaf` identity rules, and `version`, a version requirement. It also accepts `features` (default empty), `optional` (default `false`) and `target`. `loaf` and `path` are refused beside `provider`. `oven dependency list` prints it as `KEY: provider NAME REQUIREMENT`. This toolchain manages no providers, so locking, running, building or testing a project whose graph declares one is refused, naming the manifest, the key and the provider.

```toml
[dependencies]
vision = { provider = "vision-runtime", version = ">=2.0" }
```

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

Every Loaf has the standard environments `dev`, `test`, `lint` and `docs` without a table. `[envs.NAME]` adds to one of them or declares another. [`oven env`](cli_reference.md#oven-env) lists, shows and runs them.

| Key | Type | Contract |
| --- | --- | --- |
| `dependencies` | Table | Entries with the edge fields of [`[dependencies]`](#dependencies). `NAME = { workspace = true }` inherits the workspace root's shared entry. |
| `extends` | String or array of strings | The environments this one extends. `workspace:NAME` names the workspace root's `[workspace.envs.NAME]` fragment. |
| `requires-incan` | String | A version requirement on the active toolchain, as `[project].requires-incan`. |
| `cwd` | String | The directory `oven env run` runs a script in, relative to the project. Default `.`. |
| `env-vars` | Table of strings | Variables `oven env run` sets. |
| `scripts` | Table | Each script is a nonempty array of strings, its argument list, or a string, a command without arguments. |
| `detached` | Boolean | `true` leaves `dev` out of the environment's overlay chain. Default `false`. |
| `description` | String | Accepted; no command reads it. |

Other keys are refused. `rust-dependencies` is refused with the `[envs.NAME.dependencies]` form of its entry.

An environment resolves through its overlay chain: the project, then `dev` unless the environment is `dev` or `detached`, then the chain of each environment it `extends`, in order, then the environment itself. An environment appears in a chain once. A cycle is refused with ``environment `NAME` extends itself through A -> B -> NAME``, and an environment that is neither standard nor declared with ``environment `NAME` is not configured``. A later layer replaces `cwd` and adds or replaces each variable, script and dependency entry by name. Every `requires-incan` along the chain, the project's included, applies.

`incan test` selects the root project's `test` environment in addition to its production dependencies, including for a Rust facet's unit-test harness. Ordinary builds do not select that overlay, and a dependency's own test environment does not propagate to consumers.

## `[rust]`

Accepted fields are `name`, `type`, `edition`, `source`, `bin`, `lib`, `build-script`, `lints` and `facts`. The `name` default is the project identity's final component after replacing each hyphen with an underscore. `type` accepts `lib` (the default) or `proc-macro`. `edition` accepts `2015`, `2018`, `2021` or `2024`, overrides `build.rust-edition` and defaults to `2024`. `build-script` is a boolean and defaults to `false`. Local intake refuses `true` when the facet has no `[[rust.facts]]` record, naming the Loaf identity and the absence of an exact recorded build fact.

`[[rust.facts]]` records a build script's answers for one binding (RFC 119): `toolchain` (the rustc identity line), `target`, `profile` and `features` (the complete enabled feature set) name the binding, `cfg` lists the flags the crate is compiled with, `out` lists the committed generated files the crate reads from `OUT_DIR`, each with its `name`, its `path` in the Loaf and its `digest`; they must lie in one directory under their names and match their digests, and `environment` is a table of variables the compile sees. `harvested-from` is evidence and is not read. A compile takes only the record whose binding equals its own exactly, and refuses a unit with a build script and no such record, naming the unit and the binding. A `[[rust.facts.link]]` record builds a static library the unit links: its `executable` (`name`, `owner`, `path`, `digest`) is found under the first directory of `INCAN_OVEN_LINK_OWNERS` (a `:`-separated list) that holds `path` with that `digest`, every `sources` entry (a `file`, or a `tree` with `members`) must match its digests, each `objects` entry compiles from the Loaf with its `arguments` (`literal`, `owner`-relative, `input` source or `output` object), and the objects are archived in order into `library` (`kind = "static"`) under `.oven-link/` in the Loaf, in a directory named by the record's digest. A missing owner, a compiler or source of other bytes, or a failed compile refuses the unit by name. A record that declares `tool` work is refused.

`[rust.source]` accepts a required string `root`, relative to the project. The library source is `src/lib.rs` under that directory, or under the project when `root` is omitted. `rust.lib` does not override this discovery.

Each `[[rust.bin]]` requires string fields `name` and `path`; the source path is relative to the project. It also accepts `unstable_features`, `toolchain_components` and `sysroot_dependencies`, which take the values of the `[build]` keys spelled with hyphens. Other keys are refused. A nonempty `unstable_features` or `sysroot_dependencies` enables unstable-feature compilation or pinned-sysroot library linking, respectively, for the project's Rust binaries.

`[rust.lints]` sets lint levels for the Loaf's Rust units. It accepts two tables, `rust` for rustc's lints and `clippy` for clippy's; any other key is refused by name. Each entry maps a lint or lint group to `allow`, `warn`, `deny` or `forbid`, or to a table of a required `level` and an optional integer `priority` (default `0`); any other key in that table is refused by name. A lint name carries no tool prefix, since its table names the tool. `expect` is refused: rustc accepts it only as a source attribute, `#[expect(...)]`. `oven lint` passes the levels as `-A`, `-W`, `-D` and `-F` flags after the unit's own arguments, lowest priority first and by lint name within one priority, so a crate's own lint attributes still apply. Each Loaf declares its own table; a dependency's table applies to that dependency's units only.

```toml
[rust.lints.rust]
unsafe_code = "forbid"
warnings = "deny"

[rust.lints.clippy]
pedantic = { level = "warn", priority = -1 }
needless_return = "allow"
```

## `[vocab]`

Accepts a required nonempty string `entry`; `crate` and other keys are refused. [Vocabulary entries](vocabulary.md) specifies the entry contract.

## `[workspace]`

A manifest with a `[workspace]` table is a workspace root. `members`, `default-members` and `exclude` are arrays of member directories relative to the root. An entry of `members` or `exclude` may be a glob pattern (`*`, `?`, `[abc]`): it names every directory it matches that holds a `loaf.toml`, a wildcard never crosses a `/`, and a `members` pattern that matches nothing names no member; `oven workspace inspect --format json` lists it under `warnings` as `{"kind": "unmatched_glob", "pattern": PATTERN}`. [Workspaces](../../language/how-to/workspaces.md) covers member selection.

`[workspace.dependencies]` declares dependencies the members share. Each entry names a local `path`, relative to the root, or a registry `loaf` with its `version`, and accepts the fields of a `[dependencies]` entry. A member inherits one with `NAME = { workspace = true }` in its own `[dependencies]`, optionally with `features`, added to the entry's, and `optional`. The member must be the root or one of its `members`; a local `path` is rebased from the root onto the member. `[workspace.rust-dependencies]` is refused with the corresponding `[workspace.dependencies]` Loaf form. `[workspace.rust-dev-dependencies]` is refused with that form too, and with `NAME = { workspace = true }` for each member's `[envs.test.dependencies]`, which inherits a shared entry as `[dependencies]` does.

`[workspace.envs.NAME]` declares an environment fragment with the keys of `[envs.NAME]`. An environment of the root or a member reaches it only through `extends = ["workspace:NAME"]`, which applies the fragment before the environment's own layer; `oven env list` does not list fragments.

`[workspace.policy]` and `[workspace.sources]` are tables accepted as 0.5.1 accepted them and reserved for later phases; no command reads them. `oven workspace inspect --format json` reports the shared tables as `shared_dependencies` (`libraries`, `rust` and an empty `rust_dev`), `shared_environments`, `shared_policy` and `shared_sources`.

## `[tool.incan]`

`metadata` and `codegraph` are read as below. Any other `[tool.incan]` key is tolerated, as 0.5.1 tolerated it, and `run`, `check`, `build` and `test` print one warning naming each such key. `[tool.incan.envs]` is refused in favor of `[envs.NAME]`.

## `[tool.incan.metadata]`

`model-bundles` is an array of project-relative model-bundle paths supplied to the checker.

## `[tool.incan.codegraph]`

`sources` is an array of additional project paths for graph inspection. These paths do not add executable dependencies.

## `[interop.c]`

`[interop.c]` accepts `schema`, which is `1` when given (0.5.1's `[oven.interop].schema`), and `targets`. Each `[[interop.c.targets]]` entry requires `target`, a target triple declared once per package, and accepts the target's requirements, `headers`, an array of project-relative paths supplied to the checker, `artifacts` and `bindings`. Header and binding contracts are specified in [`std.interop`](../../language/reference/stdlib/interop.md).

```toml
[[interop.c.targets]]
target = "aarch64-apple-ios"
toolchain = { capability = "apple-clang", version = ">=17" }
sdk = { capability = "iphoneos", version = ">=18" }
platform = { kind = "ios", deployment-target = "13.0" }
headers = ["interop/include/bridge.h"]
definitions = ["BRIDGE_MOBILE=1"]

[[interop.c.targets.artifacts]]
name = "bridge"
kind = "static"
path = "interop/lib/libbridge.a"
origin = { source = "https://example.invalid/bridge", revision = "v1.2.3", license = "MIT" }
dependencies = ["foundation"]

[[interop.c.targets.artifacts]]
name = "foundation"
kind = "system"
capability = "apple.framework.Foundation"

[[interop.c.targets.bindings]]
module = ["bridge"]
name = "Bridge"
artifacts = ["bridge"]
```

A target's requirements describe what the package needs; they never name an installation:

| Field | Form | Meaning |
| --- | --- | --- |
| `toolchain` | Table of a non-empty `capability` and an optional semantic-version requirement `version`. | The compatible C toolchain: `apple-clang`, `clang` or `android-ndk`. |
| `sdk` | The same form. | The compatible SDK: `macosx`, `iphoneos`, `iphonesimulator` or `android`. |
| `definitions` | Array of non-empty `NAME` or `NAME=VALUE` strings. | Preprocessor definitions for verification against this target. |
| `platform` | `{ kind = "android", api-level = N }` or `{ kind = "ios", deployment-target = "major.minor" }`. | The mobile version fact that completes the target's ABI. |

An Android `platform` belongs to `aarch64-linux-android` only, requires `api-level` 21 or later and the `android` SDK capability. An iOS `platform` belongs to `aarch64-apple-ios`, which requires the `iphoneos` SDK capability, or `aarch64-apple-ios-sim`, which requires `iphonesimulator`; its `deployment-target` is numeric `major.minor`.

Each `[[interop.c.targets.artifacts]]` entry declares a package-owned archive, a package-owned runtime library or a system capability:

| Field | Form | Meaning |
| --- | --- | --- |
| `name` | Required identifier, unique within the target. | The artifact's package-local name. |
| `kind` | Required: `static`, `bundled` or `system`. | A `static` archive is linked into the units that link a binding naming it; a `bundled` dynamic library or framework is staged for a platform packager by [`oven interop stage`](cli_reference.md#oven-interop-stage); a `system` artifact is a library or framework the selected toolchain or SDK supplies. |
| `path` | `static` and `bundled`: required path inside the package; a static archive is named `lib<link>.a`. | The file; `<link>` is the link name a binding's `c.system_library("<link>")` uses. |
| `runtime-name` | `bundled` only: required, one file name. | The name the runtime loader finds the library by; the staged file's name. |
| `placement` | `bundled` only: required, non-empty. | The logical packager placement, such as `jniLibs/arm64-v8a` or `frameworks`. |
| `minimum-platform` | `bundled` only: required, non-empty. | The oldest platform version the library supports. |
| `capability` | `system` only: required, non-empty. | The toolchain or SDK capability, such as `apple.framework.Foundation` or `android.library.log`. |
| `dependencies` | Array of other artifact names of the same target. | Artifacts this one needs; archives link after the archives that need them. |
| `origin` | `static` and `bundled`: table of `source` (an HTTPS URL), `revision` and `license`, all non-empty. | Upstream provenance of the package's copy; never downloaded. |

Each `[[interop.c.targets.bindings]]` entry relates one checked binding, by its `module` (a non-empty array of names) and `name`, to the artifacts (`artifacts`, a non-empty array of distinct declared names) that provide its symbols; the relation is never inferred from a header, library name or file name. A build for the host target links every declared static artifact of the package's units as a static library, an archive before the archives it depends on, and an archive's bytes are part of the identity of each unit that links it. A `bundled` or `system` artifact is recorded in the [interop plan](cli_reference.md#oven-inspect-interop-plan), not linked.

`check`, `build`, `test` and `run` verify checked C bindings against the host. With `--interop-target TRIPLE` they verify them against the package's declared entry for `TRIPLE` instead: its ABI, its `definitions`, and its `toolchain` and `sdk` requirements, which the selected C toolchain and SDK must meet. The unit is still built for the host. See [Checked C bindings](../../language/how-to/checked_c_bindings.md#verify-against-a-declared-target).

The root project's `oven.lock` records each target's contract under `[[interop]]`, its own and each path dependency's, the latter under the dependency's `package`: its requirements, each declared header's digest, and each artifact's kind, path and digest (`missing` for a declared file that is not there), with a package-owned artifact's `origin` (source, revision and license), a bundled artifact's `runtime-name` and `placement` and a system artifact's `capability`. A changed requirement or file changes the lock, so a `--locked` command refuses it and names the artifact or target that changed.

- Refused: an unknown key; a `schema` other than `1`; a `shims` table, naming the [`[c]`](#c) facet that compiles a shim as a Clove (its `library` is the shim's `output`, its `public-headers` the shim's `headers`, its `[[c.compile]]` sources the shim's `sources`), linked with `c.loaf_library`; a missing `target`, `name`, `kind`, `path` or `capability`; a target declared twice; a capability requirement whose `version` is not a version requirement; an empty definition; a `platform` on another target, with an Android API level below 21, a malformed iOS deployment target, or without its SDK capability; a `kind` other than `static`, `bundled` and `system`; a path outside the package; a static path not named `lib<link>.a`; a bundled artifact without `path`, `runtime-name`, `placement` or `minimum-platform`, or whose `runtime-name` is not one file name; a static or bundled artifact with a `capability`; a system artifact with a `path` or `origin`; bundled fields on a static or system artifact; an artifact declared twice; a dependency or relation that names an undeclared artifact; an `origin` without an HTTPS `source`, `revision` or `license`; and, at build time, a declared archive that is not a file.

## `[c]`

A Loaf with a `[c]` table carries a C facet and is a Clove. Every path is relative to the Loaf root.

| Field | Form | Meaning |
| --- | --- | --- |
| `library` | Required string, a C identifier. | The archive's link name: `lib<library>.a`. |
| `public-headers` | Required array of paths. | The headers a [`c.loaf_library`](../../language/reference/stdlib/interop.md#binding-declarations) binding may name. |
| `include` | Array of paths. | Include roots of every compile. |
| `std` | `c89`, `c90`, `c99`, `c11`, `c17`, `c18`, `c23`, or the `gnu` form of one of these. | The language standard of the C sources. |
| `[[c.compile]]` | At least one entry. | Compile units. |
| `[c.configurations.NAME]` | Tables. | Named configurations. |

Each `[[c.compile]]` entry accepts `sources` (required: files, or globs whose `*`, `?` and `[...]` match file names in one directory), `target` (an exact triple or a `cfg(...)` predicate; the entry applies only where it holds), `include` and `defines` (a table of C identifiers to string values). A source is C (`.c`), assembly (`.S`, `.s`) or C++ (`.cc`, `.cpp`, `.cxx`) behind a C interface.

A configuration accepts `extends` (one other configuration), `sources` and `defines`. It compiles the compile entries' sources and the sources and definitions of its `extends` chain and its own. Without `[c.configurations]`, the facet has one configuration, `default`, with no definitions.

An `oven build`, `run`, `test` or `check` of a unit that reaches a Clove (its own project or a path dependency) compiles every configuration into a static archive under the Clove's own `target/incan/cloves/`, once for every unit that reaches it. Archive members, their order and their metadata are fixed, so identical inputs give identical archive bytes. A configuration is reused while its declared inputs and every file its compiles read are unchanged; a rebuilt archive rebuilds the units that link it. Optimization follows the profile: `-O0` for `debug`, `-O3` for `release`.

A Clove compiles only with the host compiler, which `INCAN_C_COMPILER=host` selects: Xcode's Clang and `libtool` on macOS, `cc`, `c++` and `ar` elsewhere. The unit records the host compiler's identity and is not publishable.

- Refused: a missing `library`, `public-headers` or `[[c.compile]]` entry; a `library` that is not a C identifier; a declared `[c.configurations]` without `default`; an `extends` that names no configuration or forms a cycle; a definition that redefines one inherited through `extends`; a source of another file kind; an absolute path, a path naming an environment variable, or a path that leaves the Loaf root; a glob that matches nothing or spans directories; compiler flags (`flags`, `cflags` and other keys); a build without `INCAN_C_COMPILER=host`; and a compile that reads a file outside the Loaf root.

## Related contracts

- [CLI reference](cli_reference.md)
- [Package features and standard packages](sdk_components_and_package_features.md)
- [Project lifecycle](../../language/reference/project_lifecycle.md)

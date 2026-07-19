# Select SDK components and package features

Use SDK components to control which official capabilities a project may use, and package features to select additive API and dependencies owned by an Incan package. They solve different problems: selecting a package feature never installs or silently enables an SDK component.

## Inspect the default project projection

A project without `[sdk]` uses the active release's `default` profile. Inspect what is available, enabled, and actually used before changing the selection:

```bash
incan inspect providers .
incan inspect providers . --format json
incan inspect features . --format json
```

Use the human output while diagnosing a project. Use JSON in CI or other tooling that needs component selection reasons, provider provenance, active features, dependency edges, or implementation facets without parsing terminal prose.

## Select a smaller SDK surface

Start from `minimal` and add only the capability groups the project needs:

```toml title="incan.toml"
[project]
name = "catalog_service"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"

[sdk]
profile = "minimal"
components = ["stdlib-system", "stdlib-data"]
```

`stdlib-core` is mandatory and is added automatically. Component dependencies are also expanded automatically; adding `stdlib-data` brings its required system and core closure. Stable source imports do not expose the physical provider split:

```incan
from std.fs import Path
from std.json import JsonValue
```

Confirm the effective selection, generate the semantic lock, and check the project:

```bash
incan inspect providers .
incan lock
incan check src/main.incn
```

Use a command-local profile override to compare another projection without editing `incan.toml`:

```bash
incan check src/main.incn --sdk-profile full
incan inspect providers . --sdk-profile full --format json
```

The override changes only the base profile for that invocation. Manifest `components` additions and `exclude-components` restrictions still apply.

## Distinguish disabled from unavailable components

These failures require different fixes:

| State                              | Meaning                                                                                    | Remedy                                                                                                      |
| ---------------------------------- | ------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------- |
| Known provider, component disabled | The SDK knows which component owns the import, but the project selection excludes it.      | Add the component under `[sdk].components`, remove the conflicting exclusion, or select a suitable profile. |
| Component enabled but unavailable  | The project selected a known component whose payload is absent from this SDK installation. | Install an SDK distribution containing that component. Compilation does not download it.                    |
| Unknown module                     | No enabled or disabled provider in the active SDK claims the import path.                  | Correct the import or add the package that owns it.                                                         |

In JSON provider inspection, compare the component and provider `available`, `enabled`, and `used` fields. Do not treat `available: true` as permission to use a provider: the project must also enable its component. Conversely, an enabled component with `available: false` is an installation problem rather than an unknown import.

## Add a package-owned feature

Declare public features under `[project.features]`:

```toml title="incan.toml"
[project]
name = "reporting"
version = "0.1.0"

[project.features]
default = ["text"]
text = []
json = []
```

Condition additive source declarations with `when feature(...)`:

```incan title="src/lib.incn"
pub def render_text(value: str) -> str:
    return value

when feature("json"):
    from std.json import JsonValue

    pub def render_json(value: str) -> JsonValue:
        return JsonValue.string(value)
```

The condition is evaluated while the compiler constructs the package's source projection. It is not a runtime branch. An inactive declaration does not participate in typechecking, generated Rust, checked library metadata, LSP, documentation, or codegraph facts.

Compare feature projections explicitly:

```bash
incan check src/main.incn --no-default-features
incan check src/main.incn --features json
incan inspect features . --features json --format json
incan test --all-features
```

`--features`, `--no-default-features`, and `--all-features` select public Incan package features. Cargo features remain on the separate `--cargo-features`, `--cargo-no-default-features`, and `--cargo-all-features` flags.

## Activate an optional Incan dependency

An optional dependency stays outside the active graph until a feature selects `dep:<name>`. A package can activate the dependency and request one of its public features in the same declaration:

```toml title="reporting/incan.toml"
[project]
name = "reporting"
version = "0.1.0"

[project.features]
default = []
json = ["dep:serializer", "serializer/json"]

[dependencies]
serializer = { path = "../serializer", optional = true, default-features = false }
```

A consumer selects the public feature on its dependency edge:

```toml title="app/incan.toml"
[project]
name = "report_app"
version = "0.1.0"

[dependencies]
reporting = { path = "../reporting", default-features = false, features = ["json"] }
```

Feature resolution is additive. If several active parents request different features from `reporting`, the compiler uses their union. Unknown features, feature cycles, `dep:` references to non-optional dependencies, and dependency-feature requests against inactive optional edges are configuration errors.

## Require an SDK component from a feature

Use the expanded feature form when different edge kinds should remain explicit:

```toml title="incan.toml"
[sdk]
profile = "minimal"
components = ["stdlib-web"]

[project.features.server]
requires-sdk-components = ["stdlib-web"]
```

Activating `server` verifies that `stdlib-web` is enabled and available; it does not add the component to `[sdk]` or install it. This keeps package-owned API selection separate from project- and installation-owned SDK composition.

## Lock and verify the intended projection

Feature and component selections affect checked facts and generated output, so record the exact closure in `incan.lock`:

```bash
incan lock --features json
incan build src/main.incn --features json --locked
incan inspect features . --features json --format json
incan inspect providers . --features json --format json
```

Changing the feature or SDK projection under `--locked` is rejected until the lock is refreshed deliberately. Use the same selection flags in inspection and compilation when comparing their output.

## See also

- [SDK components and package features reference](../reference/sdk_components_and_package_features.md)
- [How SDK components become compiled providers](../explanation/compiled_sdk_provider_flow.md)
- [Project configuration](../reference/project_configuration.md)
- [Conditional compilation](../../language/reference/conditional_compilation.md)
- [Work with a multi-project workspace](../../language/how-to/workspaces.md)

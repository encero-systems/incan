# Select package features

Declare additive features in `loaf.toml`:

```toml
[project]
name = "hello"
version = "0.1.0"

[project.features]
default = ["text"]
text = []
json = []
```

Check the same source with different selections:

```bash
incan check src/main.incn --features json
incan check src/main.incn --no-default-features
incan describe-unit src/main.incn --all-features
```

Read `declared_features` and `active_features` in the unit description. Use `when feature("json"):` for additive declarations, and `dep:ALIAS` to activate an optional dependency.

Inactive declarations remain parseable for formatting and editor navigation. Select another package-feature projection when inspecting another API surface; a tool should not infer the active API by scanning conditions independently.

## Diagnose a missing standard import

Inspect `incan describe-unit src/main.incn` and check its `loaves` entries against the installed artifact directories. Standard packages are ordinary Loaves compiled for the selected profile. Install the matching toolchain payload if one is missing. Dev.8 does not accept `[sdk]`, `--sdk-profile` or provider-inspection commands.

See [Standard packages](../reference/sdk_components_and_package_features.md) and [Managing dependencies](dependencies.md).

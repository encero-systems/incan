# Standard packages and package features

## Standard packages

Standard packages are compiled Loaves. Source imports use `std.*`. `INCAN_STDLIB` names a directory of standard-package source projects, each with `loaf.toml`. Artifacts for package `PACKAGE` and profile `PROFILE` reside under `INCAN_HOME/toolchains/0.6.0-dev.8/loaves/PACKAGE/PROFILE/`.

A unit can use the standard packages available in that installation. The unit's own package is excluded from its standard dependencies. Missing compiled artifacts fail compilation. No SDK profile or component selection is accepted.

## Package features

Features are declared under `[project.features]`. The accepted edge forms, defaults and refusals are specified in [Project configuration](project_configuration.md#projectfeatures).

| Flag | Contract |
| --- | --- |
| `--features A,B` | Selects named root-package features. |
| `--no-default-features` | Disables the root package's `default` feature. |
| `--all-features` | Selects every declared root-package feature. |

These flags apply to `run`, `check`, `test`, `build` and `describe-unit`. Selected features condition `when feature("NAME"):` declarations and active dependency edges. Multiple active edges union their requested dependency features.

# Scripts

Utility scripts for development and CI.

## Contents

- `generate_feature_inventory.incn`: Generates the feature inventory from strict compiler-checked codegraph registry records for `loaves/stdlib/core/src/features.incn`. Run `incan run scripts/generate_feature_inventory.incn`; set `INCAN` when the installed command is not on `PATH`.
- `check_us_english.incn`: Checks or rewrites tracked text using the explicit vocabulary in `english_spellings.incn` and exceptions in `check_us_english.allow`. Invoked by `make us-english-check` and `make us-english-fix`.
- `check_reference_contract.incn`: Checks changed reference pages against the prose rules and documented exceptions. Invoked by `make reference-contract-check`; `--all` selects the full reference inventory.
- `test_ci_scope.incn`: Tests the shell CI scope command with isolated Git responses and Unix permissions. Run `incan test scripts/test_ci_scope.incn` from the repository root. Checker tests run through `incan test scripts/test_check_us_english.incn` and `incan test scripts/test_check_reference_contract.incn`.
- `relink-driver.sh`: Rebuilds and installs a dev.8 driver through an installed Oven. `--release` optimizes the local Rust units; `--lowering` recompiles the lowering's Incan source.
- `build-native-runtime.sh`, `build-native-stdlib.sh` and `build-native-tools.sh`: Build the runtime, standard packages, and Oven, the contract runner or the LSP from source with an installed dev.8 toolchain.
- `run_examples.sh`: Runs example entry points, including async functions, and checks the entries declared in `examples/check-only.txt`. Installed Oven prepares dependencies automatically. Invoked by `make examples`; `bash scripts/test_run_examples.sh` checks mode selection, working directories and failure propagation with a fake command.
- `check_docs_examples.sh`: Typechecks committed `verified_*.incn` documentation snippets so tutorial examples cannot drift away from the compiler. Set `INCAN_DOCS_BUILD_WEB=1` only when the active toolchain provides a receipt-compatible Oven Loaf for backend verification.
- `../workspaces/docs-site/scripts/check_incapunk_components.incn`: Validates the Incus asset manifest, semantic pools, reusable component hooks, representative adoption, navigation, and accessibility contracts. Invoked by `make docs-check-components` and `make docs-build`.
- `../workspaces/docs-site/scripts/check_learning_journey.incn`: Validates the six-intent Learn hub, canonical setup sources, audience bridges, and project-tutorial contracts. Invoked by `make docs-check-learning` and `make docs-build`.

## Rebuild the dev.8 driver

Set `DEV8_LANE` to a directory containing the `incan` checkout, an installed Oven at `oven-bin/incan` and an installed toolchain home at `dev8-home`; `SELFBUILD_OVEN` and `SELFBUILD_HOME` select others. The installed toolchain supplies the pinned Rust compiler and registry units.

```sh
DEV8_LANE=/path/to/workspace scripts/relink-driver.sh
DEV8_LANE=/path/to/workspace scripts/relink-driver.sh --lowering --release
```

The default profile is debug. Add `--lowering` after editing the lowering's Incan source: it withdraws the lowering's build stamp so Oven compiles it again. Add `--release` to compile with Oven's release profile. Installation copies the driver into the toolchain under its content digest, builds its standard Rust ABI cache when that is missing, and points `toolchain.json` at both. Set `INCAN_DRIVER_INSTALL=0` to build without installing, for before/after measurements.

Set `INCAN_RELINK_TRACE=1` to timestamp each build log line and print the relink phase durations. `INCAN_RELINK_LOG` selects the build log path; otherwise the current profile's log is replaced under `bootstrap/`.

## Configuration

`run_examples.sh` respects these environment variables:

|         Variable         |                       Default                       |                  Description                   |
| ------------------------ | --------------------------------------------------- | ---------------------------------------------- |
| `INCAN_BIN`              | `incan` | Installed Incan command |
| `INCAN_EXAMPLES_TIMEOUT` | `120` | Deadline in seconds for each command; requires GNU `timeout` or `gtimeout` |
| `INCAN_EXAMPLES_ONLY` | Empty | Colon-separated repository-relative entry paths; empty selects all entries |
| `INCAN_EXAMPLES_OUT` | `target/examples` | Directory for `examples.tsv`, command logs and generated input snapshots |

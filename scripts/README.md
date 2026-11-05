# Scripts

Utility scripts for development and CI.

## Contents

- `generate_feature_inventory.incn`: Generates the feature inventory from strict compiler-checked codegraph registry records for `loaves/stdlib/core/src/features.incn`. Run `incan run scripts/generate_feature_inventory.incn`; set `INCAN` when the installed command is not on `PATH`.
- `check_us_english.incn`: Checks or rewrites tracked text using the explicit vocabulary in `english_spellings.incn` and exceptions in `check_us_english.allow`. Invoked by `make us-english-check` and `make us-english-fix`.
- `check_reference_contract.incn`: Checks changed reference pages against the prose rules and documented exceptions. Invoked by `make reference-contract-check`; `--all` selects the full reference inventory.
- `test_ci_scope.incn`: Tests the shell CI scope command with isolated Git responses and Unix permissions. Run `incan test scripts/test_ci_scope.incn` from the repository root. Checker tests run through `incan test scripts/test_check_us_english.incn` and `incan test scripts/test_check_reference_contract.incn`.
- `relink_driver.incn`: Rebuilds and installs a dev.8 driver through an installed Oven. `--release` optimizes the local Rust units; `--lowering` recompiles the lowering's Incan source.
- `oven_tests.incn`: Runs installed CI's Oven test step through an installed Oven: each Oven test source, each Loaf's test sources from that Loaf, and the record-scope contract in an isolated project that adds the frontend's native adapter. Each source runs as its own invocation, and the passing ids must equal `workspaces/contracts/dev8-oven-tests.txt`. Run `incan run scripts/oven_tests.incn -- OVEN EVIDENCE [--jobs N] [--memory GIB]`. At most N invocations run at once; without `--jobs`, `CI_DEV8_JOBS` or the core count selects N. Each invocation also reserves memory from measured peaks, 6 GiB for a compiler Loaf's tests and the record-scope contract and 4 GiB otherwise, and one starts only while the reservations fit `--memory`, else physical memory less 1 GiB; with nothing running, one always starts. OVEN must keep each test file's executable apart, under `target/incan/<profile>/tests/<file stem>/`. Invoked by `ci-dev8.sh`; `incan test scripts/test_oven_tests.incn` checks its options, invocation order, log joining and id parsing.
- `build-native-runtime.sh`, `build-native-stdlib.sh` and `build-native-tools.sh`: Build the runtime, standard packages, and Oven, the contract runner or the LSP from source with an installed dev.8 toolchain.
- `run_examples.incn`: Runs example entry points, including async functions, and checks the entries declared in `examples/check-only.txt`. Installed Oven prepares dependencies automatically. Invoked by `make examples`; `incan test scripts/test_run_examples.incn` checks discovery, mode selection, working directories and failure propagation.
- `check_docs_examples.sh`: Typechecks committed `verified_*.incn` documentation snippets so tutorial examples cannot drift away from the compiler. Set `INCAN_DOCS_BUILD_WEB=1` only when the active toolchain provides a receipt-compatible Oven Loaf for backend verification.
- `../workspaces/docs-site/scripts/check_incapunk_components.incn`: Validates the Incus asset manifest, semantic pools, reusable component hooks, representative adoption, navigation, and accessibility contracts. Invoked by `make docs-check-components` and `make docs-build`.
- `../workspaces/docs-site/scripts/check_learning_journey.incn`: Validates the six-intent Learn hub, canonical setup sources, audience bridges, and project-tutorial contracts. Invoked by `make docs-check-learning` and `make docs-build`.

## Rebuild the dev.8 driver

Set `DEV8_LANE` to a directory containing the `incan` checkout, an installed Oven at `oven-bin/incan` and an installed toolchain home at `dev8-home`; `SELFBUILD_OVEN` and `SELFBUILD_HOME` select others. The installed toolchain supplies the pinned Rust compiler and registry units.

```sh
DEV8_LANE=/path/to/workspace incan run scripts/relink_driver.incn
DEV8_LANE=/path/to/workspace incan run scripts/relink_driver.incn -- --lowering --release
```

The default profile is debug. Add `--lowering` after editing the lowering's Incan source: it withdraws the lowering's build stamp so Oven compiles it again. Add `--release` to compile with Oven's release profile. The build runs against a compiler home beside the installation (`compiler-build`) that links the installed Loaves, native artifacts and registry units and names every other installed artifact by its absolute path. Installation copies the driver into the toolchain under its content digest, builds its standard Rust ABI cache when that is missing, points `toolchain.json` at both, and removes the relinked driver and cache the manifest named before. Set `INCAN_DRIVER_INSTALL=0` to build without installing, for before/after measurements.

Set `INCAN_RELINK_TRACE=1` to print the relink phase durations. `INCAN_RELINK_LOG` selects the build log path; otherwise the current profile's log is replaced under `bootstrap/`.

## Configuration

`run_examples.sh` respects these environment variables:

|         Variable         |                       Default                       |                  Description                   |
| ------------------------ | --------------------------------------------------- | ---------------------------------------------- |
| `INCAN_BIN`              | `incan` | Installed Incan command |
| `INCAN_EXAMPLES_TIMEOUT` | `120` | Deadline in seconds for each command; requires GNU `timeout` or `gtimeout` |
| `INCAN_EXAMPLES_ONLY` | Empty | Colon-separated repository-relative entry paths; empty selects all entries |
| `INCAN_EXAMPLES_OUT` | `target/examples` | Directory for `examples.tsv`, command logs and generated input snapshots |

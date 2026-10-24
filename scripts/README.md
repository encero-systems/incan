# Scripts

Utility scripts for development and CI.

## Contents

- `check_us_english.incn`: Checks or rewrites tracked text using the explicit vocabulary in `english_spellings.incn` and exceptions in `check_us_english.allow`. Invoked by `make us-english-check` and `make us-english-fix`.
- `check_reference_contract.incn`: Checks changed reference pages against the prose rules and documented exceptions. Invoked by `make reference-contract-check`; `--all` selects the full reference inventory.
- `test_ci_scope.incn`: Tests the shell CI scope command with isolated Git responses and Unix permissions. Run `incan test scripts/test_ci_scope.incn` from the repository root. Checker tests run through `incan test scripts/test_check_us_english.incn` and `incan test scripts/test_check_reference_contract.incn`.
- `relink-driver.sh`: Rebuilds and installs a dev.8 bootstrap lane's driver through its installed Oven. `--release` optimizes the local Rust units; `--lowering` first refreshes the bootstrap lowering.
- `generated_rust_audit.py`: Emits an objective generated Rust strict-surface report for selected generated `.rs` files or artifact directories.
- `run_examples.sh`: Runs example entry points, including async functions, and checks the entries declared in `examples/check-only.txt`. Installed Oven prepares dependencies automatically. Invoked by `make examples`; `bash scripts/test_run_examples.sh` checks mode selection, working directories and failure propagation with a fake command.
- `test_inventory/collect.py` and `test_inventory/render.py`: the test corpus inventory (#1561). `collect.py` enumerates every Rust test and `.incn` fixture root with its lane signals, `--propose` writes a reviewer sidecar, `--check` is the gate; `render.py` writes `workspaces/docs-site/docs/contributing/reference/test_corpus_inventory.md` from `test_inventory/dispositions.json`. Invoked by `make test-inventory` and `make test-inventory-check`.
- `check_docs_examples.sh`: Typechecks committed `verified_*.incn` documentation snippets so tutorial examples cannot drift away from the compiler. Set `INCAN_DOCS_BUILD_WEB=1` only when the active toolchain provides a receipt-compatible Oven Loaf for backend verification. Invoked by `make smoke-test-examples`.
- `../workspaces/docs-site/scripts/check_incapunk_components.incn`: Validates the Incus asset manifest, semantic pools, reusable component hooks, representative adoption, navigation, and accessibility contracts. Invoked by `make docs-check-components` and `make docs-build`.
- `../workspaces/docs-site/scripts/check_learning_journey.incn`: Validates the six-intent Learn hub, canonical setup sources, audience bridges, and project-tutorial contracts. Invoked by `make docs-check-learning` and `make docs-build`.

## Rebuild a dev.8 bootstrap lane's driver

Set `DEV8_LANE` to an initialized lane containing its `incan` checkout, `oven-bin/incan`, `bootstrap/loaves/incan_mir_lowering`, and `dev8-home/toolchains/0.6.0-dev.8/toolchain.json`. The installed toolchain supplies the pinned Rust compiler and registry units.

```sh
DEV8_LANE=/path/to/lane scripts/relink-driver.sh
DEV8_LANE=/path/to/lane scripts/relink-driver.sh --lowering --release
```

The default profile is debug. Add `--lowering` after editing the lowering's Incan source; it uses `scripts/refresh-lowering.sh` and the lane's frozen stage-zero emitter. An unchanged emission is reused after checking the input and generated-output content digests. A changed emission preserves generated files with unchanged bytes and removes obsolete generated modules. Add `--release` to compile the local Rust units with Oven's release profile. Installation retains the selected runtime dependencies and native search paths, installs the driver under its content digest, and selects its standard Rust ABI cache. Set `INCAN_DRIVER_INSTALL=0` to build without installing, for before/after measurements.

The first relink snapshots the installed build toolchain description under `bootstrap/driver-build-home`; its artifact paths still refer to the original immutable files, without copying the toolchain. Later relinks use that pinned compiler to build Rust libraries, while installing their output driver into `dev8-home` for programs and contracts. Use `--refresh-build-toolchain` to select the currently installed compiler as the build compiler again; this deliberately invalidates Rust library reuse. Rust binaries retain their complete executable while their rustc, source, arguments, package environment and linked libraries remain unchanged.

Set `OVEN_TRACE=- INCAN_RELINK_TRACE=1` to record timestamped unit decisions and relink phase durations. `INCAN_RELINK_LOG` selects the build log path; otherwise the current profile's log is replaced under `bootstrap/`.

## Generated Rust audit helper

Run `make generated-rust-audit-gate` after changing the audit helper. The target is deterministic: it uses committed fixtures under `loaves/compiler/incan_driver/tests/fixtures/generated_rust_audit/` and does not require preexisting `target/incan/` generated artifacts.

For real generated artifacts, run `scripts/generated_rust_audit.py` with explicit `--artifact SURFACE_CLASS=PATH` entries. Repository-local paths are rendered relative to the repository root in reports.

## Configuration

`run_examples.sh` respects these environment variables:

|         Variable         |                       Default                       |                  Description                   |
| ------------------------ | --------------------------------------------------- | ---------------------------------------------- |
| `INCAN_BIN`              | `incan` | Installed Incan command |
| `INCAN_EXAMPLES_TIMEOUT` | `120` | Deadline in seconds for each command; requires GNU `timeout` or `gtimeout` |
| `INCAN_EXAMPLES_ONLY` | Empty | Colon-separated repository-relative entry paths; empty selects all entries |
| `INCAN_EXAMPLES_OUT` | `target/examples` | Directory for `examples.tsv`, command logs and generated input snapshots |

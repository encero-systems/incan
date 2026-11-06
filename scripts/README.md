# Scripts

Repository checks, release production, and development utilities are Incan programs. Release and CI logic calls these modules and functions directly where the work shares one process; installed compilers and external transport or version-control tools remain child processes.

## Release and installed verification

- `release.incn` builds and qualifies the complete native release closure from an installed seed. Run `incan run scripts/release.incn -- SHA OUT [--prove-fixed-point] [--jobs N] [--timeout SECONDS]`. The fixed-point flag rebuilds and compares DD-0010's six named artifacts; normal qualification omits it until the reproducibility stage lands.
- `native_build.incn` owns the typed native-build units used by release production and fixed-point verification.
- `verify_fixed_point.incn` builds through generation three, retains all three digest sets, and requires byte-identical generation-two and generation-three driver, runtime, lowering, standard core, Oven, and contract runner artifacts.
- `verify_installation.incn` audits and verifies an installed toolchain. Run `incan run scripts/verify_installation.incn -- OUT [--jobs N] [--timeout SECONDS]` with the installation selected by `INCAN_HOME` and its server selected by `INCAN_LSP`.
- `release_support.incn` holds shared release options, process evidence, receipts, census parsing, and baseline comparison. `incan test scripts/test_release_support.incn` proves its pure contracts.
- `publish_build_toolchain.incn` validates and pushes complete macOS and Linux bundles, then rewrites `build-toolchain.json`. Publication is a maintainer operation and is not part of qualification.
- `oven_tests.incn` runs every committed Oven test source with bounded concurrency and requires the exact ids in `workspaces/contracts/oven-tests.txt`. Run `incan run scripts/oven_tests.incn -- OVEN EVIDENCE [--jobs N] [--memory GIB]`; `incan test scripts/test_oven_tests.incn` proves its planning and evidence helpers.

The release writes `receipt.json`, `timings.tsv`, per-phase logs, `bundle/{incan,incan-lsp,toolchain.tar.zst}`, and installed-verification evidence below `ci/run.*/`. A failed phase leaves a refused receipt and its complete log.

## Rebuild the installed driver

`relink_driver.incn` rebuilds and optionally installs the native driver through an installed Oven. The installed support closure is release-only, so release qualification uses `--release`:

```sh
DEV8_LANE=/path/to/workspace SELFBUILD_HOME=/path/to/home SELFBUILD_OVEN=/path/to/home/bin/incan SELFBUILD_STDLIB=/path/to/checkout/loaves/stdlib INCAN_RELINK_LOG=/path/to/relink.log incan run scripts/relink_driver.incn -- --release
```

Add `--lowering` when the lowering source must be rebuilt. Set `INCAN_DRIVER_INSTALL=0` for a build-only measurement. `INCAN_RELINK_TRACE=1` prints phase durations; `INCAN_RELINK_LOG` selects the retained build log.

## Repository checks

- `run_examples.incn` discovers and runs repository examples in their declared mode. `make examples` invokes it; `incan test scripts/test_run_examples.incn` proves discovery and failure propagation.
- `check_docs_examples.incn` checks every verified documentation snippet and source-backed tutorial, then requires canonical formatting. `make docs` invokes it. Pass `--build-web` only with a receipt-compatible Oven Loaf; `--offline` makes that backend build resolve offline.
- `check_us_english.incn`, `check_changed_rustdocs.incn`, `check_agents_doc_sync.incn`, `check_doc_paths.incn`, and `check_reference_contract.incn` implement the local pre-commit checks.
- `generate_feature_inventory.incn` regenerates the compiler-checked feature inventory from the registry records in `loaves/stdlib/core/src/features.incn`.
- `bench_build_times.incn` compares cold, warm, and observable incremental builds across installed toolchains. Run `incan run scripts/bench_build_times.incn -- --toolchain LABEL=PATH [--toolchain LABEL=PATH ...] [--project PATH] [--repetitions N] [--output PATH] [--scratch PATH] [--keep-work] [--warm-max-ms N]`. Incremental timings count only when running the result proves that the edit reached the binary.

The docs site also owns `workspaces/docs-site/scripts/check_incapunk_components.incn` and `check_learning_journey.incn`; its strict build invokes those checks.

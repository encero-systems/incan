# `incan-cli`

Ring: **toolchain**

The `incan` binary: clap surface, terminal rendering, exit codes, the test runner, and the `generate_feature_inventory` tool that renders the CLI's documented feature surface. The library target `incan_cli` exists for those two binaries; nothing else depends on it.

## Depends on

`compiler` (every crate, including `incan_oven_facet`), `oven`, `kernel`. It links no standard-library facet: the runtime is the generated program's, not the command line's.

## Tests

`loaves/toolchain/incan-cli/tests/` holds the roots that exercise the command line as a whole — the `cli_*` surfaces, `integration_tests`, the RFC 031 package roots, the installer tests, the layering and vocabulary guardrails — and the fixtures only they use. Fixtures several rings share live with the harness crate, under `loaves/compiler/incan_test_support/fixtures/` (`incan_test_support::fixture("…")`). Run one with `cargo test -p incan-cli --test <root>`; the Oven compiler suite runs them all.

Should be small. If a command body grows, it belongs in the driver.

# `incan_test_support`

Ring: **compiler** (a development dependency; `publish = false`)

The harness every integration-test root shares: the checkout anchors (`repo_root()`, `repo_command()`), the compiler subprocess helpers, fixture builders for packages and CLI projects, canonical-projection and emitted-symbol readers, and the builtin-stdlib probes. It links ring crates only; no root ever spells a checkout-relative path itself.

## `fixtures/`

The fixtures several rings' roots share — `valid/` and `invalid/` Incan programs, the boundary-parity families, the standalone regression programs, `test_assert_canary.incn` (the prewarm's smoke input), and the Oven bake projects the Makefile's release-smoke lane and CI's evidence lanes copy into scratch directories (`oven_loaf_dependencies`, whose `Cargo.lock` is in every CI source-cache key; `oven_project_bake`; `oven_release_app_bake`; `oven_release_bytes_io`; `oven_release_file_lock`). Read one with `incan_test_support::fixture("valid/class_with_trait.incn")`; the directory is `fixtures_dir()`.

`behavior/` is the behavior-fixture family of the #1561 test-corpus cutover: Incan programs whose leading comment block declares the observables a run must show, discovered and run by `behavior_fixtures` from one root per area under `loaves/toolchain/incan-cli/tests/`; its `README.md` describes the format.

A fixture only one ring's roots use lives beside those roots (`loaves/compiler/incan_driver/tests/fixtures/`, `loaves/compiler/incan_emit/tests/codegen_snapshots/`, `loaves/toolchain/incan-cli/tests/fixtures/`), not here.

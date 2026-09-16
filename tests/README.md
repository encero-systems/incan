# Tests

Integration roots live in the package they exercise, under that package's `tests/` directory; unit tests live inline in the source files they test (`#[cfg(test)]` modules). The roots still in this directory are the ones that exercise the `incan` command line as a whole; they move with the CLI when it becomes `loaves/toolchain/incan-cli`.

## Where the roots live

| Ring package | What its roots prove | How to run |
| --- | --- | --- |
| `loaves/compiler/incan_emit/tests/` | Codegen snapshots (`codegen_snapshots/` inputs, `snapshots/` insta goldens), lowering, ownership and construction diagnostics | `cargo test -p incan_emit --test codegen_snapshot_tests`; `INSTA_UPDATE=1` to regenerate |
| `loaves/compiler/incan_driver/tests/` | The parity corpus (`support/parity_corpus.rs`), the replacement-backend proofs and their shadow comparisons, generated-Rust artifact, audit, callability and native-consumer tests, protected bindings, the generated cache | `cargo test -p incan_driver --test <root>` with a CLI built from the tree |
| `loaves/compiler/incan_frontend/tests/` | Checked identities, declaration identity and semantic digests, semantic-core parity, stdlib module traits | `cargo test -p incan_frontend --test <root>` |
| `loaves/compiler/incan_provider/tests/` | The stdlib effect digest | `cargo test -p incan_provider --test stdlib_effect_digest` |
| `loaves/compiler/incan_format/tests/` | Formatter properties (`property_tests.rs`, with its proptest regressions file) | `cargo test -p incan_format --test property_tests` |
| `loaves/compiler/incan_oven_facet/tests/` | Bounded Oven process-containment regressions | `make test-oven-pr-regressions` |
| `loaves/compiler/incan_test_support/` | The harness every root shares: checkout anchors, the compiler subprocess, fixture builders, artifact readers | a dev-dependency, not a root |

Every root also runs through the Oven compiler suite (`make test`, or one root with `make test-one TEST_ROOT=<path>`), which is the authority for anything that bakes or launches the compiler.

## Still here

`cli_*.rs`, `integration_tests.rs`, `rfc031_pub_import_integration_tests.rs`, `rfc081_embedded_conformance.rs`, `canonical_item_imports.rs`, `package_boundary_facade_tests.rs`, `package_executable_representation.rs`, `script_target_diagnostics.rs`, `std_encoding_algorithm_modules.rs`, `example_capability_coverage.rs`, `repository_path_tests.rs`, `toolchain_installer_tests.rs`, and the guardrails `layering_guard.rs`, `cli_layering_guardrails.rs`, `vocab_guardrails.rs`. Run one with `cargo test --test <root>`.

### `fixtures/`

Incan sources the roots here share: `fixtures/valid/` (programs that compile), `fixtures/invalid/` (programs that produce specific diagnostics), the `oven_*` bake fixtures the Makefile and CI evidence lanes use, and the fixture directories of the roots above. A fixture used by one ring's roots alone lives beside them.

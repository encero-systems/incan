# Tests

Integration roots live in the package they exercise, under that package's `tests/` directory; unit tests live inline in the source files they test (`#[cfg(test)]` modules). One root is still in this directory; the rest live in their rings, the command-line roots with the `incan` binary in `loaves/toolchain/incan-cli`.

## Where the roots live

| Ring package | What its roots prove | How to run |
| --- | --- | --- |
| `loaves/compiler/incan_emit/tests/` | Codegen snapshots (`codegen_snapshots/` inputs, `snapshots/` insta goldens), lowering, ownership and construction diagnostics | `cargo test -p incan_emit --test codegen_snapshot_tests`; `INSTA_UPDATE=1` to regenerate |
| `loaves/compiler/incan_driver/tests/` | The parity corpus (`support/parity_corpus.rs`), the replacement-backend proofs and their shadow comparisons, generated-Rust artifact, audit, callability and native-consumer tests, protected bindings, the generated cache | `cargo test -p incan_driver --test <root>` with a CLI built from the tree |
| `loaves/compiler/incan_frontend/tests/` | Checked identities, declaration identity and semantic digests, semantic-core parity, stdlib module traits | `cargo test -p incan_frontend --test <root>` |
| `loaves/compiler/incan_provider/tests/` | The stdlib effect digest | `cargo test -p incan_provider --test stdlib_effect_digest` |
| `loaves/compiler/incan_format/tests/` | Formatter properties (`property_tests.rs`, with its proptest regressions file) | `cargo test -p incan_format --test property_tests` |
| `loaves/compiler/incan_oven_facet/tests/` | Bounded Oven process-containment regressions | `make test-oven-pr-regressions` |
| `loaves/toolchain/incan-cli/tests/` | The command line as a whole: the `cli_*` surfaces, `integration_tests`, the RFC 031 package roots, the installer tests, the layering and vocabulary guardrails | `cargo test -p incan-cli --test <root>` |
| `loaves/compiler/incan_test_support/` | The harness every root shares: checkout anchors, the compiler subprocess, fixture builders, artifact readers | a dev-dependency, not a root |

Every root also runs through the Oven compiler suite (`make test`, or one root with `make test-one TEST_ROOT=<path>`), which is the authority for anything that bakes or launches the compiler.

## Still here

`rfc081_embedded_conformance.rs` alone: it reaches the language server (`incan::lsp`), which is still the root package's feature until the LSP has a package of its own. Run it with `cargo test --test rfc081_embedded_conformance --features lsp`.

The command-line roots — the `cli_*` surfaces, `integration_tests`, the RFC 031 package roots, the installer tests, the layering and vocabulary guardrails — live with the `incan` binary in `loaves/toolchain/incan-cli/tests/`; run one with `cargo test -p incan-cli --test <root>`.

### `fixtures/`

Incan sources the roots here share: `fixtures/valid/` (programs that compile), `fixtures/invalid/` (programs that produce specific diagnostics), the `oven_*` bake fixtures the Makefile and CI evidence lanes use, and the fixture directories of the roots above. A fixture used by one ring's roots alone lives beside them.

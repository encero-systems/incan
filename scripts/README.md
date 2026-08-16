# Scripts

Utility scripts for development and CI.

## Contents

- `generated_rust_audit.py`: Emits an objective generated Rust strict-surface report for selected generated `.rs` files or artifact directories.
- `run_examples.sh`: Smoke-tests all examples. It pre-builds nested example library projects, typechecks every `.incn` file under `examples/`, and then runs files that define `def main(...)` with a configurable timeout. Invoked by `make examples`.
- `check_docs_examples.sh`: Typechecks committed `verified_*.incn` documentation snippets so tutorial examples cannot drift away from the compiler. Set `INCAN_DOCS_BUILD_WEB=1` only when the active toolchain provides a receipt-compatible Oven Loaf for backend verification. Invoked by `make smoke-test-examples`.
- `../workspaces/docs-site/scripts/check_incapunk_components.py`: Validates the Incus asset manifest, semantic pools, reusable component hooks, representative adoption, navigation, and accessibility contracts. Invoked by `make docs-check-components` and `make docs-build`.
- `../workspaces/docs-site/scripts/check_learning_journey.py`: Validates the six-intent Learn hub, canonical setup sources, audience bridges, and project-tutorial contracts. Invoked by `make docs-check-learning` and `make docs-build`.

## Generated Rust audit helper

Run `make generated-rust-audit-gate` after changing the audit helper. The target is deterministic: it uses committed fixtures under `tests/fixtures/generated_rust_audit/` and does not require preexisting `target/incan/` generated artifacts.

For real generated artifacts, run `scripts/generated_rust_audit.py` with explicit `--artifact SURFACE_CLASS=PATH` entries. Repository-local paths are rendered relative to the repository root in reports.

## Configuration

`run_examples.sh` respects these environment variables:

|         Variable         |                       Default                       |                  Description                   |
| ------------------------ | --------------------------------------------------- | ---------------------------------------------- |
| `INCAN_BIN`              | `./target/release/incan` (if present), else `incan` | Path to the Incan binary                       |
| `INCAN_EXAMPLES_TIMEOUT` | `30`                                                | Per-example timeout in seconds for `incan run` |

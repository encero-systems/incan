# Scripts

Utility scripts for development and CI.

## Contents

- `check_release_surface.py`: Checks that the active 0.4 release surface is represented in release notes, docs, generated feature inventory, and named regression tests.
- `generated_rust_audit.py`: Emits an objective generated Rust strict-surface report for selected generated `.rs` files or artifact directories.
- `run_examples.sh`: Smoke-tests all examples. It pre-builds nested example library projects, typechecks every `.incn` file under `examples/`, and then runs files that define `def main(...)` with a configurable timeout. Invoked by `make examples`.

## Release surface gate

Run `make release-0-4-surface-gate` after adding or renaming public 0.4 release surfaces, moving the relevant docs, or renaming regression tests that prove one of the staged slices. The script does not replace behavioral coverage; it verifies that release notes, user/contributor docs, generated feature inventory, and representative tests still point at the same release story.

## Generated Rust audit helper

Run `make generated-rust-audit-gate` after changing the audit helper. The target is deterministic: it uses committed fixtures under `tests/fixtures/generated_rust_audit/` and does not require preexisting `target/incan/` generated artifacts.

For real generated artifacts, run `scripts/generated_rust_audit.py` with explicit `--artifact SURFACE_CLASS=PATH` entries. Repository-local paths are rendered relative to the repository root in reports.

## Configuration

`run_examples.sh` respects these environment variables:

|         Variable         |                       Default                       |                  Description                   |
| ------------------------ | --------------------------------------------------- | ---------------------------------------------- |
| `INCAN_BIN`              | `./target/release/incan` (if present), else `incan` | Path to the Incan binary                       |
| `INCAN_EXAMPLES_TIMEOUT` | `30`                                                | Per-example timeout in seconds for `incan run` |

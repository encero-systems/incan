# Rust-source backend migration notes and freeze

This page states the migration note a change to the Rust-source backend carries, the semantic owners a note names, the freeze of the emission tree, and the guardrails for backend boundaries. The deprecation policy these serve is explained in [Rust-source backend deprecation](../explanation/rust_source_backend_deprecation.md); reviewing a backend change is described in [Auditing generated Rust](../how-to/auditing_generated_rust.md#review-a-backend-change).

## Migration note

A compatibility fix in the Rust-source backend (Rust-source lowering and emission) that adds or preserves behavior belonging to the middle end carries a migration note with these fields:

| Field | Required content | `--record` option | Manifest key |
| --- | --- | --- | --- |
| Compatibility issue | The bug or release issue that needs the old backend fix. | `--issue` | `compatibility_issue` |
| Behavior evidence | The test, snapshot or downstream lane proving the behavior. | `--evidence` | `behavior_evidence` |
| Semantic owner | The owner the behavior moves to: stable IDs, semantic facts, `IncanType`, HIR, Body IR, ABI metadata, runtime-service facts, diagnostics, or package metadata (see [Semantic owners](#semantic-owners)). | `--owner` | `semantic_owner` |
| Retirement condition | What lets this compatibility path disappear or become a thin adapter. | `--retirement` | `retirement_condition` |

Where the note is written:

- For the emission tree (every `.rs` file under `loaves/compiler/incan_emit/src/emit/`), the note is the manifest `change` entry that `python3 scripts/check_emitter_freeze.py --record --issue <issue> --evidence <evidence> --owner <owner> --retirement <condition>` writes. Every modification of the tree carries one, and so does every deletion under the `frozen` policy (see [Emission tree freeze](#emission-tree-freeze)).
- For any other change to the Rust-source backend, the note is written in the code comment, test name, issue note or pull request text of the change.

## Semantic owners

| A change that needs to know... | Owner |
| --- | --- |
| Declaration, expression, statement, local, or type identity | Stable compiler IDs and semantic facts. |
| Source-level type meaning independent of Rust spelling | `IncanType` or the backend-neutral semantic type model. |
| Normalized typed program shape | HIR v0 and semantic module snapshots. |
| Ownership, borrow, move, clone, drop, or call argument use | Duckborrower facts and Body IR. |
| Runtime helper, target, allocator, panic, or service requirement | ABI v0 hooks and runtime-service metadata. |
| User-facing expected/actual facts | Diagnostics metadata and schema. |
| Generated project layout, Cargo manifest shape, or artifact reports | Backend preparation and artifact plan. |
| Public import, reexport, package, or checked API identity | Package metadata and checked API facts. |

## Emission tree freeze

The emission tree, every `.rs` file under `loaves/compiler/incan_emit/src/emit/` (recursively), is frozen against the fingerprint manifest `loaves/compiler/incan_emit/tests/fixtures/emitter_freeze/manifest.json`. `scripts/check_emitter_freeze.py` compares the tree with the manifest in `make pre-commit-fast`, in CI and in `cargo test -p incan_emit`. A file's fingerprint is its sha256 digest and line count, with `\r\n` read as `\n`.

### Policies

The manifest's `policy` decides what drift the gate admits; the committed manifest carries `frozen`.

| Policy | Added file | Modified file | Deleted file |
| --- | --- | --- | --- |
| `frozen` | Refused. | Admitted with a recorded `change` entry. | Admitted with a recorded `change` entry. |
| `deletions-only` | Refused. | Admitted with a recorded `change` entry. | Admitted with a `deletion` entry that carries no migration note. |

### Commands

| Command | Effect |
| --- | --- |
| `python3 scripts/check_emitter_freeze.py` | Checks the tree against the manifest under its policy. |
| `python3 scripts/check_emitter_freeze.py --record --issue <issue> --evidence <evidence> --owner <owner> --retirement <condition>` | Rewrites the fingerprints for the current tree and appends one `change` entry carrying the four migration-note fields. `--pr <number>` records the pull request; `--policy <policy>` sets the policy in force after the change. |
| `python3 scripts/check_emitter_freeze.py --record --prune-deletions` | Drops every missing file from the manifest and appends one `deletion` entry, which carries no migration note; `--pr <number>` records the pull request. A modified file keeps its recorded fingerprint. |

`--root <path>` and `--manifest <path>` select the repository root and the manifest for any command.

`--record` is refused when a migration-note field is missing or blank, when the tree gained a file, under `deletions-only` while the manifest lists a deleted file, and when the tree matches the manifest and the policy after the change equals the one in force. `--record --prune-deletions` is refused under `frozen`, when the tree gained a file, and when every file the manifest lists exists. `--prune-deletions` together with `--policy` or a migration-note option, `--pr` with a value below 1, and `--prune-deletions`, `--policy`, `--pr` or a migration-note option without `--record` are usage errors.

Exit status: `0` when the tree matches the manifest under its policy, `1` on drift or a refused record, `2` on a malformed manifest or a usage error.

### Manifest schema

The manifest is a JSON object with exactly the keys `tree`, `policy`, `files` and `changes`, in that order.

| Key | Content |
| --- | --- |
| `tree` | The frozen tree, `loaves/compiler/incan_emit/src/emit`. |
| `policy` | `frozen` or `deletions-only`; equal to the `policy` of the last `changes` entry. |
| `files` | One object per frozen file, with the keys `path`, `sha256` (lowercase hex) and `lines`, sorted by `path`, each path listed once. |
| `changes` | The recorded entries, oldest first. |

Each `changes` entry has the keys `kind`, `pr`, `policy` and `files`:

| Key | Content |
| --- | --- |
| `kind` | `change` or `deletion`. |
| `pr` | A positive pull request number, or `null`. |
| `policy` | The policy in force after the entry; `deletions-only` in a `deletion` entry. |
| `files` | One object per covered file, with the keys `path` and `change` (`modified` or `deleted`); a `deletion` entry lists at least one file, each `deleted`. |

A `change` entry also carries the four migration-note keys `compatibility_issue`, `behavior_evidence`, `semantic_owner` and `retirement_condition`, each a non-empty string. A `deletion` entry carries none of them.

## Guardrails

| Boundary | Guardrail |
| --- | --- |
| Stringly semantic checks in compiler code | `loaves/toolchain/incan-cli/tests/vocab_guardrails.rs` and `loaves/toolchain/incan-cli/tests/fixtures/vocab_guardrails/semantic_string_audit.json`. |
| The frozen emission tree | `scripts/check_emitter_freeze.py` and `loaves/compiler/incan_emit/tests/fixtures/emitter_freeze/manifest.json`. |
| Import/package/facade identity | `loaves/compiler/incan_test_support/fixtures/boundary_parity/README.md` and its fixture families. |
| Generated Rust public library artifacts | `loaves/compiler/incan_driver/tests/generated_rust_artifact_tests.rs`, `loaves/compiler/incan_driver/tests/generated_rust_callability_artifact_tests.rs`, and `loaves/compiler/incan_driver/tests/generated_rust_native_consumer_tests.rs`. |
| Stdlib generated-Rust coverage | [Generated Rust stdlib coverage inventory](generated_rust_stdlib_coverage.md) and `loaves/compiler/incan_emit/tests/stdlib_generated_rust_snapshot_tests.rs`. |
| Rust interop call/coercion behavior | Focused `loaves/compiler/incan_emit/tests/codegen_snapshots/rfc041_*`, `loaves/compiler/incan_emit/tests/codegen_snapshots/rfc043_*`, and `loaves/compiler/incan_emit/tests/codegen_snapshots/rust_interop_*` fixtures. |

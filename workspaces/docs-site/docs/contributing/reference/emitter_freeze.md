# Emitter freeze

The Rust-emission backend is frozen at the end of v0.6 slice 6 ([#1561](https://github.com/encero-systems/incan/issues/1561)) so that slice 7 can measure replacement-route parity against generated Rust that does not move. The freeze is a mechanical gate: `scripts/check_emitter_freeze.py` fingerprints the frozen tree and compares it with a checked manifest under the policy the manifest declares.

## Frozen path set

| Item | Value |
| --- | --- |
| Tree | `loaves/compiler/incan_emit/src/emit/` |
| Files | every `.rs` file under the tree, recursively (`loaves/compiler/incan_emit/src/emit/**/*.rs`) |
| Not fingerprinted | files with any other extension, and everything outside the tree, including `loaves/compiler/incan_emit/src/codegen.rs` and `loaves/compiler/incan_emit/src/conversions.rs` |
| Enumeration | the `files` array of the manifest lists the frozen set; the gate does not read a separate allowlist |
| Fingerprint | the file's sha256 over its bytes, and its line count (a final unterminated line counts as a line) |

## Policy states

The manifest's top-level `policy` selects the rule set. Under both policies the check compares the tree with the manifest and reports every addition, modification and deletion; the policy decides which of those fail.

| Policy | Addition | Modification | Deletion | In force |
| --- | --- | --- | --- | --- |
| `frozen` | fails; `--record` refuses it | fails; recordable with a migration note | fails; recordable with a migration note | from the close of slice 6 |
| `deletions-only` | fails; `--record` refuses it | fails; recordable with a migration note | passes; reported as permitted | from the first day of slice 7, set by `--record --policy deletions-only` |

An addition is never recordable under either policy; the two admissible routes are an existing file under the tree or a fix outside the emitter.

## Manifest

Path: `loaves/compiler/incan_emit/tests/fixtures/emitter_freeze/manifest.json`. `--record` writes it in one canonical form (this key order, files sorted by path, two-space indentation, a final newline); a hand edit that breaks the schema fails the gate with exit status 2.

| Key | Type | Content |
| --- | --- | --- |
| `tree` | string | the frozen tree, repository-relative, no trailing slash: `loaves/compiler/incan_emit/src/emit` |
| `policy` | string | `frozen` or `deletions-only` |
| `files` | array | one entry per frozen file, sorted by `path`, no duplicates |
| `files[].path` | string | repository-relative POSIX path of a `.rs` file under `tree` |
| `files[].sha256` | string | lowercase hex sha256 of the file's bytes |
| `files[].lines` | integer | line count, at least 0 |
| `changes` | array | one entry per recorded change, in the order recorded; empty at the freeze |
| `changes[].pr` | integer or null | the pull request carrying the change; `null` until the number is known |
| `changes[].policy` | string | the policy in force after this change; the last entry's value equals the top-level `policy` |
| `changes[].files` | array | the files this change fingerprinted, each an object with `path` (as in `files[].path`) and `change` (`modified` or `deleted`); empty for a policy flip alone |
| `changes[].compatibility_issue` | string | migration note: the bug or release issue that needs the emitter change |
| `changes[].behavior_evidence` | string | migration note: the test, snapshot or downstream lane proving the behavior |
| `changes[].semantic_owner` | string | migration note: the future owner of the decision (stable IDs, semantic facts, `IncanType`, HIR, Body IR, ABI metadata, runtime-service facts, diagnostics or package metadata) |
| `changes[].retirement_condition` | string | migration note: what lets this emitter path disappear or become a thin adapter |

The four migration-note fields are the template of the [Rust-source backend deprecation policy](rust_source_backend_deprecation.md#allowed-old-backend-compatibility-fixes). Every one of them must be a non-blank string.

## Checker

Path: `scripts/check_emitter_freeze.py` (Python 3, standard library only). Run it from the repository root.

### Modes

| Invocation | Effect |
| --- | --- |
| `python3 scripts/check_emitter_freeze.py` | Check: fingerprint the tree, compare with the manifest, apply the policy. Prints one line per drift with the rule it breaks, or `permitted under <policy>` for a deletion under `deletions-only`, followed by the exact `--record` invocation to run next. |
| `python3 scripts/check_emitter_freeze.py --record --issue ... --evidence ... --owner ... --retirement ... [--pr N] [--policy P]` | Record: rewrite every fingerprint for the current tree and append one `changes` entry. Refuses when a note field is missing or blank, when the tree gained a file, or when the tree matches the manifest and the policy is unchanged. The manifest is not written on a refusal. |
| `... --record --policy deletions-only ...` | Policy flip, recorded as a change with its own migration note. A flip to the policy already in force is refused. |

### Options

| Option | Applies to | Meaning |
| --- | --- | --- |
| `--root PATH` | both | repository root; defaults to the checkout containing the script |
| `--manifest PATH` | both | manifest to check or rewrite; defaults to the committed manifest under `--root` |
| `--record` | record | switch from check to record |
| `--policy {frozen,deletions-only}` | record | the policy in force after this change |
| `--issue TEXT` | record | `compatibility_issue` |
| `--evidence TEXT` | record | `behavior_evidence` |
| `--owner TEXT` | record | `semantic_owner` |
| `--retirement TEXT` | record | `retirement_condition` |
| `--pr N` | record | `pr`; a positive integer, `null` when omitted |

`--policy`, `--pr` and the four note options are rejected without `--record`.

### Exit status

| Status | Meaning |
| --- | --- |
| 0 | check: the tree matches the manifest under its policy; record: the manifest was rewritten |
| 1 | check: at least one drift breaks the policy; record: refused |
| 2 | the manifest is missing, unreadable, invalid JSON, or breaks the schema above; or the options are misused |

### Where it runs

| Entry point | Runs |
| --- | --- |
| `make emitter-freeze` | the checker's unit tests (`scripts/test_check_emitter_freeze.py`) and then the check |
| `make emitter-freeze-ci` | the check alone |
| `make pre-commit-fast` | `emitter-freeze-ci`, after the rustdoc gate |
| CI job `Rustdoc Gate` (`.github/workflows/ci.yml`), step `Check the emitter freeze` | the unit tests and the check |
| `cargo test -p incan_emit --test emitter_freeze_tests` (`loaves/compiler/incan_emit/tests/emitter_freeze_tests.rs`) | the check, and a refused record against a scratch copy of the manifest |

## Change rule

- A change under `loaves/compiler/incan_emit/src/emit/**` is recorded in the same pull request with `--record`, carrying the four migration-note fields; the entry is the migration note the deprecation policy requires, and the gate refuses a record without it.
- A bug rooted in the middle end (registry, checker, Body IR, ownership, call or type facts) is fixed there; the emitter only consumes the recorded fact and needs no entry. A bug rooted in the generated Rust itself is fixed in the emitter with an entry, and that fix is expected to be deleted with the cutover rather than extended.
- Under `deletions-only`, a deleted file passes the check without an entry. Recording the deletion, so that the manifest lists only files that exist, still carries the four fields.
- From slice 7, a deletion under the tree additionally requires a twin: a keep or re-point test, named in the test corpus inventory, covering the behavior the deleted emitter code asserted. That rule is enforced by the test corpus inventory gate, documented on its own reference page, not by this checker.

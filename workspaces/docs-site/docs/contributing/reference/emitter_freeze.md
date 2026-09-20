# Emitter freeze

The Rust-emission backend is frozen at the end of v0.6 slice 6 ([#1561](https://github.com/encero-systems/incan/issues/1561)) so that slice 7 can measure replacement-route parity against generated Rust that does not move. The freeze is a mechanical gate: `scripts/check_emitter_freeze.py` fingerprints the frozen tree and compares it with a checked manifest under the policy the manifest declares.

## Frozen path set

| Item | Value |
| --- | --- |
| Tree | `loaves/compiler/incan_emit/src/emit/` |
| Files | every `.rs` file under the tree, recursively (`loaves/compiler/incan_emit/src/emit/**/*.rs`) |
| Not fingerprinted | files with any other extension, and everything outside the tree, including `loaves/compiler/incan_emit/src/codegen.rs` and `loaves/compiler/incan_emit/src/conversions.rs` |
| Enumeration | the `files` array of the manifest lists the frozen set; the gate does not read a separate allowlist |
| Fingerprint | the file's sha256 over its bytes with `\r\n` read as `\n`, and its line count (a final unterminated line counts as a line); a CRLF checkout fingerprints the same as an LF one |

Only `.rs` files are fingerprinted because only they reach the compiler: cargo compiles nothing else under the tree, an `include!` of another file would itself modify a fingerprinted `.rs` file, and hashing every file would trip on editor and operating-system droppings.

## Policy states

The manifest's top-level `policy` selects the rule set. Under both policies the check compares the tree with the manifest and reports every addition, modification and deletion; the policy decides which of those fail.

| Policy | Addition | Modification | Deletion | In force |
| --- | --- | --- | --- | --- |
| `frozen` | fails; `--record` refuses it | fails; recordable with a migration note | fails; recordable with a migration note | from the close of slice 6 |
| `deletions-only` | fails; `--record` refuses it | fails; recordable with a migration note | fails while the manifest still lists the file; `--record --prune-deletions` drops it without a note | from the first day of slice 7, set by `--record --policy deletions-only` |

An addition is never recordable under either policy; the two admissible routes are an existing file under the tree or a fix outside the emitter. The flip is reversible by design: switching back to `frozen` is an ordinary recorded change, `--record --policy frozen` with its migration note.

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
| `changes` | array | one entry per recorded change or prune, in the order recorded; empty at the freeze |
| `changes[].kind` | string | `change` (written by `--record` with a migration note) or `deletion` (written by `--record --prune-deletions`, no note) |
| `changes[].pr` | integer or null | the pull request carrying the change; `null` until the number is known |
| `changes[].policy` | string | the policy in force after this change; the last entry's value equals the top-level `policy`; always `deletions-only` for a `deletion` entry |
| `changes[].files` | array | the files this entry covers, each an object with `path` (as in `files[].path`) and `change` (`modified` or `deleted`); empty for a policy flip alone; at least one file, every one `deleted`, for a `deletion` entry |
| `changes[].compatibility_issue` | string | migration note: the bug or release issue that needs the emitter change |
| `changes[].behavior_evidence` | string | migration note: the test, snapshot or downstream lane proving the behavior |
| `changes[].semantic_owner` | string | migration note: the future owner of the decision (stable IDs, semantic facts, `IncanType`, HIR, Body IR, ABI metadata, runtime-service facts, diagnostics or package metadata) |
| `changes[].retirement_condition` | string | migration note: what lets this emitter path disappear or become a thin adapter |

The four migration-note fields are the template of the [Rust-source backend deprecation policy](rust_source_backend_deprecation.md#allowed-old-backend-compatibility-fixes). On a `change` entry every one of them must be a non-blank string; a `deletion` entry carries none of them (the canonical form omits the keys, and a `null` value is accepted and dropped on the next write).

## Checker

Path: `scripts/check_emitter_freeze.py` (Python 3, standard library only). The checker resolves the checkout from its own location, so it runs from any working directory; the `--record` and `--prune-deletions` hints it prints are relative to the repository root and repeat the `--root` and `--manifest` the check ran with.

### Modes

| Invocation | Effect |
| --- | --- |
| `python3 scripts/check_emitter_freeze.py` | Check: fingerprint the tree, compare with the manifest, apply the policy. Prints one line per drift with the rule it breaks, followed by the command that resolves it: the `--prune-deletions` invocation for a deletion under `deletions-only`, the `--record` invocation with a placeholder per note field for a modification (or a deletion under `frozen`), and the two admissible routes for an addition. Any drift is exit status 1. |
| `python3 scripts/check_emitter_freeze.py --record --issue ... --evidence ... --owner ... --retirement ... [--pr N] [--policy P]` | Record: rewrite every fingerprint for the current tree and append one `change` entry. Refuses when a note field is missing or blank, when the tree gained a file, when a deletion under `deletions-only` is still unpruned, or when the tree matches the manifest and the policy is unchanged. The manifest is not written on a refusal. |
| `python3 scripts/check_emitter_freeze.py --record --prune-deletions [--pr N]` | Prune: drop every missing file from `files` and append one `deletion` entry listing them, with no migration note. Refuses under `frozen`, when the tree gained a file, and when every listed file exists. A modified file keeps its recorded fingerprint, so it still needs its own `--record` afterwards. |
| `... --record --policy deletions-only ...` | Policy flip, recorded as a change with its own migration note. A flip to the policy already in force is refused. |

### Options

| Option | Applies to | Meaning |
| --- | --- | --- |
| `--root PATH` | both | repository root; defaults to the checkout containing the script |
| `--manifest PATH` | both | manifest to check or rewrite; defaults to the committed manifest under `--root` |
| `--record` | record | switch from check to record |
| `--prune-deletions` | record | with `--record`: prune instead of recording a change; takes no `--policy` and no note option |
| `--policy {frozen,deletions-only}` | record | the policy in force after this change |
| `--issue TEXT` | record | `compatibility_issue` |
| `--evidence TEXT` | record | `behavior_evidence` |
| `--owner TEXT` | record | `semantic_owner` |
| `--retirement TEXT` | record | `retirement_condition` |
| `--pr N` | record | `pr`; a positive integer, `null` when omitted |

`--prune-deletions`, `--policy`, `--pr` and the four note options are rejected without `--record`; `--policy` and the note options are rejected with `--prune-deletions`.

### Exit status

| Status | Meaning |
| --- | --- |
| 0 | check: the tree matches the manifest under its policy; record: the manifest was rewritten |
| 1 | check: the tree drifted from the manifest; record or prune: refused |
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
- The manifest only ever lists files that exist, so a path that was deleted cannot come back as a "modification". Under `deletions-only`, a deleted file needs no migration note but the check fails while the manifest still lists it, naming the prune: `python3 scripts/check_emitter_freeze.py --record --prune-deletions [--pr N]` drops every missing file and appends a `deletion` entry. A `--record` with a note refuses while a deletion is pending, so a note's `files` never sweeps up an unrelated deletion; prune first, then record. Under `frozen`, a deletion is a recorded change like any other: it carries the four fields in a `change` entry, and the prune is refused. A file re-created at a pruned path is an addition, refused by check, record and prune alike.
- This checker passes a pruned deletion under `deletions-only` without consulting any inventory. The rule that a retire-class test's deletion needs a named twin (a keep or re-point test covering the same behavior) belongs to the test-corpus inventory that the "Test corpus" section of [#1561](https://github.com/encero-systems/incan/issues/1561) describes, and is not enforced by this checker.

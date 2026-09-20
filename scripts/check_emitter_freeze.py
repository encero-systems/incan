#!/usr/bin/env python3
"""Fail when the frozen Rust-emission tree drifts from its fingerprint manifest without a migration note.

The Rust-emission backend, every `.rs` file under `loaves/compiler/incan_emit/src/emit/`, is frozen at the end of
v0.6 slice 6 (#1561). This gate fingerprints that tree (sha256 and line count per file), compares it with
`loaves/compiler/incan_emit/tests/fixtures/emitter_freeze/manifest.json`, and applies the manifest's `policy`:

- `frozen`: no additions; a modification or a deletion needs a migration note.
- `deletions-only`: deletions pass without a note; a modification needs a migration note; no additions.

`--record` rewrites the fingerprints for the current tree and appends one change entry carrying the four
migration-note fields of the Rust-source backend deprecation policy (compatibility issue, behavior evidence, semantic
owner, retirement condition). It refuses when any field is missing, when there is nothing to record, or when the
tree gained a file. `--policy` flips the policy as part of a recorded change.

Exit status: 0 when the tree matches the manifest under its policy, 1 on drift or a refused record, 2 on a malformed
manifest or a usage error. The contributor reference is
`workspaces/docs-site/docs/contributing/reference/emitter_freeze.md`.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MANIFEST_PATH = "loaves/compiler/incan_emit/tests/fixtures/emitter_freeze/manifest.json"
REFERENCE_PAGE = "workspaces/docs-site/docs/contributing/reference/emitter_freeze.md"
POLICIES = ("frozen", "deletions-only")
# An addition is never recordable, so a change entry only ever lists these two kinds.
RECORDABLE_CHANGES = ("modified", "deleted")

# One row per migration-note field: the command-line option, the manifest key, and the placeholder the next-step hint
# shows. The keys mirror the template in `rust_source_backend_deprecation.md`.
NOTE_FIELDS = (
    ("issue", "compatibility_issue", "#<the bug or release issue that needs the emitter change>"),
    ("evidence", "behavior_evidence", "<the test, snapshot or downstream lane proving the behavior>"),
    ("owner", "semantic_owner", "<the future semantic owner: stable IDs, semantic facts, Body IR, ...>"),
    ("retirement", "retirement_condition", "<what lets this emitter path disappear or become a thin adapter>"),
)
MANIFEST_KEYS = ("tree", "policy", "files", "changes")
FILE_KEYS = ("path", "sha256", "lines")
CHANGE_KEYS = ("pr", "policy", "files") + tuple(key for _, key, _ in NOTE_FIELDS)
CHANGED_FILE_KEYS = ("path", "change")


class ManifestError(ValueError):
    """The manifest cannot be trusted: a schema, ordering or consistency defect rather than tree drift."""


@dataclass(frozen=True)
class Fingerprint:
    """One frozen file: its repository-relative POSIX path, content digest and line count."""

    path: str
    sha256: str
    lines: int


@dataclass(frozen=True)
class Drift:
    """One difference between the tree and the manifest.

    `before` is the manifest's fingerprint (absent for an addition) and `after` the tree's (absent for a deletion).
    """

    kind: str
    path: str
    before: Fingerprint | None
    after: Fingerprint | None


# ============================================================================
# Fingerprinting
# ============================================================================


def fingerprint_file(root: Path, path: Path) -> Fingerprint:
    """Digest one file's bytes and count its lines the way Python's `splitlines` does (a final unterminated line counts)."""
    data = path.read_bytes()
    return Fingerprint(
        path=path.relative_to(root).as_posix(),
        sha256=hashlib.sha256(data).hexdigest(),
        lines=len(data.splitlines()),
    )


def fingerprint_tree(root: Path, tree: str) -> list[Fingerprint]:
    """Fingerprint every `.rs` file under `tree`, recursively, sorted by path so the manifest is deterministic.

    A missing tree is an empty tree, not an error: under `deletions-only` the last file may legitimately be gone.
    """
    directory = root / tree
    if not directory.is_dir():
        return []
    files = sorted(path for path in directory.rglob("*.rs") if path.is_file())
    return [fingerprint_file(root, path) for path in files]


# ============================================================================
# Manifest schema
# ============================================================================


def require_keys(what: str, value: object, keys: tuple[str, ...]) -> dict:
    """Require `value` to be an object with exactly `keys`, in that order, so hand edits cannot drift the schema."""
    if not isinstance(value, dict):
        raise ManifestError(f"{what} must be a JSON object")
    if tuple(value) != keys:
        raise ManifestError(f"{what} must have exactly the keys {list(keys)} in that order, found {list(value)}")
    return value


def require_text(what: str, value: object) -> str:
    """Require a non-blank string; a blank migration-note field is a missing one."""
    if not isinstance(value, str) or not value.strip():
        raise ManifestError(f"{what} must be a non-empty string")
    return value


def parse_fingerprint(what: str, tree: str, value: object) -> Fingerprint:
    """Validate one `files` entry: a `.rs` path inside the frozen tree, a hex sha256 and a non-negative line count."""
    entry = require_keys(what, value, FILE_KEYS)
    path = require_text(f"{what}.path", entry["path"])
    if not path.startswith(f"{tree}/") or not path.endswith(".rs"):
        raise ManifestError(f"{what}.path `{path}` is not a `.rs` file under `{tree}/`")
    digest = require_text(f"{what}.sha256", entry["sha256"])
    if len(digest) != 64 or any(character not in "0123456789abcdef" for character in digest):
        raise ManifestError(f"{what}.sha256 `{digest}` is not a lowercase hex sha256")
    lines = entry["lines"]
    if not isinstance(lines, int) or isinstance(lines, bool) or lines < 0:
        raise ManifestError(f"{what}.lines must be a non-negative integer")
    return Fingerprint(path=path, sha256=digest, lines=lines)


def parse_change(what: str, tree: str, value: object) -> dict:
    """Validate one `changes` entry: a PR number or null, the policy in force after it, its files and the four note fields."""
    entry = require_keys(what, value, CHANGE_KEYS)
    pr = entry["pr"]
    if pr is not None and (not isinstance(pr, int) or isinstance(pr, bool) or pr <= 0):
        raise ManifestError(f"{what}.pr must be a positive integer or null")
    if entry["policy"] not in POLICIES:
        raise ManifestError(f"{what}.policy must be one of {list(POLICIES)}, found {entry['policy']!r}")
    if not isinstance(entry["files"], list):
        raise ManifestError(f"{what}.files must be a list")
    for index, changed in enumerate(entry["files"]):
        changed_what = f"{what}.files[{index}]"
        changed_entry = require_keys(changed_what, changed, CHANGED_FILE_KEYS)
        path = require_text(f"{changed_what}.path", changed_entry["path"])
        if not path.startswith(f"{tree}/") or not path.endswith(".rs"):
            raise ManifestError(f"{changed_what}.path `{path}` is not a `.rs` file under `{tree}/`")
        if changed_entry["change"] not in RECORDABLE_CHANGES:
            raise ManifestError(f"{changed_what}.change must be one of {list(RECORDABLE_CHANGES)}")
    for _, key, _ in NOTE_FIELDS:
        require_text(f"{what}.{key}", entry[key])
    return entry


def load_manifest(path: Path) -> dict:
    """Read and validate the manifest; every defect raises `ManifestError` with the offending key."""
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as error:
        raise ManifestError(f"cannot read `{path}`: {error}") from error
    try:
        raw = json.loads(text)
    except json.JSONDecodeError as error:
        raise ManifestError(f"`{path}` is not valid JSON: {error}") from error
    manifest = require_keys("manifest", raw, MANIFEST_KEYS)
    tree = require_text("manifest.tree", manifest["tree"]).strip("/")
    if manifest["policy"] not in POLICIES:
        raise ManifestError(f"manifest.policy must be one of {list(POLICIES)}, found {manifest['policy']!r}")
    if not isinstance(manifest["files"], list) or not isinstance(manifest["changes"], list):
        raise ManifestError("manifest.files and manifest.changes must be lists")

    files = [parse_fingerprint(f"manifest.files[{index}]", tree, entry) for index, entry in enumerate(manifest["files"])]
    for previous, current in zip(files, files[1:]):
        if previous.path == current.path:
            raise ManifestError(f"manifest.files lists `{current.path}` twice")
        if previous.path > current.path:
            raise ManifestError(f"manifest.files is not sorted: `{previous.path}` appears before `{current.path}`")

    changes = [parse_change(f"manifest.changes[{index}]", tree, entry) for index, entry in enumerate(manifest["changes"])]
    if changes and changes[-1]["policy"] != manifest["policy"]:
        raise ManifestError(
            f"manifest.policy is {manifest['policy']!r} but the last recorded change left it {changes[-1]['policy']!r}"
        )
    return {"tree": tree, "policy": manifest["policy"], "files": files, "changes": changes}


def render_manifest(manifest: dict) -> str:
    """Serialize the manifest in its one canonical form: fixed key order, sorted files, two-space indent, final newline."""
    document = {
        "tree": manifest["tree"],
        "policy": manifest["policy"],
        "files": [
            {"path": entry.path, "sha256": entry.sha256, "lines": entry.lines}
            for entry in sorted(manifest["files"], key=lambda entry: entry.path)
        ],
        "changes": [{key: change[key] for key in CHANGE_KEYS} for change in manifest["changes"]],
    }
    return json.dumps(document, indent=2, ensure_ascii=False) + "\n"


# ============================================================================
# Comparison
# ============================================================================


def compare(manifest_files: list[Fingerprint], tree_files: list[Fingerprint]) -> list[Drift]:
    """Return every addition, modification and deletion between the manifest and the tree, sorted by path."""
    recorded = {entry.path: entry for entry in manifest_files}
    actual = {entry.path: entry for entry in tree_files}
    drifts: list[Drift] = []
    for path in sorted(set(recorded) | set(actual)):
        before = recorded.get(path)
        after = actual.get(path)
        if before is None:
            drifts.append(Drift("added", path, None, after))
        elif after is None:
            drifts.append(Drift("deleted", path, before, None))
        elif before.sha256 != after.sha256:
            drifts.append(Drift("modified", path, before, after))
    return drifts


def rule_broken(policy: str, kind: str) -> str | None:
    """Name the rule a drift of `kind` breaks under `policy`, or `None` when the policy permits it."""
    if policy == "frozen":
        return {
            "added": "the frozen tree takes no new files",
            "modified": "a change to the frozen tree needs a migration note",
            "deleted": "a deletion from the frozen tree needs a migration note",
        }[kind]
    return {
        "added": "the deletions-only tree takes no new files",
        "modified": "a change to the deletions-only tree needs a migration note",
        "deleted": None,
    }[kind]


def plural(count: int, noun: str) -> str:
    """`1 line`, `2 lines`: the counts in gate output read as prose."""
    return f"{count} {noun}" if count == 1 else f"{count} {noun}s"


def describe(drift: Drift) -> str:
    """One line per drift: the kind, the path, and the digests and line counts on both sides."""
    if drift.kind == "added" and drift.after is not None:
        return f"added: {drift.path} ({plural(drift.after.lines, 'line')}, sha256 {drift.after.sha256[:12]})"
    if drift.kind == "deleted" and drift.before is not None:
        return f"deleted: {drift.path} ({plural(drift.before.lines, 'line')}, sha256 {drift.before.sha256[:12]})"
    if drift.before is not None and drift.after is not None:
        return (
            f"modified: {drift.path} (sha256 {drift.before.sha256[:12]} -> {drift.after.sha256[:12]}, "
            f"{drift.before.lines} -> {drift.after.lines} lines)"
        )
    return f"{drift.kind}: {drift.path}"


def display_path(root: Path, path: Path) -> str:
    """A path as gate output shows it: repository-relative when it is inside the checkout, absolute otherwise."""
    return path.relative_to(root).as_posix() if path.is_relative_to(root) else str(path)


def record_invocation(policy: str | None = None) -> str:
    """The exact `--record` command the next step needs, with a placeholder per migration-note field."""
    lines = ["  python3 scripts/check_emitter_freeze.py --record \\"]
    if policy is not None:
        lines.append(f"      --policy {policy} \\")
    for option, _, placeholder in NOTE_FIELDS:
        lines.append(f"      --{option} '{placeholder}' \\")
    lines.append("      --pr <pull request number>")
    return "\n".join(lines)


# ============================================================================
# Modes
# ============================================================================


def check(manifest: dict, root: Path, manifest_path: Path) -> int:
    """Report every drift with the rule it breaks under the current policy; exit 1 when any rule is broken."""
    policy = manifest["policy"]
    drifts = compare(manifest["files"], fingerprint_tree(root, manifest["tree"]))
    violations = [(drift, rule_broken(policy, drift.kind)) for drift in drifts]
    broken = [(drift, rule) for drift, rule in violations if rule is not None]
    permitted = [drift for drift, rule in violations if rule is None]

    if not broken:
        print(
            f"emitter freeze gate passed: {plural(len(manifest['files']), 'recorded file')} under {manifest['tree']}/** "
            f"(policy: {policy})"
        )
        for drift in permitted:
            print(f"  permitted under {policy}: {describe(drift)}")
        return 0

    print(f"emitter freeze gate: {manifest['tree']}/** drifted from {display_path(root, manifest_path)} (policy: {policy})")
    print()
    for drift, rule in broken:
        print(f"- {describe(drift)} -- {rule}")
    for drift in permitted:
        print(f"- {describe(drift)} -- permitted under {policy}")
    print()
    print("The Rust-emission backend is frozen (#1561). Fix the root in the middle end where possible; the emitter only")
    print("consumes recorded facts. If the change must stay in the emitter, record it with its migration note:")
    print()
    print(record_invocation())
    if any(drift.kind == "added" for drift, _ in broken):
        print()
        print("An added file cannot be recorded: put the code in an existing file, or fix the root outside the emitter.")
    print()
    print(f"See {REFERENCE_PAGE}.")
    return 1


def record(manifest: dict, root: Path, manifest_path: Path, args: argparse.Namespace) -> int:
    """Rewrite the fingerprints for the current tree and append the change entry; refuse an incomplete or empty record."""
    missing = [option for option, _, _ in NOTE_FIELDS if not (getattr(args, option) or "").strip()]
    if missing:
        print("emitter freeze gate: --record refused, the migration note is incomplete.")
        print(f"Missing: {', '.join(f'--{option}' for option in missing)}. Every recorded change carries all four fields:")
        print()
        print(record_invocation(args.policy))
        return 1

    new_policy = args.policy or manifest["policy"]
    drifts = compare(manifest["files"], fingerprint_tree(root, manifest["tree"]))
    added = [drift for drift in drifts if drift.kind == "added"]
    if added:
        print(f"emitter freeze gate: --record refused, the {new_policy} tree takes no new files:")
        for drift in added:
            print(f"- {describe(drift)}")
        print()
        print("Put the code in an existing file, or fix the root outside the emitter.")
        return 1
    if not drifts and new_policy == manifest["policy"]:
        print(f"emitter freeze gate: --record refused, the tree matches the manifest and the policy is already {new_policy}.")
        return 1

    change = {
        "pr": args.pr,
        "policy": new_policy,
        "files": [{"path": drift.path, "change": drift.kind} for drift in drifts],
    }
    for option, key, _ in NOTE_FIELDS:
        change[key] = getattr(args, option).strip()
    updated = {
        "tree": manifest["tree"],
        "policy": new_policy,
        "files": fingerprint_tree(root, manifest["tree"]),
        "changes": [*manifest["changes"], change],
    }
    manifest_path.write_text(render_manifest(updated), encoding="utf-8")
    print(
        f"emitter freeze gate: recorded change #{len(updated['changes'])} in {display_path(root, manifest_path)} "
        f"(policy: {new_policy})"
    )
    for drift in drifts:
        print(f"- {describe(drift)}")
    if new_policy != manifest["policy"]:
        print(f"- policy: {manifest['policy']} -> {new_policy}")
    return 0


def parse_args(argv: list[str]) -> argparse.Namespace:
    """Parse the gate's options: check by default, `--record` with the four note fields, `--policy` to flip."""
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", type=Path, default=ROOT, help="repository root (default: this checkout)")
    parser.add_argument("--manifest", type=Path, help=f"manifest to check or rewrite (default: {MANIFEST_PATH})")
    parser.add_argument(
        "--record",
        action="store_true",
        help="rewrite the fingerprints for the current tree and append a change entry (needs the four note fields)",
    )
    parser.add_argument("--policy", choices=POLICIES, help="with --record: the policy in force after this change")
    for option, key, _ in NOTE_FIELDS:
        parser.add_argument(f"--{option}", help=f"with --record: the migration note's `{key}`")
    parser.add_argument("--pr", type=int, help="with --record: the pull request number carrying the change")
    args = parser.parse_args(argv)
    if not args.record and any(
        value is not None for value in (args.policy, args.pr, *(getattr(args, option) for option, _, _ in NOTE_FIELDS))
    ):
        parser.error("--policy, --pr and the migration-note fields only apply with --record")
    if args.pr is not None and args.pr <= 0:
        parser.error("--pr must be a positive pull request number")
    return args


def main(argv: list[str] | None = None) -> int:
    """Run the gate; 0 on pass, 1 on drift or a refused record, 2 on a malformed manifest or usage error."""
    args = parse_args(sys.argv[1:] if argv is None else argv)
    root = args.root.resolve()
    manifest_path = (args.manifest if args.manifest is not None else root / MANIFEST_PATH).resolve()
    try:
        manifest = load_manifest(manifest_path)
    except ManifestError as error:
        print(f"emitter freeze gate: {error}", file=sys.stderr)
        return 2
    if args.record:
        return record(manifest, root, manifest_path, args)
    return check(manifest, root, manifest_path)


if __name__ == "__main__":
    sys.exit(main())

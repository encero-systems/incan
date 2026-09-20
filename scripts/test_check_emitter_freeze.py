"""Regression tests for the emitter freeze gate through its command-line interface.

Every test builds a scratch checkout whose frozen tree mirrors the real one's layout, so the committed manifest and
the real `loaves/compiler/incan_emit/src/emit/` are never touched.
"""

from pathlib import Path
import json
import subprocess
import sys
import tempfile
import unittest


CHECKER = Path(__file__).with_name("check_emitter_freeze.py")
TREE = "loaves/compiler/incan_emit/src/emit"
NOTE = ["--issue", "#1561", "--evidence", "codegen_snapshot_tests", "--owner", "Body IR", "--retirement", "dies with #654"]
NOTE_ENTRY = {
    "compatibility_issue": "#1561",
    "behavior_evidence": "codegen_snapshot_tests",
    "semantic_owner": "Body IR",
    "retirement_condition": "dies with #654",
}
PRUNE_HINT = "python3 scripts/check_emitter_freeze.py --record --prune-deletions"
PR_PLACEHOLDER = "[--pr <pull request number>]"


def load_gate():
    """Import the checker as a module, for the fingerprint and render helpers the tests seed manifests with."""
    sys.path.insert(0, str(CHECKER.parent))
    try:
        import check_emitter_freeze as gate
    finally:
        sys.path.pop(0)
    return gate


class EmitterFreezeTests(unittest.TestCase):
    """Exercise check, record, prune and the policy flip against isolated trees and manifests."""

    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        self.manifest = self.root / "manifest.json"
        self.write(f"{TREE}/mod.rs", "//! emit\nmod a;\nmod b;\n")
        self.write(f"{TREE}/a.rs", "fn a() {}\n")
        self.write(f"{TREE}/nested/b.rs", "fn b() {}\n")
        self.write(f"{TREE}/notes.md", "not fingerprinted\n")
        self.seed_manifest()

    def write(self, path, text=""):
        destination = self.root / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(text, encoding="utf-8")

    def seed_manifest(self, policy="frozen"):
        """Render the manifest for the scratch tree as it stands, with no recorded changes."""
        gate = load_gate()
        manifest = {"tree": TREE, "policy": policy, "files": gate.fingerprint_tree(self.root, TREE), "changes": []}
        self.manifest.write_text(gate.render_manifest(manifest), encoding="utf-8")

    def gate(self, *arguments):
        return subprocess.run(
            [sys.executable, str(CHECKER), "--root", str(self.root), "--manifest", str(self.manifest), *arguments],
            capture_output=True, text=True, check=False,
        )

    def manifest_json(self):
        return json.loads(self.manifest.read_text(encoding="utf-8"))

    def delete(self, path):
        (self.root / path).unlink()

    # ---- Check mode ----

    def test_unchanged_tree_passes_and_only_rust_files_are_fingerprinted(self):
        result = self.gate()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("emitter freeze gate passed: 3 recorded files", result.stdout)
        self.assertEqual(
            [entry["path"] for entry in self.manifest_json()["files"]],
            [f"{TREE}/a.rs", f"{TREE}/mod.rs", f"{TREE}/nested/b.rs"],
        )

    def test_a_file_beside_a_directory_of_the_same_name_sorts_like_the_manifest(self):
        # `x.rs` beside `x/y.rs`: sorting `Path` objects puts the directory first, the manifest's string key the file.
        # A manifest written straight from `fingerprint_tree`, without `render_manifest`'s re-sort, must still load.
        self.write(f"{TREE}/x.rs", "mod y;\n")
        self.write(f"{TREE}/x/y.rs", "fn y() {}\n")
        gate = load_gate()
        tree = [entry.path for entry in gate.fingerprint_tree(self.root, TREE)]
        self.assertEqual(tree, sorted(tree))
        self.assertLess(tree.index(f"{TREE}/x.rs"), tree.index(f"{TREE}/x/y.rs"))
        self.manifest.write_text(
            json.dumps(
                {
                    "tree": TREE,
                    "policy": "frozen",
                    "files": [
                        {"path": entry.path, "sha256": entry.sha256, "lines": entry.lines}
                        for entry in gate.fingerprint_tree(self.root, TREE)
                    ],
                    "changes": [],
                }
            ),
            encoding="utf-8",
        )
        result = self.gate()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual([entry.path for entry in gate.load_manifest(self.manifest)["files"]], tree)

    def test_crlf_line_endings_fingerprint_like_lf(self):
        # Slice 10 is Windows: a checkout that converts to CRLF must not read as a modification of every file.
        gate = load_gate()
        lf = gate.fingerprint_file(self.root, self.root / TREE / "mod.rs")
        (self.root / TREE / "mod.rs").write_bytes(b"//! emit\r\nmod a;\r\nmod b;\r\n")
        crlf = gate.fingerprint_file(self.root, self.root / TREE / "mod.rs")
        self.assertEqual(crlf, lf)
        result = self.gate()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_modification_fails_under_frozen_with_the_rule_and_the_record_command(self):
        self.write(f"{TREE}/a.rs", "fn a() {}\n// touched\n")
        result = self.gate()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(f"- modified: {TREE}/a.rs (sha256 ", result.stdout)
        self.assertIn("1 -> 2 lines) -- a change to the frozen tree needs a migration note", result.stdout)
        self.assertIn("python3 scripts/check_emitter_freeze.py --record \\", result.stdout)
        for option in ("--issue", "--evidence", "--owner", "--retirement"):
            self.assertIn(f"      {option} '", result.stdout)
        self.assertNotIn(PRUNE_HINT, result.stdout)

    def test_record_hint_carries_root_and_manifest_and_marks_pr_optional(self):
        self.write(f"{TREE}/a.rs", "fn a() {}\n// touched\n")
        result = self.gate()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(f"      --root {self.root} \\\n      --manifest {self.manifest} \\\n      --issue", result.stdout)
        self.assertIn(f"      {PR_PLACEHOLDER}\n", result.stdout)
        self.assertNotIn("--pr <pull request number>\n\n", result.stdout.replace(PR_PLACEHOLDER, ""))

    def test_addition_and_deletion_fail_under_frozen(self):
        self.write(f"{TREE}/new.rs", "fn new() {}\n")
        self.delete(f"{TREE}/nested/b.rs")
        result = self.gate()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(f"- added: {TREE}/new.rs (1 line, sha256 ", result.stdout)
        self.assertIn("-- the frozen tree takes no new files", result.stdout)
        self.assertIn(f"- deleted: {TREE}/nested/b.rs (1 line, sha256 ", result.stdout)
        self.assertIn("-- a deletion from the frozen tree needs a migration note", result.stdout)
        self.assertIn("An added file cannot be recorded", result.stdout)
        self.assertNotIn(PRUNE_HINT, result.stdout)

    def test_an_addition_alone_gets_no_record_command(self):
        self.write(f"{TREE}/new.rs", "fn new() {}\n")
        result = self.gate()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("An added file cannot be recorded", result.stdout)
        self.assertNotIn("--record", result.stdout)

    def test_deletion_under_deletions_only_fails_until_pruned(self):
        self.seed_manifest(policy="deletions-only")
        self.delete(f"{TREE}/nested/b.rs")
        result = self.gate()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(
            f"- deleted: {TREE}/nested/b.rs (1 line, sha256 ", result.stdout,
        )
        self.assertIn("-- a permitted deletion the manifest still lists; prune it", result.stdout)
        self.assertIn(
            "A deletion under deletions-only needs no migration note, only a prune so that the manifest lists files "
            "that exist:\n\n"
            f"  {PRUNE_HINT} --root {self.root} --manifest {self.manifest} {PR_PLACEHOLDER}\n",
            result.stdout,
        )
        self.assertNotIn("migration note:", result.stdout)
        self.assertNotIn("--issue", result.stdout)

    def test_deletion_and_modification_under_deletions_only_name_both_remedies_prune_first(self):
        self.seed_manifest(policy="deletions-only")
        self.delete(f"{TREE}/nested/b.rs")
        self.write(f"{TREE}/a.rs", "fn a() {}\n// touched\n")
        result = self.gate()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("-- a change to the deletions-only tree needs a migration note", result.stdout)
        self.assertIn("-- a permitted deletion the manifest still lists; prune it", result.stdout)
        self.assertIn("(prune first: a record with a note refuses while a deletion is pending):", result.stdout)
        self.assertLess(result.stdout.index(PRUNE_HINT), result.stdout.index("--record \\"))

        self.write(f"{TREE}/new.rs", "fn new() {}\n")
        added = self.gate()
        self.assertEqual(added.returncode, 1, added.stdout + added.stderr)
        self.assertIn("-- the deletions-only tree takes no new files", added.stdout)

    def test_an_absent_tree_is_all_deletions_and_prunes_to_an_empty_manifest(self):
        self.seed_manifest(policy="deletions-only")
        for path in (self.root / TREE).rglob("*.rs"):
            path.unlink()
        result = self.gate()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertEqual(result.stdout.count("-- a permitted deletion the manifest still lists; prune it"), 3)

        pruned = self.gate("--record", "--prune-deletions")
        self.assertEqual(pruned.returncode, 0, pruned.stdout + pruned.stderr)
        self.assertEqual(self.manifest_json()["files"], [])
        passed = self.gate()
        self.assertEqual(passed.returncode, 0, passed.stdout + passed.stderr)
        self.assertIn("emitter freeze gate passed: 0 recorded files", passed.stdout)

    # ---- Record mode ----

    def test_record_refuses_without_every_note_field(self):
        self.write(f"{TREE}/a.rs", "fn a() {}\n// touched\n")
        before = self.manifest.read_text(encoding="utf-8")
        result = self.gate("--record", "--issue", "#1561", "--pr", "1700")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("--record refused, the migration note is incomplete", result.stdout)
        self.assertIn("Missing: --evidence, --owner, --retirement", result.stdout)
        self.assertEqual(self.manifest.read_text(encoding="utf-8"), before)

    def test_record_refuses_a_blank_note_field(self):
        self.write(f"{TREE}/a.rs", "fn a() {}\n// touched\n")
        result = self.gate("--record", *NOTE[:-1], "   ")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("Missing: --retirement", result.stdout)

    def test_record_writes_fingerprints_and_the_change_entry(self):
        self.write(f"{TREE}/a.rs", "fn a() {}\n// touched\n")
        self.delete(f"{TREE}/nested/b.rs")
        result = self.gate("--record", *NOTE, "--pr", "1700")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("recorded change #1", result.stdout)
        manifest = self.manifest_json()
        self.assertEqual([entry["path"] for entry in manifest["files"]], [f"{TREE}/a.rs", f"{TREE}/mod.rs"])
        self.assertEqual(
            manifest["changes"],
            [
                {
                    "kind": "change",
                    "pr": 1700,
                    "policy": "frozen",
                    "files": [
                        {"path": f"{TREE}/a.rs", "change": "modified"},
                        {"path": f"{TREE}/nested/b.rs", "change": "deleted"},
                    ],
                    **NOTE_ENTRY,
                }
            ],
        )
        self.assertEqual(self.gate().returncode, 0)

    def test_record_refuses_an_addition_and_a_no_op(self):
        no_op = self.gate("--record", *NOTE)
        self.assertEqual(no_op.returncode, 1, no_op.stdout + no_op.stderr)
        self.assertIn("the tree matches the manifest and the policy is already frozen", no_op.stdout)

        self.write(f"{TREE}/new.rs", "fn new() {}\n")
        added = self.gate("--record", *NOTE)
        self.assertEqual(added.returncode, 1, added.stdout + added.stderr)
        self.assertIn("--record refused, the frozen tree takes no new files", added.stdout)
        self.assertEqual(self.manifest_json()["changes"], [])

    def test_record_with_a_note_refuses_while_a_deletion_is_pending(self):
        # The sweep that must not happen: a note for `a.rs` listing the unrelated, permitted deletion of `nested/b.rs`.
        self.seed_manifest(policy="deletions-only")
        self.delete(f"{TREE}/nested/b.rs")
        self.write(f"{TREE}/a.rs", "fn a() {}\n// touched\n")
        before = self.manifest.read_text(encoding="utf-8")
        result = self.gate("--record", *NOTE, "--pr", "1700")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(
            "--record refused, prune deletions first so that the note covers only its own files:\n"
            f"- deleted: {TREE}/nested/b.rs (1 line, sha256 ",
            result.stdout,
        )
        self.assertIn(f"  {PRUNE_HINT} --root {self.root} --manifest {self.manifest} {PR_PLACEHOLDER}\n", result.stdout)
        self.assertEqual(self.manifest.read_text(encoding="utf-8"), before)

        # Pruned, the note records only its own file.
        self.assertEqual(self.gate("--record", "--prune-deletions", "--pr", "1700").returncode, 0)
        recorded = self.gate("--record", *NOTE, "--pr", "1700")
        self.assertEqual(recorded.returncode, 0, recorded.stdout + recorded.stderr)
        self.assertEqual(
            [(change["kind"], [file["path"] for file in change["files"]]) for change in self.manifest_json()["changes"]],
            [("deletion", [f"{TREE}/nested/b.rs"]), ("change", [f"{TREE}/a.rs"])],
        )
        self.assertEqual(self.gate().returncode, 0)

    def test_prune_deletions_writes_a_deletion_entry_and_the_check_passes(self):
        self.seed_manifest(policy="deletions-only")
        self.delete(f"{TREE}/nested/b.rs")
        result = self.gate("--record", "--prune-deletions", "--pr", "1800")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("emitter freeze gate: pruned 1 deletion from ", result.stdout)
        self.assertIn(" as change #1 (policy: deletions-only)\n", result.stdout)
        self.assertIn(f"- deleted: {TREE}/nested/b.rs (1 line, sha256 ", result.stdout)
        manifest = self.manifest_json()
        self.assertEqual([entry["path"] for entry in manifest["files"]], [f"{TREE}/a.rs", f"{TREE}/mod.rs"])
        self.assertEqual(
            manifest["changes"],
            [
                {
                    "kind": "deletion",
                    "pr": 1800,
                    "policy": "deletions-only",
                    "files": [{"path": f"{TREE}/nested/b.rs", "change": "deleted"}],
                }
            ],
        )
        passed = self.gate()
        self.assertEqual(passed.returncode, 0, passed.stdout + passed.stderr)
        self.assertIn("emitter freeze gate passed: 2 recorded files", passed.stdout)

    def test_prune_deletions_leaves_a_pending_modification_unrecorded(self):
        self.seed_manifest(policy="deletions-only")
        self.delete(f"{TREE}/nested/b.rs")
        self.write(f"{TREE}/a.rs", "fn a() {}\n// touched\n")
        recorded = self.manifest_json()["files"][0]
        self.assertEqual(self.gate("--record", "--prune-deletions").returncode, 0)
        self.assertEqual(self.manifest_json()["files"][0], recorded)
        result = self.gate()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("-- a change to the deletions-only tree needs a migration note", result.stdout)
        self.assertNotIn(PRUNE_HINT, result.stdout)

    def test_a_file_recreated_at_a_pruned_path_is_an_addition(self):
        self.seed_manifest(policy="deletions-only")
        self.delete(f"{TREE}/nested/b.rs")
        self.assertEqual(self.gate("--record", "--prune-deletions").returncode, 0)

        self.write(f"{TREE}/nested/b.rs", "fn b() {}\n")
        checked = self.gate()
        self.assertEqual(checked.returncode, 1, checked.stdout + checked.stderr)
        self.assertIn(f"- added: {TREE}/nested/b.rs (1 line, sha256 ", checked.stdout)
        self.assertIn("-- the deletions-only tree takes no new files", checked.stdout)
        recorded = self.gate("--record", *NOTE)
        self.assertEqual(recorded.returncode, 1, recorded.stdout + recorded.stderr)
        self.assertIn("--record refused, the deletions-only tree takes no new files", recorded.stdout)
        pruned = self.gate("--record", "--prune-deletions")
        self.assertEqual(pruned.returncode, 1, pruned.stdout + pruned.stderr)
        self.assertIn("--record refused, the deletions-only tree takes no new files", pruned.stdout)
        self.assertEqual(len(self.manifest_json()["changes"]), 1)

    def test_deletion_under_frozen_still_needs_the_note_and_prune_is_refused(self):
        self.delete(f"{TREE}/nested/b.rs")
        before = self.manifest.read_text(encoding="utf-8")
        pruned = self.gate("--record", "--prune-deletions")
        self.assertEqual(pruned.returncode, 1, pruned.stdout + pruned.stderr)
        self.assertIn(
            "--prune-deletions refused, a deletion from the frozen tree needs a migration note:\n\n"
            "  python3 scripts/check_emitter_freeze.py --record \\\n",
            pruned.stdout,
        )
        self.assertEqual(self.manifest.read_text(encoding="utf-8"), before)

        recorded = self.gate("--record", *NOTE)
        self.assertEqual(recorded.returncode, 0, recorded.stdout + recorded.stderr)
        change = self.manifest_json()["changes"][0]
        self.assertEqual(change["kind"], "change")
        self.assertEqual(change["files"], [{"path": f"{TREE}/nested/b.rs", "change": "deleted"}])
        self.assertEqual(self.gate().returncode, 0)

    def test_prune_deletions_refuses_when_nothing_is_missing(self):
        self.seed_manifest(policy="deletions-only")
        before = self.manifest.read_text(encoding="utf-8")
        result = self.gate("--record", "--prune-deletions")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("--prune-deletions refused, every file the manifest lists exists.", result.stdout)
        self.assertEqual(self.manifest.read_text(encoding="utf-8"), before)

    def test_policy_flip_is_a_recorded_change_and_is_not_repeatable(self):
        result = self.gate("--record", "--policy", "deletions-only", *NOTE, "--pr", "1800")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("- policy: frozen -> deletions-only", result.stdout)
        manifest = self.manifest_json()
        self.assertEqual(manifest["policy"], "deletions-only")
        self.assertEqual(manifest["changes"][0]["kind"], "change")
        self.assertEqual(manifest["changes"][0]["policy"], "deletions-only")
        self.assertEqual(manifest["changes"][0]["files"], [])

        repeat = self.gate("--record", "--policy", "deletions-only", *NOTE)
        self.assertEqual(repeat.returncode, 1, repeat.stdout + repeat.stderr)
        self.assertIn("the policy is already deletions-only", repeat.stdout)

        back = self.gate("--record", "--policy", "frozen", *NOTE)
        self.assertEqual(back.returncode, 0, back.stdout + back.stderr)
        self.assertIn("- policy: deletions-only -> frozen", back.stdout)
        self.assertEqual(self.manifest_json()["policy"], "frozen")

    def test_record_options_are_refused_in_check_mode(self):
        result = self.gate("--issue", "#1561")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("only apply with --record", result.stderr)

        prune = self.gate("--prune-deletions")
        self.assertEqual(prune.returncode, 2, prune.stdout + prune.stderr)
        self.assertIn("only apply with --record", prune.stderr)

    def test_prune_deletions_takes_no_policy_and_no_note(self):
        self.seed_manifest(policy="deletions-only")
        self.delete(f"{TREE}/nested/b.rs")
        with_note = self.gate("--record", "--prune-deletions", "--issue", "#1561")
        self.assertEqual(with_note.returncode, 2, with_note.stdout + with_note.stderr)
        self.assertIn("carries no --policy and no migration note", with_note.stderr)
        with_policy = self.gate("--record", "--prune-deletions", "--policy", "frozen")
        self.assertEqual(with_policy.returncode, 2, with_policy.stdout + with_policy.stderr)
        self.assertEqual(self.manifest_json()["changes"], [])

    # ---- Manifest validation ----

    def test_malformed_manifest_is_a_configuration_error(self):
        manifest = self.manifest_json()
        manifest["files"].reverse()
        self.manifest.write_text(json.dumps(manifest), encoding="utf-8")
        unsorted = self.gate()
        self.assertEqual(unsorted.returncode, 2, unsorted.stdout + unsorted.stderr)
        self.assertIn("manifest.files is not sorted", unsorted.stderr)

        manifest = self.manifest_json()
        manifest["policy"] = "thawed"
        self.manifest.write_text(json.dumps(manifest), encoding="utf-8")
        policy = self.gate()
        self.assertEqual(policy.returncode, 2, policy.stdout + policy.stderr)
        self.assertIn("manifest.policy must be one of", policy.stderr)

        self.manifest.write_text("{", encoding="utf-8")
        invalid = self.gate()
        self.assertEqual(invalid.returncode, 2, invalid.stdout + invalid.stderr)
        self.assertIn("is not valid JSON", invalid.stderr)

    def test_manifest_policy_must_match_the_last_recorded_change(self):
        self.assertEqual(self.gate("--record", "--policy", "deletions-only", *NOTE).returncode, 0)
        manifest = self.manifest_json()
        manifest["policy"] = "frozen"
        self.manifest.write_text(json.dumps(manifest), encoding="utf-8")
        result = self.gate()
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("the last recorded change left it 'deletions-only'", result.stderr)

    def test_change_entries_carry_a_kind_that_decides_the_note(self):
        self.seed_manifest(policy="deletions-only")
        self.delete(f"{TREE}/nested/b.rs")
        self.assertEqual(self.gate("--record", "--prune-deletions").returncode, 0)
        canonical = self.manifest.read_text(encoding="utf-8")
        deletion = self.manifest_json()["changes"][0]

        # A null note field on a deletion entry is tolerated and dropped again on the next write.
        manifest = self.manifest_json()
        manifest["changes"][0] = {**deletion, **{key: None for key in NOTE_ENTRY}}
        self.manifest.write_text(json.dumps(manifest), encoding="utf-8")
        self.assertEqual(self.gate().returncode, 0)
        self.write(f"{TREE}/a.rs", "fn a() {}\n// touched\n")
        self.assertEqual(self.gate("--record", *NOTE).returncode, 0)
        self.assertEqual(self.manifest_json()["changes"][0], deletion)
        self.manifest.write_text(canonical, encoding="utf-8")
        self.write(f"{TREE}/a.rs", "fn a() {}\n")

        # A note on a deletion entry, a deletion entry under `frozen`, a `change` entry without its note, and an entry
        # without a kind are all schema errors.
        for broken, message in (
            ({**deletion, "compatibility_issue": "#1561"}, "must have exactly the keys"),
            ({**deletion, **NOTE_ENTRY, "compatibility_issue": None, "behavior_evidence": "x"},
             "manifest.changes[0].behavior_evidence must be absent or null in a deletion entry"),
            ({**deletion, "policy": "frozen"}, "manifest.changes[0].policy must be 'deletions-only' in a deletion entry"),
            ({**deletion, "files": []}, "manifest.changes[0].files must list at least one deleted file"),
            ({**deletion, "files": [{"path": f"{TREE}/nested/b.rs", "change": "modified"}]},
             "manifest.changes[0].files[0].change must be 'deleted' in a deletion entry"),
            ({**deletion, "kind": "change"}, "must have exactly the keys"),
            ({**deletion, "kind": "change", **NOTE_ENTRY, "semantic_owner": ""},
             "manifest.changes[0].semantic_owner must be a non-empty string"),
            ({key: value for key, value in deletion.items() if key != "kind"}, "manifest.changes[0].kind must be one of"),
        ):
            manifest = self.manifest_json()
            manifest["changes"] = [broken]
            self.manifest.write_text(json.dumps(manifest), encoding="utf-8")
            result = self.gate()
            self.assertEqual(result.returncode, 2, f"{broken!r}\n{result.stdout}{result.stderr}")
            self.assertIn(message, result.stderr, repr(broken))


if __name__ == "__main__":
    unittest.main()

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


class EmitterFreezeTests(unittest.TestCase):
    """Exercise check, record and the policy flip against isolated trees and manifests."""

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
        sys.path.insert(0, str(CHECKER.parent))
        try:
            import check_emitter_freeze as gate
        finally:
            sys.path.pop(0)
        manifest = {"tree": TREE, "policy": policy, "files": gate.fingerprint_tree(self.root, TREE), "changes": []}
        self.manifest.write_text(gate.render_manifest(manifest), encoding="utf-8")

    def gate(self, *arguments):
        return subprocess.run(
            [sys.executable, str(CHECKER), "--root", str(self.root), "--manifest", str(self.manifest), *arguments],
            capture_output=True, text=True, check=False,
        )

    def manifest_json(self):
        return json.loads(self.manifest.read_text(encoding="utf-8"))

    # ---- Check mode ----

    def test_unchanged_tree_passes_and_only_rust_files_are_fingerprinted(self):
        result = self.gate()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("emitter freeze gate passed: 3 recorded files", result.stdout)
        self.assertEqual(
            [entry["path"] for entry in self.manifest_json()["files"]],
            [f"{TREE}/a.rs", f"{TREE}/mod.rs", f"{TREE}/nested/b.rs"],
        )

    def test_modification_fails_under_frozen_with_the_rule_and_the_record_command(self):
        self.write(f"{TREE}/a.rs", "fn a() {}\n// touched\n")
        result = self.gate()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(f"- modified: {TREE}/a.rs (sha256 ", result.stdout)
        self.assertIn("1 -> 2 lines) -- a change to the frozen tree needs a migration note", result.stdout)
        self.assertIn("python3 scripts/check_emitter_freeze.py --record \\", result.stdout)
        for option in ("--issue", "--evidence", "--owner", "--retirement", "--pr"):
            self.assertIn(option, result.stdout)

    def test_addition_and_deletion_fail_under_frozen(self):
        self.write(f"{TREE}/new.rs", "fn new() {}\n")
        (self.root / TREE / "nested/b.rs").unlink()
        result = self.gate()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(f"- added: {TREE}/new.rs (1 line, sha256 ", result.stdout)
        self.assertIn("-- the frozen tree takes no new files", result.stdout)
        self.assertIn(f"- deleted: {TREE}/nested/b.rs (1 line, sha256 ", result.stdout)
        self.assertIn("-- a deletion from the frozen tree needs a migration note", result.stdout)
        self.assertIn("An added file cannot be recorded", result.stdout)

    def test_deletion_passes_under_deletions_only_but_modification_and_addition_fail(self):
        self.seed_manifest(policy="deletions-only")
        (self.root / TREE / "nested/b.rs").unlink()
        passed = self.gate()
        self.assertEqual(passed.returncode, 0, passed.stdout + passed.stderr)
        self.assertIn(f"permitted under deletions-only: deleted: {TREE}/nested/b.rs", passed.stdout)

        self.write(f"{TREE}/a.rs", "fn a() {}\n// touched\n")
        modified = self.gate()
        self.assertEqual(modified.returncode, 1, modified.stdout + modified.stderr)
        self.assertIn("-- a change to the deletions-only tree needs a migration note", modified.stdout)
        self.assertIn(f"- deleted: {TREE}/nested/b.rs (1 line, sha256 ", modified.stdout)
        self.assertIn("-- permitted under deletions-only", modified.stdout)

        self.write(f"{TREE}/new.rs", "fn new() {}\n")
        added = self.gate()
        self.assertEqual(added.returncode, 1, added.stdout + added.stderr)
        self.assertIn("-- the deletions-only tree takes no new files", added.stdout)

    def test_an_absent_tree_is_all_deletions(self):
        self.seed_manifest(policy="deletions-only")
        for path in (self.root / TREE).rglob("*.rs"):
            path.unlink()
        result = self.gate()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout.count("permitted under deletions-only: deleted:"), 3)

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
        (self.root / TREE / "nested/b.rs").unlink()
        result = self.gate("--record", *NOTE, "--pr", "1700")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("recorded change #1", result.stdout)
        manifest = self.manifest_json()
        self.assertEqual([entry["path"] for entry in manifest["files"]], [f"{TREE}/a.rs", f"{TREE}/mod.rs"])
        self.assertEqual(
            manifest["changes"],
            [
                {
                    "pr": 1700,
                    "policy": "frozen",
                    "files": [
                        {"path": f"{TREE}/a.rs", "change": "modified"},
                        {"path": f"{TREE}/nested/b.rs", "change": "deleted"},
                    ],
                    "compatibility_issue": "#1561",
                    "behavior_evidence": "codegen_snapshot_tests",
                    "semantic_owner": "Body IR",
                    "retirement_condition": "dies with #654",
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

    def test_policy_flip_is_a_recorded_change_and_is_not_repeatable(self):
        result = self.gate("--record", "--policy", "deletions-only", *NOTE, "--pr", "1800")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("- policy: frozen -> deletions-only", result.stdout)
        manifest = self.manifest_json()
        self.assertEqual(manifest["policy"], "deletions-only")
        self.assertEqual(manifest["changes"][0]["policy"], "deletions-only")
        self.assertEqual(manifest["changes"][0]["files"], [])

        repeat = self.gate("--record", "--policy", "deletions-only", *NOTE)
        self.assertEqual(repeat.returncode, 1, repeat.stdout + repeat.stderr)
        self.assertIn("the policy is already deletions-only", repeat.stdout)

    def test_record_options_are_refused_in_check_mode(self):
        result = self.gate("--issue", "#1561")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("only apply with --record", result.stderr)

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


if __name__ == "__main__":
    unittest.main()

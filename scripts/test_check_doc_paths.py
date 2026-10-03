"""Regression tests for contributor path checking through its command-line interface."""

from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


CHECKER = Path(__file__).with_name("check_doc_paths.py")


class DocPathTests(unittest.TestCase):
    """Exercise real files, Markdown and explicit exceptions in isolated repositories."""

    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        self.write("scripts/check_doc_paths.allow", "")

    def write(self, path, text=""):
        destination = self.root / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(text, encoding="utf-8")

    def run_check(self, *arguments):
        return subprocess.run(
            [sys.executable, str(CHECKER), "--root", str(self.root), *arguments],
            capture_output=True, text=True, check=False,
        )

    def test_missing_file_and_directory_report_document_and_line(self):
        self.write("AGENTS.md", "`src/missing.rs`\n`loaves/missing/`\n")
        result = self.run_check()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("AGENTS.md:1: src/missing.rs", result.stdout)
        self.assertIn("AGENTS.md:2: loaves/missing/", result.stdout)

    def test_discovers_ring_readmes_and_agent_docs_but_not_runtime_state(self):
        self.write("loaves/kernel/example/README.md", "`src/absent.rs`")
        self.write(".agents/test-suite.md", "`tests/absent.rs`")
        self.write(".agents/state/private.md", "`src/private.rs`")
        result = self.run_check()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("loaves/kernel/example/README.md", result.stdout)
        self.assertIn(".agents/test-suite.md", result.stdout)
        self.assertNotIn("private.rs", result.stdout)

    def test_shell_commands_and_diagrams_both_check_concrete_paths(self):
        self.write("AGENTS.md", "```text\nsrc/diagram.rs\n```\n```sh\ncat src/missing.rs\n```\n")
        result = self.run_check()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("AGENTS.md:5: src/missing.rs", result.stdout)
        self.assertIn("AGENTS.md:2: src/diagram.rs", result.stdout)

    def test_nested_fences_do_not_hide_concrete_paths(self):
        self.write("AGENTS.md", "````text\n```\nsrc/diagram.rs\n````\n~~~sh\ncat src/missing.rs\n~~~\n")
        result = self.run_check()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("src/diagram.rs", result.stdout)
        self.assertIn("src/missing.rs", result.stdout)

    def test_scoped_exception_does_not_hide_same_path_in_other_documents(self):
        self.write("AGENTS.md", "`src/example.rs`")
        self.write("CONTRIBUTING.md", "`src/example.rs`")
        self.write("scripts/check_doc_paths.allow", "AGENTS.md\tsrc/example.rs\tIllustrative name\n")
        result = self.run_check()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertNotIn("AGENTS.md:1:", result.stdout)
        self.assertIn("CONTRIBUTING.md:1:", result.stdout)

    def test_runtime_prefix_exception_does_not_hide_other_agent_paths(self):
        self.write("AGENTS.md", "`.agents/state/report.md` `.agents/missing.md`")
        self.write("scripts/check_doc_paths.allow", "*\t.agents/state/**\tRuntime state\n")
        result = self.run_check()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertNotIn("AGENTS.md:1: .agents/state/report.md", result.stdout)
        self.assertIn("AGENTS.md:1: .agents/missing.md", result.stdout)

    def test_missing_or_malformed_allowlist_fails_closed(self):
        result = self.run_check("--allow", str(self.root / "absent.allow"))
        self.assertEqual(result.returncode, 2, result.stdout)
        self.write("scripts/check_doc_paths.allow", "src/missing.rs\n")
        result = self.run_check()
        self.assertEqual(result.returncode, 2, result.stdout)

    def test_brace_alternatives_and_globs_require_matching_paths(self):
        self.write("AGENTS.md", "`src/{one,two}.rs` `tests/*.rs`\n")
        self.write("src/one.rs")
        self.write("tests/works.rs")
        result = self.run_check()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("src/two.rs", result.stdout)
        self.write("src/two.rs")
        self.assertEqual(self.run_check().returncode, 0)

    def test_links_and_line_suffixes_resolve_without_external_url_fragments(self):
        self.write("AGENTS.md", "[file](src/one.rs#anchor) `src/one.rs:42`. https://example.test/src/missing.rs\n")
        self.write("src/one.rs")
        result = self.run_check("--list")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("src/one.rs", result.stdout)
        self.assertNotIn("missing.rs", result.stdout)

    def test_fast_gate_stops_on_doc_failure_before_starting_cargo(self):
        self.write("AGENTS.md", "`src/missing.rs`")
        self.write("scripts/check_doc_paths.py", CHECKER.read_text())
        self.write(
            "Makefile", (CHECKER.parent.parent / "Makefile").read_text()
            + "\nfmt-check-ci rustdoc-gate-ci emitter-freeze-ci:\n\t@true\ncheck-fast-ci:\n\t@touch cargo-started\n",
        )
        result = subprocess.run(
            ["make", "-s", "pre-commit-fast"],
            cwd=self.root, capture_output=True, text=True, check=False,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("AGENTS.md:1: src/missing.rs", result.stdout)
        self.assertFalse((self.root / "cargo-started").exists())


if __name__ == "__main__":
    unittest.main()

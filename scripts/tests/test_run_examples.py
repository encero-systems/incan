"""Exercise example routing without compiling providers or running example programs."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


RUNNER = Path(__file__).resolve().parents[1] / "run_examples.sh"
CHECK_ONLY = ("vocab_markform", "vocab_scriptkit", "vocab_styleforge")


class ExampleRunnerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        (self.root / "scripts").mkdir()
        shutil.copy2(RUNNER, self.root / "scripts/run_examples.sh")
        self.compiler = self.root / "fake-incan"
        self.compiler.write_text(
            "#!/usr/bin/env python3\n"
            "import json, os, pathlib, sys\n"
            "root = pathlib.Path(os.environ['EXAMPLE_TEST_ROOT'])\n"
            "with (root / 'calls.jsonl').open('a') as log:\n"
            "    log.write(json.dumps({'cwd': str(pathlib.Path.cwd().relative_to(root)), 'args': sys.argv[1:]}) + '\\n')\n"
            "sys.exit(7 if sys.argv[1:2] == ['--check'] and os.environ.get('FAIL_CHECK') == sys.argv[-1] else 0)\n"
        )
        self.compiler.chmod(0o755)

    def add_pair(self, name):
        base = self.root / "examples/pro" / name
        for side in ("producer", "consumer"):
            project = base / side
            (project / "src").mkdir(parents=True)
            dependency = '\n[dependencies]\nprovider = { path = "../producer" }\n' if side == "consumer" else ""
            (project / "loaf.toml").write_text('[project]\nname = "fixture"\n' + dependency)
            source = "lib.incn" if side == "producer" else "main.incn"
            body = "def main() -> None:\n    pass\n" if side == "consumer" else "pub def value() -> int:\n    return 1\n"
            (project / "src" / source).write_text(body)
        return f"examples/pro/{name}/consumer/src/main.incn"

    def run_runner(self, selected="", fail_check=""):
        env = dict(os.environ, INCAN_BIN=str(self.compiler), EXAMPLE_TEST_ROOT=str(self.root),
                   INCAN_EXAMPLES_ONLY=selected, FAIL_CHECK=fail_check,
                   INCAN_EXAMPLES_REQUIRE_CARGO_FREE="0", INCAN_EXAMPLES_TIMEOUT="5")
        result = subprocess.run(["bash", "scripts/run_examples.sh"], cwd=self.root, env=env,
                                text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=30)
        calls = [json.loads(line) for line in (self.root / "calls.jsonl").read_text().splitlines()]
        return result, calls

    def test_conformance_consumers_are_checked_and_producers_still_bake(self):
        paths = [self.add_pair(name) for name in CHECK_ONLY]
        result, calls = self.run_runner()
        self.assertEqual(result.returncode, 0, result.stdout)
        for name, path in zip(CHECK_ONLY, paths):
            self.assertIn({"cwd": ".", "args": ["--check", path]}, calls)
            self.assertNotIn({"cwd": ".", "args": ["run", path]}, calls)
            self.assertIn({"cwd": f"examples/pro/{name}/producer", "args": ["oven", "bake", "--project", "."]}, calls)
            self.assertIn({"cwd": f"examples/pro/{name}/producer", "args": ["build", "--lib"]}, calls)
            self.assertNotIn({"cwd": f"examples/pro/{name}/consumer", "args": ["oven", "bake", "--project", "."]}, calls)
        self.assertIn("check-only", result.stdout)

    def test_runtime_consumer_still_bakes_and_runs(self):
        path = self.add_pair("vocab_querykit")
        result, calls = self.run_runner()
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn({"cwd": "examples/pro/vocab_querykit/consumer", "args": ["oven", "bake", "--project", "."]}, calls)
        self.assertIn({"cwd": ".", "args": ["run", path]}, calls)

    def test_selection_keeps_only_required_producer_preparation(self):
        selected = self.add_pair("vocab_markform")
        self.add_pair("vocab_styleforge")
        result, calls = self.run_runner(selected)
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertTrue(any(call["cwd"] == "examples/pro/vocab_markform/producer" for call in calls))
        self.assertFalse(any("vocab_styleforge" in json.dumps(call) for call in calls))

    def test_conformance_typecheck_failure_fails_gate(self):
        path = self.add_pair("vocab_markform")
        result, _ = self.run_runner(fail_check=path)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn(f"FAILED: check {path}", result.stdout)


if __name__ == "__main__":
    unittest.main()

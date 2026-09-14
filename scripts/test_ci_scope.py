"""Run the workflow's scope command against event and changed-path scenarios."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("ci_scope.sh")


class CiScopeTests(unittest.TestCase):
    def scope(self, event="workflow_dispatch", *, heavy=False, reference=False, base="0.6.0-dev.4",
              branch="0.6.0-dev.4", labels="", files=("src/main.rs",)):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            git = root / "git"
            git.write_text(f"#!{sys.executable}\n" + '''
import json, os, sys
if sys.argv[1] == "fetch":
    sys.exit(0)
if sys.argv[1:3] == ["diff", "--name-only"]:
    sys.stdout.buffer.write(b"".join(path.encode() + b"\\0" for path in json.loads(os.environ["PROBE_CHANGED_FILES"])))
else:
    sys.exit("unexpected Git command")
''')
            git.chmod(0o755)
            output = root / "outputs"
            environment = dict(os.environ, PATH=str(root) + os.pathsep + os.environ["PATH"],
                               EVENT_NAME=event, DISPATCH_HEAVY=str(heavy).lower(),
                               DISPATCH_REFERENCE=str(reference).lower(), BASE_REF=base,
                               REF_NAME=branch, PR_LABELS=labels, GITHUB_OUTPUT=str(output),
                               PROBE_CHANGED_FILES=json.dumps(files))
            result = subprocess.run(["bash", str(SCRIPT)], env=environment, cwd=root,
                                    capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            return dict(line.split("=", 1) for line in output.read_text().splitlines())

    def test_reference_only_dispatch_does_not_enable_heavy_work(self):
        self.assertEqual(self.scope(reference=True), {"docs_only": "false", "heavy": "false", "reference": "true"})

    def test_default_light_dispatch_does_not_enable_reference(self):
        self.assertEqual(self.scope(), {"docs_only": "false", "heavy": "false", "reference": "false"})

    def test_every_heavy_event_also_selects_the_producer(self):
        for arguments in [dict(heavy=True), dict(event="pull_request", base="main"),
                          dict(event="pull_request", base="release/0.6"),
                          dict(event="pull_request", labels="compiler,full-ci"),
                          dict(event="push", branch="main"), dict(event="push", branch="release/0.6")]:
            with self.subTest(arguments=arguments):
                result = self.scope(**arguments)
                self.assertEqual((result["heavy"], result["reference"]), ("true", "true"))

    def test_reference_input_does_not_change_ordinary_push_or_pr_scope(self):
        for event in ["push", "pull_request"]:
            with self.subTest(event=event):
                self.assertEqual(self.scope(event=event, reference=True), self.scope(event=event))

    def test_registry_and_verified_example_prs_still_select_reference(self):
        for path in ["crates/incan_core/src/lang/example.rs",
                     "workspaces/docs-site/docs/language/reference/language.md",
                     "workspaces/docs-site/docs/_snippets/language/examples/verified_web.incn"]:
            with self.subTest(path=path):
                self.assertEqual(self.scope(event="pull_request", files=(path,)),
                                 {"docs_only": "false", "heavy": "false", "reference": "true"})

    def test_ordinary_docs_only_pr_stays_docs_only(self):
        self.assertEqual(self.scope(event="pull_request", files=("workspaces/docs-site/docs/index.md",)),
                         {"docs_only": "true", "heavy": "false", "reference": "false"})

    def test_empty_change_set_is_not_misreported_as_docs_only(self):
        self.assertEqual(self.scope(event="pull_request", files=())["docs_only"], "false")


if __name__ == "__main__":
    unittest.main()

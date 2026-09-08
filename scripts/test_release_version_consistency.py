"""Check release-branch version enforcement through the real command-line gate."""

from pathlib import Path
import re
import subprocess
import sys
import unittest


ROOT = Path(__file__).resolve().parent.parent
GATE = ROOT / "scripts/check_release_version_consistency.py"
VERSION = re.search(r'^version = "([^"]+)"$', (ROOT / "Cargo.toml").read_text(), re.MULTILINE).group(1)


class ReleaseBaselineTests(unittest.TestCase):
    def gate(self, head: str, base: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(GATE), "--release-branch", head, "--base-branch", base],
            cwd=ROOT, text=True, capture_output=True, check=False,
        )

    def test_matching_release_baseline_passes(self):
        result = self.gate(VERSION, "main")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_previous_cohort_cannot_land_as_a_new_release(self):
        result = self.gate("0.6.0-dev.999", "main")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("0.6.0-dev.999", result.stdout)

    def test_topic_branch_does_not_declare_a_release(self):
        result = self.gate("bugfix/1068-script-targets", "main")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_development_integration_does_not_force_a_release(self):
        result = self.gate("0.6.0-dev.999", "0.6.0-dev.4")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()

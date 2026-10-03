#!/usr/bin/env python3
"""Unit tests for `check_reference_contract.py`."""

import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import check_reference_contract as gate


class ReferenceContractTests(unittest.TestCase):
    """Each rule fires on prose and stays quiet on code, inline code, links and See also sections."""

    def findings(self, text: str, rel: str = "language/reference/sample.md") -> list[str]:
        """Return the rule names the gate reports for `text` on a page at `rel`."""
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "page.md"
            path.write_text(text)
            return [finding.rule for finding in gate.page_findings(path, rel, [])]

    def test_flags_each_rule_in_prose(self) -> None:
        """Every lexical rule reports a violating prose sentence."""
        self.assertIn("comparison with another language", self.findings("Renders as Python spells it.\n"))
        self.assertIn("time-bound wording", self.findings("This is currently refused.\n"))
        self.assertIn("issue or RFC number", self.findings("Refused (#1767).\n"))
        self.assertIn("advice", self.findings("Prefer `@derive(Eq, Hash)`.\n"))
        self.assertIn("rationale", self.findings("It is refused because the type has no fields.\n"))
        self.assertIn("walkthrough heading", self.findings("## Example: chaining errors\n"))
        self.assertIn("compiler-internal vocabulary", self.findings("The call lowers to a method.\n"))
        self.assertIn("quoted diagnostic output", self.findings("```bash\ntype error: Unknown derive 'Debg'\n```\n"))
        self.assertIn("example comment", self.findings('```incan\nx = f(1)   # the first alternative binds x\n```\n'))

    def test_example_comments_say_accepted_or_refused(self) -> None:
        """Accepted and refused annotations, file labels and `#` inside strings pass."""
        text = (
            "```incan\n"
            "# accounts.incn\n"
            "x = f(1)          # accepted\n"
            "y = g(\"#tag\")    # refused: g takes an int\n"
            "```\n"
        )
        self.assertEqual(self.findings(text), [])

    def test_ignores_code_links_and_see_also(self) -> None:
        """Code, inline code, link targets and See also sections are not prose."""
        text = (
            "Refused with `INCAN-T0001` (see [RFC notes](../x/rfc-123.md)).\n"
            "| Alternation | <code>p1 &#124; p2</code> |\n"
            "```incan\n# refused: currently refused, because\n```\n"
            "## See also\n\n- [Rust types for Python developers](../how-to/rust.md)\n"
        )
        self.assertEqual(self.findings(text), [])

    def test_internal_vocabulary_applies_to_language_pages_only(self) -> None:
        """Tooling references document the toolchain and may name its parts."""
        self.assertEqual(self.findings("Shows the generated Rust.\n", rel="tooling/reference/cli.md"), [])

    def test_changed_pages_include_untracked_reference_pages(self) -> None:
        """Default working-tree discovery includes untracked reference pages."""
        results = [
            type("Result", (), {"returncode": 0, "stdout": "", "stderr": ""})(),
            type("Result", (), {"returncode": 0, "stdout": "", "stderr": ""})(),
            type(
                "Result",
                (),
                {
                    "returncode": 0,
                    "stdout": "workspaces/docs-site/docs/tooling/reference/new_contract.md\n",
                    "stderr": "",
                },
            )(),
        ]
        with patch.object(gate.subprocess, "run", side_effect=results), patch.object(Path, "exists", return_value=True):
            self.assertEqual(gate.changed_pages(None), ["tooling/reference/new_contract.md"])


if __name__ == "__main__":
    unittest.main()

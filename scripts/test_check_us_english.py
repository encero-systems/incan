"""Regression tests for the US-English guard: its word table, its word boundaries and its exceptions."""

from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))

import check_us_english  # noqa: E402


CHECKER = Path(__file__).with_name("check_us_english.py")


class WordTableTests(unittest.TestCase):
    """The table respells the UK families and leaves the words that end in `-ise` in both dialects alone."""

    def respell(self, line):
        return [(word, replacement) for _, _, word, replacement in check_us_english.hits(line)]

    def test_inflections_are_generated_from_the_base_forms(self):
        line = "behaviours initialisation SERIALISED catalogued cataloguing neighbouring colours centred analysed"
        self.assertEqual(
            [replacement for _, replacement in self.respell(line)],
            ["behaviors", "initialization", "SERIALIZED", "cataloged", "cataloging", "neighboring", "colors",
             "centered", "analyzed"],
        )

    def test_doubled_consonants_and_odd_nouns_are_listed_one_by_one(self):
        line = "labelled labelling modelled programme programmes whilst judgement acknowledgements licence"
        self.assertEqual(
            [replacement for _, replacement in self.respell(line)],
            ["labeled", "labeling", "modeled", "program", "programs", "while", "judgment", "acknowledgments",
             "license"],
        )

    def test_words_that_end_in_ise_in_both_dialects_are_untouched(self):
        line = ("advertise compromise exercise otherwise precise premise promise raise surprise wise noise cruise "
                "expertise enterprise analyses programmed programming")
        self.assertEqual(self.respell(line), [])

    def test_the_cancel_family_is_deliberately_kept(self):
        self.assertEqual(self.respell("cancelled Cancelled cancelling cancellation is_cancelled"), [])

    def test_case_is_preserved_and_mixed_case_is_left_alone(self):
        self.assertEqual(self.respell("colour Colour COLOUR cOLOUR"), [("colour", "color"), ("Colour", "Color"),
                                                                       ("COLOUR", "COLOR")])


class WordBoundaryTests(unittest.TestCase):
    """snake_case parts and CamelCase word starts are words; a run of letters inside a CamelCase word is not."""

    def words(self, line):
        return [word for _, _, word, _ in check_us_english.hits(line)]

    def test_snake_case_parts_and_camel_case_words_match(self):
        self.assertEqual(self.words("cli_catalogue_forms BackgroundColour colourValue COLOUR_MAP"),
                         ["catalogue", "Colour", "colour", "COLOUR"])

    def test_letters_inside_a_camel_case_word_do_not_match(self):
        self.assertEqual(self.words("EntityRef tyre_ref Tyre centreLine"), ["tyre", "Tyre", "centre"])


class CommandLineTests(unittest.TestCase):
    """Exercise real files and the allow file in an isolated repository."""

    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)
        self.write("scripts/check_us_english.allow", "")

    def write(self, path, text=""):
        destination = self.root / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(text, encoding="utf-8")
        subprocess.run(["git", "-C", str(self.root), "-c", "core.safecrlf=false", "add", path], check=True)

    def run_check(self, *arguments):
        return subprocess.run(
            [sys.executable, str(CHECKER), "--root", str(self.root), *arguments],
            capture_output=True, text=True, check=False,
        )

    def test_check_reports_path_line_word_and_replacement(self):
        self.write("src/lib.rs", "// the colour\nfn behaviour() {}\n")
        result = self.run_check()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("src/lib.rs:1: colour → color", result.stdout)
        self.assertIn("src/lib.rs:2: behaviour → behavior", result.stdout)

    def test_fix_rewrites_in_place_and_keeps_line_endings(self):
        self.write("doc.md", "The colour.\r\nThe Catalogue.\r\n")
        result = self.run_check("--fix")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("doc.md: 2", result.stdout)
        self.assertEqual((self.root / "doc.md").read_bytes(), b"The color.\r\nThe Catalog.\r\n")
        self.assertEqual(self.run_check().returncode, 0)

    def test_allow_file_exempts_a_path_a_word_or_an_enclosing_phrase(self):
        self.write("vendored/x.rs", "colour")
        self.write("src/task.rs", "error.is_favoured(); let favoured = 1; // labelled\n")
        self.write(
            "scripts/check_us_english.allow",
            "vendored/**\t*\tupstream\nsrc/task.rs\tis_favoured\tupstream name\n*\tlabelled\tdemo\n",
        )
        result = self.run_check()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(result.stdout.count("→"), 1)
        self.assertIn("src/task.rs:1: favoured → favored", result.stdout)

    def test_malformed_allow_line_is_a_configuration_error(self):
        self.write("scripts/check_us_english.allow", "no tabs here\n")
        result = self.run_check()
        self.assertEqual(result.returncode, 2)
        self.assertIn("expected path, token and reason", result.stderr)


if __name__ == "__main__":
    unittest.main()

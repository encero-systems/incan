"""Unit tests for check_changed_rustdocs.py; run with `python3 -m unittest scripts/test_check_changed_rustdocs.py`."""

import unittest

import check_changed_rustdocs as gate


def lines(source: str) -> list[str]:
    return source.strip("\n").split("\n")


class DocCommentTests(unittest.TestCase):
    def test_a_doc_block_directly_above_the_signature_counts(self):
        src = lines("/// Doc.\nfn f() {}")
        self.assertTrue(gate.has_doc_comment(src, 1))

    def test_a_single_line_attribute_between_doc_and_signature_is_stepped_over(self):
        src = lines("/// Doc.\n#[inline]\nfn f() {}")
        self.assertTrue(gate.has_doc_comment(src, 2))

    def test_a_multi_line_attribute_between_doc_and_signature_is_stepped_over(self):
        src = lines('/// Doc.\n#[allow(\n    dead_code,\n    reason = "later"\n)]\nfn f() {}')
        self.assertTrue(gate.has_doc_comment(src, 5))

    def test_an_undocumented_function_behind_a_multi_line_attribute_is_still_reported(self):
        src = lines('fn g() {}\n\n#[allow(\n    dead_code,\n    reason = "later"\n)]\nfn f() {}')
        self.assertFalse(gate.has_doc_comment(src, 6))

    def test_an_undocumented_function_is_reported(self):
        src = lines("fn g() {}\n\nfn f() {}")
        self.assertFalse(gate.has_doc_comment(src, 2))


if __name__ == "__main__":
    unittest.main()

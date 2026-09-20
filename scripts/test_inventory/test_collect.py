#!/usr/bin/env python3
"""Unit tests for the test-corpus scanner: masking, brace matching, test enumeration, signals and the gate."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import collect  # noqa: E402


class MaskRustTests(unittest.TestCase):
    """Strings, chars and comments must vanish so braces inside them cannot unbalance the scanner."""

    def test_strings_and_comments_become_spaces_but_newlines_survive(self) -> None:
        text = 'let a = "{"; // {\nlet b = r#"fn main() {"#; /* { */ let c = \'{\';\n'
        masked = collect.mask_rust(text)
        self.assertEqual(len(masked), len(text))
        self.assertEqual(masked.count("\n"), text.count("\n"))
        self.assertNotIn("{", masked)

    def test_lifetimes_are_not_char_literals(self) -> None:
        text = "fn f<'a>(x: &'a str) -> &'a str { x }"
        masked = collect.mask_rust(text)
        self.assertIn("{ x }", masked)
        self.assertIn("'a", masked)

    def test_escaped_chars_and_byte_strings(self) -> None:
        text = "let q = '\\''; let n = '\\n'; let b = b\"{\"; let u = '\\u{1F600}'; { }"
        masked = collect.mask_rust(text)
        self.assertEqual(masked.count("{"), 1)
        self.assertEqual(masked.count("}"), 1)

    def test_nested_block_comments(self) -> None:
        text = "/* outer /* inner { */ still } */ { }"
        masked = collect.mask_rust(text)
        self.assertEqual(masked.count("{"), 1)


class ScanFileTests(unittest.TestCase):
    """Test enumeration handles multi-line attributes, nested modules, string-embedded attributes and helpers."""

    SOURCE = '''
use x::y;

fn generate_rust(source: &str) -> String {
    IrCodegen::new().try_generate(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain() {
        let code = generate_rust("x");
        assert!(code.contains("fn main"));
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(
        not(any(a, b)),
        ignore = "reason"
    )]
    async fn gated() -> Result<(), Box<dyn std::error::Error>> {
        run_incan(&dir, &["run"])?;
        Ok(())
    }

    mod inner {
        #[test]
        fn plain() {
            let src = "#[test]\\nfn embedded() {}";
            let _ = src;
        }
    }
}
'''

    def test_enumerates_tests_with_qualified_duplicates(self) -> None:
        scanned = collect.scan_file("loaves/x/src/lib.rs", self.SOURCE)
        self.assertIsNotNone(scanned)
        assert scanned is not None
        self.assertEqual(scanned.keys, ["tests::plain", "gated", "tests::inner::plain"])
        self.assertEqual(scanned.anomalies, [])

    def test_signals_follow_file_local_helpers(self) -> None:
        scanned = collect.scan_file("loaves/x/src/lib.rs", self.SOURCE)
        assert scanned is not None
        by_key = {test.qualified: test for test in scanned.tests}
        self.assertIn("codegen", by_key["tests::plain"].lanes)
        self.assertIn("generated_text", by_key["tests::plain"].lanes)
        self.assertEqual(by_key["tests::plain"].via_helpers, ("generate_rust",))
        self.assertEqual(by_key["gated"].lanes, ("build_run",))
        self.assertEqual(by_key["tests::inner::plain"].lanes, ())

    def test_string_embedded_attribute_is_not_a_test(self) -> None:
        scanned = collect.scan_file("loaves/x/src/lib.rs", self.SOURCE)
        assert scanned is not None
        self.assertNotIn("embedded", [test.name for test in scanned.tests])

    def test_test_region_is_the_cfg_test_module_for_a_source_file(self) -> None:
        scanned = collect.scan_file("loaves/x/src/lib.rs", self.SOURCE)
        assert scanned is not None
        self.assertLess(scanned.test_lines, scanned.lines)
        whole = collect.scan_file("loaves/x/tests/lib_tests.rs", self.SOURCE)
        assert whole is not None
        self.assertEqual(whole.test_lines, whole.lines)

    def test_file_without_tests_is_skipped(self) -> None:
        self.assertIsNone(collect.scan_file("loaves/x/src/lib.rs", "fn main() {}\n"))


class GateTests(unittest.TestCase):
    """The gate names the exact failure for each inventory drift."""

    def corpus(self) -> collect.Corpus:
        scanned = collect.scan_file("loaves/x/src/lib.rs", ScanFileTests.SOURCE)
        assert scanned is not None
        return collect.Corpus(files=[scanned], fixture_roots=[])

    def test_unclassified_file_fails(self) -> None:
        failures = collect.check(self.corpus(), {"files": {}}, None)
        self.assertTrue(any("unclassified" in f and "loaves/x/src/lib.rs" in f for f in failures))

    def test_override_naming_a_missing_test_fails(self) -> None:
        dispositions = {
            "files": {
                "loaves/x/src/lib.rs": {"disposition": "keep", "tests": {"vanished": {"disposition": "retire"}}}
            }
        }
        failures = collect.check(self.corpus(), dispositions, None)
        self.assertTrue(any("vanished" in f for f in failures))

    def test_unresolvable_twin_fails_and_resolvable_twin_passes(self) -> None:
        base = {
            "disposition": "retire",
            "tests": {"gated": {"disposition": "keep"}},
        }
        bad = {"files": {"loaves/x/src/lib.rs": dict(base, twin="loaves/x/src/lib.rs::nope")}}
        self.assertTrue(any("twin" in f for f in collect.check(self.corpus(), bad, None)))
        good = {"files": {"loaves/x/src/lib.rs": dict(base, twin="loaves/x/src/lib.rs::gated")}}
        self.assertEqual(collect.check(self.corpus(), good, None), [])

    def test_twin_must_be_keep_or_re_point(self) -> None:
        dispositions = {
            "files": {
                "loaves/x/src/lib.rs": {
                    "disposition": "retire",
                    "twin": "loaves/x/src/lib.rs::gated",
                }
            }
        }
        failures = collect.check(self.corpus(), dispositions, None)
        self.assertTrue(any("not keep or re-point" in f for f in failures))

    def test_split_flag_must_match_the_measured_region(self) -> None:
        dispositions = {
            "split_threshold_lines": 5,
            "files": {"loaves/x/src/lib.rs": {"disposition": "keep", "split_required": False}},
        }
        failures = collect.check(self.corpus(), dispositions, None)
        self.assertTrue(any("split_required" in f for f in failures))

    def test_stale_row_and_stale_page_fail(self) -> None:
        dispositions = {
            "files": {
                "loaves/x/src/lib.rs": {"disposition": "keep"},
                "loaves/gone.rs": {"disposition": "keep"},
            }
        }
        failures = collect.check(self.corpus(), dispositions, "rendered page is stale")
        self.assertTrue(any("stale row" in f for f in failures))
        self.assertIn("rendered page is stale", failures)


if __name__ == "__main__":
    unittest.main()

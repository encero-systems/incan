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

    def test_test_region_is_the_cfg_test_module_whatever_the_file_is_called(self) -> None:
        scanned = collect.scan_file("loaves/x/src/lib.rs", self.SOURCE)
        assert scanned is not None
        self.assertLess(scanned.test_lines, scanned.lines)
        # A runner module named `*_test.rs` with a `#[cfg(test)]` region is measured by the region, not the file.
        runner = collect.scan_file("loaves/x/src/native_test.rs", self.SOURCE)
        assert runner is not None
        self.assertEqual(runner.test_lines, scanned.test_lines)

    def test_file_without_a_cfg_test_region_is_measured_whole(self) -> None:
        source = "use x::y;\n\n#[test]\nfn plain() {\n    assert!(true);\n}\n"
        whole = collect.scan_file("loaves/x/tests/lib_tests.rs", source)
        assert whole is not None
        self.assertEqual(whole.test_lines, whole.lines)

    def test_file_without_tests_is_skipped(self) -> None:
        self.assertIsNone(collect.scan_file("loaves/x/src/lib.rs", "fn main() {}\n"))

    def test_test_attribute_with_a_trailing_comment_is_counted(self) -> None:
        source = "#[test] // the only test attribute in the file\nfn commented() {\n    assert!(true);\n}\n"
        scanned = collect.scan_file("loaves/x/tests/commented_tests.rs", source)
        self.assertIsNotNone(scanned)
        assert scanned is not None
        self.assertEqual(scanned.keys, ["commented"])
        self.assertEqual(scanned.anomalies, [])

    def test_array_type_in_a_signature_is_not_a_bodiless_fn(self) -> None:
        source = (
            "trait T {\n    fn declared() -> [u8; 4];\n}\n\n"
            "#[test]\nfn sized() -> Result<(), Box<dyn std::error::Error>> {\n    let _: [u8; 4] = [0; 4];\n    Ok(())\n}\n\n"
            "#[test]\nfn arrayed() -> [u8; 4] {\n    [0; 4]\n}\n"
        )
        scanned = collect.scan_file("loaves/x/tests/array_tests.rs", source)
        assert scanned is not None
        self.assertEqual(scanned.keys, ["sized", "arrayed"])
        self.assertEqual(scanned.anomalies, [])
        self.assertEqual([name for name, _, _, _ in collect.function_spans(collect.mask_rust(source))], ["sized", "arrayed"])

    def test_cli_invocation_counts_as_build_run_only_beside_a_compiling_subcommand(self) -> None:
        frontend_only = (
            "#[test]\nfn fmt_only() {\n    let status = incan_command().arg(\"fmt\").arg(&path).status();\n"
            "    let _ = run_incan(&dir, &[\"check\", \"src/main.incn\"]);\n    let _ = incan_command().arg(\"--help\").output();\n}\n"
        )
        self.assertEqual(collect.lane_signals(frontend_only), {})
        compiling = "#[test]\nfn builds() {\n    let out = run_incan(&dir, &[\"build\", \"src/main.incn\"]);\n}\n"
        self.assertEqual(collect.lane_signals(compiling)["build_run"], 1)
        generated_read = (
            "#[test]\nfn reads() {\n    let out = incan_command().arg(\"lock\").output();\n"
            "    let rust = fs::read_to_string(dir.join(\"target/incan/app/src/main.rs\"));\n}\n"
        )
        signals = collect.lane_signals(generated_read)
        self.assertEqual(signals["build_run"], 1)
        self.assertEqual(signals["codegen"], 1)
        self.assertIn("codegen", collect.lane_signals("let out = run_incan(&dir, &[\"--emit-rust\", \"src/main.incn\"]);"))


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

    def test_dangling_twin_on_a_keep_row_fails_too(self) -> None:
        for disposition in ("keep", "re-point"):
            with self.subTest(disposition=disposition):
                dispositions = {
                    "files": {
                        "loaves/x/src/lib.rs": {"disposition": disposition, "twin": "loaves/x/src/lib.rs::vanished"}
                    }
                }
                failures = collect.check(self.corpus(), dispositions, None)
                self.assertEqual(len([f for f in failures if "twin" in f]), 1, failures)

    def test_dispositions_option_reads_another_record(self) -> None:
        import tempfile

        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as handle:
            handle.write('{"schema": 1, "fixture_roots": {}, "files": {}}')
        args = collect.parse_args(["--check", "--dispositions", handle.name])
        self.assertEqual(args.dispositions, Path(handle.name))
        self.assertEqual(collect.load_dispositions(args.dispositions)["files"], {})
        Path(handle.name).unlink()

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

    def test_dies_needs_a_retire_row_and_a_reason(self) -> None:
        corpus = self.corpus()
        # `gated` is an override with its own twin; `dies` there needs a reason on the override.
        missing_reason = {
            "files": {"loaves/x/src/lib.rs": {"disposition": "retire", "twin": "dies"}}
        }
        failures = collect.check(corpus, missing_reason, None)
        self.assertTrue(any('needs a `"dies": "<reason>"`' in f for f in failures), failures)
        on_keep = {
            "files": {"loaves/x/src/lib.rs": {"disposition": "keep", "twin": "dies", "dies": "generated project"}}
        }
        failures = collect.check(corpus, on_keep, None)
        self.assertTrue(any("only a retire test dies" in f for f in failures), failures)
        recorded = {
            "files": {"loaves/x/src/lib.rs": {"disposition": "retire", "twin": "dies", "dies": "inspect rust output"}}
        }
        self.assertEqual(collect.check(corpus, recorded, None), [])
        self.assertEqual(collect.retire_totals(corpus, recorded), {"dies": 3})
        stray_reason = {
            "files": {"loaves/x/src/lib.rs": {"disposition": "retire", "twin": "", "dies": "orphan reason"}}
        }
        failures = collect.check(corpus, stray_reason, None)
        self.assertTrue(any('needs `"twin": "dies"` beside it' in f for f in failures), failures)

    def test_file_level_dies_does_not_reach_an_override_with_another_disposition(self) -> None:
        dispositions = {
            "files": {
                "loaves/x/src/lib.rs": {
                    "disposition": "retire",
                    "twin": "dies",
                    "dies": "generated project shape",
                    "tests": {"gated": {"disposition": "keep"}},
                }
            }
        }
        corpus = self.corpus()
        self.assertEqual(collect.check(corpus, dispositions, None), [])
        self.assertEqual(collect.effective_twin(dispositions["files"]["loaves/x/src/lib.rs"], "gated"), "")
        self.assertEqual(collect.retire_totals(corpus, dispositions), {"dies": 2})

    def test_retire_totals_split_twinned_dies_and_open(self) -> None:
        dispositions = {
            "files": {
                "loaves/x/src/lib.rs": {
                    "disposition": "retire",
                    "twin": "",
                    "tests": {
                        "gated": {"disposition": "keep"},
                        "tests::plain": {"twin": "loaves/x/src/lib.rs::gated"},
                        "tests::inner::plain": {"twin": "dies", "dies": "data-structure invariant of a dying crate"},
                    },
                }
            }
        }
        corpus = self.corpus()
        self.assertEqual(collect.check(corpus, dispositions, None), [])
        self.assertEqual(collect.retire_totals(corpus, dispositions), {"twinned": 1, "dies": 1})

    def test_stale_row_and_stale_page_fail(self) -> None:
        dispositions = {
            "files": {
                "loaves/gone.rs": {"disposition": "keep"},
                "loaves/x/src/lib.rs": {"disposition": "keep"},
            }
        }
        failures = collect.check(self.corpus(), dispositions, "rendered page is stale")
        self.assertTrue(any("stale row" in f for f in failures))
        self.assertIn("rendered page is stale", failures)

    def test_dies_reason_beside_a_twin_is_reported_for_every_key(self) -> None:
        # Three tests share the file's twin; the stray `dies` reason is on the file row, so every key carries it.
        dispositions = {
            "files": {
                "loaves/x/src/lib.rs": {
                    "disposition": "retire",
                    "twin": "loaves/x/src/lib.rs::gated",
                    "dies": "a reason that contradicts the twin",
                    "tests": {"gated": {"disposition": "keep", "twin": "", "dies": ""}},
                }
            }
        }
        failures = [f for f in collect.check(self.corpus(), dispositions, None) if "beside a twin that is not" in f]
        self.assertEqual(
            sorted(failures),
            [
                "`loaves/x/src/lib.rs::tests::inner::plain`: a `dies` reason beside a twin that is not `dies`",
                "`loaves/x/src/lib.rs::tests::plain`: a `dies` reason beside a twin that is not `dies`",
            ],
        )

    def test_unsorted_record_is_refused_naming_the_first_pair_out_of_order(self) -> None:
        sorted_record = {
            "fixture_roots": {},
            "files": {"loaves/x/src/lib.rs": {"disposition": "keep"}},
        }
        self.assertEqual(collect.check(self.corpus(), sorted_record, None), [])
        unsorted_files = {
            "files": {
                "loaves/x/src/lib.rs": {"disposition": "keep"},
                "loaves/a.rs": {"disposition": "keep"},
            }
        }
        failures = collect.check(self.corpus(), unsorted_files, None)
        self.assertIn(
            "`files` is not sorted by key: `loaves/a.rs` comes after `loaves/x/src/lib.rs`; "
            "keep the record sorted so a row is found by position and merges stay clean",
            failures,
        )
        unsorted_roots = {
            "fixture_roots": {
                "loaves/b/fixtures": {"disposition": "keep"},
                "loaves/a/fixtures": {"disposition": "keep"},
            },
            "files": {"loaves/x/src/lib.rs": {"disposition": "keep"}},
        }
        failures = collect.check(self.corpus(), unsorted_roots, None)
        self.assertTrue(any(f.startswith("`fixture_roots` is not sorted by key: `loaves/a/fixtures`") for f in failures), failures)


class BehaviorFixtureTwinTests(unittest.TestCase):
    """A behaviour fixture and the row it retires must name each other, and a fixture twin must exist."""

    SOURCE = (
        "#[test]\nfn generated_shape() {\n    let code = generate_rust(\"x\");\n    assert!(code.contains(\"fn \"));\n}\n\n"
        "#[test]\nfn other_shape() {\n    let code = generate_rust(\"y\");\n    assert!(code.contains(\"impl \"));\n}\n"
    )

    def setUp(self) -> None:
        import tempfile

        self.tmp = tempfile.TemporaryDirectory()
        self.original_root = collect.ROOT
        collect.ROOT = Path(self.tmp.name)
        self.area = collect.BEHAVIOR_FIXTURES_ROOT + "/probe"
        area_dir = collect.ROOT / self.area
        area_dir.mkdir(parents=True)
        (area_dir / "shape.incn").write_text(
            "# behavior: prints the shape\n"
            "# retires: loaves/emit/src/codegen.rs::generated_shape\n"
            "# expect-stdout:\n#   shape\n\ndef main() -> None:\n    println(\"shape\")\n",
            encoding="utf-8",
        )
        (area_dir / "modules").mkdir()
        (area_dir / "modules" / "main.incn").write_text(
            "# behavior: modules\n# retires: loaves/emit/src/codegen.rs::other_shape\n# expect-exit: 0\n", encoding="utf-8"
        )
        scanned = collect.scan_file("loaves/emit/src/codegen.rs", self.SOURCE)
        assert scanned is not None
        self.scanned = scanned

    def tearDown(self) -> None:
        collect.ROOT = self.original_root
        self.tmp.cleanup()

    def dispositions(self, **overrides: dict) -> dict:
        return {
            "fixture_roots": {self.area: {"pattern": "<name>.incn or <name>/", "disposition": "re-point"}},
            "files": {
                "loaves/emit/src/codegen.rs": {
                    "disposition": "retire",
                    "twin": "",
                    "tests": overrides,
                }
            },
        }

    def corpus(self, dispositions: dict) -> collect.Corpus:
        return collect.Corpus(files=[self.scanned], fixture_roots=collect.count_fixture_roots(dispositions))

    def test_fixture_area_counts_one_case_per_fixture(self) -> None:
        roots = collect.count_fixture_roots(self.dispositions())
        self.assertEqual([(root.root, root.cases) for root in roots], [(self.area, 2)])

    def test_both_sides_agree(self) -> None:
        dispositions = self.dispositions(
            generated_shape={"twin": f"{self.area}/shape.incn"},
            other_shape={"twin": f"{self.area}/modules"},
        )
        self.assertEqual(collect.check(self.corpus(dispositions), dispositions, None), [])
        self.assertEqual(collect.retire_totals(self.corpus(dispositions), dispositions), {"twinned": 2})

    def test_fixture_that_names_a_test_whose_row_does_not_point_back_fails(self) -> None:
        dispositions = self.dispositions(other_shape={"twin": f"{self.area}/modules"})
        failures = collect.check(self.corpus(dispositions), dispositions, None)
        self.assertEqual(len(failures), 1, failures)
        self.assertIn("retires `loaves/emit/src/codegen.rs::generated_shape`, but that row's twin is nothing", failures[0])

    def test_row_that_points_at_a_fixture_which_does_not_name_it_fails(self) -> None:
        dispositions = self.dispositions(
            generated_shape={"twin": f"{self.area}/modules"},
            other_shape={"twin": f"{self.area}/modules"},
        )
        failures = collect.check(self.corpus(dispositions), dispositions, None)
        self.assertTrue(any("does not name it in a `# retires:` line" in f for f in failures), failures)
        self.assertTrue(any("retires `loaves/emit/src/codegen.rs::generated_shape`, but that row's twin is" in f for f in failures), failures)

    def test_missing_fixture_file_is_refused(self) -> None:
        dispositions = self.dispositions(
            generated_shape={"twin": f"{self.area}/shape.incn"},
            other_shape={"twin": f"{self.area}/vanished.incn"},
        )
        failures = collect.check(self.corpus(dispositions), dispositions, None)
        self.assertTrue(any("twin fixture" in f and "does not exist" in f for f in failures), failures)

    def test_fixture_retiring_a_keep_test_is_refused(self) -> None:
        dispositions = self.dispositions(
            generated_shape={"twin": f"{self.area}/shape.incn"},
            other_shape={"disposition": "keep"},
        )
        failures = collect.check(self.corpus(dispositions), dispositions, None)
        self.assertTrue(any("which is `keep`; only a retire test has a twin" in f for f in failures), failures)

    def test_malformed_retires_key_is_a_failure_line_not_a_traceback(self) -> None:
        (collect.ROOT / self.area / "broken.incn").write_text(
            "# behavior: broken\n# retires: not-a-test\n# retires: loaves/emit/src/code gen.rs::other_shape\n"
            "# retires: loaves/emit/src/codegen.rs::\n# retires: .rs::other_shape\n# expect-exit: 0\n",
            encoding="utf-8",
        )
        dispositions = self.dispositions(
            generated_shape={"twin": f"{self.area}/shape.incn"},
            other_shape={"twin": f"{self.area}/modules"},
        )
        failures = collect.check(self.corpus(dispositions), dispositions, None)
        self.assertIn(f"`{self.area}/broken.incn` retires `not-a-test`, which is not `<path>.rs::<fn>`", failures)
        self.assertIn(f"`{self.area}/broken.incn` retires `loaves/emit/src/codegen.rs::`, which is not `<path>.rs::<fn>`", failures)
        self.assertIn(f"`{self.area}/broken.incn` retires `.rs::other_shape`, which is not `<path>.rs::<fn>`", failures)
        # The value with whitespace is read whole and refused for it, so the runner and the collector agree.
        self.assertIn(
            f"`{self.area}/broken.incn` retires `loaves/emit/src/code gen.rs::other_shape`, which is not `<path>.rs::<fn>` (no whitespace)",
            failures,
        )
        # A malformed value never reaches the map, so the well-formed rows are judged on their own and pass.
        self.assertEqual(len(failures), 4, failures)

    def test_retires_line_is_read_only_as_the_runner_reads_it(self) -> None:
        header = collect.ROOT / self.area / "spelling.incn"
        header.write_text(
            "# behavior: spelling\n"
            "#retires: loaves/emit/src/codegen.rs::no_space\n"
            "# retires:   loaves/emit/src/codegen.rs::padded   \n"
            "#  retires: loaves/emit/src/codegen.rs::two_spaces_is_a_block_item\n"
            "#\tretires: loaves/emit/src/codegen.rs::tab\n"
            "# retires : loaves/emit/src/codegen.rs::space_before_colon\n"
            "# expect-exit: 0\n"
            "\n"
            "# retires: loaves/emit/src/codegen.rs::after_the_header\n",
            encoding="utf-8",
        )
        self.assertEqual(
            collect.read_retires(header),
            ["loaves/emit/src/codegen.rs::no_space", "loaves/emit/src/codegen.rs::padded"],
        )

    def test_retires_key_problem_mirrors_the_runner(self) -> None:
        self.assertIsNone(collect.retires_key_problem("loaves/a.rs::t"))
        self.assertIsNone(collect.retires_key_problem("loaves/a.rs::tests::inner::t"))
        for value in ("", "not-a-test", "a.rs::", "::t", ".rs::t", "a.txt::t", "a b.rs::t", "a.rs::t u"):
            with self.subTest(value=value):
                self.assertIsNotNone(collect.retires_key_problem(value))

    def test_two_fixtures_retiring_one_test_is_refused(self) -> None:
        (collect.ROOT / self.area / "dup.incn").write_text(
            "# behavior: dup\n# retires: loaves/emit/src/codegen.rs::generated_shape\n# expect-exit: 0\n", encoding="utf-8"
        )
        dispositions = self.dispositions(
            generated_shape={"twin": f"{self.area}/shape.incn"},
            other_shape={"twin": f"{self.area}/modules"},
        )
        failures = collect.check(self.corpus(dispositions), dispositions, None)
        self.assertTrue(any("both retire" in f for f in failures), failures)


if __name__ == "__main__":
    unittest.main()

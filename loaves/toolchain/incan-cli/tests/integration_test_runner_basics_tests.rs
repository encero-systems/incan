#![allow(dead_code)] // Split test binaries intentionally use different parts of shared support fragments.

//! Integration tests for the Incan compiler frontend

include!("support/integration_tests_root.rs");

mod test_runner_e2e {
    include!("support/integration_tests_test_runner_e2e.rs");

    // ---- Passing test ----

    /// Issue #815: generic and `Self`-returning Index adoptions must compile in the generated test package.
    #[test]
    fn e2e_issue815_generic_index_trait_adoptions_compile() {
        let dir = write_test_project(
            "test_issue815_generic_index.incn",
            r#"
from std.testing import assert_eq
from std.traits.indexing import Index

class GenericBox[T with Clone] with Index[str, str]:
    pub label: str
    pub witness: list[T]

    def __getitem__(self, key: str) for Index[str, str] -> str:
        return key

class PlainBox with Index[list[str], Self]:
    pub label: str

    def __getitem__(self, key: list[str]) for Index[list[str], Self] -> Self:
        return self

class GenericSelfBox[T with Clone] with Index[list[str], Self]:
    pub label: str
    pub witness: list[T]

    def __getitem__(self, key: list[str]) for Index[list[str], Self] -> Self:
        return self

def test_generic_index() -> None:
    box = GenericBox[int](label="orders", witness=[1])
    assert_eq(box["amount"], "amount")

def test_self_returning_index() -> None:
    box = PlainBox(label="nested")
    assert_eq(box[["name"]].label, "nested")

def test_generic_self_returning_index() -> None:
    box = GenericSelfBox[int](label="nested-generic", witness=[1])
    assert_eq(box[["name"]].label, "nested-generic")
"#,
        );

        let output = run_incan_test(&dir);
        assert!(
            output.status.success(),
            "expected issue #815 test package to compile and pass.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn e2e_basic_reporting_decorator_filter_and_capture_share_one_project() {
        let dir = write_test_project(
            "test_runner_surface.incn",
            r#"
from std.testing import assert_eq, test

def test_addition() -> None:
    assert_eq(1 + 1, 2)

def test_one() -> None:
    assert_eq(1, 1)

def test_two() -> None:
    assert_eq(2, 2)

@test
def verifies_total() -> None:
    assert_eq(40 + 2, 42)

def test_alpha() -> None:
    assert_eq(1, 1)

def test_beta() -> None:
    assert_eq(2, 2)

def test_prints() -> None:
    print("VISIBLE_CAPTURE")
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected both tests to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("PASSED") || stdout.contains("passed"),
            "expected PASSED in output.\nstdout:\n{}",
            stdout,
        );
        assert!(
            stdout.contains("test_runner_surface.incn::test_one")
                && stdout.contains("test_runner_surface.incn::test_two")
                && stdout.contains("test_runner_surface.incn::verifies_total"),
            "expected basic and decorated test names in reporter output.\nstdout:\n{}",
            stdout,
        );
        assert!(
            stdout.match_indices("PASSED").count() >= 6,
            "expected passing result lines for all basic surface tests.\nstdout:\n{}",
            stdout,
        );

        // Stable root-relative IDs and keyword selection are pure runner rules.
        // The marker collection matrix retains the single CLI-level list path.
        let captured = run_incan_test_with_args(&dir, &["--nocapture", "-k", "test_prints"]);
        let captured_stdout = String::from_utf8_lossy(&captured.stdout);
        let captured_stderr = String::from_utf8_lossy(&captured.stderr);
        assert!(
            captured.status.success(),
            "expected nocapture run to succeed.\nstdout:\n{}\nstderr:\n{}",
            captured_stdout,
            captured_stderr,
        );
        assert!(captured_stdout.contains("VISIBLE_CAPTURE"));
    }

    #[test]
    fn e2e_generated_harness_success_reports_the_incan_test_identity_issue996() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = write_test_project(
            "loaf.toml",
            "[project]\nname = \"test_runner_repro\"\nversion = \"0.1.0\"\n",
        );
        std::fs::create_dir_all(dir.join("src"))?;
        std::fs::create_dir_all(dir.join("tests"))?;
        std::fs::write(dir.join("src/lib.incn"), "pub def answer() -> int:\n  return 42\n")?;
        std::fs::write(
            dir.join("tests/test_smoke.incn"),
            "def test_smoke__reports_pass() -> None:\n  assert 42 == 42\n",
        )?;

        let output = run_incan_test_relative(&dir, "tests");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected the passing generated harness to preserve its Incan result.\nstdout:\n{stdout}\nstderr:\n{stderr}",
        );
        assert!(
            stdout.contains("test_smoke.incn::test_smoke__reports_pass") && stdout.contains("PASSED"),
            "expected the runner to report the collected Incan identity as passing.\nstdout:\n{stdout}",
        );
        assert!(
            !stdout.contains("Test runner did not report outcome"),
            "a successful native harness must not become a missing-outcome failure.\nstdout:\n{stdout}",
        );
        Ok(())
    }

    #[test]
    fn e2e_generated_harness_oven_bake_is_reused() {
        let dir = write_test_project(
            "test_oven_bake_reuse.incn",
            r#"
from std.testing import assert_eq

def test_oven_bake_reuse() -> None:
    assert_eq(1, 1)
"#,
        );

        let first = run_incan_test_with_args(&dir, &["-v"]);
        let first_stdout = String::from_utf8_lossy(&first.stdout);
        let first_stderr = String::from_utf8_lossy(&first.stderr);
        assert!(
            first.status.success(),
            "expected first Oven run to succeed.\nstdout:\n{}\nstderr:\n{}",
            first_stdout,
            first_stderr,
        );
        assert!(
            first_stdout.contains("Oven test phases"),
            "expected verbose first run to report Oven phases.\nstdout:\n{}",
            first_stdout,
        );
        assert!(
            first_stdout.contains("planned 1 generated harness unit(s)"),
            "expected verbose run to report generated harness planning.\nstdout:\n{}",
            first_stdout,
        );
        assert!(
            first_stdout.contains("native bake"),
            "expected first run to bake its caller-owned native output.\nstdout:\n{}",
            first_stdout,
        );

        let second = run_incan_test_with_args(&dir, &["-v"]);
        let second_stdout = String::from_utf8_lossy(&second.stdout);
        let second_stderr = String::from_utf8_lossy(&second.stderr);
        assert!(
            second.status.success(),
            "expected second Oven run to succeed.\nstdout:\n{}\nstderr:\n{}",
            second_stdout,
            second_stderr,
        );
        assert!(
            second_stdout.contains("native reuse"),
            "expected second run to reuse its stored native output.\nstdout:\n{}",
            second_stdout,
        );
    }

    #[test]
    fn e2e_cross_file_batch_falls_back_when_top_level_names_collide() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_a.incn",
            r#"
from std.testing import assert_eq

model Order:
    id: int

def test_a() -> None:
    order = Order(id=1)
    assert_eq(order.id, 1)
"#,
        );
        std::fs::write(
            dir.join("test_b.incn"),
            r#"
from std.testing import assert_eq

model Order:
    id: int

def test_b() -> None:
    order = Order(id=2)
    assert_eq(order.id, 2)
"#,
        )?;

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected same-named top-level declarations in different files to run in isolated harnesses.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("test_a.incn::test_a") && stdout.contains("test_b.incn::test_b"),
            "expected both tests in reporter output.\nstdout:\n{}",
            stdout,
        );
        Ok(())
    }

    #[test]
    fn e2e_cross_file_batch_rebases_spans_for_type_info_issue692() -> Result<(), Box<dyn std::error::Error>> {
        fn source_with_call_offset(header: &str, call_prefix: &str, call_and_tail: &str, offset: usize) -> String {
            let fixed_len = header.len() + call_prefix.len();
            assert!(
                offset >= fixed_len + 6,
                "test fixture offset leaves no room for padding"
            );
            let padding = format!("    #{}\n", "x".repeat(offset - fixed_len - 6));
            format!("{header}{padding}{call_prefix}{call_and_tail}")
        }

        let target_offset = 320;
        let dir = write_test_project(
            "test_constructor_marker.incn",
            &source_with_call_offset(
                "model Box:\n    value: int\n\ndef test_type_constructor() -> None:\n",
                "    item = ",
                "Box(value=1)\n    assert item.value == 1\n",
                target_offset,
            ),
        );
        std::fs::write(
            dir.join("test_zero_arg_call.incn"),
            source_with_call_offset(
                "def tap() -> str:\n    return \"ok\"\n\ndef test_zero_arg_call_in_list() -> None:\n",
                "    values = [",
                "tap()]\n    assert values[0] == \"ok\"\n",
                target_offset,
            ),
        )?;

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected same-span constructor and zero-argument calls from different files not to share type-info facts.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("test_constructor_marker.incn::test_type_constructor")
                && stdout.contains("test_zero_arg_call.incn::test_zero_arg_call_in_list"),
            "expected both files to run in one directory test batch.\nstdout:\n{}",
            stdout,
        );
        Ok(())
    }

    #[test]
    fn e2e_imported_default_expression_expands_with_required_scope_issue395() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "default_expr_import_test_repro"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        let tests_dir = dir.join("tests");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&tests_dir)?;
        std::fs::write(
            src_dir.join("defaults.incn"),
            r#"
pub def fallback() -> int:
    return 2
"#,
        )?;
        std::fs::write(
            src_dir.join("helper.incn"),
            r#"
from defaults import fallback

pub def combine(left: int, middle: int = fallback(), right: int = 3) -> int:
    return left + middle + right
"#,
        )?;
        std::fs::write(
            tests_dir.join("test_default_expr_import.incn"),
            r#"
from std.testing import assert_eq
from helper import combine

def test_imported_default_expression_expands_with_required_imports() -> None:
    assert_eq(combine(left=1, right=4), 7, "default expression helper should be available after expansion")
"#,
        )?;

        let output = run_incan_test_relative(&dir, "tests");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected imported default expression test to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains(
                "test_default_expr_import.incn::test_imported_default_expression_expands_with_required_imports"
            ),
            "expected issue 395 test name in reporter output.\nstdout:\n{}",
            stdout,
        );
        Ok(())
    }

    #[test]
    fn e2e_report_formats_share_one_project() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_report_formats.incn",
            r#"
from std.testing import assert_eq

def test_report_one() -> None:
    assert_eq(1, 1)
"#,
        );

        let json_output = run_incan_test_with_args(&dir, &["--format", "json", "--shuffle", "--seed", "7"]);
        let json_stdout = String::from_utf8_lossy(&json_output.stdout);
        let json_stderr = String::from_utf8_lossy(&json_output.stderr);
        assert!(
            json_output.status.success(),
            "expected JSON-format run to succeed.\nstdout:\n{}\nstderr:\n{}",
            json_stdout,
            json_stderr,
        );

        let mut saw_result = false;
        let mut saw_summary = false;
        for line in json_stdout.lines().filter(|line| !line.trim().is_empty()) {
            let value: serde_json::Value = serde_json::from_str(line)?;
            if value.get("test_id").is_some() {
                saw_result = true;
                assert_eq!(
                    value.get("schema_version").and_then(|v| v.as_str()),
                    Some("incan.test.v1")
                );
                assert_eq!(
                    value.get("test_id").and_then(|v| v.as_str()),
                    Some("test_report_formats.incn::test_report_one")
                );
                assert_eq!(value.get("status").and_then(|v| v.as_str()), Some("passed"));
            }
            if value.get("summary").is_some() {
                saw_summary = true;
                assert_eq!(
                    value
                        .get("summary")
                        .and_then(|summary| summary.get("shuffle_seed"))
                        .and_then(|v| v.as_u64()),
                    Some(7)
                );
            }
        }
        assert!(
            saw_result,
            "expected at least one JSON result record.\nstdout:\n{}",
            json_stdout
        );
        assert!(saw_summary, "expected a JSON summary record.\nstdout:\n{}", json_stdout);

        let report = dir.join("reports").join("junit.xml");
        let report_arg = report.to_string_lossy().to_string();
        let junit_output = run_incan_test_with_args(&dir, &["--junit", report_arg.as_str()]);
        let junit_stdout = String::from_utf8_lossy(&junit_output.stdout);
        let junit_stderr = String::from_utf8_lossy(&junit_output.stderr);
        assert!(
            junit_output.status.success(),
            "expected JUnit report run to succeed.\nstdout:\n{}\nstderr:\n{}",
            junit_stdout,
            junit_stderr,
        );
        let xml = std::fs::read_to_string(&report)?;
        assert!(
            xml.contains("<testsuite") && xml.contains("test_report_one"),
            "expected JUnit XML with test case, got:\n{}",
            xml,
        );
        Ok(())
    }

    #[test]
    fn e2e_run_xfail_treats_xfail_as_ordinary_test() {
        let dir = write_test_project(
            "test_run_xfail.incn",
            r#"
from std.testing import assert_eq, xfail

@xfail("currently passes")
def test_xpass() -> None:
    assert_eq(1, 1)
"#,
        );

        let default = run_incan_test(&dir);
        let default_stdout = String::from_utf8_lossy(&default.stdout);
        let default_stderr = String::from_utf8_lossy(&default.stderr);
        assert!(
            !default.status.success(),
            "expected default xpass to fail.\nstdout:\n{}\nstderr:\n{}",
            default_stdout,
            default_stderr,
        );

        let run_xfail = run_incan_test_with_args(&dir, &["--run-xfail"]);
        let stdout = String::from_utf8_lossy(&run_xfail.stdout);
        let stderr = String::from_utf8_lossy(&run_xfail.stderr);
        assert!(
            run_xfail.status.success(),
            "expected --run-xfail to treat xfail marker as ordinary.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("test_run_xfail.incn::test_xpass") && stdout.contains("PASSED"),
            "expected ordinary passing output.\nstdout:\n{}",
            stdout,
        );
    }

    #[test]
    fn e2e_conftest_nearest_fixture_override_project() {
        let override_dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "nested_conftest_precedence"
version = "0.1.0"
"#,
        );
        let override_tests_dir = override_dir.join("tests");
        let override_unit_dir = override_tests_dir.join("unit");
        if let Err(err) = std::fs::create_dir_all(&override_unit_dir) {
            panic!("failed to create nested tests dir: {}", err);
        }
        if let Err(err) = std::fs::write(
            override_tests_dir.join("conftest.incn"),
            r#"
from std.testing import fixture

@fixture
def shared() -> str:
    return "parent"
"#,
        ) {
            panic!("failed to write parent conftest: {}", err);
        }
        if let Err(err) = std::fs::write(
            override_unit_dir.join("conftest.incn"),
            r#"
from std.testing import fixture

@fixture
def shared() -> str:
    return "child"
"#,
        ) {
            panic!("failed to write nested conftest: {}", err);
        }
        if let Err(err) = std::fs::write(
            override_unit_dir.join("test_precedence.incn"),
            r#"
from std.testing import assert_eq

def test_uses_nearest_fixture(shared: str) -> None:
    assert_eq(shared, "child")
"#,
        ) {
            panic!("failed to write nested conftest test: {}", err);
        }

        let output = run_incan_test(&override_dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected nearest conftest fixture to override parent fixture without duplicate generated functions.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr
        );
        assert!(stdout.contains("test_uses_nearest_fixture"));
    }

    #[test]
    fn e2e_builtin_fixture_and_assert_helper_share_one_project() {
        let dir = write_test_project(
            "test_builtin_fixture_and_assert_helper.incn",
            r#"
from std.testing import assert_eq
import std.testing as testing
from rust::std::path import PathBuf

def test_tmp_path_fixture(tmp_path: PathBuf) -> None:
    assert_eq(tmp_path.exists(), true)

def test_assert_helper() -> None:
    testing.assert(True)
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected built-in tmp_path fixture to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(stdout.contains("test_assert_helper"));
    }

    #[test]
    fn e2e_markers_parametrize_timeout_and_collection_errors_share_projects() {
        let platform = std::env::consts::OS;
        let dir = write_test_project(
            "test_runner_collection_surface.incn",
            &format!(
                r#"
from rust::std::thread import sleep
from rust::std::time import Duration
from std.testing import assert_eq, feature, mark, param_case, parametrize, platform, skipif, slow, timeout, xfail, xfailif

const TEST_MARKERS: FrozenList[str] = ["api", "db", "smoke"]
const TEST_MARKS: FrozenList[str] = ["smoke"]

def test_inherited_smoke() -> None:
    assert_eq(1, 1)

@mark("api")
def test_api() -> None:
    assert_eq(1, 1)

@mark("api")
@slow
def test_api_slow() -> None:
    assert_eq(1, 1)

@mark("db")
def test_db() -> None:
    assert_eq(1, 1)

def test_fast() -> None:
    assert_eq(1, 1)

@slow
def test_slow_case() -> None:
    assert_eq(1, 1)

@parametrize("x, expected", [
    param_case((1, 3), marks=[xfail("known")], id="one-three"),
    (2, 4),
], ids=["ignored", "two-four"])
def test_marked_double(x: int, expected: int) -> None:
    assert_eq(x * 2, expected)

@parametrize("x", [1, 2], ids=["one", "two"])
@parametrize("y", [10, 20], ids=["ten", "twenty"])
def test_pair(x: int, y: int) -> None:
    assert_eq(x < y, true)

@parametrize("a, b, expected", [(1, 2, 3), (10, 20, 30), (0, 0, 0)])
def test_add(a: int, b: int, expected: int) -> None:
    assert_eq(a + b, expected)

@parametrize("x, expected", [(2, 4), (3, 7)])
def test_double_failure(x: int, expected: int) -> None:
    assert_eq(x * 2, expected)

@skipif(platform() == "{platform}", reason="host platform")
def test_skip_on_platform_probe() -> None:
    assert_eq(1, 0)

@xfailif(feature("known_bug"), reason="feature-gated known issue")
def test_feature_xfail() -> None:
    assert_eq(1, 0)

@timeout("1ms")
def test_timeout_marker() -> None:
    sleep(Duration.from_millis(100))
"#
            ),
        );

        // One list invocation exercises CLI-to-runner marker wiring. Pure parser, strict-registration, keyword, and
        // slow-selection combinations are unit tested below the process boundary rather than rebuilding this project.
        let marker_list = run_incan_test_with_args(
            &dir,
            &["--list", "-m", "api and not slow", "--strict-markers", "--slow"],
        );
        let marker_stdout = String::from_utf8_lossy(&marker_list.stdout);
        let marker_stderr = String::from_utf8_lossy(&marker_list.stderr);
        assert!(
            marker_list.status.success(),
            "expected boolean marker expression to collect.\nstdout:\n{}\nstderr:\n{}",
            marker_stdout,
            marker_stderr,
        );
        assert!(marker_stdout.contains("test_runner_collection_surface.incn::test_api"));
        assert!(!marker_stdout.contains("test_runner_collection_surface.incn::test_api_slow"));
        assert!(!marker_stdout.contains("test_runner_collection_surface.incn::test_db"));

        // This one ordinary execution covers the outcome matrix below. Its constituent cases previously rebuilt the
        // same generated test project five times with different keyword filters, despite no filter-specific behavior
        // being under test here.
        let ordinary = run_incan_test_with_args(&dir, &["--verbose"]);
        let ordinary_stdout = String::from_utf8_lossy(&ordinary.stdout);
        let ordinary_stderr = String::from_utf8_lossy(&ordinary.stderr);
        let ordinary_combined = format!("{ordinary_stdout}\n{ordinary_stderr}");
        assert!(
            !ordinary.status.success(),
            "expected the ordinary matrix to report its failing parameter, feature-disabled xfail, and timeout cases.\n{}",
            ordinary_combined,
        );
        assert!(
            ordinary_combined.contains("xfailed") || ordinary_combined.contains("XFAIL"),
            "expected the marked parametrized case to remain xfailed.\n{}",
            ordinary_combined,
        );
        for expected in [
            "test_add[1-2-3]",
            "test_add[10-20-30]",
            "test_add[0-0-0]",
            "test_double_failure[3-7]",
            "test_skip_on_platform_probe",
            "test_feature_xfail",
            "test_timeout_marker",
            "timed out after",
        ] {
            assert!(
                ordinary_combined.contains(expected),
                "expected ordinary outcome matrix to report `{expected}`.\n{}",
                ordinary_combined,
            );
        }
        assert!(
            ordinary_combined.contains("SKIPPED") || ordinary_combined.contains("skipped"),
            "expected the host-platform skip to remain reported.\n{}",
            ordinary_combined,
        );

        // Parsing `--feature` and turning a true `xfailif(feature(...))` into the runner's XFail marker are direct
        // CLI/discovery contracts. The ordinary execution above already proves that an XFail marker renders correctly
        // in the generated runner, so it need not rebuild this same project with a one-test keyword filter.
    }
}

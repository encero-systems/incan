//! Integration tests for the Incan compiler frontend

include!("support/integration_tests_root.rs");

mod test_runner_e2e {
    include!("support/integration_tests_test_runner_e2e.rs");

    #[test]
    fn e2e_jobs_fail_fast_stops_launching_pending_units() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_a_fail.incn",
            r#"
def test_a_fail() -> None:
    assert 1 == 2

def test_c_pending() -> None:
    pass
"#,
        );
        std::fs::write(
            dir.join("test_b_slow.incn"),
            r#"
from rust::std::thread import sleep
from rust::std::time import Duration

def test_b_slow() -> None:
    sleep(Duration.from_millis(800))
"#,
        )?;
        let output = run_incan_test_with_args(&dir, &["--jobs", "2", "-x"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "expected fail-fast run to fail.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("test_a_fail"),
            "expected failing test to be reported.\nstdout:\n{}",
            stdout,
        );
        assert!(
            !stdout.contains("test_c_pending"),
            "expected fail-fast scheduler not to launch pending units after the first completed failure.\nstdout:\n{}",
            stdout,
        );
        Ok(())
    }

    #[test]
    fn e2e_jobs_run_independent_files_concurrently() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_sleep_a.incn",
            "from rust::std::thread import sleep\nfrom rust::std::time import Duration\n\ndef test_sleep_a() -> None:\n    sleep(Duration.from_millis(600))\n",
        );
        std::fs::write(
            dir.join("test_sleep_b.incn"),
            "from rust::std::thread import sleep\nfrom rust::std::time import Duration\n\ndef test_sleep_b() -> None:\n    sleep(Duration.from_millis(600))\n",
        )?;
        let output = run_incan_test_with_args(&dir, &["--jobs", "2"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "parallel run failed:\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let running_a = stdout
            .find("test_sleep_a.incn (1 item(s))")
            .ok_or("missing sleep_a start")?;
        let running_b = stdout
            .find("test_sleep_b.incn (1 item(s))")
            .ok_or("missing sleep_b start")?;
        let passed_a = stdout
            .find("test_sleep_a.incn::test_sleep_a PASSED")
            .ok_or("missing sleep_a pass")?;
        let passed_b = stdout
            .find("test_sleep_b.incn::test_sleep_b PASSED")
            .ok_or("missing sleep_b pass")?;
        assert!(
            running_a < passed_a.min(passed_b) && running_b < passed_a.min(passed_b),
            "--jobs 2 did not start both independent batches before either completed:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn e2e_sequential_single_file_runs_do_not_cross_wire_paths() {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "session_isolation_relative"
version = "0.1.0"
"#,
        );
        let tests_dir = dir.join("tests");
        if let Err(err) = std::fs::create_dir_all(&tests_dir) {
            panic!("failed to create tests dir: {}", err);
        }
        if let Err(err) = std::fs::write(
            tests_dir.join("test_alpha.incn"),
            r#"
from std.testing import assert_eq

def test_alpha_one() -> None:
    assert_eq(1, 1)

def test_alpha_two() -> None:
    assert_eq(2, 2)
"#,
        ) {
            panic!("failed to write test_alpha.incn: {}", err);
        }
        if let Err(err) = std::fs::write(
            tests_dir.join("test_beta.incn"),
            r#"
from std.testing import assert_eq

def test_beta_only() -> None:
    assert_eq(3, 3)
"#,
        ) {
            panic!("failed to write test_beta.incn: {}", err);
        }

        let first = run_incan_test_relative(&dir, "tests/test_alpha.incn");
        let first_stdout = String::from_utf8_lossy(&first.stdout);
        let first_stderr = String::from_utf8_lossy(&first.stderr);
        assert!(
            first.status.success(),
            "expected first single-file run to succeed.\nstdout:\n{}\nstderr:\n{}",
            first_stdout,
            first_stderr,
        );

        let second = run_incan_test_relative(&dir, "tests/test_beta.incn");
        let second_stdout = String::from_utf8_lossy(&second.stdout);
        let second_stderr = String::from_utf8_lossy(&second.stderr);
        let second_combined = format!("{second_stdout}\n{second_stderr}");
        assert!(
            second.status.success(),
            "expected second single-file run to succeed.\nstdout:\n{}\nstderr:\n{}",
            second_stdout,
            second_stderr,
        );
        assert!(
            second_combined.contains("test_beta.incn::test_beta_only"),
            "expected the requested beta test to run.\noutput:\n{}",
            second_combined,
        );
        assert!(
            !second_combined.contains("test_alpha.incn::test_alpha_one")
                && !second_combined.contains("test_alpha.incn::test_alpha_two"),
            "expected no alpha tests in second single-file run.\noutput:\n{}",
            second_combined,
        );
        assert!(
            !second_combined.contains("Test runner did not report outcome"),
            "expected no missing-outcome diagnostic in second run.\noutput:\n{}",
            second_combined,
        );

        let alpha_absolute_path = tests_dir.join("test_alpha_abs.incn");
        let beta_absolute_path = tests_dir.join("test_beta_abs.incn");
        if let Err(err) = std::fs::write(
            &alpha_absolute_path,
            r#"
from std.testing import assert_eq

def test_alpha_abs_one() -> None:
    assert_eq(10, 10)
"#,
        ) {
            panic!("failed to write test_alpha_abs.incn: {}", err);
        }
        if let Err(err) = std::fs::write(
            &beta_absolute_path,
            r#"
from std.testing import assert_eq

def test_beta_abs_only() -> None:
    assert_eq(20, 20)
"#,
        ) {
            panic!("failed to write test_beta_abs.incn: {}", err);
        }

        let first = run_incan_test_path(&alpha_absolute_path);
        let first_stdout = String::from_utf8_lossy(&first.stdout);
        let first_stderr = String::from_utf8_lossy(&first.stderr);
        assert!(
            first.status.success(),
            "expected first absolute-path run to succeed.\nstdout:\n{}\nstderr:\n{}",
            first_stdout,
            first_stderr,
        );

        let second = run_incan_test_path(&beta_absolute_path);
        let second_stdout = String::from_utf8_lossy(&second.stdout);
        let second_stderr = String::from_utf8_lossy(&second.stderr);
        let second_combined = format!("{second_stdout}\n{second_stderr}");
        assert!(
            second.status.success(),
            "expected second absolute-path run to succeed.\nstdout:\n{}\nstderr:\n{}",
            second_stdout,
            second_stderr,
        );
        assert!(
            second_combined.contains("test_beta_abs.incn::test_beta_abs_only"),
            "expected the requested absolute-path beta test to run.\noutput:\n{}",
            second_combined,
        );
        assert!(
            !second_combined.contains("test_alpha_abs.incn::test_alpha_abs_one"),
            "expected no alpha absolute-path tests in second run.\noutput:\n{}",
            second_combined,
        );
        assert!(
            !second_combined.contains("Test runner did not report outcome"),
            "expected no missing-outcome diagnostic in second absolute-path run.\noutput:\n{}",
            second_combined,
        );
    }

    #[test]
    fn e2e_nested_package_modules_in_tests_succeed() {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "nested_test"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        let tests_dir = dir.join("tests");

        if let Err(err) = std::fs::create_dir_all(src_dir.join("dataset")) {
            panic!("failed to create nested src dirs: {}", err);
        }
        if let Err(err) = std::fs::create_dir_all(&tests_dir) {
            panic!("failed to create tests dir: {}", err);
        }
        if let Err(err) = std::fs::write(
            src_dir.join("dataset").join("mod.incn"),
            "pub const DATASET_VERSION: int = 1\n",
        ) {
            panic!("failed to write dataset mod source: {}", err);
        }
        if let Err(err) = std::fs::write(
            src_dir.join("dataset").join("ops.incn"),
            "from dataset import DATASET_VERSION\npub def filter_ds(value: int) -> int:\n    return value + DATASET_VERSION\n",
        ) {
            panic!("failed to write dataset ops source: {}", err);
        }
        if let Err(err) = std::fs::write(
            tests_dir.join("test_dataset.incn"),
            r#"
from std.testing import assert_eq
from dataset import DATASET_VERSION
from dataset.ops import filter_ds

def test_nested_dataset_modules() -> None:
    assert_eq(DATASET_VERSION, 1)
    assert_eq(filter_ds(41), 42)
"#,
        ) {
            panic!("failed to write nested dataset test: {}", err);
        }

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected nested package module test to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            !stderr.contains("file for module `dataset` found at both"),
            "expected no stale flat-vs-nested module collision.\nstderr:\n{}",
            stderr,
        );
    }

    #[test]
    fn e2e_test_runner_preserves_fixture_cwd_for_file_and_batch_runs() {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "fixture_cwd_parity"
version = "0.1.0"
"#,
        );
        let tests_dir = dir.join("tests");
        let fixtures_dir = tests_dir.join("fixtures");

        if let Err(err) = std::fs::create_dir_all(&fixtures_dir) {
            panic!("failed to create fixture dir: {}", err);
        }
        if let Err(err) = std::fs::write(fixtures_dir.join("orders.csv"), "id\n1\n") {
            panic!("failed to write fixture file: {}", err);
        }
        if let Err(err) = std::fs::write(
            tests_dir.join("test_fixture_path.incn"),
            r#"
from std.testing import assert_eq
from rust::std::path import Path

const FIXTURE: str = "tests/fixtures/orders.csv"

def test_fixture_path_exists() -> None:
    assert_eq(Path.new(FIXTURE).exists(), true)
"#,
        ) {
            panic!("failed to write fixture path test: {}", err);
        }

        let single = run_incan_test_relative(&dir, "tests/test_fixture_path.incn");
        let single_stdout = String::from_utf8_lossy(&single.stdout);
        let single_stderr = String::from_utf8_lossy(&single.stderr);
        assert!(
            single.status.success(),
            "expected single-file fixture-path run to succeed.\nstdout:\n{}\nstderr:\n{}",
            single_stdout,
            single_stderr,
        );

        let batch = run_incan_test_relative(&dir, "tests");
        let batch_stdout = String::from_utf8_lossy(&batch.stdout);
        let batch_stderr = String::from_utf8_lossy(&batch.stderr);
        assert!(
            batch.status.success(),
            "expected batched fixture-path run to succeed.\nstdout:\n{}\nstderr:\n{}",
            batch_stdout,
            batch_stderr,
        );

        use std::time::{SystemTime, UNIX_EPOCH};

        let mut bare_dir = std::env::temp_dir();
        let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) else {
            panic!("system time before UNIX epoch");
        };
        bare_dir.push(format!("incan_e2e_test_nomani_{}", duration.as_nanos()));
        if let Err(err) = std::fs::create_dir_all(&bare_dir) {
            panic!("failed to create temp dir: {}", err);
        }
        let tests_dir = bare_dir.join("tests");
        let fixtures_dir = tests_dir.join("fixtures");

        if let Err(err) = std::fs::create_dir_all(&fixtures_dir) {
            panic!("failed to create fixture dir: {}", err);
        }
        if let Err(err) = std::fs::write(fixtures_dir.join("ok.txt"), "ok\n") {
            panic!("failed to write fixture file: {}", err);
        }
        if let Err(err) = std::fs::write(
            tests_dir.join("test_cwd.incn"),
            r#"
from std.testing import assert_eq
from rust::std::path import Path

def test_cwd__fixture_path_is_repo_relative() -> None:
    assert_eq(
        Path.new("tests/fixtures/ok.txt").exists(),
        true,
        "fixture path should resolve from the project root in both per-file and batched test runs",
    )
"#,
        ) {
            panic!("failed to write fixture path test: {}", err);
        }

        let single = run_incan_test_relative(&bare_dir, "tests/test_cwd.incn");
        let single_stdout = String::from_utf8_lossy(&single.stdout);
        let single_stderr = String::from_utf8_lossy(&single.stderr);
        assert!(
            single.status.success(),
            "expected manifest-less single-file fixture-path run to succeed.\nstdout:\n{}\nstderr:\n{}",
            single_stdout,
            single_stderr,
        );

        let batch = run_incan_test_relative(&bare_dir, "tests");
        let batch_stdout = String::from_utf8_lossy(&batch.stdout);
        let batch_stderr = String::from_utf8_lossy(&batch.stderr);
        assert!(
            batch.status.success(),
            "expected manifest-less batched fixture-path run to succeed.\nstdout:\n{}\nstderr:\n{}",
            batch_stdout,
            batch_stderr,
        );
    }

    #[test]
    fn e2e_inline_and_imported_surfaces_share_one_project() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "inline_and_imported_surface_batch"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        let tests_dir = dir.join("tests");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&tests_dir)?;
        std::fs::write(src_dir.join("widgets.incn"), "pub static MARKER: int = 41\n")?;
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
            src_dir.join("helpers.incn"),
            r#"
pub def count_names(names: List[str]) -> int:
    return len(names)
"#,
        )?;
        std::fs::write(
            src_dir.join("registry.incn"),
            r#"
pub const TOKEN: str = "token"
pub const DECORATOR_TOKEN: str = "probe.value"

def keep_int(func: (int) -> int) -> (int) -> int:
    return func

pub def registered(_name: str) -> Callable[(int) -> int, (int) -> int]:
    return keep_int
"#,
        )?;
        let entry = src_dir.join("main.incn");
        std::fs::write(
            &entry,
            r#"
def add(a: int, b: int) -> int:
    return a + b

def secret() -> str:
    return "private"

def main() -> None:
    println("production")

module tests:
    from rust::incan_std_testing import TestEnv
    from rust::std::path import PathBuf
    import std.testing as testing
    from std.testing import assert_eq, assert_is_some, fixture, test

    @fixture(autouse=true)
    def seed() -> int:
        return 40

    @fixture
    def answer(seed: int) -> int:
        return seed + 2

    @fixture(autouse=true)
    def isolate_env(mut env: TestEnv) -> None:
        env.unset("INCAN_INLINE_ENV_FIXTURE")

    def test_inline_addition(seed: int) -> None:
        assert_eq(seed, 40)
        assert_eq(add(2, 3), 5)

    def test_inline_private_access(seed: int) -> None:
        assert_eq(seed, 40)
        assert_eq(secret(), "private")

    def test_inline_assert_helper(seed: int) -> None:
        assert_eq(seed, 40)
        testing.assert(True)

    @test
    def decorated_inline_case(seed: int) -> None:
        assert_eq(seed, 40)
        assert_eq(add(20, 22), 42)

    def test_inline_fixture_and_tmp_path(answer: int, tmp_path: PathBuf) -> None:
        assert_eq(answer, 42)
        assert_eq(tmp_path.exists(), true)

    def test_inline_tmp_workdir(tmp_workdir: PathBuf) -> None:
        assert_eq(tmp_workdir.exists(), true)

    def test_inline_env_fixture(mut env: TestEnv) -> None:
        env.set("INCAN_INLINE_ENV_FIXTURE", "set")
        assert_eq(assert_is_some(env.get("INCAN_INLINE_ENV_FIXTURE")), "set")
        env.unset("INCAN_INLINE_ENV_FIXTURE")
        assert_eq(env.get("INCAN_INLINE_ENV_FIXTURE"), None)
"#,
        )?;
        std::fs::write(
            tests_dir.join("test_imported_surface_batch.incn"),
            r#"
from std.testing import assert_eq
from helper import combine
from helpers import count_names
from registry import DECORATOR_TOKEN, TOKEN, registered
from widgets import MARKER

def identity(value: str) -> str:
    return value

@registered(DECORATOR_TOKEN)
def increment(value: int) -> int:
    return value + 1

def test_imported_const_str_call_arguments_materialize() -> None:
    local: str = TOKEN
    assert_eq(identity(TOKEN), "token")
    assert_eq(identity(TOKEN.to_string()), "token")
    assert_eq(identity(local), "token")
    assert_eq(TOKEN.upper(), "TOKEN")

def test_imported_decorator_factory_const_str_argument_materializes() -> None:
    assert_eq(increment(1), 2)

def test_imported_pub_static_scalar_read() -> None:
    assert_eq(MARKER, 41)

def test_empty_names() -> None:
    assert_eq(count_names([]), 0)

def test_assert_statement_sugar() -> None:
    assert 1 + 1 == 2
    assert 3 != 4
    assert not False
    assert True

def test_imported_default_expression_expands_with_required_imports() -> None:
    assert_eq(combine(left=1, right=4), 7, "default expression helper should be available after expansion")
"#,
        )?;
        let production_entry = src_dir.join("production_only.incn");
        std::fs::write(
            &production_entry,
            r#"
def main() -> None:
    println("production")

module tests:
    from std.testing import assert_eq

    def test_production() -> None:
        assert_eq(1 + 1, 2)
"#,
        )?;

        let mut bake_command = incan_command();
        bake_command
            .args(["oven", "bake", "--project", "."])
            .current_dir(&*dir)
            .env("CARGO_NET_OFFLINE", "true");
        super::support::configure_explicit_oven_bake_command(&mut bake_command)?;
        let bake_output = bake_command.output()?;
        assert!(
            bake_output.status.success(),
            "expected one explicit Oven bake to prepare the complete inline/imported test project.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&bake_output.stdout),
            String::from_utf8_lossy(&bake_output.stderr),
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected batched inline/imported test-runner surfaces to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("main.incn::test_inline_addition")
                && stdout.contains("main.incn::test_inline_private_access")
                && stdout.contains("main.incn::decorated_inline_case")
                && stdout.contains("main.incn::test_inline_fixture_and_tmp_path")
                && stdout.contains("test_imported_surface_batch.incn::test_imported_pub_static_scalar_read")
                && stdout.contains(
                    "test_imported_surface_batch.incn::test_imported_default_expression_expands_with_required_imports"
                ),
            "expected representative batched inline/imported test names.\nstdout:\n{}",
            stdout
        );
        assert!(
            !stderr.contains("str_as_str") && !stderr.contains("expected `String`, found `&str`"),
            "imported const str call and decorator arguments should materialize as owned strings.\nstderr:\n{}",
            stderr,
        );
        assert!(
            !stderr.contains("type annotations needed"),
            "expected no Rust inference failure for empty string list.\nstderr:\n{}",
            stderr,
        );
        assert!(
            !stderr.contains("vec![].into_iter().map(|s| s.to_string()).collect()"),
            "expected no untyped empty string-list conversion in generated Rust.\nstderr:\n{}",
            stderr,
        );

        let out_dir = dir.join("out");
        let build_output = run_incan_build(&production_entry, &out_dir);
        let build_stderr = String::from_utf8_lossy(&build_output.stderr);

        assert!(
            build_output.status.success(),
            "expected production build to ignore inline test imports.\nstderr:\n{}",
            build_stderr,
        );
        let main_rs = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        assert!(
            !main_rs.contains("__incan_std::testing"),
            "inline test import should not leak into generated production code:\n{}",
            main_rs,
        );
        assert!(
            !main_rs.contains("test_inline_addition"),
            "inline test function should not leak into generated production code:\n{}",
            main_rs,
        );
        Ok(())
    }
}

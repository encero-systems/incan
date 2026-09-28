//! Integration tests for the Incan compiler frontend

include!("support/integration_tests_root.rs");

mod test_runner_e2e {
    include!("support/integration_tests_test_runner_e2e.rs");

    #[test]
    fn e2e_imported_generic_decorator_factory_preserves_function_signatures() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "generic_decorator_factory"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        let tests_dir = dir.join("tests");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&tests_dir)?;
        std::fs::write(
            src_dir.join("registry.incn"),
            r#"
pub def registered[F](name: str) -> ((F) -> F):
    return (func) => func
"#,
        )?;
        std::fs::write(
            src_dir.join("columns.incn"),
            r#"
from registry import registered

pub model ColumnExpr:
    pub name: str

@registered[(str) -> ColumnExpr]("incql.functions.col")
pub def col(name: str) -> ColumnExpr:
    return ColumnExpr(name=name)

@registered("incql.functions.literal")
pub def literal() -> ColumnExpr:
    return ColumnExpr(name="literal")
"#,
        )?;
        std::fs::write(
            tests_dir.join("test_generic_decorator_factory.incn"),
            r#"
from std.testing import assert_eq
from columns import col, literal

def test_explicit_generic_decorator_factory_signature() -> None:
    assert_eq(col("id").name, "id")

def test_inferred_generic_decorator_factory_signature() -> None:
    assert_eq(literal().name, "literal")
"#,
        )?;

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected imported generic decorator factory project to pass.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        Ok(())
    }

    #[test]
    fn e2e_inline_decorated_sum_shadows_builtin_sum_issue677() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "decorated_sum_inline"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        std::fs::create_dir_all(&src_dir)?;
        let source_path = src_dir.join("functions.incn");
        std::fs::write(
            &source_path,
            r#"
pub model IntExpr:
    pub value: int

pub model TextExpr:
    pub value: str

pub type Expr = IntExpr | TextExpr

pub model Measure:
    pub kind: str

pub def registered[F](function_ref: str) -> ((F) -> F):
    return (func) => func

pub def expr(value: int) -> Expr:
    return IntExpr(value=value)

@registered("demo.sum")
pub def sum(value: Expr) -> Measure:
    return Measure(kind="local")

module tests:
    def test_inline_test_resolves_decorated_sum_before_builtin_sum() -> None:
        measure = sum(expr(1))
        assert measure.kind == "local"
"#,
        )?;

        let output = run_incan_test_path(&source_path);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected decorated inline sum test to pass.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("functions.incn::test_inline_test_resolves_decorated_sum_before_builtin_sum"),
            "expected the #677 inline test to run.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        Ok(())
    }

    #[test]
    fn e2e_conventional_test_batches_split_import_declaration_collisions_issue676()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "import_collision_batch"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        let tests_dir = dir.join("tests");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&tests_dir)?;
        std::fs::write(
            src_dir.join("helpers.incn"),
            r#"
pub def col() -> int:
    return 1
"#,
        )?;
        std::fs::write(
            tests_dir.join("test_imports_col.incn"),
            r#"
from helpers import col

def test_imported_col() -> None:
    assert col() == 1
"#,
        )?;
        std::fs::write(
            tests_dir.join("test_declares_col.incn"),
            r#"
def col() -> int:
    return 2

def test_local_col() -> None:
    assert col() == 2
"#,
        )?;

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected import/local declaration collision batch to split and pass.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("test_imported_col") && stdout.contains("test_local_col"),
            "expected both split test files to run.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        Ok(())
    }

    #[test]
    fn e2e_method_call_decorator_factories_use_checked_receiver_lowering() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "method_call_decorator_factories"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(
            src_dir.join("main.incn"),
            r#"
class Registry:
    pub names: list[str]

    @staticmethod
    def new() -> Self:
        return Registry(names=[])

    @staticmethod
    def add_static[F](name: str) -> (F) -> F:
        FUNCTIONS.names.append(name)
        return (func) => func

    def add[F](mut self, name: str) -> (F) -> F:
        self.names.append(name)
        return (func) => func


static FUNCTIONS: Registry = Registry.new()


@Registry::add_static("static")
def static_col(name: str) -> str:
    return name


@FUNCTIONS.add("instance")
def instance_col(name: str) -> str:
    return name


def main() -> None:
    println(static_col("amount"))
    println(instance_col("price"))
    println(len(FUNCTIONS.names))
"#,
        )?;

        let out_dir = dir.join("out");
        let output = run_incan_build(&src_dir.join("main.incn"), &out_dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected method-call decorator factories to build.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );

        let generated = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        // Assert the syntax, not the method's spelling. RFC 120 projects `add_static`, so pinning the source name
        // tested the projection rather than the associated-function lowering this case exists for. Requiring the
        // decorator's own argument to arrive at a `Registry::`-qualified callee keeps it specific to this decorator.
        let normalized: String = generated
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        let lowered_as_associated_function = normalized.split("Registry::").skip(1).any(|tail| {
            tail.split_once('(').is_some_and(|(callee, arguments)| {
                callee.starts_with(incan_semantics_core::INCAN_SYMBOL_RUST_PREFIX)
                    && arguments.starts_with("\"static\".to_string()")
            })
        });
        assert!(
            lowered_as_associated_function,
            "class static method decorator should lower as associated function syntax:\n{}",
            generated,
        );
        // Same again for the instance half: `add` is projected, and the storage access is what this case pins.
        // Requiring the materialized argument to arrive at a method on the borrowed static keeps that specific.
        let reaches_receiver_through_static_storage = normalized.split("__incan_static_value.").skip(1).any(|tail| {
            tail.split_once('(').is_some_and(|(method, arguments)| {
                method.starts_with(incan_semantics_core::INCAN_SYMBOL_RUST_PREFIX)
                    && arguments.starts_with("__incan_static_arg_0")
            })
        });
        assert!(
            normalized.contains(".with_mut(|__incan_static_value|")
                && (normalized.contains("let__incan_static_arg_0=\"instance\".to_string();")
                    || normalized.contains("let__incan_static_arg_0=\"instance\".into();"))
                && reaches_receiver_through_static_storage,
            "static registry receiver should lower through static storage access:\n{}",
            generated,
        );
        Ok(())
    }

    #[test]
    fn build_lib_imported_static_decorator_receiver_materializes_string_arg_issue671()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "imported_static_decorator_receiver"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(
            src_dir.join("probe_registry.incn"),
            r#"
@derive(Clone)
pub class ProbeRegistry:
    @staticmethod
    def new() -> Self:
        return ProbeRegistry()

    def add[F](mut self, name: str, value: int) -> (F) -> F:
        return (func) => func


pub static PROBE_REGISTRY: ProbeRegistry = ProbeRegistry.new()
"#,
        )?;
        std::fs::write(
            src_dir.join("probe_decorated.incn"),
            r#"
from probe_registry import PROBE_REGISTRY

@PROBE_REGISTRY.add("decorated", 1)
pub def decorated(value: int) -> int:
    return value
"#,
        )?;
        std::fs::write(src_dir.join("lib.incn"), "pub from probe_decorated import decorated\n")?;

        let output = incan_command()
            .args(["build", "--lib"])
            .current_dir(&*dir)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected imported static decorator receiver project to build for #671.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );

        let generated = std::fs::read_to_string(dir.join("target/lib/src/probe_decorated.rs"))?;
        assert!(
            (generated.contains("let __incan_static_arg_0 = \"decorated\".into();")
                || generated.contains("let __incan_static_arg_0 = \"decorated\".to_string();"))
                && !generated.contains("__incan_static_arg_0.clone()"),
            "imported static decorator string argument should materialize as owned String:\n{}",
            generated,
        );
        Ok(())
    }

    #[test]
    fn build_static_receiver_option_model_lookup_issue674() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "main.incn",
            r#"
@derive(Clone)
model Entry:
    value: int


@derive(Clone)
class Registry:
    entries: list[Entry]

    @staticmethod
    def new() -> Self:
        return Registry(entries=[Entry(value=1)])

    def entry(self, name: str) -> Option[Entry]:
        if len(self.entries) == 0:
            return None
        return Some(self.entries[0])


static REGISTRY: Registry = Registry.new()


pub def lookup() -> int:
    match REGISTRY.entry("decorated"):
        Some(entry) => return entry.value
        None => return 0


def main() -> None:
    println(lookup())
"#,
        );

        let out_dir = dir.join("out");
        let output = run_incan_build(&dir.join("main.incn"), &out_dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected static receiver Option model lookup to build for #674.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );

        let generated = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        assert!(
            generated.contains("match {\n        let __incan_static_arg_0 = \"decorated\".to_string();")
                || generated.contains("match {\n        let __incan_static_arg_0 = \"decorated\".into();"),
            "static receiver match scrutinee should materialize args inside an expression block:\n{}",
            generated,
        );
        Ok(())
    }

    #[test]
    fn e2e_directory_run_preserves_per_file_inline_test_modules_issue676() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "inline_directory_batch"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(
            src_dir.join("alpha.incn"),
            r#"
const ALPHA_OFFSET: int = 10
static alpha_runs: int = 0

model AlphaRecord:
    value: int
    label: str

def alpha_value() -> int:
    return 1

def alpha_record() -> AlphaRecord:
    return AlphaRecord(value=alpha_value() + ALPHA_OFFSET, label="alpha")


module tests:
    def test_alpha_value() -> None:
        alpha_runs += 1
        record = alpha_record()
        assert alpha_value() == 1
        assert record.value == 11
        assert record.label == "alpha"
        assert alpha_runs == 1
"#,
        )?;
        std::fs::write(
            src_dir.join("beta.incn"),
            r#"
const BETA_OFFSET: int = 20
static beta_runs: int = 0

model BetaRecord:
    value: int
    label: str

def beta_value() -> int:
    return 2

def beta_record() -> BetaRecord:
    return BetaRecord(value=beta_value() + BETA_OFFSET, label="beta")


module tests:
    def test_beta_value() -> None:
        beta_runs += 1
        record = beta_record()
        assert beta_value() == 2
        assert record.value == 22
        assert record.label == "beta"
        assert beta_runs == 1
"#,
        )?;
        let functions_dir = src_dir.join("functions");
        std::fs::create_dir_all(&functions_dir)?;
        std::fs::write(
            functions_dir.join("columns.incn"),
            r#"
const COLUMN_OFFSET: int = 30
static column_runs: int = 0

model Column:
    value: int
    label: str

pub def col() -> int:
    return 3

def column() -> Column:
    return Column(value=col() + COLUMN_OFFSET, label="column")


module tests:
    def test_col() -> None:
        column_runs += 1
        item = column()
        assert col() == 3
        assert item.value == 33
        assert item.label == "column"
        assert column_runs == 1
"#,
        )?;
        std::fs::write(
            functions_dir.join("uses_columns.incn"),
            r#"
from functions.columns import col

const USES_COLUMN_OFFSET: int = 40
static uses_column_runs: int = 0

model UsesColumn:
    value: int
    label: str

def uses_col() -> int:
    return col() + 1

def uses_column() -> UsesColumn:
    return UsesColumn(value=uses_col() + USES_COLUMN_OFFSET, label="uses-column")


module tests:
    def test_uses_col() -> None:
        uses_column_runs += 1
        item = uses_column()
        assert uses_col() == 4
        assert item.value == 44
        assert item.label == "uses-column"
        assert uses_column_runs == 1
"#,
        )?;

        let uses_columns = run_incan_test_path(&functions_dir.join("uses_columns.incn"));
        assert!(
            uses_columns.status.success(),
            "expected direct imported inline test run to pass.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&uses_columns.stdout),
            String::from_utf8_lossy(&uses_columns.stderr),
        );

        let directory = run_incan_test_path(&src_dir);
        let stdout = String::from_utf8_lossy(&directory.stdout);
        let stderr = String::from_utf8_lossy(&directory.stderr);
        assert!(
            directory.status.success(),
            "expected directory inline test run to keep per-file parser context.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("alpha.incn::test_alpha_value")
                && stdout.contains("beta.incn::test_beta_value")
                && stdout.contains("columns.incn::test_col")
                && stdout.contains("uses_columns.incn::test_uses_col"),
            "expected every inline source file to run from directory discovery.\nstdout:\n{}",
            stdout,
        );
        assert!(
            !stdout.contains("Only one `module tests:` block") && !stderr.contains("Only one `module tests:` block"),
            "directory batching should not report duplicate inline modules across files.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            !stderr.contains("the name `col` is defined multiple times"),
            "directory batching should keep imported names inside their source module scope.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );

        Ok(())
    }

    #[test]
    fn e2e_inline_module_parametrize_markers_strict_and_timeout() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "inline_parametrize_markers"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(
            src_dir.join("math.incn"),
            r#"
module tests:
    from rust::std::thread import sleep
    from rust::std::time import Duration
    from std.testing import assert_eq, mark, param_case, parametrize, timeout, xfail

    const TEST_MARKERS: FrozenList[str] = ["smoke"]
    const TEST_MARKS: FrozenList[str] = ["smoke"]

    @parametrize("x, expected", [
        param_case((1, 3), marks=[xfail("known")], id="one-three"),
        (2, 4),
    ], ids=["ignored", "two-four"])
    def test_double(x: int, expected: int) -> None:
        assert_eq(x * 2, expected)

    @mark("smoke")
    @timeout("1ms")
    def test_timeout_marker() -> None:
        sleep(Duration.from_millis(100))
"#,
        )?;

        // Discovery-level coverage proves inline marker registration and default marks. One execution preserves the
        // distinct generated-harness outcome contract without rebuilding this project for a list-only query.
        let run = run_incan_test_with_args(&dir, &["--verbose"]);
        let run_stdout = String::from_utf8_lossy(&run.stdout);
        let run_stderr = String::from_utf8_lossy(&run.stderr);
        let run_combined = format!("{run_stdout}\n{run_stderr}");
        assert!(
            !run.status.success(),
            "expected the inline timeout marker to make the ordinary run fail.\n{}",
            run_combined,
        );
        assert!(
            run_combined.contains("XFAIL") || run_combined.contains("xfailed"),
            "expected the inline parametrized xfail/pass cases to be reported.\n{}",
            run_combined,
        );
        assert!(
            run_combined.contains("test_timeout_marker") && run_combined.contains("timed out after"),
            "expected the inline timeout marker to be reported.\n{}",
            run_combined,
        );
        Ok(())
    }

    #[test]
    fn e2e_fixture_lifetime_success_scenarios_share_one_project() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "fixture_lifetime_success_batch"
version = "0.1.0"
"#,
        );
        let tests_dir = dir.join("tests");
        std::fs::create_dir_all(&tests_dir)?;
        std::fs::write(
            tests_dir.join("conftest.incn"),
            r#"
from rust::std::path import Path
from std.testing import fixture

@fixture(scope="session")
def session_value() -> int:
    marker = Path.new("session-marker.txt")
    if marker.exists():
        return 2
    write_file("session-marker.txt", "created")
    return 1
"#,
        )?;
        std::fs::write(
            tests_dir.join("test_a.incn"),
            r#"
from std.testing import assert_eq

def test_a(session_value: int) -> None:
    assert_eq(session_value, 1)
"#,
        )?;
        std::fs::write(
            tests_dir.join("test_b.incn"),
            r#"
from std.testing import assert_eq

def test_b(session_value: int) -> None:
    assert_eq(session_value, 1)
"#,
        )?;
        std::fs::write(
            tests_dir.join("test_fixture_lifetimes.incn"),
            r#"
from std.async import sleep_ms
from std.testing import assert_eq, fixture, parametrize

static module_scope_calls: int = 0
static yield_observed: int = 0
static module_yield_calls: int = 0
static teardown_order: int = 0
static async_order: int = 0
static async_reverse_order: str = ""
static async_param_setups: int = 0

@fixture(scope="module")
def once() -> int:
    module_scope_calls += 1
    return module_scope_calls

def test_module_scope_first(once: int) -> None:
    assert_eq(once, 1)

def test_module_scope_second(once: int) -> None:
    assert_eq(once, 1)

@fixture
def captured_resource() -> int:
    value: int = 41
    yield value + 1
    yield_observed += value

def test_yield_capture_body(captured_resource: int) -> None:
    assert_eq(captured_resource, 42)

def test_yield_capture_after_teardown() -> None:
    assert_eq(yield_observed, 41)

@fixture(scope="module")
def module_shared() -> int:
    yield 10
    assert_eq(module_yield_calls, 2)

def test_module_yield_first(module_shared: int) -> None:
    module_yield_calls += 1
    assert_eq(module_shared, 10)

def test_module_yield_second(module_shared: int) -> None:
    module_yield_calls += 1
    assert_eq(module_shared, 10)

@fixture
def outer() -> int:
    yield 1
    assert_eq(teardown_order, 1)
    teardown_order += 1

@fixture
def inner(outer: int) -> int:
    yield outer + 1
    assert_eq(teardown_order, 0)
    teardown_order += 1

def test_reverse_teardown_body(inner: int) -> None:
    assert_eq(inner, 2)

def test_reverse_teardown_after() -> None:
    assert_eq(teardown_order, 2)

@fixture
def seed() -> int:
    async_order += 1
    return 40

@fixture
async def resource(seed: int) -> int:
    await sleep_ms(1)
    async_order += 1
    yield seed + 2
    await sleep_ms(1)
    async_order += 10

def test_1_uses_async_fixture(resource: int) -> None:
    assert_eq(resource, 42)
    assert_eq(async_order, 2)

def test_2_observes_async_teardown() -> None:
    assert_eq(async_order, 12)

@fixture
async def parent() -> int:
    async_reverse_order += "setup-parent;"
    await sleep_ms(1)
    yield 1
    await sleep_ms(1)
    async_reverse_order += "teardown-parent;"

@fixture
async def child(parent: int) -> int:
    async_reverse_order += "setup-child;"
    await sleep_ms(1)
    yield parent + 1
    await sleep_ms(1)
    async_reverse_order += "teardown-child;"

def test_1_uses_child(child: int) -> None:
    assert_eq(child, 2)
    assert_eq(async_reverse_order, "setup-parent;setup-child;")

def test_2_observes_reverse_teardown() -> None:
    assert_eq(async_reverse_order, "setup-parent;setup-child;teardown-child;teardown-parent;")

@fixture
async def base() -> int:
    async_param_setups += 1
    await sleep_ms(1)
    yield 10

@parametrize("value", [1, 2])
async def test_param_async_fixture(value: int, base: int) -> None:
    await sleep_ms(1)
    assert_eq(base, 10)
    assert_eq(value > 0, true)

def test_after_param_cases() -> None:
    assert_eq(async_param_setups, 2)
"#,
        )?;

        let output = run_incan_test_with_args(&dir, &["--jobs", "1"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected fixture lifetime success batch to pass.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(stdout.contains("test_module_scope_first") && stdout.contains("test_module_scope_second"));
        assert!(stdout.contains("test_param_async_fixture[1]") && stdout.contains("test_param_async_fixture[2]"));
        Ok(())
    }

    #[test]
    fn e2e_fixture_teardown_failure_scenarios_share_one_project() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_yield_fixture_teardown.incn",
            r#"
from std.testing import assert_eq, fixture

static calls: int = 0

@fixture
def resource() -> int:
    calls += 1
    yield calls
    calls += 10

def test_1_fails(resource: int) -> None:
    assert_eq(resource, 99)

def test_2_observes_teardown() -> None:
    assert_eq(calls, 11)
"#,
        );
        std::fs::write(
            dir.join("test_yield_fixture_teardown_failure.incn"),
            r#"
from std.testing import assert_eq, fixture

@fixture
def resource() -> int:
    yield 42
    assert_eq(1, 2)

def test_body_passes(resource: int) -> None:
    assert_eq(resource, 42)
"#,
        )?;
        std::fs::write(
            dir.join("test_yield_fixture_teardown_aggregate.incn"),
            r#"
from std.testing import assert_eq, fixture

@fixture
def parent() -> int:
    yield 1
    assert_eq(1, 2, "parent teardown failed")

@fixture
def child(parent: int) -> int:
    yield parent + 1
    assert_eq(3, 4, "child teardown failed")

def test_body_passes(child: int) -> None:
    assert_eq(child, 2)
"#,
        )?;
        std::fs::write(
            dir.join("test_async_yield_fixture_failure.incn"),
            r#"
from std.async import sleep_ms
from std.testing import assert_eq, fixture

static calls: int = 0

@fixture
async def resource() -> int:
    calls += 1
    await sleep_ms(1)
    yield calls
    await sleep_ms(1)
    calls += 10

def test_1_fails(resource: int) -> None:
    assert_eq(resource, 99)

def test_2_observes_async_teardown() -> None:
    assert_eq(calls, 11)
"#,
        )?;

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{stdout}\n{stderr}");
        assert!(
            !output.status.success(),
            "expected fixture teardown failure batch to fail.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            combined.contains("test_2_observes_teardown PASSED")
                && combined.contains("test_2_observes_async_teardown PASSED")
                && combined.contains("test_body_passes")
                && combined.contains("fixture teardown failed")
                && combined.contains("child teardown failed")
                && combined.contains("parent teardown failed"),
            "expected teardown diagnostics and observer tests in failure batch.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        Ok(())
    }

    #[test]
    fn e2e_inline_module_missing_fixture_is_collection_error() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "inline_missing_fixture"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(
            src_dir.join("main.incn"),
            r#"
module tests:
    def test_missing_fixture(missing: int) -> None:
        pass
"#,
        )?;

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "expected missing inline fixture to fail collection.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stderr.contains("missing fixture `missing`"),
            "expected collection-time missing fixture diagnostic.\nstderr:\n{}",
            stderr,
        );
        assert!(
            !stdout.contains("could not compile") && !stderr.contains("could not compile"),
            "missing fixtures should not fall through to generated Rust compile errors.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        Ok(())
    }

    #[test]
    fn e2e_conftest_does_not_apply_to_inline_src_tests() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "inline_conftest_boundary"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        let tests_dir = dir.join("tests");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&tests_dir)?;
        std::fs::write(
            tests_dir.join("conftest.incn"),
            r#"
from std.testing import fixture

@fixture
def shared() -> int:
    return 42
"#,
        )?;
        std::fs::write(
            src_dir.join("main.incn"),
            r#"
module tests:
    from std.testing import assert_eq

    def test_src_inline(shared: int) -> None:
        assert_eq(shared, 42)
"#,
        )?;

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "expected tests/conftest fixture not to apply to src inline tests.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(stderr.contains("missing fixture `shared`"));
        Ok(())
    }

    #[test]
    fn e2e_failure_skip_and_assert_reporting_share_one_project() {
        let dir = write_test_project(
            "test_failure_skip_and_assert_reporting.incn",
            r#"
from std.testing import assert_eq, skip

def test_message() -> None:
    assert False, "custom boom"

def test_eq_message() -> None:
    assert 1 == 2, "math broke"

def test_wrong() -> None:
    assert_eq(1 + 1, 99)

@skip("not implemented yet")
def test_todo() -> None:
    pass
"#,
        );

        // The three failing forms share one project and the normal runner reports every failure before returning its
        // aggregate non-zero exit. Keep one complete failure report rather than rebuilding that project separately for
        // each asserted diagnostic.
        let failures = run_incan_test_with_args(&dir, &["--verbose"]);
        let failures_stdout = String::from_utf8_lossy(&failures.stdout);
        let failures_stderr = String::from_utf8_lossy(&failures.stderr);
        let failures_combined = format!("{failures_stdout}\n{failures_stderr}");
        assert!(
            !failures.status.success(),
            "expected assertion failures to make the complete report fail.\n{}",
            failures_combined,
        );
        for expected in [
            "AssertionError: custom boom",
            "AssertionError: math broke",
            "left != right",
            "test_wrong",
        ] {
            assert!(
                failures_combined.contains(expected),
                "expected complete failure report to contain `{expected}`.\n{}",
                failures_combined,
            );
        }
        assert!(
            failures_combined.contains("FAILED") || failures_combined.contains("failed"),
            "expected generic failed status in output.\n{}",
            failures_combined,
        );

        let skip = run_incan_test_with_args(&dir, &["-k", "test_todo"]);
        let skip_stdout = String::from_utf8_lossy(&skip.stdout);

        assert!(
            skip.status.success(),
            "expected skipped test to succeed overall.\nstdout:\n{}",
            skip_stdout,
        );
        assert!(
            skip_stdout.contains("SKIPPED") || skip_stdout.contains("skipped"),
            "expected SKIPPED in output.\nstdout:\n{}",
            skip_stdout,
        );
    }
}

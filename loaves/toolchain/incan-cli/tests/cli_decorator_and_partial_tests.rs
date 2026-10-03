//! Decorator and partial-preset metadata surviving imports, facades, and compiled test batches.
//!
//! One of nine roots split out of the former `tests/cli_integration.rs`. The shared command surface lives in
//! `incan_test_support::cli_project`.

use std::fs;

use incan_test_support::cli_project::*;

#[test]
fn build_lib_materializes_facade_decorator_metadata_projection_issue695() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let producer_root = tmp.path().join("metadata_registry");
    let src = producer_root.join("src");
    let operators = src.join("functions").join("operators");
    fs::create_dir_all(&operators)?;
    fs::write(
        producer_root.join("loaf.toml"),
        r#"[project]
name = "metadata_registry"
version = "0.1.0"
"#,
    )?;
    fs::write(
        src.join("registry.incn"),
        r#"pub def registered[F](spec: str) -> ((F) -> F):
    return (func) => func
"#,
    )?;
    fs::write(
        operators.join("eq.incn"),
        r#"from registry import registered

pub model ColumnExpr:
    pub name: str

@registered("equal")
pub def eq(left: ColumnExpr, right: ColumnExpr) -> ColumnExpr:
    return left
"#,
    )?;
    fs::write(
        operators.join("mod.incn"),
        "pub from functions.operators.eq import eq\n",
    )?;
    fs::write(src.join("lib.incn"), "pub from functions.operators.mod import eq\n")?;

    let producer_build = run_incan(&producer_root, &["build", "--lib"])?;
    assert_success(
        &producer_build,
        "producer build --lib for decorator metadata projection issue695",
    );

    let manifest_path = producer_root
        .join("target")
        .join("lib")
        .join("metadata_registry.incnlib");
    let manifest: serde_json::Value = serde_json::from_str(&fs::read_to_string(&manifest_path)?)?;
    assert!(
        manifest.pointer("/exports/aliases/0/projected_function").is_some(),
        "reexport-only facade should materialize callable alias projection in manifest exports, got:\n{manifest}"
    );
    let api_modules = manifest
        .pointer("/contract_metadata/api/modules")
        .and_then(|value| value.as_array())
        .ok_or("expected checked API modules in manifest")?;
    let lib_alias = api_modules
        .iter()
        .flat_map(|module| {
            module
                .pointer("/declarations")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
        })
        .find(|decl| {
            decl.pointer("/kind").and_then(|value| value.as_str()) == Some("alias")
                && decl.pointer("/name").and_then(|value| value.as_str()) == Some("eq")
                && decl.pointer("/projected_function").is_some()
        })
        .ok_or("expected projected eq alias declaration in checked API metadata")?;
    assert_eq!(
        lib_alias
            .pointer("/projected_function/callable/name")
            .and_then(|value| value.as_str()),
        Some("eq")
    );
    assert_eq!(
        lib_alias
            .pointer("/projected_function/source_path")
            .and_then(|value| value.as_array())
            .map(|values| values.iter().filter_map(|value| value.as_str()).collect::<Vec<_>>()),
        Some(vec!["functions", "operators", "eq", "eq"])
    );
    assert!(
        lib_alias
            .pointer("/projected_function/decorators/0/decorated_callable/name")
            .and_then(|value| value.as_str())
            == Some("eq"),
        "projected decorator metadata should carry decorated callable identity/signature, got:\n{lib_alias}"
    );
    Ok(())
}

#[test]
fn test_imported_partial_preset_defaults_survive_decorator_argument_issue698() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "imported_partial_decorator_argument", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        src_dir.join("presets.incn"),
        r#"pub model Spec:
    pub namespace: str
    pub policy: str
    pub klass: str
    pub lifecycle: str


"""Build a core portable spec."""
pub core_spec = partial Spec(namespace="core", policy="portable")
"#,
    )?;
    fs::write(
        src_dir.join("function_registry.incn"),
        r#"pub model FunctionSpec:
    pub namespace: str
    pub deterministic: bool
    pub lifecycle: str


pub static registered_names: list[str] = []
pub static registered_namespaces: list[str] = []


pub def capture(func: (int) -> int) -> ((int) -> int):
    registered_names.append(func.__name__)
    return func


pub def add(spec: FunctionSpec) -> (((int) -> int) -> ((int) -> int)):
    registered_namespaces.append(spec.namespace)
    return capture


pub deterministic_spec = partial FunctionSpec(namespace="core", deterministic=true)
"#,
    )?;
    fs::write(
        src_dir.join("helpers.incn"),
        r#"from function_registry import add, deterministic_spec


@add(deterministic_spec(lifecycle="stable"))
pub def normalize(value: int) -> int:
    return value
"#,
    )?;
    fs::write(
        src_dir.join("registry_facade.incn"),
        r#"pub from function_registry import add, deterministic_spec
"#,
    )?;
    fs::write(
        src_dir.join("facade_helpers.incn"),
        r#"from registry_facade import add, deterministic_spec


@add(deterministic_spec(lifecycle="stable"))
pub def facade_normalize(value: int) -> int:
    return value
"#,
    )?;
    fs::write(
        tests_dir.join("test_registry_intent.incn"),
        r#"from function_registry import registered_names, registered_namespaces
from helpers import normalize
from facade_helpers import facade_normalize
from presets import core_spec


def test_imported_partial_preset_keeps_presets() -> None:
    spec = core_spec(klass="scalar", lifecycle="v1")
    assert spec.namespace == "core"
    assert spec.policy == "portable"
    assert spec.klass == "scalar"
    assert spec.lifecycle == "v1"


def test_decorator_can_infer_name_with_imported_partial_spec() -> None:
    assert normalize(7) == 7
    assert registered_names[0] == "normalize"
    assert registered_namespaces[0] == "core"


def test_decorator_can_use_reexported_partial_spec() -> None:
    assert facade_normalize(8) == 8
    assert registered_names[1] == "facade_normalize"
    assert registered_namespaces[1] == "core"
"#,
    )?;

    let test_path = tests_dir.join("test_registry_intent.incn");
    let test_output = run_incan(
        tmp.path(),
        &["test", test_path.to_str().ok_or("test path was not valid UTF-8")?],
    )?;
    assert_success(
        &test_output,
        "incan test for imported partial in decorator argument issue698",
    );
    Ok(())
}

#[test]
fn test_imported_partial_default_symbols_survive_decorator_argument_issue701() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "imported_partial_default_symbols_decorator", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        src_dir.join("registry.incn"),
        r#"pub const DEFAULT_NAMESPACE: str = "core"


pub enum Policy(str):
    Portable = "portable"


pub model Spec:
    pub namespace: str
    pub policy: Policy
    pub lifecycle: str


pub static namespaces: list[str] = []
pub static names: list[str] = []


pub spec = partial Spec(namespace=DEFAULT_NAMESPACE, policy=Policy.Portable)


pub def capture(func: (int) -> int) -> ((int) -> int):
    names.append(func.__name__)
    return func


pub def add(spec_value: Spec) -> (((int) -> int) -> ((int) -> int)):
    namespaces.append(spec_value.namespace)
    return capture
"#,
    )?;
    fs::write(
        src_dir.join("helpers.incn"),
        r#"from registry import add, spec


@add(spec(lifecycle="v1"))
pub def sample(value: int) -> int:
    return value + 1
"#,
    )?;
    fs::write(
        tests_dir.join("test_partial_default_symbols.incn"),
        r#"from helpers import sample
from registry import names, namespaces


def test_partial_default_symbols_in_decorator() -> None:
    assert sample(1) == 2
    assert names[0] == "sample"
    assert namespaces[0] == "core"
"#,
    )?;

    let test_path = tests_dir.join("test_partial_default_symbols.incn");
    let test_output = run_incan(
        tmp.path(),
        &["test", test_path.to_str().ok_or("test path was not valid UTF-8")?],
    )?;
    assert_success(&test_output, "incan test for imported partial default symbols issue701");
    Ok(())
}

#[test]
fn test_partial_constructor_presets_materialize_const_metadata_issue753() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "partial_constructor_const_metadata", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    fs::write(
        src_dir.join("metadata.incn"),
        r#"pub model Policy:
    pub family: FrozenStr
    pub role: FrozenStr
    pub enabled: bool


pub policy = partial Policy(family="hyperloglog", enabled=true)


pub const CONSTRUCT_POLICY: Policy = policy(role="construct")
pub const MERGE_POLICY: Policy = policy(role="merge", enabled=false)


pub def construct_enabled() -> bool:
    return CONSTRUCT_POLICY.enabled


pub def merge_enabled() -> bool:
    return MERGE_POLICY.enabled
"#,
    )?;
    fs::write(
        src_dir.join("runtime_consumer.incn"),
        r#"from metadata import policy


pub def runtime_policy_enabled() -> bool:
    return policy(role="runtime").enabled
"#,
    )?;
    fs::write(
        &main_path,
        r#"from metadata import Policy, construct_enabled, merge_enabled, policy
from runtime_consumer import runtime_policy_enabled


const IMPORTED_POLICY: Policy = policy(role="imported")


def main() -> None:
    assert construct_enabled()
    assert not merge_enabled()
    assert IMPORTED_POLICY.enabled
    assert runtime_policy_enabled()
"#,
    )?;

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "incan build for partial constructor const metadata issue753",
    );
    Ok(())
}

#[test]
fn test_qualified_partial_constructor_presets_cross_package_const_metadata_issue699()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let provider_root = tmp.path().join("partialkit_provider");
    fs::create_dir_all(provider_root.join("src"))?;
    fs::write(
        provider_root.join("loaf.toml"),
        "[project]\nname = \"partialkit\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        provider_root.join("src/models.incn"),
        r#"pub model Policy:
    pub family: FrozenStr
    pub role: FrozenStr
    pub enabled: bool
"#,
    )?;
    fs::write(
        provider_root.join("src/lib.incn"),
        r#"import models
pub from models import Policy


pub policy = partial models.Policy(family="cross-package", enabled=true)
"#,
    )?;

    let provider_output = run_explicit_oven_bake(&provider_root)?;
    assert_success(
        &provider_output,
        "explicit Oven bake for qualified partial constructor metadata issue699",
    );

    let consumer_root = tmp.path().join("consumer");
    fs::create_dir_all(consumer_root.join("src"))?;
    fs::write(
        consumer_root.join("loaf.toml"),
        "[project]\nname = \"consumer\"\n\n[dependencies]\npartialkit = { path = \"../partialkit_provider\" }\n",
    )?;
    let main_path = consumer_root.join("src/main.incn");
    fs::write(
        &main_path,
        r#"from pub::partialkit import Policy, policy


const DEFAULT_POLICY: Policy = policy(role="consumer")


def main() -> None:
    assert DEFAULT_POLICY.enabled
"#,
    )?;

    let consumer_bake = run_explicit_oven_bake(&consumer_root)?;
    assert_success(
        &consumer_bake,
        "explicit Oven bake for qualified partial constructor consumer issue699",
    );

    let consumer_output = run_incan(
        &consumer_root,
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &consumer_output,
        "consumer build for qualified partial constructor metadata issue699",
    );
    Ok(())
}

#[test]
fn test_decorated_functions_preserve_default_argument_calls_issue703() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "decorated_default_argument_calls", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    fs::write(
        src_dir.join("columns.incn"),
        r#"pub model ColumnExpr:
    pub value: str


pub model Ref:
    pub name: str


pub model Literal:
    pub value: int


pub type Expr = Union[Ref, Literal]


pub def col(value: str) -> ColumnExpr:
    return ColumnExpr(value=value)


pub def union_col(name: str) -> Expr:
    return Ref(name=name)
"#,
    )?;
    fs::write(
        src_dir.join("defaults.incn"),
        r#"pub model Ref:
    pub name: str


pub model Literal:
    pub value: int


pub type Expr = Union[Ref, Literal]


pub def col(name: str) -> Expr:
    return Ref(name=name)


def identity(func: (Expr) -> int) -> (Expr) -> int:
    return func


@identity
pub def decorated_default(expr: Expr = col("")) -> int:
    return 1
"#,
    )?;
    fs::write(
        src_dir.join("facade.incn"),
        r#"pub from defaults import decorated_default
"#,
    )?;
    fs::write(
        src_dir.join("facade_chain.incn"),
        r#"pub from facade import decorated_default
"#,
    )?;
    fs::write(
        src_dir.join("facade_alias.incn"),
        r#"pub from defaults import decorated_default as public_decorated_default
"#,
    )?;
    let functions_dir = src_dir.join("functions");
    let aggregates_dir = functions_dir.join("aggregates");
    fs::create_dir_all(&aggregates_dir)?;
    fs::write(
        aggregates_dir.join("count.incn"),
        r#"from defaults import Expr, col


def identity(func: (Expr) -> int) -> (Expr) -> int:
    return func


@identity
pub def count(expr: Expr = col("")) -> int:
    return 1
"#,
    )?;
    fs::write(
        functions_dir.join("mod.incn"),
        r#"pub from functions.aggregates.count import count
"#,
    )?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        tests_dir.join("test_decorated_default_probe.incn"),
        r#"from columns import ColumnExpr, Expr, col, union_col


def identity(func: (int) -> int) -> ((int) -> int):
    return func


class Box:
    value: int

    @method_identity
    def decorated_method_default(self, value: int = 11) -> int:
        return value


def method_identity(func: (Box, int) -> int) -> ((Box, int) -> int):
    return func


@identity
def decorated_default(value: int = 7) -> int:
    return value


def count_identity(func: (ColumnExpr) -> int) -> ((ColumnExpr) -> int):
    return func


@count_identity
def count(expr: ColumnExpr = col("")) -> int:
    return 1


def union_count_identity(func: (Expr) -> int) -> ((Expr) -> int):
    return func


@union_count_identity
def union_count(expr: Expr = union_col("")) -> int:
    return 1


def adapted_impl(value: str) -> int:
    return 7


def string_adapter(func: (int) -> int) -> ((str) -> int):
    return adapted_impl


@string_adapter
def surface_changed(value: int = 7) -> int:
    return value


def plain_default(value: int = 7) -> int:
    return value


def plain_union_default(expr: Expr = union_col("")) -> int:
    return 1


def test_decorated_default_probe() -> None:
    assert plain_default() == 7
    assert plain_union_default() == 1
    assert plain_union_default(union_col("orders")) == 1
    assert decorated_default() == 7
    assert decorated_default(3) == 3
    box = Box(value=1)
    assert box.decorated_method_default() == 11
    assert box.decorated_method_default(5) == 5
    assert count() == 1
    assert count(col("orders")) == 1
    assert union_count() == 1
    assert union_count(union_col("orders")) == 1
    assert surface_changed("changed") == 7
"#,
    )?;
    // These import routes have distinct lowering contracts, but source-file test
    // execution shares their compilation journey. Keep individual named cases
    // for precise failures without compiling one source file per façade.
    fs::write(
        src_dir.join("test_decorated_default_imports.incn"),
        r#"from defaults import decorated_default as direct_default
from facade import decorated_default as facade_default
from facade_chain import decorated_default as chained_default
from facade_alias import public_decorated_default as aliased_default
from functions import count


def test_imported_decorated_default_call() -> None:
    assert direct_default() == 1


def test_reexported_decorated_default_call() -> None:
    assert facade_default() == 1


def test_chained_reexported_decorated_default_call() -> None:
    assert chained_default() == 1


def test_aliased_reexported_decorated_default_call() -> None:
    assert aliased_default() == 1


def test_nested_reexported_decorated_default_call() -> None:
    assert count() == 1
"#,
    )?;
    let imported_path = src_dir.join("test_decorated_default_imports.incn");
    let imported_output = run_incan(
        tmp.path(),
        &[
            "test",
            imported_path
                .to_str()
                .ok_or("imported decorated-default path was not valid UTF-8")?,
        ],
    )?;
    assert_success(
        &imported_output,
        "incan test for imported decorated default argument routes issue703",
    );
    let imported_stdout = String::from_utf8_lossy(&imported_output.stdout);
    for test_name in [
        "test_imported_decorated_default_call",
        "test_reexported_decorated_default_call",
        "test_chained_reexported_decorated_default_call",
        "test_aliased_reexported_decorated_default_call",
        "test_nested_reexported_decorated_default_call",
    ] {
        assert!(
            imported_stdout.contains(test_name),
            "expected shared imported decorated-default route to execute `{test_name}`:\n{imported_stdout}"
        );
    }

    let probe_path = tests_dir.join("test_decorated_default_probe.incn");
    let probe_output = run_incan(
        tmp.path(),
        &[
            "test",
            probe_path
                .to_str()
                .ok_or("decorated-default probe path was not valid UTF-8")?,
        ],
    )?;
    assert_success(
        &probe_output,
        "incan test for local decorated default argument forms issue703",
    );
    assert!(
        String::from_utf8_lossy(&probe_output.stdout).contains("test_decorated_default_probe"),
        "expected local decorated-default probe to execute:\n{}",
        String::from_utf8_lossy(&probe_output.stdout)
    );
    Ok(())
}

#[test]
fn test_facade_reexport_preserves_declared_source_import_alias_target_issue57() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "facade_reexport_import_alias_target", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    let references_dir = src_dir.join("functions").join("references");
    let aggregates_dir = src_dir.join("functions").join("aggregates");
    fs::create_dir_all(&references_dir)?;
    fs::create_dir_all(&aggregates_dir)?;
    fs::write(
        src_dir.join("projection_builders.incn"),
        r#"pub model ColumnRefExpr:
    pub name: str


pub model ScalarFunctionExpr:
    pub name: str


pub type ColumnExpr = Union[ColumnRefExpr, ScalarFunctionExpr]


pub def col(name: str) -> ColumnRefExpr:
    return ColumnRefExpr(name=name)
"#,
    )?;
    fs::write(
        src_dir.join("aggregate_builders.incn"),
        r#"from projection_builders import ColumnExpr, ScalarFunctionExpr


pub model AggregateMeasure:
    pub has_expr: bool


pub def col(name: str) -> ColumnExpr:
    return ScalarFunctionExpr(name=name)


pub def count(expr: Option[ColumnExpr] = None) -> AggregateMeasure:
    if let Some(_) = expr:
        return AggregateMeasure(has_expr=true)
    return AggregateMeasure(has_expr=false)
"#,
    )?;
    fs::write(
        references_dir.join("col.incn"),
        r#"from projection_builders import ColumnRefExpr, col as col_builder


pub def col(name: str) -> ColumnRefExpr:
    return col_builder(name)
"#,
    )?;
    fs::write(
        aggregates_dir.join("count.incn"),
        r#"from aggregate_builders import AggregateMeasure, count as count_builder
from projection_builders import ColumnExpr


pub def count(expr: Option[ColumnExpr] = None) -> AggregateMeasure:
    return count_builder(expr)


pub def count_expr(expr: ColumnExpr) -> AggregateMeasure:
    return count(expr)
"#,
    )?;
    fs::write(
        src_dir.join("functions.incn"),
        r#"pub from functions.references.col import col
pub from functions.aggregates.count import count, count_expr
"#,
    )?;

    let facade_path = src_dir.join("functions.incn");
    let emit_output = run_incan(
        tmp.path(),
        &[
            "--emit-rust",
            facade_path.to_str().ok_or("facade path was not valid UTF-8")?,
        ],
    )?;
    assert_success(
        &emit_output,
        "emit-rust for facade re-export with colliding source import alias target",
    );
    Ok(())
}

#[test]
fn test_facade_reexport_preserves_decorated_helper_signature_issue57() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "facade_decorated_helper_signature", "")?;
    let src_dir = main_path.parent().ok_or("main path did not have a parent")?;
    let functions_dir = src_dir.join("functions");
    let operators_dir = functions_dir.join("operators");
    let references_dir = functions_dir.join("references");
    fs::create_dir_all(&operators_dir)?;
    fs::create_dir_all(&references_dir)?;
    fs::write(
        src_dir.join("projection_builders.incn"),
        r#"pub model ColumnRefExpr:
    pub name: str


pub model StringLiteralExpr:
    pub value: str


pub type ColumnExpr = Union[ColumnRefExpr, StringLiteralExpr]


pub def col(name: str) -> ColumnRefExpr:
    return ColumnRefExpr(name=name)
"#,
    )?;
    fs::write(
        src_dir.join("registry.incn"),
        r#"pub def register[F]() -> (F) -> F:
    return (func) => func
"#,
    )?;
    fs::write(
        src_dir.join("filter_builders.incn"),
        r#"from projection_builders import ColumnExpr


pub def eq(left: ColumnExpr, right: ColumnExpr) -> ColumnExpr:
    return left
"#,
    )?;
    fs::write(
        src_dir.join("functions").join("inputs.incn"),
        r#"from projection_builders import ColumnExpr


pub type ScalarValueOrColumn = Union[ColumnExpr, str]
"#,
    )?;
    fs::write(
        references_dir.join("col.incn"),
        r#"from projection_builders import ColumnRefExpr, col as col_builder
from registry import register


@register()
pub def col(name: str) -> ColumnRefExpr:
    return col_builder(name)
"#,
    )?;
    fs::write(
        operators_dir.join("eq.incn"),
        r#"from functions.inputs import ScalarValueOrColumn
from registry import register


@register()
pub def eq(left: ScalarValueOrColumn, right: ScalarValueOrColumn) -> None:
    return
"#,
    )?;
    fs::write(
        src_dir.join("functions").join("mod.incn"),
        r#"pub from functions.inputs import ScalarValueOrColumn
pub from functions.references.col import col
pub from functions.operators.eq import eq
pub from filter_builders import eq as filter_eq
"#,
    )?;
    let scratch_dir = tmp.path().join(".agents").join("tmp");
    fs::create_dir_all(&scratch_dir)?;
    let scratch_path = scratch_dir.join("repro_facade_eq.incn");
    fs::write(
        &scratch_path,
        r#"from functions import col, eq


pub def repro() -> None:
    eq(col("status"), "paid")
"#,
    )?;

    let check_output = run_incan(
        tmp.path(),
        &[
            "--check",
            scratch_path.to_str().ok_or("scratch path was not valid UTF-8")?,
        ],
    )?;
    assert_success(
        &check_output,
        "incan check for facade re-export preserving decorated helper signature",
    );
    Ok(())
}

#[test]
fn test_decorator_callable_exposes_source_name_issue694() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "decorator_callable_name", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        &main_path,
        r#"def main() -> None:
    pass
"#,
    )?;
    fs::write(
        src_dir.join("registry.incn"),
        r#"pub static names: list[str] = []


pub def capture(func: (int) -> int) -> ((int) -> int):
    names.append(func.__name__)
    return func


pub def registered() -> (((int) -> int) -> ((int) -> int)):
    return capture
"#,
    )?;
    fs::write(
        src_dir.join("registry_facade.incn"),
        r#"pub from registry import names, registered
"#,
    )?;
    fs::write(
        src_dir.join("generic_registry.incn"),
        r#"pub static names: list[str] = []


pub def capture[F](func: F) -> F:
    names.append(func.__name__)
    return func


pub def registered[F]() -> ((F) -> F):
    return (func) => capture[F](func)
"#,
    )?;
    fs::write(
        src_dir.join("generic_helpers.incn"),
        r#"from generic_registry import registered


@registered[(int) -> int]()
pub def sample(value: int) -> int:
    return value + 1
"#,
    )?;
    fs::write(
        tests_dir.join("test_callable_name.incn"),
        r#"from registry import names, registered
from registry_facade import registered as facade_registered
from generic_registry import names as generic_names
from generic_helpers import sample as generic_sample


@registered()
pub def sample(value: int) -> int:
    return value + 1


@facade_registered()
pub def facade_sample(value: int) -> int:
    return value + 2


def test_decorator_can_read_specific_callable_name() -> None:
    assert sample(1) == 2
    assert names[0] == "sample"
    assert facade_sample(1) == 3
    assert names[1] == "facade_sample"


def test_generic_decorator_can_read_callable_name() -> None:
    assert generic_sample(1) == 2
    assert generic_names[0] == "sample"
"#,
    )?;

    let test_path = tests_dir.join("test_callable_name.incn");
    let test_output = run_incan(
        tmp.path(),
        &["test", test_path.to_str().ok_or("test path was not valid UTF-8")?],
    )?;
    assert_success(&test_output, "incan test for decorator callable name issue694");
    Ok(())
}

#[test]
fn test_generic_decorator_callable_name_accepts_imported_alias_union_issue701() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "generic_callable_name_imported_alias_union", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        src_dir.join("types.incn"),
        r#"pub model A:
    pub value: int


pub model B:
    pub value: int


pub type Expr = Union[A, B]
"#,
    )?;
    fs::write(
        src_dir.join("registry.incn"),
        r#"pub static names: list[str] = []


pub def capture[F](func: F) -> F:
    names.append(func.__name__)
    return func


pub def register[F]() -> ((F) -> F):
    return (func) => capture[F](func)
"#,
    )?;
    fs::write(
        src_dir.join("helpers.incn"),
        r#"from registry import register
from types import Expr


@register[(Expr) -> Expr]()
pub def identity_expr(value: Expr) -> Expr:
    return value
"#,
    )?;
    fs::write(
        tests_dir.join("test_alias_union_callable_name.incn"),
        r#"from helpers import identity_expr
from registry import names
from types import A


def test_alias_union_callable_name() -> None:
    identity_expr(A(value=1))
    assert names[0] == "identity_expr"
"#,
    )?;

    let test_path = tests_dir.join("test_alias_union_callable_name.incn");
    let test_output = run_incan(
        tmp.path(),
        &["test", test_path.to_str().ok_or("test path was not valid UTF-8")?],
    )?;
    assert_success(
        &test_output,
        "incan test for alias/union generic callable name issue701",
    );
    Ok(())
}

#[test]
fn test_generic_callable_name_planning_ignores_unrelated_async_signatures_issue701()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "generic_callable_name_with_async_noise", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        src_dir.join("registry.incn"),
        r#"pub static names: list[str] = []


pub def capture[F](func: F) -> F:
    names.append(func.__name__)
    return func


pub def register[F]() -> ((F) -> F):
    return (func) => capture[F](func)
"#,
    )?;
    fs::write(
        src_dir.join("helpers.incn"),
        r#"from registry import register


@register[(int) -> int]()
pub def sample(value: int) -> int:
    return value + 1
"#,
    )?;
    fs::write(
        src_dir.join("noise.incn"),
        r#"pub async def unrelated_async(delay: float) -> None:
    return


pub def unrelated_generic[T](value: T) -> T:
    return value
"#,
    )?;
    fs::write(
        tests_dir.join("test_scoped_callable_name_planning.incn"),
        r#"from helpers import sample
from registry import names


def test_generic_callable_name_ignores_unrelated_signatures() -> None:
    assert sample(1) == 2
    assert names[0] == "sample"
"#,
    )?;

    let test_path = tests_dir.join("test_scoped_callable_name_planning.incn");
    let test_output = run_incan(
        tmp.path(),
        &["test", test_path.to_str().ok_or("test path was not valid UTF-8")?],
    )?;
    assert_success(
        &test_output,
        "incan test for scoped generic callable-name planning issue701",
    );
    Ok(())
}

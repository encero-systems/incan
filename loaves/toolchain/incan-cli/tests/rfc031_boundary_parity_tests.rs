//! RFC 031 pub-import integration tests.
//!
//! These regressions each drive a full `incan build` of a library plus one or more consumers, so they are the most
//! expensive cases in the suite. They live in their own libtest root because the Oven replay partitioner packs whole
//! roots: kept alongside the rest of the frontend integration tests they made one indivisible root that no split
//! could shorten.

include!("support/rfc031_pub_import_integration_tests_root.rs");

mod rfc031_pub_import_integration_tests {
    include!("support/rfc031_pub_import_integration_tests_rfc031_pub_import_integration_tests.rs");

    /// Verifies absolute crate imports across direct build, library re-export, and test-batch compilation.
    #[test]
    fn boundary_parity_preserves_absolute_crate_public_types_issue882() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("absolute_crate_public_types");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::create_dir_all(project_root.join("tests"))?;
        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"absolute_crate_public_types\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            project_root.join("src/types.incn"),
            r#"pub enum Access:
    Allowed
    Denied


pub model Decision:
    pub admitted: bool
    pub reason: str
"#,
        )?;
        let consumer_path = project_root.join("src/consumer.incn");
        std::fs::write(
            &consumer_path,
            r#"from crate.types import Access, Decision


pub def allowed() -> Access:
    return Access.Allowed


pub def explain(decision: Decision) -> str:
    if decision.admitted:
        return decision.reason
    return "denied"


def main() -> None:
    decision = Decision(admitted=true, reason="allowed")
    assert allowed() == Access.Allowed
    assert explain(decision) == "allowed"
"#,
        )?;
        std::fs::write(
            project_root.join("src/lib.incn"),
            r#"pub from crate.consumer import allowed, explain
pub from crate.types import Access, Decision
"#,
        )?;
        std::fs::write(
            project_root.join("tests/test_public_types.incn"),
            r#"from crate.consumer import allowed, explain
from crate.types import Access, Decision


def test_absolute_crate_public_types() -> None:
    decision = Decision(admitted=true, reason="allowed")
    assert allowed() == Access.Allowed
    assert explain(decision) == "allowed"
"#,
        )?;

        let direct_build = run_build(&consumer_path, &project_root.join("out"))?;
        assert!(
            direct_build.status.success(),
            "expected direct build to preserve absolute crate import metadata.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&direct_build.stdout),
            String::from_utf8_lossy(&direct_build.stderr)
        );

        let library_build = run_build_lib(&project_root)?;
        assert!(
            library_build.status.success(),
            concat!(
                "expected library build and re-export to preserve absolute crate import metadata.\n",
                "stdout:\n{}\nstderr:\n{}",
            ),
            String::from_utf8_lossy(&library_build.stdout),
            String::from_utf8_lossy(&library_build.stderr)
        );

        let test_output = run_test(&project_root.join("tests"))?;
        assert!(
            test_output.status.success(),
            "expected test-batch parity for absolute crate public types.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&test_output.stdout),
            String::from_utf8_lossy(&test_output.stderr)
        );
        Ok(())
    }

    #[test]
    fn compiled_provider_annotations_and_transitive_test_scopes_share_one_batch_issues902_898()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let provider_root = tmp.path().join("batch_provider");
        std::fs::create_dir_all(provider_root.join("src"))?;
        std::fs::write(
            provider_root.join("loaf.toml"),
            "[project]\nname = \"batch_provider\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            provider_root.join("src/lib.incn"),
            r#"pub model Count:
  pub value: int

pub model Record:
  pub value: int

pub def marker() -> int:
  return 7
"#,
        )?;

        let provider_build = bake_library_provider(&provider_root)?;
        assert!(
            provider_build.status.success(),
            "expected #902 provider library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&provider_build.stdout),
            String::from_utf8_lossy(&provider_build.stderr),
        );

        let consumer_root = tmp.path().join("consumer");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::create_dir_all(consumer_root.join("tests"))?;
        std::fs::write(
            consumer_root.join("loaf.toml"),
            "[project]\nname = \"batch_consumer\"\nversion = \"0.1.0\"\n\n[dependencies]\nbatch_provider = { path = \"../batch_provider\" }\n",
        )?;
        std::fs::write(
            consumer_root.join("src/bridge.incn"),
            r#"from pub::batch_provider import Count, Record, marker

pub def make_count(value: int) -> Count:
  return Count(value=value)

pub def bridged() -> int:
  _ = Record(value=marker())
  return marker()

pub def bridged_record() -> Record:
  return Record(value=marker())
"#,
        )?;
        std::fs::write(
            consumer_root.join("src/lib.incn"),
            "pub def bake_marker() -> int:\n  return 1\n",
        )?;
        std::fs::write(
            consumer_root.join("tests/aaa_compiled_annotation_test.incn"),
            r#"from pub::batch_provider import Count
from crate.bridge import make_count

def identity(value: Count) -> Count:
  return value

def test_compiled_provider_annotation() -> None:
  count: Count = identity(make_count(7))
  assert count.value == 7
"#,
        )?;
        std::fs::write(
            consumer_root.join("tests/bbb_indirect_import_test.incn"),
            r#"from crate.bridge import bridged, bridged_record

def test_indirect_import() -> None:
  assert bridged() == 7
  assert bridged_record().value == 7
"#,
        )?;
        std::fs::write(
            consumer_root.join("tests/zzz_direct_import_test.incn"),
            r#"from pub::batch_provider import Record, marker
from crate.bridge import bridged, bridged_record

def test_direct_import() -> None:
  record: Record = bridged_record()
  assert record.value == 7
  assert marker() == 7
  assert bridged() == 7
"#,
        )?;

        let consumer_bake = bake_project(&consumer_root)?;
        assert!(
            consumer_bake.status.success(),
            "expected an explicit Oven bake to import the compiled-provider test closure.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );

        let batch = run_test(&consumer_root.join("tests"))?;
        let stdout = String::from_utf8_lossy(&batch.stdout);
        let stderr = String::from_utf8_lossy(&batch.stderr);
        assert!(
            batch.status.success(),
            "expected #902 compiled-provider test batch to pass.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("aaa_compiled_annotation_test.incn::test_compiled_provider_annotation")
                && stdout.contains("bbb_indirect_import_test.incn::test_indirect_import")
                && stdout.contains("zzz_direct_import_test.incn::test_direct_import"),
            "expected all #902/#898 regressions to run.\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            !stderr.contains("already in scope"),
            "transitive dependency imports must not leak into a later test module.\nstderr:\n{stderr}",
        );

        Ok(())
    }

    #[test]
    fn boundary_parity_preserves_dependency_owned_union_helpers_through_facade()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("boundarykit_provider");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"boundarykit\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/exprs.incn"),
            r#"@derive(Clone)
pub model ColumnRefExpr:
  pub name: str

@derive(Clone)
pub model SortExpr:
  pub direction: str

pub type ColumnExpr = Union[ColumnRefExpr, SortExpr]

@derive(Clone)
pub class Frame:
  def order_by(self, columns: list[ColumnExpr]) -> Self:
    return self

pub def frame() -> Frame:
  return Frame()

pub def col(name: str) -> ColumnRefExpr:
  return ColumnRefExpr(name=name)

pub def desc(expr: ColumnExpr) -> ColumnExpr:
  return SortExpr(direction="desc")
"#,
        )?;
        std::fs::write(
            producer_root.join("src/facade.incn"),
            "pub from exprs import ColumnExpr, ColumnRefExpr, SortExpr, Frame, frame, col, desc\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub from facade import ColumnExpr, ColumnRefExpr, SortExpr, Frame, frame, col, desc\n",
        )?;

        let producer_build = bake_library_provider(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected boundarykit provider library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );

        let consumer_root = tmp.path().join("consumer");
        let main_path = write_project_files(
            &consumer_root,
            "[project]\nname = \"boundarykit_consumer\"\n\n[dependencies]\nboundarykit = { path = \"../boundarykit_provider\" }\n",
            r#"from pub::boundarykit import Frame, frame
from pub::boundarykit import col as vocab_helper_boundarykit_col
from pub::boundarykit import desc as vocab_helper_boundarykit_desc

def main() -> None:
  ordered: Frame = frame().order_by([
    vocab_helper_boundarykit_desc(vocab_helper_boundarykit_col("amount"))
  ])
  ordered.order_by([])
"#,
        )?;

        let consumer_bake = bake_project(&consumer_root)?;
        assert!(
            consumer_bake.status.success(),
            "expected an explicit Oven bake to import the boundarykit package closure.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );
        let out_dir = consumer_root.join("out");
        let consumer_build = run_build(&main_path, &out_dir)?;
        let generated_main = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        assert!(
            consumer_build.status.success(),
            "expected boundary parity consumer build to preserve dependency-owned union identity.\ngenerated main.rs:\n{}\nstdout:\n{}\nstderr:\n{}",
            generated_main,
            String::from_utf8_lossy(&consumer_build.stdout),
            String::from_utf8_lossy(&consumer_build.stderr)
        );
        assert!(
            !generated_main.contains("pub enum __IncanUnion"),
            "consumer must not re-own provider anonymous unions.\ngenerated main.rs:\n{generated_main}"
        );
        assert!(
            generated_main.contains("boundarykit::__IncanUnion"),
            "expected public helper calls to use provider-qualified union wrappers.\ngenerated main.rs:\n{generated_main}"
        );

        Ok(())
    }

    /// Regression for #1198: a sealed public provider and its consumer share the flattened union variant order.
    #[test]
    fn external_pub_consumer_uses_flattened_nested_union_variant_index_issue1198()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let prior_provider_root = tmp.path().join("prior_provider");
        let provider_root = tmp.path().join("provider");
        let consumer_root = tmp.path().join("consumer");

        for (relative_path, contents) in [
            (
                "loaf.toml",
                include_str!("fixtures/pub_union_consumer_issue1198/provider/loaf.toml"),
            ),
            (
                "src/projection_builders.incn",
                include_str!("fixtures/pub_union_consumer_issue1198/provider/src/projection_builders.incn"),
            ),
            (
                "src/functions/literals/always_true.incn",
                include_str!("fixtures/pub_union_consumer_issue1198/provider/src/functions/literals/always_true.incn"),
            ),
            (
                "src/dataset.incn",
                include_str!("fixtures/pub_union_consumer_issue1198/provider/src/dataset.incn"),
            ),
            (
                "src/lib.incn",
                include_str!("fixtures/pub_union_consumer_issue1198/provider/src/lib.incn"),
            ),
        ] {
            write_fixture_file(&provider_root, relative_path, contents)?;
        }
        // A package must retain a receipt-specific copy of a direct plan even when a prior source revision produced
        // byte-identical generated Rust. This module-docstring revision changes source authority without changing
        // the provider closure, reproducing the reusable-plan collision without relying on shared test state.
        for relative_path in [
            "loaf.toml",
            "src/projection_builders.incn",
            "src/functions/literals/always_true.incn",
            "src/dataset.incn",
            "src/lib.incn",
        ] {
            let contents = std::fs::read_to_string(provider_root.join(relative_path))?;
            let contents = if relative_path == "src/lib.incn" {
                contents.replace(
                    "Public facade for the #1198 provider's nested projection-union regression API.",
                    "Earlier facade prose for the same public regression API.",
                )
            } else {
                contents
            };
            write_fixture_file(&prior_provider_root, relative_path, &contents)?;
        }
        let prior_provider_bake = bake_library_provider(&prior_provider_root)?;
        assert!(
            prior_provider_bake.status.success(),
            "expected the prior #1198 provider source revision to bake.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&prior_provider_bake.stdout),
            String::from_utf8_lossy(&prior_provider_bake.stderr)
        );
        for (relative_path, contents) in [
            (
                "loaf.toml",
                include_str!("fixtures/pub_union_consumer_issue1198/consumer/loaf.toml"),
            ),
            (
                "src/main.incn",
                include_str!("fixtures/pub_union_consumer_issue1198/consumer/src/main.incn"),
            ),
        ] {
            write_fixture_file(&consumer_root, relative_path, contents)?;
        }

        let provider_bake = bake_library_provider(&provider_root)?;
        assert!(
            provider_bake.status.success(),
            "expected the #1198 provider bake to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&provider_bake.stdout),
            String::from_utf8_lossy(&provider_bake.stderr)
        );

        let consumer_bake = bake_project(&consumer_root)?;
        assert!(
            consumer_bake.status.success(),
            "expected the #1198 consumer bake to select the provider's flattened union variant.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );
        let generated_main =
            std::fs::read_to_string(consumer_root.join("target/incan/issue1198_union_consumer/src/main.rs"))?;
        assert!(
            generated_main.contains("issue1198_union_provider::__IncanUnion60eb06ef6f070724::V1(\n                issue1198_union_provider::always_true()"),
            "expected BoolLiteralExpr to use V1 in the provider's flattened union.\ngenerated main.rs:\n{generated_main}"
        );

        Ok(())
    }

    /// Regression for #1697: a consumer narrows a dependency-owned union alias reached through a dependency-owned
    /// model field with the same provider-qualified wrapper patterns as a local binding of that union.
    ///
    /// The field, a binding copied from the field, a binding holding a public helper's result, and a `List` field's
    /// elements in a `for` loop all narrow through `::querykit::__IncanUnion…::V<n>(…)` and run to the expected lines.
    #[test]
    fn external_pub_consumer_narrows_dependency_owned_union_through_model_field_issue1697()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let provider_root = tmp.path().join("querykit");
        std::fs::create_dir_all(provider_root.join("src"))?;
        std::fs::write(
            provider_root.join("loaf.toml"),
            "[project]\nname = \"querykit\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            provider_root.join("src/lib.incn"),
            r#"pub model IntLiteralExpr:
  pub value: int

pub model StringLiteralExpr:
  pub value: str

pub type ColumnExpr = Union[IntLiteralExpr, StringLiteralExpr]

pub model AggregateMeasure:
  pub expr: ColumnExpr

pub model Projection:
  pub columns: List[ColumnExpr]

pub def lit(value: int) -> ColumnExpr:
  return IntLiteralExpr(value=value)

pub def col(name: str) -> ColumnExpr:
  return StringLiteralExpr(value=name)
"#,
        )?;
        let provider_bake = bake_library_provider(&provider_root)?;
        assert!(
            provider_bake.status.success(),
            "expected the #1697 querykit provider bake to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&provider_bake.stdout),
            String::from_utf8_lossy(&provider_bake.stderr)
        );

        let consumer_root = tmp.path().join("consumer");
        let consumer_main = write_project_files(
            &consumer_root,
            "[project]\nname = \"issue1697_union_field_consumer\"\nversion = \"0.1.0\"\n\n[dependencies]\nquerykit = { path = \"../querykit\" }\n",
            r#"from pub::querykit import AggregateMeasure, IntLiteralExpr, Projection, StringLiteralExpr, col, lit

def describe_field(measure: AggregateMeasure) -> str:
  match measure.expr:
    IntLiteralExpr(inner) => return f"int {inner.value}"
    StringLiteralExpr(inner) => return f"str {inner.value}"

def describe_field_binding(measure: AggregateMeasure) -> str:
  expr = measure.expr
  match expr:
    IntLiteralExpr(inner) => return f"int {inner.value}"
    StringLiteralExpr(inner) => return f"str {inner.value}"

def describe_helper_binding() -> str:
  expr = lit(5)
  match expr:
    IntLiteralExpr(inner) => return f"int {inner.value}"
    StringLiteralExpr(inner) => return f"str {inner.value}"

def describe_columns(projection: Projection) -> None:
  for column in projection.columns:
    match column:
      IntLiteralExpr(inner) => println(f"int {inner.value}")
      StringLiteralExpr(inner) => println(f"str {inner.value}")

def main() -> None:
  println(describe_field(AggregateMeasure(expr=IntLiteralExpr(value=5))))
  println(describe_field(AggregateMeasure(expr=StringLiteralExpr(value="orders"))))
  println(describe_field_binding(AggregateMeasure(expr=StringLiteralExpr(value="bound"))))
  println(describe_helper_binding())
  describe_columns(Projection(columns=[lit(7), col("name")]))
"#,
        )?;

        let consumer_bake = bake_project(&consumer_root)?;
        let generated_main_path = consumer_root.join("target/incan/issue1697_union_field_consumer/src/main.rs");
        let generated_main = std::fs::read_to_string(&generated_main_path).unwrap_or_default();
        assert!(
            consumer_bake.status.success(),
            "expected the #1697 consumer bake to narrow the dependency-owned union through the model field.\ngenerated main.rs:\n{}\nstdout:\n{}\nstderr:\n{}",
            generated_main,
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );
        assert!(
            !generated_main.contains("pub enum __IncanUnion"),
            "the consumer must not re-own the provider's union.\ngenerated main.rs:\n{generated_main}"
        );
        assert!(
            generated_main.contains("querykit::__IncanUnion"),
            "expected every narrowing to use the provider-qualified union wrapper.\ngenerated main.rs:\n{generated_main}"
        );

        let consumer_run = incan_command()
            .current_dir(&consumer_root)
            .args(["run", consumer_main.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            consumer_run.status.success(),
            "expected the #1697 consumer to run.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_run.stdout),
            String::from_utf8_lossy(&consumer_run.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&consumer_run.stdout),
            "int 5\nstr orders\nstr bound\nint 5\nint 7\nstr name\n"
        );

        Ok(())
    }

    #[test]
    fn boundary_parity_preserves_decorated_alias_partial_identity_through_facade()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("callkit_provider");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"callkit\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/registry.incn"),
            r#"pub model FunctionSpec:
  pub namespace: str
  pub name: str
  pub deterministic: bool

pub static registered_names: list[str] = []
pub static registered_specs: list[FunctionSpec] = []
pub static package_markers: list[str] = []

pub deterministic_spec = partial FunctionSpec(namespace="core", deterministic=true)

pub def register[F](spec: FunctionSpec) -> ((F) -> F):
  registered_specs.append(spec)
  return (func) => capture[F](func)

def capture[F](func: F) -> F:
  registered_names.append(func.__name__)
  return func

@register(deterministic_spec(name="scale"))
pub def scale(value: int) -> int:
  return value * 2

pub scale_alias = alias scale

pub def registered_count() -> int:
  return len(registered_names)

pub def registered_name(index: int) -> str:
  return registered_names[index]

pub def registered_spec_name(index: int) -> str:
  return registered_specs[index].name
"#,
        )?;
        std::fs::write(
            producer_root.join("src/facade.incn"),
            r#"pub from registry import FunctionSpec, deterministic_spec, package_markers, registered_count, registered_name, registered_spec_name, scale, scale_alias
"#,
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub from facade import FunctionSpec, deterministic_spec, package_markers, registered_count, registered_name, registered_spec_name, scale, scale_alias\n",
        )?;
        std::fs::create_dir_all(producer_root.join("tests"))?;
        std::fs::write(
            producer_root.join("tests/test_direct_identity.incn"),
            r#"from registry import registered_count, registered_name, registered_spec_name, scale, scale_alias


def test_direct_source_import_identity() -> None:
  assert scale(3) == 6
  assert scale_alias(4) == 8
  assert registered_count() == 1
  assert registered_name(0) == "scale"
  assert registered_spec_name(0) == "scale"
"#,
        )?;
        std::fs::write(
            producer_root.join("tests/test_facade_identity.incn"),
            r#"from facade import registered_count, registered_name, registered_spec_name, scale, scale_alias


def test_facade_source_import_identity() -> None:
  assert scale(5) == 10
  assert scale_alias(6) == 12
  assert registered_count() == 1
  assert registered_name(0) == "scale"
  assert registered_spec_name(0) == "scale"
"#,
        )?;
        std::fs::write(
            producer_root.join("tests/test_mixed_identity.incn"),
            r#"from registry import registered_count, registered_name, registered_spec_name, scale as direct_scale, scale_alias as direct_scale_alias
from facade import scale as facade_scale, scale_alias as facade_scale_alias


def test_direct_and_facade_identity_share_one_static() -> None:
  assert direct_scale(3) == 6
  assert direct_scale_alias(4) == 8
  assert facade_scale(5) == 10
  assert facade_scale_alias(6) == 12
  assert registered_count() == 1
  assert registered_name(0) == "scale"
  assert registered_spec_name(0) == "scale"
"#,
        )?;

        let producer_build = bake_library_provider(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected callkit provider library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        let producer_tests = run_test(&producer_root.join("tests"))?;
        assert!(
            producer_tests.status.success(),
            "expected decorated alias partial identity source and facade test batch to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_tests.stdout),
            String::from_utf8_lossy(&producer_tests.stderr)
        );

        let consumer_root = tmp.path().join("consumer");
        let consumer_project_name = "callkit_consumer";
        let consumer_manifest = format!(
            "[project]\nname = \"{consumer_project_name}\"\n\n[dependencies]\ncallkit = {{ path = \"../callkit_provider\" }}\n"
        );
        let main_path = write_project_files(
            &consumer_root,
            &consumer_manifest,
            r#"from pub::callkit import package_markers, registered_count, registered_name, registered_spec_name, scale, scale_alias

def main() -> None:
  assert scale(3) == 6
  assert scale_alias(4) == 8
  assert registered_count() == 1
  assert registered_name(0) == "scale"
  assert registered_spec_name(0) == "scale"
  package_markers.append(f"consumer")
  assert len(package_markers) == 1
"#,
        )?;

        let consumer_bake = bake_project(&consumer_root)?;
        assert!(
            consumer_bake.status.success(),
            "expected an explicit Oven bake to import the callkit package closure.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );
        let mut consumer_build_command = super::incan_command();
        consumer_build_command
            .args(["build", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true");
        let consumer_build = run_timed_incan_command("incan build", consumer_build_command)?;
        assert!(
            consumer_build.status.success(),
            "expected decorated alias partial identity consumer build to reuse its completed Oven output.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_build.stdout),
            String::from_utf8_lossy(&consumer_build.stderr)
        );
        // The preceding normal build selected the exact completed output. Run its canonical artifact directly to
        // exercise package-static behavior without compiling the same source again through a custom output path.
        let consumer_binary = consumer_root
            .join("target")
            .join("incan")
            .join(consumer_project_name)
            .join("oven")
            .join("release")
            .join(consumer_project_name);
        assert!(
            consumer_binary.is_file(),
            "expected Oven to produce the decorated-alias consumer executable at {}",
            consumer_binary.display()
        );
        let consumer_run = Command::new(&consumer_binary).output()?;
        assert!(
            consumer_run.status.success(),
            "expected built decorated alias partial identity consumer to execute shared package statics.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_run.stdout),
            String::from_utf8_lossy(&consumer_run.stderr)
        );
        Ok(())
    }

    #[test]
    fn boundary_parity_preserves_enum_method_defaults_through_facade() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("enumkit_provider");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"enumkit\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/status.incn"),
            r#"pub enum Status(str):
  Ready = "ready"
  Paused = "paused"

  def label(self, prefix: str = "state") -> str:
    match self:
      Status.Ready => return f"{prefix}:ready"
      Status.Paused => return f"{prefix}:paused"
"#,
        )?;
        std::fs::write(producer_root.join("src/facade.incn"), "pub from status import Status\n")?;
        std::fs::write(producer_root.join("src/lib.incn"), "pub from facade import Status\n")?;

        let producer_build = bake_library_provider(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected enumkit provider library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );

        let consumer_root = tmp.path().join("consumer");
        let main_path = write_project_files(
            &consumer_root,
            "[project]\nname = \"enumkit_consumer\"\n\n[dependencies]\nenumkit = { path = \"../enumkit_provider\" }\n",
            r#"from pub::enumkit import Status


def main() -> None:
  assert Status.Ready.label() == "state:ready"
  assert Status.Paused.label(prefix="custom") == "custom:paused"
"#,
        )?;
        let consumer_bake = bake_project(&consumer_root)?;
        assert!(
            consumer_bake.status.success(),
            "expected an explicit Oven bake to import the enumkit package closure.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );
        let out_dir = consumer_root.join("out");
        let consumer_build = run_build(&main_path, &out_dir)?;
        assert!(
            consumer_build.status.success(),
            "expected enum method defaults to survive provider/facade/consumer boundary.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_build.stdout),
            String::from_utf8_lossy(&consumer_build.stderr)
        );
        Ok(())
    }

    #[test]
    fn boundary_parity_activates_dependency_vocab_across_check_fmt_and_test() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let provider_root = tmp.path().join("deps/widgets");
        std::fs::create_dir_all(provider_root.join("src"))?;
        std::fs::write(
            provider_root.join("loaf.toml"),
            "[project]\nname = \"widgets_core\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        std::fs::write(
            provider_root.join("src/lib.incn"),
            "pub def widgets_fixture_identity() -> int:\n    return 1\n",
        )?;
        write_vocab_companion_crate_with_assert_keyword(&provider_root, "vocab_companion", "widgets_vocab_companion")?;
        let provider_bake = bake_library_provider(&provider_root)?;
        assert!(
            provider_bake.status.success(),
            "expected one explicit Oven bake to publish the source-backed widgets vocabulary provider.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&provider_bake.stdout),
            String::from_utf8_lossy(&provider_bake.stderr)
        );

        let consumer_root = tmp.path().join("consumer");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::create_dir_all(consumer_root.join("tests"))?;
        std::fs::write(
            consumer_root.join("loaf.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nwidgets = { path = \"../deps/widgets\" }\n",
        )?;
        let main_path = consumer_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            r#"import pub::widgets


def main() -> None:
    assert true
"#,
        )?;
        let test_path = consumer_root.join("tests/test_vocab.incn");
        std::fs::write(
            &test_path,
            r#"import pub::widgets


def test_external_vocab_assert_keyword() -> None:
    assert true
"#,
        )?;
        let consumer_bake = bake_project(&consumer_root)?;
        assert!(
            consumer_bake.status.success(),
            "expected one explicit Oven bake to prepare the dependency-vocab consumer and its test envelope.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );

        let check_output = run_check(&main_path)?;
        assert!(
            check_output.status.success(),
            "expected dependency vocab to activate for --check.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&check_output.stdout),
            String::from_utf8_lossy(&check_output.stderr)
        );
        let fmt_check_output = run_fmt_check(&consumer_root.join("src"))?;
        assert!(
            fmt_check_output.status.success(),
            "expected dependency vocab to activate for fmt --check.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&fmt_check_output.stdout),
            String::from_utf8_lossy(&fmt_check_output.stderr)
        );
        let test_output = run_test(&consumer_root.join("tests"))?;
        assert!(
            test_output.status.success(),
            "expected dependency vocab to activate for incan test.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&test_output.stdout),
            String::from_utf8_lossy(&test_output.stderr)
        );
        Ok(())
    }

    #[test]
    fn fmt_dependency_collection_does_not_prepare_persistent_library_artifact() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("widgets_provider");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"widgets_core\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        write_vocab_companion_crate_with_assert_keyword(&producer_root, "vocab_companion", "widgets_vocab_companion")?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            r#"pub model Marker:
    pub value: int
"#,
        )?;

        let consumer_root = tmp.path().join("consumer");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::write(
            consumer_root.join("loaf.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nwidgets = { path = \"../widgets_provider\" }\n",
        )?;
        std::fs::write(
            consumer_root.join("src/main.incn"),
            r#"import pub::widgets


def main() -> None:
    assert true
"#,
        )?;

        let output = run_fmt_check(&consumer_root.join("src"))?;
        assert!(
            output.status.success(),
            "expected fmt --check to parse with source-backed dependency context without failing.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !producer_root.join("target/lib/widgets_core.incnlib").exists(),
            "fmt dependency collection should not create a persistent provider .incnlib manifest"
        );
        let combined_output = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !combined_output.contains("Preparing missing pub::widgets dependency artifact"),
            "fmt dependency collection should not invoke persistent artifact preparation, got: {combined_output}"
        );
        Ok(())
    }

    #[test]
    fn build_lib_artifact_only_writes_manifest_without_nested_cargo_build() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("widgets_provider");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"widgets_core\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            project_root.join("src/lib.incn"),
            r#"pub model Marker:
    pub value: int

pub def marker(value: int) -> Marker:
    return Marker(value=value)
"#,
        )?;

        let output = run_build_lib_artifact_only(&project_root)?;
        assert!(
            output.status.success(),
            "expected artifact-only build --lib to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            project_root.join("target/lib/widgets_core.incnlib").exists(),
            "artifact-only build should still write the provider library manifest"
        );
        assert!(
            !project_root.join("target/lib/target").exists(),
            "artifact-only build should not run generated Cargo build or create a nested target directory"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn relocated_artifact_only_provider_keeps_transitive_feature_and_rust_dependency_graph()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let original = tmp.path().join("original");
        let serializer_root = original.join("serializer");
        std::fs::create_dir_all(serializer_root.join("src"))?;
        std::fs::write(
            serializer_root.join("loaf.toml"),
            r#"[project]
name = "serializer_core"
version = "0.5.0"

[project.features]
default = ["json"]
json = []
"#,
        )?;
        std::fs::write(
            serializer_root.join("src/lib.incn"),
            "pub def serialized_value() -> int:\n    return 7\n",
        )?;
        let serializer_build =
            bake_project_with_package_features(&serializer_root, &["--no-default-features", "--features", "json"])?;
        assert!(
            serializer_build.status.success(),
            "expected leaf provider package Loafs to bake.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&serializer_build.stdout),
            String::from_utf8_lossy(&serializer_build.stderr)
        );

        let reporting_root = original.join("reporting");
        std::fs::create_dir_all(reporting_root.join("src"))?;
        std::fs::write(
            reporting_root.join("loaf.toml"),
            r#"[project]
name = "reporting_core"
version = "0.5.0"

[project.features]
default = ["json"]
json = ["dep:serializer", "serializer/json"]

[dependencies]
serializer = { path = "../serializer", optional = true, default-features = false }
"#,
        )?;
        std::fs::write(
            reporting_root.join("src/lib.incn"),
            "pub def report_value() -> int:\n    return 11\n",
        )?;
        let reporting_build = bake_library_provider(&reporting_root)?;
        assert!(
            reporting_build.status.success(),
            "expected parent provider and transitive package Loafs to bake.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&reporting_build.stdout),
            String::from_utf8_lossy(&reporting_build.stderr)
        );
        let reporting_manifest =
            LibraryManifest::read_from_path(&reporting_root.join("target/lib/reporting_core.incnlib"))?;
        let dependency = reporting_manifest
            .contract_metadata
            .provider
            .provider_dependencies
            .first()
            .ok_or("reporting artifact omitted its active provider dependency")?;
        assert_eq!(dependency.dependency_key, "serializer");
        assert_eq!(dependency.requested_features, BTreeSet::from(["json".to_string()]));
        assert!(dependency.relative_artifact_path.starts_with("../"));

        let consumer_root = original.join("consumer");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::write(
            consumer_root.join("loaf.toml"),
            "[project]\nname = \"artifact_consumer\"\n\n[dependencies]\nreporting = { path = \"../reporting\" }\n",
        )?;
        std::fs::write(consumer_root.join("src/main.incn"), "def main() -> None:\n    pass\n")?;

        let relocated = tmp.path().join("relocated");
        std::fs::rename(&original, &relocated)?;
        std::fs::remove_file(relocated.join("serializer/loaf.toml"))?;
        std::fs::remove_file(relocated.join("reporting/loaf.toml"))?;
        std::fs::remove_dir_all(relocated.join("serializer/src"))?;
        std::fs::remove_dir_all(relocated.join("reporting/src"))?;

        let relocated_consumer = relocated.join("consumer");
        let consumer_bake = bake_project(&relocated_consumer)?;
        assert!(
            consumer_bake.status.success(),
            "expected the relocated consumer to import the complete source-free provider Loaf collection.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );
        let cargo_marker = tmp.path().join("cargo-was-started");
        let output = run_incan_with_failing_cargo_guard(
            &relocated_consumer,
            &tmp.path().join("cargo-guard"),
            &cargo_marker,
            &["build", "src/main.incn"],
        )?;
        assert!(
            output.status.success(),
            "expected the relocated source-free transitive provider graph to reuse its completed project Loaf.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("reused sealed project Loaf"),
            "normal build did not report completed project-output reuse:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !cargo_marker.exists(),
            "normal relocated completed-output replay launched the guarded Cargo executable"
        );
        Ok(())
    }
}

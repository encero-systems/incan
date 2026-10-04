#![allow(dead_code)] // Split test binaries intentionally use different parts of shared support fragments.

//! RFC 031 pub-import integration tests.
//!
//! These regressions each drive a full `incan build` of a library plus one or more consumers, so they are the most
//! expensive cases in the suite. They live in their own libtest root because the Oven replay partitioner packs whole
//! roots: kept alongside the rest of the frontend integration tests they made one indivisible root that no split
//! could shorten.

include!("support/rfc031_pub_import_integration_tests_root.rs");

use support::strip_ansi_escapes;

mod rfc031_pub_import_integration_tests {
    use oven_model::manifest::{INTERNAL_MANIFEST_OVERRIDE_ENV, INTERNAL_PROJECT_ROOT_OVERRIDE_ENV};

    include!("support/rfc031_pub_import_integration_tests_rfc031_pub_import_integration_tests.rs");

    #[test]
    fn consumer_check_passes_scoped_query_surface_artifacts_to_desugarer() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::statements(vec![incan_vocab::IncanStatement::Let {
            name: "query_generated".to_string(),
            mutable: false,
            value: incan_vocab::IncanExpr::Int(1),
        }]);
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm_requiring_request_substring(
            &output_payload,
            "missing scoped query surface artifact",
            r#""descriptor_key":"query.field""#,
        )?;
        write_pub_library_with_querykit_surface_desugarer(tmp.path(), &wasm)?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"scoped_query_surface_consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            r#"import pub::querykit

def main() -> None:
  query:
    .amount > 100
    .customer_id
"#,
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected check to succeed when querykit-style leading-dot artifacts reach the desugarer.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let negative_main_path = write_project_files(
            tmp.path().join("negative_consumer").as_path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"../deps/querykit\" }\n",
            r#"import pub::querykit

def main() -> None:
  query:
    amount > 100
"#,
        )?;
        let negative_output = run_check(&negative_main_path)?;
        assert!(
            !negative_output.status.success(),
            "expected check to fail when no scoped query artifact reaches the desugarer.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&negative_output.stdout),
            String::from_utf8_lossy(&negative_output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&negative_output.stderr).contains("missing scoped query surface artifact"),
            "expected desugarer failure to prove the request substring assertion was active.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&negative_output.stdout),
            String::from_utf8_lossy(&negative_output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_check_passes_expr_list_item_metadata_to_desugarer_issue724() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::statements(vec![incan_vocab::IncanStatement::Let {
            name: "query_generated".to_string(),
            mutable: false,
            value: incan_vocab::IncanExpr::Int(1),
        }]);
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm_requiring_request_substring(
            &output_payload,
            "missing expression-list modifier payload",
            r#""keyword":"with""#,
        )?;
        write_pub_library_with_querykit_select_desugarer(tmp.path(), &wasm)?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            r#"import pub::querykit

def main() -> None:
  query:
    SELECT:
      sum(amount) as total for customer with context
"#,
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected check to pass expression-list item metadata to the desugarer.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_check_desugars_colon_vocab_expression_in_assignment_issue727() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::expression(incan_vocab::IncanExpr::Int(7));
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm_requiring_request_substring(
            &output_payload,
            "missing expression-desugaring declaration payload",
            r#""keyword":"query""#,
        )?;
        write_pub_library_with_querykit_select_desugarer(tmp.path(), &wasm)?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            r#"import pub::querykit

def main() -> None:
  value: int = query:
    SELECT:
      amount as total
"#,
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected check to desugar expression-position vocab block in assignment.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_check_desugars_colon_vocab_expression_in_return_issue727() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::expression(incan_vocab::IncanExpr::Int(7));
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm_requiring_request_substring(
            &output_payload,
            "missing expression-desugaring declaration payload",
            r#""keyword":"query""#,
        )?;
        write_pub_library_with_querykit_select_desugarer(tmp.path(), &wasm)?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            r#"import pub::querykit

def build_value() -> int:
  return query:
    SELECT:
      amount as total
"#,
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected check to desugar expression-position vocab block in return.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_check_desugars_colon_vocab_expression_preserves_inline_clauses_issue727()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::expression(incan_vocab::IncanExpr::Int(7));
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm_requiring_request_substring(
            &output_payload,
            "missing inline FROM clause payload",
            r#""keyword":"FROM""#,
        )?;
        write_pub_library_with_querykit_expression_clause_desugarer(tmp.path(), &wasm)?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            r#"import pub::querykit

def main() -> None:
  selected: int = query:
    FROM orders
    SELECT:
      amount as total
"#,
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected check to pass inline colon-expression clauses to the desugarer.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_check_desugars_braced_vocab_expression_with_compound_clauses_issue727()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::expression(incan_vocab::IncanExpr::Int(7));
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm_requiring_request_substring(
            &output_payload,
            "missing compound clause payload",
            r#""compound_tokens":["BY"]"#,
        )?;
        write_pub_library_with_querykit_expression_clause_desugarer(tmp.path(), &wasm)?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            r#"import pub::querykit

def main() -> None:
  value: int = query { FROM orders GROUP BY amount as grouped SELECT total as total }
"#,
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected check to desugar braced expression-position vocab block.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_check_desugared_public_field_callee_call_typechecks_as_method_issue727()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::expression(incan_vocab::IncanExpr::Call {
            callee: Box::new(incan_vocab::IncanExpr::Field {
                object: Box::new(incan_vocab::IncanExpr::Name("orders".to_string())),
                field: "select".to_string(),
            }),
            args: Vec::new(),
        });
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm_requiring_request_substring(
            &output_payload,
            "missing FROM clause payload",
            r#""keyword":"FROM""#,
        )?;
        write_pub_library_with_querykit_expression_clause_desugarer(tmp.path(), &wasm)?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            r#"import pub::querykit

class LazyFrame:
  def select(self) -> Self:
    return self

def main() -> None:
  orders = LazyFrame()
  selected: LazyFrame = query { FROM orders SELECT amount as amount }
"#,
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected public field-callee desugar output to typecheck as a method call.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_check_desugared_generic_method_call_uses_expected_return_type_issue735()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::expression(incan_vocab::IncanExpr::Call {
            callee: Box::new(incan_vocab::IncanExpr::Field {
                object: Box::new(incan_vocab::IncanExpr::Name("orders".to_string())),
                field: "select".to_string(),
            }),
            args: vec![incan_vocab::IncanExpr::List(vec![incan_vocab::IncanExpr::Call {
                callee: Box::new(incan_vocab::IncanExpr::Name("with_column_assignment".to_string())),
                args: vec![
                    incan_vocab::IncanExpr::Str("customer".to_string()),
                    incan_vocab::IncanExpr::Call {
                        callee: Box::new(incan_vocab::IncanExpr::Name("current_field".to_string())),
                        args: vec![
                            incan_vocab::IncanExpr::Name("orders".to_string()),
                            incan_vocab::IncanExpr::Str("customer_id".to_string()),
                        ],
                    },
                ],
            }])],
        });
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm_requiring_request_substring(
            &output_payload,
            "missing SELECT clause payload",
            r#""keyword":"SELECT""#,
        )?;
        write_pub_library_with_querykit_expression_clause_desugarer(tmp.path(), &wasm)?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            r#"import pub::querykit

@derive(Clone)
model Order:
  customer_id: str

@derive(Clone)
model Selected:
  customer: str

@derive(Clone)
model ColumnExpr:
  source: str

@derive(Clone)
model ColumnAssignment[T with Clone]:
  name: str

def current_field[T with Clone](_frame: LazyFrame[T], source: str) -> ColumnExpr:
  return ColumnExpr(source=source)

def with_column_assignment[T with Clone](name: str, _expr: ColumnExpr) -> ColumnAssignment[T]:
  return ColumnAssignment[T](name=name)

@derive(Clone)
class LazyFrame[T with Clone]:
  _type_witness: list[T]

  def select[U with Clone](self, columns: list[ColumnAssignment[U]]) -> LazyFrame[U]:
    return LazyFrame[U](_type_witness=[])

def direct_method_call(orders: LazyFrame[Order]) -> LazyFrame[Selected]:
  return orders.select([with_column_assignment("customer", current_field(orders, "customer_id"))])

def query_block_call(orders: LazyFrame[Order]) -> LazyFrame[Selected]:
  return query { FROM orders SELECT customer_id as customer }
"#,
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected desugared generic method call to use the same contextual return type as direct source.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_test_activates_dependency_vocab_surfaces_issue730_issue756() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_and_bake_source_vocab_fixture_provider(
            tmp.path(),
            "querykit",
            "querykit_core",
            "pub def querykit_fixture_identity() -> int:\n    return 7\n",
            "querykit_expression_clause_vocab_companion",
            querykit_expression_clause_vocab_companion_source(),
        )?;

        write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            "def main() -> None:\n  return\n",
        )?;
        let tests_dir = tmp.path().join("tests");
        std::fs::create_dir_all(&tests_dir)?;
        let test_path = tests_dir.join("test_query_vocab.incn");
        std::fs::write(
            &test_path,
            r#"import pub::querykit

def test_dependency_vocab_query_block() -> None:
    selected: int = query {
        FROM orders
        GROUP BY
            amount as grouped,
            region as region_group
        SELECT
            amount as total
        ORDER BY amount
        WINDOW BY
            rank = amount
    }
    assert selected == 7
"#,
        )?;

        let fmt_output = run_fmt(&test_path)?;
        assert!(
            fmt_output.status.success(),
            "expected incan fmt to parse dependency-activated vocab in a nested package test file.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&fmt_output.stdout),
            String::from_utf8_lossy(&fmt_output.stderr)
        );

        let formatted_source = std::fs::read_to_string(&test_path)?;
        for clause in [
            "selected: int = query {",
            "        GROUP BY\n            amount as grouped,\n            region as region_group",
            "        ORDER BY amount",
            "        WINDOW BY\n            rank = amount",
        ] {
            assert!(
                formatted_source.contains(clause),
                "expected incan fmt to preserve dependency-vocab expression block shape `{clause}`.\nformatted source:\n{}",
                formatted_source
            );
        }
        for rejected_clause in ["query:", "GROUP BY:", "ORDER BY:", "WINDOW BY:"] {
            assert!(
                !formatted_source.contains(rejected_clause),
                "expected incan fmt not to rewrite expression vocab block through colon clause `{rejected_clause}`.\nformatted source:\n{}",
                formatted_source
            );
        }

        let fmt_output = run_fmt_check(&test_path)?;
        assert!(
            fmt_output.status.success(),
            "expected formatted dependency-activated vocab file to pass incan fmt --check.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&fmt_output.stdout),
            String::from_utf8_lossy(&fmt_output.stderr)
        );

        let consumer_bake = bake_project(tmp.path())?;
        assert!(
            consumer_bake.status.success(),
            "expected one source-stable Oven bake to prepare the formatted dependency-vocab test.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );

        let test_output = run_test(&test_path)?;
        assert!(
            test_output.status.success(),
            "expected incan test to parse and run dependency-activated vocab in a test file.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&test_output.stdout),
            String::from_utf8_lossy(&test_output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_build_and_test_desugars_quality_expression_vocab_clauses_issue813()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_and_bake_source_vocab_fixture_provider(
            tmp.path(),
            "querykit",
            "querykit_core",
            "pub def quality_fixture_identity() -> int:\n  return 7\n",
            "quality_querykit_vocab_companion",
            quality_vocab_companion_source(),
        )?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"quality_querykit_consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            r#"import pub::querykit

def main() -> None:
    checks: int = quality {
        FROM orders
        REQUIRE row_count() >= 1 as non_empty_orders
        GROUP BY .customer_id
        EXPECT count() >= 1 as customer_groups_present
    }
    assert checks == 7
"#,
        )?;
        let tests_dir = tmp.path().join("tests");
        std::fs::create_dir_all(&tests_dir)?;
        let test_path = tests_dir.join("test_quality.incn");
        std::fs::write(
            &test_path,
            r#"import pub::querykit

def test_quality_expression_vocab_result() -> None:
    checks: int = quality {
        FROM orders
        REQUIRE row_count() >= 1 as non_empty_orders
        GROUP BY .customer_id
        EXPECT count() >= 1 as customer_groups_present
    }
    assert checks == 7
"#,
        )?;

        let consumer_bake = bake_project(tmp.path())?;
        assert!(
            consumer_bake.status.success(),
            "expected the consumer bake to import the querykit package Loafs.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );

        let out_dir = tmp.path().join("out");
        let build_output = run_build(&main_path, &out_dir)?;
        let generated_main = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        assert!(
            build_output.status.success(),
            "expected generated Rust build for the quality expression vocab declaration to succeed.\ngenerated main.rs:\n{}\nstdout:\n{}\nstderr:\n{}",
            generated_main,
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );
        assert!(
            generated_main.contains("checks"),
            "expected generated Rust to retain the desugared typed assignment.\ngenerated main.rs:\n{generated_main}"
        );

        let test_output = run_test(&test_path)?;
        assert!(
            test_output.status.success(),
            "expected incan test to execute a typed result from the quality expression vocab declaration.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&test_output.stdout),
            String::from_utf8_lossy(&test_output.stderr)
        );
        Ok(())
    }

    #[test]
    fn fmt_activates_clean_source_dependency_vocab_before_parsing_issue756() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("deps").join("querykit");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"querykit\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub def ready() -> int:\n  return 1\n",
        )?;
        write_vocab_companion_crate_with_source(
            &producer_root,
            "vocab_companion",
            "querykit_vocab_companion",
            r#"use incan_vocab::{ClauseSurface, DeclarationSurface, DslSurface, VocabRegistration};

pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new().with_surface(
        DslSurface::on_import("querykit").with_declaration(
            DeclarationSurface::named("query")
                .with_clause_body()
                .desugars_to_expression()
                .with_clauses([
                    ClauseSurface::expr("FROM").required(),
                    ClauseSurface::expr_list("SELECT").required(),
                ]),
        ),
    )
}
"#,
        )?;

        let consumer_root = tmp.path().join("consumer");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::write(
            consumer_root.join("loaf.toml"),
            "[project]\nname = \"consumer\"\nversion = \"0.1.0\"\n\n[dependencies]\nquerykit = { path = \"../deps/querykit\" }\n",
        )?;
        let main_path = consumer_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            r#"import pub::querykit

def main() -> None:
    value = query {
        FROM orders
        SELECT
            amount as total
    }
"#,
        )?;

        let artifact_root = producer_root.join("target").join("lib");
        assert!(
            !artifact_root.exists(),
            "regression must start from a clean source dependency without prebuilt library artifacts"
        );

        let fmt_output = super::incan_command()
            .args(["fmt", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .env(INTERNAL_MANIFEST_OVERRIDE_ENV, consumer_root.join("loaf.toml"))
            .env(INTERNAL_PROJECT_ROOT_OVERRIDE_ENV, &consumer_root)
            .output()?;
        assert!(
            fmt_output.status.success(),
            "expected fmt to prepare source dependency vocab before parsing clean query block, even when the parent command carries an internal manifest override.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&fmt_output.stdout),
            String::from_utf8_lossy(&fmt_output.stderr)
        );
        assert!(
            !artifact_root.join("querykit.incnlib").exists(),
            "fmt should activate parser vocab from source without preparing a persistent dependency artifact"
        );
        let combined_output = format!(
            "{}\n{}",
            String::from_utf8_lossy(&fmt_output.stdout),
            String::from_utf8_lossy(&fmt_output.stderr)
        );
        assert!(
            !combined_output.contains("Preparing missing pub::querykit dependency artifact"),
            "fmt source vocab activation should not invoke persistent artifact preparation, got: {combined_output}"
        );

        Ok(())
    }

    #[test]
    fn provider_requirements_and_pub_vocab_flow_through_build_test_and_lock() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::create_dir_all(project_root.join("tests"))?;

        write_and_bake_source_provider_with_requirements_and_assert_keyword(project_root, "0.8")?;

        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"provider_requirements_consumer\"\n\n[dependencies]\nwidgets = { path = \"deps/widgets\" }\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(&main_path, "def main() -> None:\n  pass\n")?;
        std::fs::write(
            project_root.join("tests/test_provider.incn"),
            "import pub::widgets\n\ndef test_provider_parity() -> None:\n  assert true\n",
        )?;

        let consumer_bake = bake_project(project_root)?;
        assert!(
            consumer_bake.status.success(),
            "expected the consumer bake to import the widgets provider package Loafs.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );
        let lock_path = project_root.join("oven.lock");
        let baked_lock_bytes = std::fs::read(&lock_path)?;

        let build_out_dir = project_root.join("out");
        let build_output = run_build(&main_path, &build_out_dir)?;
        assert!(
            build_output.status.success(),
            "expected build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );

        let test_output = run_test(&project_root.join("tests"))?;
        assert!(
            test_output.status.success(),
            "expected test run to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&test_output.stdout),
            String::from_utf8_lossy(&test_output.stderr)
        );
        assert_eq!(
            std::fs::read(&lock_path)?,
            baked_lock_bytes,
            "normal build and test must consume the canonical lock published by the explicit bake without rewriting it"
        );

        let build_manifest_path = build_out_dir.join("Cargo.toml");
        let build_toml = std::fs::read_to_string(&build_manifest_path).map_err(|err| {
            format!(
                "failed reading generated build Cargo.toml at {}: {err}",
                build_manifest_path.display()
            )
        })?;
        let test_manifest_path = test_runner_batch_manifest_path(project_root)?;
        let test_toml = std::fs::read_to_string(&test_manifest_path).map_err(|err| {
            std::io::Error::new(
                err.kind(),
                format!(
                    "failed reading test runner Cargo.toml at {}: {err}",
                    test_manifest_path.display()
                ),
            )
        })?;

        for cargo_toml in [&build_toml, &test_toml] {
            assert!(
                cargo_toml.contains(r#"axum = "0.8""#),
                "expected provider dependency in generated Cargo.toml, got:\n{cargo_toml}"
            );
            assert!(
                cargo_toml.contains("incan_std_core"),
                "expected stdlib dependency in generated Cargo.toml, got:\n{cargo_toml}"
            );
            assert!(
                cargo_toml.contains("incan_std_web"),
                "expected the web facet the provider requires in generated Cargo.toml, got:\n{cargo_toml}"
            );
        }

        let initial_lock = oven_model::lock::IncanLock::load(&lock_path)?;
        assert!(
            initial_lock.deps_fingerprint.starts_with("sha256:"),
            "expected the semantic lock to retain a SHA-256 dependency fingerprint, got: {}",
            initial_lock.deps_fingerprint
        );
        assert_eq!(
            initial_lock.cargo_lock_payload, "version = 4\n",
            "normal Oven lock files retain an inert legacy Cargo payload"
        );
        assert!(
            !project_root.join("target/incan_lock/Cargo.toml").exists(),
            "normal Oven locking must not recreate a Cargo workspace projection"
        );

        write_and_bake_source_provider_with_requirements_and_assert_keyword(project_root, "=0.8.9")?;
        let refreshed_lock_output = run_lock(&main_path)?;
        assert!(
            refreshed_lock_output.status.success(),
            "expected lock after provider requirement update to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&refreshed_lock_output.stdout),
            String::from_utf8_lossy(&refreshed_lock_output.stderr)
        );
        let refreshed_lock = oven_model::lock::IncanLock::load(&lock_path)?;
        assert_eq!(
            refreshed_lock.cargo_lock_payload, "version = 4\n",
            "normal Oven lock files retain an inert legacy Cargo payload"
        );
        assert_ne!(
            initial_lock.deps_fingerprint, refreshed_lock.deps_fingerprint,
            "provider requirements must participate in the semantic lock fingerprint"
        );

        Ok(())
    }

    #[test]
    fn conflicting_provider_requirements_fail_normal_build() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("src"))?;

        write_pub_library_with_provider_requirements(
            project_root,
            "widgets",
            "widgets_core",
            vec![incan_vocab::CargoDependency {
                crate_name: "serde_json".to_string(),
                source: incan_vocab::CargoDependencySource::Version("1.0".to_string()),
            }],
            vec![],
        )?;
        write_pub_library_with_provider_requirements(
            project_root,
            "analytics",
            "analytics_core",
            vec![incan_vocab::CargoDependency {
                crate_name: "serde_json".to_string(),
                source: incan_vocab::CargoDependencySource::Version("2.0".to_string()),
            }],
            vec![],
        )?;

        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nwidgets = { path = \"deps/widgets\" }\nanalytics = { path = \"deps/analytics\" }\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(&main_path, "def main() -> None:\n  pass\n")?;
        let build_output = run_build(&main_path, &project_root.join("out"))?;
        assert!(
            !build_output.status.success(),
            "expected build to fail for conflicting provider deps.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );
        let build_stderr = strip_ansi_escapes(&String::from_utf8_lossy(&build_output.stderr));
        assert!(
            build_stderr.contains("failed to merge provider requirements"),
            "expected provider conflict diagnostic in build stderr, got:\n{build_stderr}"
        );
        assert!(
            build_stderr.contains("serde_json"),
            "expected conflicting crate name in build stderr, got:\n{build_stderr}"
        );

        Ok(())
    }
}

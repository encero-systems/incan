//! RFC 031 pub-import integration tests.
//!
//! These regressions each drive a full `incan build` of a library plus one or more consumers, so they are the most
//! expensive cases in the suite. They live in their own libtest root because the Oven replay partitioner packs whole
//! roots: kept alongside the rest of the frontend integration tests they made one indivisible root that no split
//! could shorten.

include!("support/rfc031_pub_import_integration_tests_root.rs");

mod rfc031_pub_import_integration_tests {
    include!("support/rfc031_pub_import_integration_tests_rfc031_pub_import_integration_tests.rs");

    #[test]
    fn consumer_check_desugars_external_vocab_block_via_wasm() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::statements(vec![incan_vocab::IncanStatement::Let {
            name: "generated".to_string(),
            mutable: false,
            value: incan_vocab::IncanExpr::Int(1),
        }]);
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm(0, &output_payload, "")?;
        write_pub_library_with_vocab_desugarer(tmp.path(), "routes", "routes_core", &wasm, "route")?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nroutes = { path = \"deps/routes\" }\n",
            "import pub::routes\n\ndef main() -> None:\n  route \"/health\":\n    pass\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected check to succeed after wasm desugaring.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_check_passes_request_payload_into_external_vocab_desugarer() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::statements(vec![incan_vocab::IncanStatement::Let {
            name: "generated".to_string(),
            mutable: false,
            value: incan_vocab::IncanExpr::Int(1),
        }]);
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm_requiring_request(&output_payload, "missing request payload")?;
        write_pub_library_with_vocab_desugarer(tmp.path(), "routes", "routes_core", &wasm, "route")?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nroutes = { path = \"deps/routes\" }\n",
            "import pub::routes\n\ndef main() -> None:\n  route \"/health\":\n    pass\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected check to succeed when request payload is visible to the wasm desugarer.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_check_accepts_expression_desugar_output_in_statement_position() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::expression(incan_vocab::IncanExpr::Int(1));
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm(0, &output_payload, "")?;
        write_pub_library_with_vocab_desugarer(tmp.path(), "routes", "routes_core", &wasm, "route")?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nroutes = { path = \"deps/routes\" }\n",
            "import pub::routes\n\ndef main() -> None:\n  route \"/health\":\n    pass\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected check to succeed when wasm desugarer returns expression output.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_check_reports_external_vocab_desugarer_failure() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let wasm = compile_desugarer_wasm(1, "", "boom from wasm desugarer")?;
        write_pub_library_with_vocab_desugarer(tmp.path(), "routes", "routes_core", &wasm, "route")?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nroutes = { path = \"deps/routes\" }\n",
            "import pub::routes\n\ndef main() -> None:\n  route \"/health\":\n    pass\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            !output.status.success(),
            "expected check to fail when wasm desugarer reports failure.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&output.stderr));
        assert!(
            stderr.contains("vocab desugar pass failed"),
            "expected desugar-pass error prefix, got:\n{stderr}"
        );
        assert!(
            stderr.contains("boom from wasm desugarer"),
            "expected wasm runtime error message, got:\n{stderr}"
        );
        Ok(())
    }

    #[test]
    fn consumer_build_uses_provider_paths_for_vocab_desugarer_calls() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_and_bake_source_vocab_fixture_provider(
            tmp.path(),
            "filterkit",
            "filterkit_core",
            "pub def filter(value: int) -> int:\n  return value\n",
            "filterkit_vocab_companion",
            filterkit_vocab_companion_source(),
        )?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"filterkit_consumer\"\n\n[dependencies]\nfilterkit = { path = \"deps/filterkit\" }\n",
            "import pub::filterkit\n\ndef main() -> None:\n  where true:\n    pass\n",
        )?;

        let consumer_bake = bake_project(tmp.path())?;
        assert!(
            consumer_bake.status.success(),
            "expected the consumer bake to import the filterkit package Loafs.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );

        let out_dir = tmp.path().join("out");
        let build_output = run_build(&main_path, &out_dir)?;
        assert!(
            build_output.status.success(),
            "expected build to succeed when desugared output uses a provider helper binding.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );

        let generated_main_rs = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        assert!(
            !generated_main_rs.contains("__incan_vocab_helper_"),
            "expected generated Rust to avoid hidden helper aliases, got:\n{generated_main_rs}"
        );
        assert!(
            generated_main_rs.contains("filterkit::filter"),
            "expected generated Rust to call the provider helper through its dependency crate path, got:\n{generated_main_rs}"
        );
        Ok(())
    }

    #[test]
    fn consumer_build_plans_vocab_helper_calls_like_ordinary_calls_issue729() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        write_and_bake_source_vocab_fixture_provider(
            tmp.path(),
            "helperkit",
            "helperkit_core",
            "pub def lit(value: int) -> int:\n  return value\n\npub def aggregate_as(_value: int, label: str) -> str:\n  return label\n",
            "helperkit_vocab_companion",
            helperkit_vocab_companion_source(),
        )?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"helperkit_consumer\"\n\n[dependencies]\nhelperkit = { path = \"deps/helperkit\" }\n",
            r#"import pub::helperkit

def main() -> None:
  where true:
    pass
"#,
        )?;

        let consumer_bake = bake_project(tmp.path())?;
        assert!(
            consumer_bake.status.success(),
            "expected the consumer bake to import the helperkit package Loafs.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );

        let out_dir = tmp.path().join("out");
        let output = run_build(&main_path, &out_dir)?;
        assert!(
            output.status.success(),
            "expected helper-backed desugared calls to use normal call planning.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let generated_main_rs = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        let normalized: String = generated_main_rs.chars().filter(|ch| !ch.is_whitespace()).collect();
        assert!(
            normalized.contains("helperkit::aggregate_as(helperkit::lit(5),\"total\".to_string()")
                || normalized.contains(
                    "__incan_vocab_helper_9_helperkit_aggregate_as(__incan_vocab_helper_9_helperkit_lit(5),\"total\".to_string()"
                ),
            "expected nested helper calls to keep independent call planning, got:\n{generated_main_rs}"
        );
        Ok(())
    }

    #[test]
    fn consumer_build_plans_source_backed_vocab_helper_calls_with_defaults_and_unions_issue729()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_source_pub_library_with_vocab_desugarer_and_query_helpers(tmp.path(), true)?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"source_vocab_helper_consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            r#"import pub::querykit

def main() -> None:
  where true:
    pass
"#,
        )?;

        let consumer_bake = bake_project(tmp.path())?;
        assert!(
            consumer_bake.status.success(),
            "expected the source-backed vocab helper consumer to import the sealed querykit package.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );

        let out_dir = tmp.path().join("out");
        let output = run_build(&main_path, &out_dir)?;
        let generated_main_rs = std::fs::read_to_string(out_dir.join("src/main.rs")).unwrap_or_default();
        assert!(
            output.status.success(),
            "expected source-backed helper calls to keep defaults, union wrapping, and string planning.\ngenerated main.rs:\n{}\nstdout:\n{}\nstderr:\n{}",
            generated_main_rs,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        assert!(
            generated_main_rs.contains("querykit::count(")
                || generated_main_rs.contains("__incan_vocab_helper_8_querykit_count("),
            "expected omitted count() argument to be filled from the helper's default expression, got:\n{generated_main_rs}"
        );
        assert!(
            !generated_main_rs.contains("__incan_vocab_helper_8_querykit_count()"),
            "helper default planning must not emit a zero-argument Rust count call, got:\n{generated_main_rs}"
        );
        assert!(
            generated_main_rs.contains("querykit::helpers::COUNT_SENTINEL"),
            "dependency-owned const defaults must keep the defining provider module path, got:\n{generated_main_rs}"
        );
        assert!(
            !generated_main_rs.contains("pub enum __IncanUnion"),
            "public dependency helper unions must stay owned by the dependency crate, got:\n{generated_main_rs}"
        );
        assert!(
            generated_main_rs.contains(".to_string()"),
            "expected helper string arguments to use normal owned-string conversion, got:\n{generated_main_rs}"
        );
        Ok(())
    }

    #[test]
    fn consumer_build_plans_source_backed_pub_helper_calls_with_defaults_and_unions_issue729()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_source_pub_library_with_vocab_desugarer_and_query_helpers(tmp.path(), false)?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"source_pub_helper_consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            r#"from pub::querykit import aggregate_as, aggregate_default, count, lit

def main() -> None:
  aggregate_as(lit(5), "adjusted")
  aggregate_as(count(), "order_count")
  aggregate_default(lit(7))
"#,
        )?;

        let consumer_bake = bake_project(tmp.path())?;
        assert!(
            consumer_bake.status.success(),
            "expected the ordinary source-backed helper consumer to import the sealed querykit package.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );

        let out_dir = tmp.path().join("out");
        let output = run_build(&main_path, &out_dir)?;
        let generated_main_rs = std::fs::read_to_string(out_dir.join("src/main.rs")).unwrap_or_default();
        assert!(
            output.status.success(),
            "expected ordinary pub helper calls to share exported default, union, and string planning.\ngenerated main.rs:\n{}\nstdout:\n{}\nstderr:\n{}",
            generated_main_rs,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            generated_main_rs.contains("querykit::count(")
                || generated_main_rs.contains("__incan_vocab_helper_8_querykit_count("),
            "expected omitted count() argument to be filled from the helper's default expression, got:\n{generated_main_rs}"
        );
        assert!(
            !generated_main_rs.contains("querykit::count()")
                && !generated_main_rs.contains("__incan_vocab_helper_8_querykit_count()"),
            "ordinary pub helper default planning must not emit a zero-argument Rust count call, got:\n{generated_main_rs}"
        );
        assert!(
            generated_main_rs.contains("querykit::helpers::COUNT_SENTINEL"),
            "ordinary public dependency const defaults must keep the defining provider module path, got:\n{generated_main_rs}"
        );
        assert!(
            !generated_main_rs.contains("pub enum __IncanUnion"),
            "ordinary public dependency calls must not re-own dependency anonymous unions, got:\n{generated_main_rs}"
        );
        assert!(
            generated_main_rs.contains(".to_string()"),
            "expected ordinary pub helper string arguments to use normal owned-string conversion, got:\n{generated_main_rs}"
        );
        assert!(
            generated_main_rs.contains("querykit::helpers::DEFAULT_LABEL"),
            "expected public const defaults to emit through their provider module path, got:\n{generated_main_rs}"
        );
        Ok(())
    }
}

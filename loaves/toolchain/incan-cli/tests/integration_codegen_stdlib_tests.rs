#![allow(dead_code)] // Split test binaries intentionally use different parts of shared support fragments.

//! Integration tests for the Incan compiler frontend

include!("support/integration_tests_root.rs");

mod codegen_tests {
    include!("support/integration_tests_codegen_tests.rs");

    #[test]
    fn test_run_repro_model_traits() {
        let Ok(output) = incan_command()
            .arg("run")
            .arg(incan_test_support::fixture("repro_model_traits.incn"))
            // This should not require network access (workspace deps should already be available).
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "incan run repro_model_traits failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("[Ada] hello"),
            "expected repro output; got:\n{}",
            stdout
        );
    }

    /// RFC 021: Runtime verification that __fields__() returns correct FieldInfo values
    #[test]
    fn test_run_field_info_reflection() {
        let Ok(output) = incan_command()
            .arg("run")
            .arg(incan_test_support::fixture("field_info_reflection.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "incan run field_info_reflection failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Verify __class_name__
        assert!(
            stdout.contains("Account"),
            "expected __class_name__ to return 'Account'; got:\n{}",
            stdout
        );

        // Verify field info for type_ (has alias)
        assert!(
            stdout.contains("field:type_|wire:type|type:str|default:false"),
            "expected type_ field info with alias='type'; got:\n{}",
            stdout
        );

        // Verify field info for balance (has default)
        assert!(
            stdout.contains("field:balance|wire:balance|type:int|default:true"),
            "expected balance field info with default=true; got:\n{}",
            stdout
        );

        // Verify field info for name (no alias, no default)
        assert!(
            stdout.contains("field:name|wire:name|type:str|default:false"),
            "expected name field info; got:\n{}",
            stdout
        );

        // Empty models should produce no FieldInfo entries
        assert!(
            stdout.contains("empty_fields:0"),
            "expected empty model to return 0 fields; got:\n{}",
            stdout
        );

        // Nested generics should use Incan type formatting
        assert!(
            stdout.contains("settings_field:complex|type:list[dict[str, int]]"),
            "expected nested generic type name; got:\n{}",
            stdout
        );

        // User-defined field types should use their Incan type name
        assert!(
            stdout.contains("user_field:address|type:Address"),
            "expected user-defined field type name; got:\n{}",
            stdout
        );

        // Public inherited class fields should appear in __fields__().
        assert!(
            stdout.contains("child_field:base_id|type:int"),
            "expected inherited base field in __fields__; got:\n{}",
            stdout
        );
        assert!(
            !stdout.contains("child_field:name|type:str"),
            "private child fields must not appear in __fields__; got:\n{}",
            stdout
        );
    }

    /// RFC 023: Runtime parity check for source-defined stdlib surfaces migrated off helper stubs.
    #[test]
    fn test_run_rfc023_stdlib_behavior_parity() {
        let Ok(output) = incan_command()
            .arg("run")
            .arg(incan_test_support::fixture("rfc023_stdlib_behavior_parity.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "incan run rfc023_stdlib_behavior_parity failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("{\"value\":1,\"player\":\"Ada\"}"),
            "expected explicit Serialize adoption to preserve JSON output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("Score"),
            "expected reflection class name output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("true\ntrue"),
            "expected clone/equality and ordering behavior from derive-backed traits; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("{\"value\":0,\"player\":\"\"}"),
            "expected Default derive to preserve zero-value JSON output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("field:value|wire:value|type:int|default:true"),
            "expected reflection metadata for value field; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("field:player|wire:player|type:str|default:true"),
            "expected reflection metadata for player field; got:\n{}",
            stdout
        );
    }

    #[test]
    fn test_run_rfc030_std_collections_behavior() {
        let Ok(output) = incan_command()
            .arg("run")
            .arg(incan_test_support::fixture("rfc030_std_collections_behavior.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "incan run rfc030_std_collections_behavior failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn test_run_rfc088_source_owned_iterator_sum() {
        let Ok(output) = incan_command()
            .arg("run")
            .arg(repo_root().join("loaves/compiler/incan_emit/tests/codegen_snapshots/rfc088_iterator_adapters.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "incan run rfc088_iterator_adapters failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "2\n3\n15\n15\n15\n3\n3\n",
            "source-owned Iterator.sum() must preserve adapter, primitive, and checked-newtype behavior"
        );
    }

    #[test]
    fn test_run_iterator_adapters_as_loop_and_comprehension_sources_issue950_953()
    -> Result<(), Box<dyn std::error::Error>> {
        let output =
            incan_command()
                .arg("run")
                .arg(repo_root().join(
                    "loaves/compiler/incan_emit/tests/codegen_snapshots/issue950_953_iterator_adapter_sources.incn",
                ))
                .env("CARGO_NET_OFFLINE", "true")
                .output()?;

        assert!(
            output.status.success(),
            "incan run issue950_953_iterator_adapter_sources failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "11.0\n11.0\n11.0\n0:beta\n1:alpha\n2\nalpha\n2\n",
            "iterator adapters and builtin zip must preserve item types and source-owned polling across loops and comprehensions"
        );
        Ok(())
    }

    #[test]
    fn test_run_builtin_zip_only_keeps_generated_iterator_support_issue950() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .arg("run")
            .arg(repo_root().join("loaves/compiler/incan_emit/tests/codegen_snapshots/issue950_builtin_zip_only.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "incan run issue950_builtin_zip_only failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), "11\nalpha:1\n");
        Ok(())
    }

    #[test]
    fn test_run_set_constructor_from_values_issue951() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .arg("run")
            .arg(repo_root().join("loaves/compiler/incan_emit/tests/codegen_snapshots/issue951_set_constructor.incn"))
            .env("INCAN_SOURCE_ROOT", repo_root())
            .env("INCAN_STDLIB", repo_root().join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "incan run issue951_set_constructor failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "source:3\ngeneric-source:3\nset-source:2\n2:2:2:2:2:2:2:1:2:0\n",
            "set constructors must deduplicate values without consuming a source collection that is used later"
        );
        Ok(())
    }

    #[test]
    fn test_run_set_add_issue963() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .arg("run")
            .arg(repo_root().join("loaves/compiler/incan_emit/tests/codegen_snapshots/issue963_set_add.incn"))
            .env("INCAN_SOURCE_ROOT", repo_root())
            .env("INCAN_STDLIB", repo_root().join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "incan run issue963_set_add failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "1\n",
            "Set.add must mutate the set and preserve HashSet deduplication"
        );
        Ok(())
    }

    #[test]
    fn test_run_user_defined_set_shadowing_issue951() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .arg("run")
            .arg(repo_root().join("loaves/compiler/incan_emit/tests/codegen_snapshots/issue951_set_shadowing.incn"))
            .env("INCAN_SOURCE_ROOT", repo_root())
            .env("INCAN_STDLIB", repo_root().join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "incan run issue951_set_shadowing failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "3\n",
            "a source-defined set function must remain an ordinary call through generated Rust"
        );
        Ok(())
    }

    #[test]
    fn test_run_rfc088_iterator_sum_float_and_newtype_matrix() {
        let Ok(output) = incan_command()
            .arg("run")
            .arg(incan_test_support::fixture("rfc088_iterator_sum_runtime.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "incan run rfc088_iterator_sum_runtime failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "6\n3.75\n3.75\n3\n",
            "source-owned Iterator.sum() must support int, float, and checked/unchecked newtypes"
        );
    }

    #[test]
    fn test_run_rfc064_std_encoding_behavior() {
        let Ok(output) = incan_command()
            .arg("run")
            .arg(incan_test_support::fixture("rfc064_std_encoding_behavior.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "incan run rfc064_std_encoding_behavior failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("strict-padding-error")
                && stdout.contains("bech32-checksum-error")
                && stdout.contains("rfc064-encoding-ok"),
            "expected strict error markers and success marker; got:\n{}",
            stdout
        );
    }

    #[test]
    fn test_std_uuid_surface_runs_as_native_test_issue1051() -> Result<(), Box<dyn std::error::Error>> {
        // Anchor `std.uuid`'s generated native-test dependency in the root test build so offline CI shards do not
        // depend on cache order.
        let mut rng = rand::thread_rng();
        let _ = rand::Rng::gen_range(&mut rng, 0..1);

        let output = incan_command()
            .arg("test")
            .arg(incan_test_support::fixture("valid/test_std_uuid_surface.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "incan test std_uuid_surface failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("test_std_uuid_surface"),
            "the UUID native-test regression must execute its declared test. stdout:\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
        Ok(())
    }

    #[test]
    fn test_run_std_ordinal_map_surface() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .arg("run")
            .arg(incan_test_support::fixture("valid/std_ordinal_map_surface.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()
            .map_err(|error| format!("failed to run std.ordinal_map fixture: {error}"))?;

        assert!(
            output.status.success(),
            "incan run std_ordinal_map_surface failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "std.ordinal_map ok");

        // Normal Oven output is project-local. This fixture is its own project root, so do not accidentally assert
        // the old Cargo-era process-working-directory target path when the direct-rustc consumer is correct.
        let generated_project = incan_test_support::fixture("valid/target/incan/std_ordinal_map_surface");
        let generated_main = fs::read_to_string(generated_project.join("src/main.rs"))
            .map_err(|error| format!("failed to read generated std.ordinal_map consumer: {error}"))?;
        assert!(
            generated_main.contains("__incan_ordinal_require_str("),
            "OrdinalMap[str] literal lookup should lower through the borrowed string fast path:\n{generated_main}"
        );
        assert!(
            generated_main.contains("pub use crate::__incan_std::collections::OrdinalMap;"),
            "OrdinalMap should be supplied through the stable compiled-provider facade:\n{generated_main}"
        );
        assert!(
            !generated_project.join("src/__incan_std/collections.rs").exists(),
            "a compiled std.collections module must not be materialized in the consumer"
        );
        let artifact_root = compiled_sdk_provider_artifact_root(&generated_project, "incan_stdlib_data")?;
        let generated_collections = fs::read_to_string(artifact_root.join("src/collections.rs"))
            .map_err(|error| format!("failed to read compiled std.collections artifact: {error}"))?;
        // The splice now passes the two support functions it needs, and RFC 120 projects their names, so the
        // invocation reads `!(<projection>, <projection>);` rather than the argument-free form this assertion was
        // written against. Require the splice and both arguments without pinning either spelling or a line break.
        let compact_collections: String = generated_collections.split_whitespace().collect();
        let spliced_with_support_functions = compact_collections
            .split_once("incan_std_data::__incan_ordinal_map_string_fast_impls!(")
            .and_then(|(_, rest)| rest.split_once(");"))
            .is_some_and(|(arguments, _)| {
                arguments.matches(',').count() == 1
                    && arguments
                        .split(',')
                        .all(|argument| argument.starts_with(incan_semantics_core::INCAN_SYMBOL_RUST_PREFIX))
            });
        assert!(
            spliced_with_support_functions,
            "the compiled std.collections artifact should splice in the stdlib-owned OrdinalMap string support:\n{generated_collections}"
        );
        Ok(())
    }

    #[test]
    fn test_run_std_regex_rfc059_surface_from_sealed_sdk_inventory() -> Result<(), Box<dyn std::error::Error>> {
        assert_std_regex_surface_from_sealed_sdk_inventory()
    }

    #[test]
    fn explicit_stale_sdk_inventory_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let source = tmp.path().join("std_regex_surface.incn");
        fs::copy(incan_test_support::fixture("valid/std_regex_surface.incn"), &source)?;
        let stale_inventory = tmp.path().join("stale-sdk-inventory.json");
        fs::write(
            &stale_inventory,
            r#"{
  "schema_version": 2,
  "sdk_id": "incan",
  "sdk_version": "0.6.0",
  "compiler_requirement": ">=0.6.0-dev.0,<0.7.0",
  "provider_codegen_revision": 4,
  "components": {},
  "profiles": {"default": []}
}"#,
        )?;
        let provider_store = tmp.path().join("unused-sdk-provider-store");
        let generated_cargo_target = tmp.path().join("generated-cargo-target");

        let output = incan_command()
            .current_dir(tmp.path())
            .env_remove("INCAN_STDLIB")
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_SDK_INVENTORY", &stale_inventory)
            .env("INCAN_INTERNAL_SDK_PROVIDER_STORE", &provider_store)
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .arg("run")
            .arg(&source)
            .output()?;
        assert!(
            !output.status.success(),
            "an explicit SDK inventory is authoritative and must not be replaced from nearby source: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&output.stderr));
        assert!(
            stderr.contains("generated with provider codegen revision 4")
                && stderr.contains(&format!(
                    "requires revision {}",
                    incan_lang::version::SDK_PROVIDER_CODEGEN_REVISION
                )),
            "the failure should report the exact incompatible provider revision:\n{stderr}"
        );
        assert!(
            !provider_store.exists(),
            "rejecting an explicit incompatible SDK inventory must not publish replacement providers"
        );
        Ok(())
    }

    #[test]
    fn explicit_legacy_sdk_inventory_blocks_lock_preheat() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project = tmp.path().join("provider_backed_library");
        fs::create_dir_all(project.join("src"))?;
        fs::write(
            project.join("loaf.toml"),
            "[project]\nname = \"provider_backed_library\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(project.join("src/lib.incn"), "pub def answer() -> int:\n  return 42\n")?;
        let stale_inventory = tmp.path().join("stale-sdk-inventory.json");
        fs::write(
            &stale_inventory,
            r#"{
  "schema_version": 1,
  "sdk_id": "incan",
  "sdk_version": "0.5.0",
  "compiler_requirement": ">=0.5.0-dev.6,<0.6.0",
  "components": {},
  "profiles": {"default": []}
}"#,
        )?;
        let provider_store = tmp.path().join("unused-sdk-provider-store");
        let generated_cargo_target = tmp.path().join("generated-cargo-target");
        let configure = |command: &mut Command| {
            command
                .current_dir(&project)
                .env_remove("INCAN_STDLIB")
                .env_remove("INCAN_STDLIB_DIR")
                .env("INCAN_SDK_INVENTORY", &stale_inventory)
                .env("INCAN_INTERNAL_SDK_PROVIDER_STORE", &provider_store)
                .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target);
        };

        let mut lock = incan_command();
        configure(&mut lock);
        let lock = lock.arg("lock").output()?;
        assert!(
            !lock.status.success(),
            "an explicit legacy SDK inventory must fail before lock preheat can replace it: status={:?}\nstdout:\n{}\nstderr:\n{}",
            lock.status,
            String::from_utf8_lossy(&lock.stdout),
            String::from_utf8_lossy(&lock.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&lock.stderr));
        assert!(
            stderr.contains("unsupported SDK inventory schema 1") && stderr.contains("expected 2"),
            "the failure should report the exact unsupported inventory schema:\n{stderr}"
        );
        assert!(
            !provider_store.exists(),
            "rejecting an explicit legacy SDK inventory must not publish replacement providers"
        );
        Ok(())
    }

    #[test]
    fn test_run_std_regex_unsupported_safe_engine_pattern_reports_error() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
from std.regex import Regex

def main() -> None:
    match Regex("(?<=prefix)\\w+"):
        Ok(_) => println("unexpected-ok")
        Err(err) =>
            println("unsupported")
            println(err.kind())
            println(err.message())
"#,
            ])
            .output()?;

        assert!(
            output.status.success(),
            "std.regex unsupported-pattern program should report RegexError without failing the process: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        assert!(
            stdout.contains("unsupported") && !stdout.contains("unexpected-ok"),
            "expected safe-engine rejection branch, got:\n{stdout}"
        );
        assert!(
            stdout.contains("compile_error"),
            "expected stable RegexError kind, got:\n{stdout}"
        );
        assert!(
            stdout.to_ascii_lowercase().contains("look"),
            "expected diagnostic to identify the unsupported lookaround boundary, got:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_run_u128_modulo_floor_div() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .arg("run")
            .arg(incan_test_support::fixture("valid/u128_modulo_floor_div.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "incan run u128_modulo_floor_div failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "u128 modulo ok");
        Ok(())
    }

    #[test]
    fn test_run_rfc030_field_overlay_reflection() {
        let Ok(output) = incan_command()
            .arg("run")
            .arg(incan_test_support::fixture("rfc030_field_overlay_reflection.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "incan run rfc030_field_overlay_reflection failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn test_check_cyclic_explicit_call_site_generics_cross_module_succeeds() -> Result<(), Box<dyn std::error::Error>> {
        let project_dir = make_temp_dir("incan_cycle_explicit_call_site_check");
        let main_path = super::write_cycle_explicit_call_site_generics_project(&project_dir)?;

        let output = incan_command()
            .arg("--check")
            .arg(main_path)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "incan --check cyclic explicit call-site generics failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn test_run_cyclic_explicit_call_site_generics_cross_module_succeeds() -> Result<(), Box<dyn std::error::Error>> {
        let project_dir = make_temp_dir("incan_cycle_explicit_call_site_run");
        let main_path = super::write_cycle_explicit_call_site_generics_project(&project_dir)?;

        let output = incan_command()
            .arg("run")
            .arg(main_path)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "incan run cyclic explicit call-site generics failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains('1'),
            "expected runtime output to contain 1, got:\n{}",
            stdout
        );
        Ok(())
    }
}

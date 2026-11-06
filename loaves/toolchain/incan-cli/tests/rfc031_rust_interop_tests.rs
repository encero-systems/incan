#![allow(dead_code)] // Split test binaries intentionally use different parts of shared support fragments.

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
    fn consumer_build_infers_rust_generic_return_from_unwrap_context_issue852() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let _main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"generic_json_return_repro\"\n\n[rust-dependencies.serde_json]\nversion = \"1.0\"\n",
            r#"from helpers import accept_value, parse_value
from rust::serde_json import Value
from rust::serde_json import from_str as json_parse


def main() -> None:
    value: Value = parse_value()
    accept_value(json_parse("{}").unwrap())
"#,
        )?;
        std::fs::write(
            tmp.path().join("src/helpers.incn"),
            r#"from rust::serde_json import Value
from rust::serde_json import from_str as json_parse


pub def parse_value() -> Value:
    return json_parse("{}").unwrap()


pub def accept_value(value: Value) -> None:
    pass
"#,
        )?;

        let tests_dir = tmp.path().join("tests");
        std::fs::create_dir_all(&tests_dir)?;
        std::fs::write(
            tests_dir.join("test_generic_json_return.incn"),
            r#"from helpers import accept_value, parse_value
from rust::serde_json import Value
from rust::serde_json import from_str as json_parse


def test_generic_json_result_infers_from_parameter_context() -> None:
    value: Value = parse_value()
    accept_value(value)
    accept_value(json_parse("{}").unwrap())
    assert true
"#,
        )?;
        let project_bake = bake_project(tmp.path())?;
        assert!(
            project_bake.status.success(),
            "expected one explicit Oven bake to prepare the generic JSON test dependency envelope.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&project_bake.stdout),
            String::from_utf8_lossy(&project_bake.stderr)
        );
        let test_output = run_test(&tests_dir)?;
        assert!(
            test_output.status.success(),
            "expected the package test batch to compile and run both inferred generic JSON result contexts.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&test_output.stdout),
            String::from_utf8_lossy(&test_output.stderr)
        );
        Ok(())
    }

    /// One provider journey carries the three related `receiver_factory` contracts.
    ///
    /// The former three tests independently built the same Rust dependency graph, then each asked a compiled Incan
    /// provider and consumer to traverse it. The contracts are independent, but the journeys were not: one provider
    /// build and one consumer build prove all three without repeating the same expensive inspection boundary in a
    /// package-test batch.
    #[test]
    fn compiled_provider_preserves_shared_rust_interop_contracts_issues834_835_961()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_receiver_factory_dependency(tmp.path())?;
        let provider_root = tmp.path().join("receiver_factory_api");
        std::fs::create_dir_all(provider_root.join("src"))?;
        std::fs::write(
            provider_root.join("loaf.toml"),
            "[project]\nname = \"receiver_factory_api\"\nversion = \"0.1.0\"\n\n[rust-dependencies.receiver_factory]\npath = \"../receiver_factory\"\n",
        )?;
        std::fs::write(
            provider_root.join("src/lib.incn"),
            r#"pub from rust::receiver_factory import ConstructionError, DeviceTrait, Factory, Mode, OutputCallbackInfo, PairFactory, device


def write_silence(_data: &mut list[f32], _info: &OutputCallbackInfo) -> None:
    pass


def report_error(_error: str) -> None:
    pass


def consume(_value: f32) -> None:
    pass


def accept_factory(result: Result[Factory[f32], ConstructionError]) -> None:
    match result:
        Ok(_) => pass
        Err(_) => pass


def accept_pair(value: PairFactory[i64, str]) -> None:
    pass


pub def exercise_receiver_generics() -> None:
    explicit: Result[Factory[f32], ConstructionError] = Factory.new[f32](8, Mode.Input)
    contextual: Result[Factory[f32], ConstructionError] = Factory.new(8, Mode.Input)
    explicit_pair: PairFactory[i64, str] = PairFactory.new[i64, str](7, "marker")
    contextual_pair: PairFactory[i64, str] = PairFactory.new(7, "marker")
    first: str = PairFactory.first[str, i64]("value")
    accept_factory(explicit)
    accept_factory(contextual)
    accept_factory(Factory.new(8, Mode.Input))
    accept_pair(explicit_pair)
    accept_pair(contextual_pair)
    accept_pair(PairFactory.new(7, "marker"))
    if len(first) == 0:
        print(first)


pub def build_stream() -> None:
    stream = device()
    stream.build_output_stream[f32, _, _](1.0, consume, consume)


pub def exercise_callbacks() -> None:
    stream = device()
    stream.run_callbacks[f32, _, _](write_silence, report_error)
    stream.run_callbacks[f32, _, _]((_data, _info) => println(len(_data)), report_error)
"#,
        )?;

        let provider_build = bake_library_provider(&provider_root)?;
        assert!(
            provider_build.status.success(),
            "expected shared receiver-factory Rust contracts to compile in a provider.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&provider_build.stdout),
            String::from_utf8_lossy(&provider_build.stderr)
        );
        let provider_manifest =
            LibraryManifest::read_from_path(&provider_root.join("target/lib/receiver_factory_api.incnlib"))?;
        let factory_metadata = provider_manifest
            .rust_abi
            .as_ref()
            .and_then(|abi| abi.get("receiver_factory::PairFactory"))
            .ok_or("expected PairFactory metadata in compiled provider")?;
        let incan_lang::interop::RustItemKind::Type(factory_metadata) = &factory_metadata.kind else {
            return Err("expected compiled PairFactory type metadata".into());
        };
        assert_eq!(factory_metadata.type_params, ["T", "U"]);
        assert!(!factory_metadata.has_const_params);
        let generated_provider = std::fs::read_to_string(provider_root.join("target/lib/src/lib.rs"))?;
        // `prettyplease` adds a trailing comma when it breaks an argument list across lines, so the same turbofish
        // reads as `::<f32,_,_>` or `::<f32,_,_,>`, and the same call as `f(x)` or `f(x,)`, depending only on where
        // the line happened to break. Compare against the form that does not depend on that.
        let compact_generated_provider = generated_provider
            .split_whitespace()
            .collect::<String>()
            .replace(",>", ">")
            .replace(",)", ")");
        assert!(
            compact_generated_provider.contains(".build_output_stream::<f32,_,_>"),
            "expected the complete method turbofish in generated provider Rust:\n{generated_provider}"
        );
        assert!(
            generated_provider.contains("&mut [f32]"),
            "expected the named callback to retain the inspected borrowed-slice type:\n{generated_provider}"
        );
        assert!(
            !generated_provider.contains("&mut Vec<f32>"),
            "borrowed-slice callbacks must not lower to a borrowed Vec:\n{generated_provider}"
        );
        assert!(
            compact_generated_provider.contains("PairFactory::<i64,String>::new"),
            "expected receiver-side turbofish for both owner type parameters:\n{generated_provider}"
        );
        assert!(
            compact_generated_provider.contains("PairFactory::<String,i64>::first"),
            "expected owner specialization on a non-Self return:\n{generated_provider}"
        );
        assert!(
            // Assert the specialization, not the callee's spelling. RFC 120 projects `accept_pair`, so pinning the
            // source name tested the projection rather than the contextual receiver specialization this case exists
            // for. The leading paren still requires the factory call to reach the callee as the argument itself,
            // built without a turbofish and keeping its owned `String`.
            compact_generated_provider.contains("(PairFactory::new(7,\"marker\".into()))")
                || compact_generated_provider.contains("(PairFactory::new(7,\"marker\".to_string()))"),
            "expected contextual receiver specialization to preserve the owned String parameter:\n{generated_provider}"
        );

        let consumer_root = tmp.path().join("consumer");
        let consumer_main = write_project_files(
            &consumer_root,
            "[project]\nname = \"receiver_factory_compiled_consumer\"\n\n[dependencies]\nreceiver_factory_api = { path = \"../receiver_factory_api\" }\n",
            r#"from pub::receiver_factory_api import ConstructionError, Factory, Mode, PairFactory, build_stream, exercise_callbacks, exercise_receiver_generics


def accept_factory(result: Result[Factory[f32], ConstructionError]) -> None:
    match result:
        Ok(_) => pass
        Err(_) => pass


def accept_pair(value: PairFactory[i64, str]) -> None:
    pass


def main() -> None:
    accept_factory(Factory.new[f32](8, Mode.Input))
    accept_factory(Factory.new(8, Mode.Input))
    pair: PairFactory[i64, str] = PairFactory.new[i64, str](7, "marker")
    inferred: PairFactory[i64, str] = PairFactory.new(7, "marker")
    accept_pair(pair)
    accept_pair(inferred)
    accept_pair(PairFactory.new(7, "marker"))
    if PairFactory.first[str, i64]("value") == "value":
        pass
    exercise_receiver_generics()
    build_stream()
    exercise_callbacks()
"#,
        )?;
        let consumer_bake = bake_project(&consumer_root)?;
        assert!(
            consumer_bake.status.success(),
            "expected an explicit Oven bake to import the receiver-factory package closure.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );
        let consumer_build = run_build(&consumer_main, &tmp.path().join("consumer_out"))?;
        assert!(
            consumer_build.status.success(),
            "expected a compiled-provider consumer to build receiver generics, trait-method arity, and borrowed-slice callbacks.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_build.stdout),
            String::from_utf8_lossy(&consumer_build.stderr)
        );

        Ok(())
    }

    #[test]
    fn oven_build_projects_structural_mutable_reference_generics_for_an_unrelated_provider()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = tempfile::tempdir()?;
        write_foreign_reference_provider(fixture.path())?;
        let project_root = fixture.path().join("consumer");
        let _main_path = write_project_files(
            &project_root,
            "[project]\nname = \"foreign_reference_projection\"\nversion = \"0.1.0\"\n\n[project.scripts]\nmain = \"src/main.incn\"\n\n[rust-dependencies]\nforeign_reference_provider = { path = \"../foreign_reference_provider\" }\n",
            r#"from rust::foreign_reference_provider import Entity, FooBar, Gadget, Widget

def update(mut values: FooBar[tuple[Widget, Gadget]]) -> None:
    pass

def inspect(values: FooBar[Entity]) -> None:
    pass

def main() -> None:
    update(FooBar.new())
    inspect(FooBar.new())
"#,
        )?;

        let oven_home = fixture.path().join("oven-home");
        let mut bake = super::incan_command();
        bake.args(["oven", "bake", "--project", "."])
            .current_dir(&project_root)
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_HOME", &oven_home);
        support::configure_explicit_oven_bake_command(&mut bake)?;
        let bake_output = run_timed_incan_command("incan oven bake --project", bake)?;
        assert!(
            bake_output.status.success(),
            "expected the unrelated provider closure to bake.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&bake_output.stdout),
            String::from_utf8_lossy(&bake_output.stderr)
        );
        let inspection_root = project_root.join("target/incan_lock/rust_inspect");
        let inspection_workspaces = std::fs::read_dir(&inspection_root)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect::<Vec<_>>();
        assert!(
            !inspection_workspaces.is_empty(),
            "expected the bake to prepare a Rust inspection workspace"
        );
        assert!(
            inspection_workspaces
                .iter()
                .all(|workspace| workspace.join(rust_inspect::OVEN_DIRECT_INSPECTION_MARKER).is_file()),
            "a successful bake must retire every exact Cargo bootstrap workspace to direct inspection: {inspection_workspaces:?}"
        );
        assert!(
            inspection_workspaces.iter().all(|workspace| !workspace
                .join(rust_inspect::OVEN_CARGO_BOOTSTRAP_INSPECTION_MARKER)
                .exists()),
            "ordinary consumers must not inherit a completed bake's Cargo inspection capability: {inspection_workspaces:?}"
        );

        let mut check = super::incan_command();
        check
            .args(["check", "src/main.incn", "--format", "json"])
            .current_dir(&project_root)
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_HOME", &oven_home);
        let check_output = run_timed_incan_command("incan check after oven bake", check)?;
        assert!(
            check_output.status.success(),
            "expected a successful bake to authorize a separate typecheck process.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&check_output.stdout),
            String::from_utf8_lossy(&check_output.stderr)
        );

        let mut build = super::incan_command();
        build
            .args(["build", "--locked"])
            .current_dir(&project_root)
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_HOME", &oven_home);
        let build_output = run_timed_incan_command("incan build --locked", build)?;
        assert!(
            build_output.status.success(),
            "expected the freshly baked closure to build in a separate compiler process.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );

        let generated =
            std::fs::read_to_string(project_root.join("target/incan/foreign_reference_projection/src/main.rs"))?;
        assert!(
            generated.contains("FooBar<(&mut Widget, &mut Gadget)>"),
            "the generic mutable-reference contract must project mutable tuple leaves without provider-name matching:\n{generated}"
        );
        assert!(
            generated.contains("FooBar<Entity>"),
            "the same generic contract must retain a directly valid owned argument:\n{generated}"
        );
        Ok(())
    }

    #[test]
    fn oven_bake_fails_closed_when_a_derive_probe_differs_from_the_real_type() -> Result<(), Box<dyn std::error::Error>>
    {
        let fixture = tempfile::tempdir()?;
        write_shape_sensitive_reference_provider(fixture.path())?;
        let project_root = fixture.path().join("consumer");
        let _main_path = write_project_files(
            &project_root,
            "[project]\nname = \"shape_sensitive_projection\"\nversion = \"0.1.0\"\n\n[project.scripts]\nmain = \"src/main.incn\"\n\n[rust-dependencies]\nshape_sensitive_provider = { path = \"../shape_sensitive_provider\" }\n",
            r#"from rust::shape_sensitive_provider import Component, FooBar

@rust.derive(Component)
model Widget:
    value: int

def update(mut values: FooBar[Widget]) -> None:
    pass

def main() -> None:
    update(FooBar.new())
"#,
        )?;

        let oven_home = fixture.path().join("oven-home");
        let mut bake = super::incan_command();
        bake.args(["oven", "bake", "--project", "."])
            .current_dir(&project_root)
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_HOME", &oven_home);
        support::configure_explicit_oven_bake_command(&mut bake)?;
        let bake_output = run_timed_incan_command("incan oven bake --project", bake)?;
        let stderr = String::from_utf8_lossy(&bake_output.stderr);
        assert!(
            !bake_output.status.success(),
            "an input-sensitive derive must not produce a runnable artifact from probe-only evidence"
        );
        assert!(
            stderr.contains("E0277")
                && stderr.contains("Widget: shape_sensitive_provider::Component")
                && stderr.contains("FooBar<&mut Widget>"),
            "native rustc must reject the candidate ABI when the real derive omits the probed trait.\nstderr:\n{stderr}"
        );
        Ok(())
    }

    #[test]
    fn oven_bake_replays_after_format_one_lock_migrates_issue1194() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let main_path = write_project_files(
            project.path(),
            "[project]\nname = \"format_one_rebake\"\nversion = \"0.1.0\"\n",
            "def main() -> None:\n  pass\n",
        )?;
        let lock_output = run_lock(&main_path)?;
        assert!(
            lock_output.status.success(),
            "expected the fixture lock to resolve before exercising format migration.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&lock_output.stdout),
            String::from_utf8_lossy(&lock_output.stderr)
        );
        let lock_path = project.path().join("oven.lock");
        let mut lock = oven_model::lock::IncanLock::load(&lock_path)?;
        lock.format = 1;
        lock.write(&lock_path)?;

        let bake = || -> Result<std::process::Output, Box<dyn std::error::Error>> {
            let mut command = super::incan_command();
            command
                .args(["oven", "bake", "--project", "."])
                .current_dir(project.path())
                .env("CARGO_NET_OFFLINE", "true")
                .env("INCAN_HOME", project.path().join("oven-home"));
            support::configure_explicit_oven_bake_command(&mut command)?;
            run_timed_incan_command("incan oven bake --project", command)
        };

        let first_bake = bake()?;
        assert!(
            first_bake.status.success(),
            "expected a format-1 lock to migrate during its explicit bake.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&first_bake.stdout),
            String::from_utf8_lossy(&first_bake.stderr)
        );
        assert_eq!(
            oven_model::lock::IncanLock::load(&lock_path)?.format,
            2,
            "the first bake must publish the current lock format"
        );

        let replay_bake = bake()?;
        assert!(
            replay_bake.status.success(),
            "the bake that migrated the lock must replay without changing source authority.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&replay_bake.stdout),
            String::from_utf8_lossy(&replay_bake.stderr)
        );
        Ok(())
    }

    #[test]
    fn source_built_compiler_bakes_vocab_provider_without_scheduler_authority_issue1193()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = tempfile::tempdir()?;
        let provider_root = fixture.path().join("source-vocab-provider");
        std::fs::create_dir_all(provider_root.join("src"))?;
        std::fs::write(
            provider_root.join("loaf.toml"),
            "[project]\nname = \"source_vocab_provider\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        std::fs::write(
            provider_root.join("src/lib.incn"),
            "pub def filter(value: int) -> int:\n  return value\n",
        )?;
        write_vocab_companion_crate_with_source(
            &provider_root,
            "vocab_companion",
            "source_vocab_provider_companion",
            filterkit_vocab_companion_source(),
        )?;

        let mut command = super::incan_command();
        support::configure_explicit_oven_bake_command(&mut command)?;
        command
            .args(["oven", "bake", "--project", "."])
            .current_dir(&provider_root)
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_HOME", fixture.path().join("oven-home"))
            .env_remove("INCAN_INTERNAL_OVEN_LOAF_EXECUTION")
            .env_remove("INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT");
        let output = run_timed_incan_command("source incan oven bake --project", command)?;
        assert!(
            output.status.success(),
            "expected the source-built compiler to bake a [vocab] provider without scheduler-only authority.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            provider_root.join("target/lib/source_vocab_provider.incnlib").is_file(),
            "the ordinary source bake must publish the vocabulary provider artifact"
        );
        Ok(())
    }
}

//! RFC 031 pub-import integration tests.
//!
//! These regressions each drive a full `incan build` of a library plus one or more consumers, so they are the most
//! expensive cases in the suite. They live in their own libtest root because the Oven replay partitioner packs whole
//! roots: kept alongside the rest of the frontend integration tests they made one indivisible root that no split
//! could shorten.

use std::path::Path;
use std::process::Command;

use incan_test_support as support;

use support::{incan_command, strip_ansi_escapes, unique_test_project_name};

mod rfc031_pub_import_integration_tests {
    use super::*;
    use incan_frontend::library_manifest::{FieldVisibilityExport, LibraryManifest, ModelExport, TypeRef};
    use oven_model::manifest::{INTERNAL_MANIFEST_OVERRIDE_ENV, INTERNAL_PROJECT_ROOT_OVERRIDE_ENV};
    use sha2::{Digest, Sha256};
    use std::collections::BTreeSet;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    fn run_timed_incan_command(
        label: &str,
        mut command: std::process::Command,
    ) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        let timing = super::support::command_timing_started();
        let output = command.output()?;
        super::support::report_command_timing(label, timing);
        Ok(output)
    }

    /// Run one normal build with its existing JSON report retained only for opt-in timing attribution.
    fn run_profiled_build_command(
        label: &str,
        mut command: std::process::Command,
    ) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        command.args(["--report", "json"]);
        let output = run_timed_incan_command(label, command)?;
        super::support::report_build_phase_timing(label, &output);
        Ok(output)
    }

    /// Ensure the normal-library report retains the internal preparation boundaries used by Oven performance work.
    fn assert_library_build_phase_keys(
        output: &std::process::Output,
        expected: &[&str],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        let timings = report
            .get("timings_ms")
            .and_then(serde_json::Value::as_object)
            .ok_or("expected a library JSON timing report")?;
        for phase in expected {
            assert!(
                timings.contains_key(*phase),
                "expected library build timing report to contain `{phase}`: {report}"
            );
        }
        Ok(())
    }

    fn write_project_files(
        root: &Path,
        manifest_content: &str,
        main_source: &str,
    ) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
        std::fs::create_dir_all(root.join("src"))?;
        std::fs::write(root.join("loaf.toml"), manifest_content)?;
        let main_path = root.join("src").join("main.incn");
        std::fs::write(&main_path, main_source)?;
        Ok(main_path)
    }

    /// Materialize one file from an integration fixture with its parent directories.
    fn write_fixture_file(root: &Path, relative_path: &str, contents: &str) -> Result<(), Box<dyn std::error::Error>> {
        let path = root.join(relative_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, contents)?;
        Ok(())
    }

    fn run_check(main_path: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        let mut command = super::incan_command();
        command.arg("--check").arg(main_path);
        run_timed_incan_command("incan --check", command)
    }

    /// Check a consumer against this checkout's SDK rather than an ambient developer installation.
    fn run_check_against_checkout_sdk(
        main_path: &Path,
        generated_cargo_target: &Path,
    ) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        let checkout = support::repo_root();
        Ok(super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", generated_cargo_target)
            // Lock-workspace preheat is covered by its own integration suite. This C-binding consumer proof needs
            // the generated program path, and its temporary project otherwise exposes macOS's `/tmp` alias as a
            // duplicate Cargo package identity.
            .env("INCAN_LOCK_PREHEAT", "0")
            .arg("check")
            .arg(main_path)
            .output()?)
    }

    fn run_build(main_path: &Path, out_dir: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        let mut command = super::incan_command();
        command
            .args([
                "build",
                main_path.to_string_lossy().as_ref(),
                out_dir.to_string_lossy().as_ref(),
            ])
            .env("CARGO_NET_OFFLINE", "true");
        run_timed_incan_command("incan build", command)
    }

    /// Write the shared Rust dependency used by receiver-generic application and compiled-provider acceptance tests.
    fn write_receiver_factory_dependency(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let rust_dir = root.join("receiver_factory");
        std::fs::create_dir_all(rust_dir.join("src"))?;
        std::fs::write(
            rust_dir.join("Cargo.toml"),
            "[package]\nname = \"receiver_factory\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        std::fs::write(
            rust_dir.join("src/lib.rs"),
            r#"pub struct Factory<T> {
    marker: std::marker::PhantomData<T>,
}

pub struct ConstructionError;

pub enum Mode {
    Input,
}

pub struct Device;

pub fn device() -> Device {
    Device
}

pub trait DeviceTrait {
    fn build_output_stream<T, D, E>(&self, value: T, data_callback: D, error_callback: E)
    where
        T: Copy,
        D: FnMut(T),
        E: FnMut(T);

    fn run_callbacks<T, D, E>(&self, data_callback: D, error_callback: E)
    where
        T: Copy + Default,
        D: FnMut(&mut [T], &OutputCallbackInfo) + Send + 'static,
        E: FnMut(String);
}

impl DeviceTrait for Device {
    fn build_output_stream<T, D, E>(&self, value: T, mut data_callback: D, mut error_callback: E)
    where
        T: Copy,
        D: FnMut(T),
        E: FnMut(T),
    {
        data_callback(value);
        error_callback(value);
    }

    fn run_callbacks<T, D, E>(&self, mut data_callback: D, mut error_callback: E)
    where
        T: Copy + Default,
        D: FnMut(&mut [T], &OutputCallbackInfo) + Send + 'static,
        E: FnMut(String),
    {
        let mut data = [T::default(); 2];
        let info = OutputCallbackInfo;
        data_callback(&mut data, &info);
        error_callback("synthetic callback error".to_string());
    }
}

pub struct OutputCallbackInfo;

pub struct PairFactory<T, U> {
    value: T,
    marker: U,
}

impl<T> Factory<T> {
    pub fn new(_size: i64, _mode: Mode) -> Result<Self, ConstructionError> {
        Ok(Self {
            marker: std::marker::PhantomData,
        })
    }
}

impl<T, U> PairFactory<T, U> {
    pub fn new(value: T, marker: U) -> Self {
        Self { value, marker }
    }

    pub fn first(value: T) -> T {
        value
    }
}
"#,
        )?;
        Ok(())
    }

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

    fn run_lock(entry_path: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        let mut command = super::incan_command();
        command
            .args(["lock", entry_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true");
        run_timed_incan_command("incan lock", command)
    }

    fn run_test(target: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        let mut command = super::incan_command();
        command
            .args(["test", target.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_TEST_SHARED_TARGET_DIR", shared_test_runner_target_dir());
        run_timed_incan_command("incan test", command)
    }

    fn run_fmt(target: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        Ok(super::incan_command()
            .args(["fmt", target.to_string_lossy().as_ref()])
            .output()?)
    }

    fn run_fmt_check(target: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        Ok(super::incan_command()
            .args(["fmt", "--check", target.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?)
    }

    fn shared_test_runner_target_dir() -> PathBuf {
        support::repo_root().join("target").join("incan_e2e_shared_target")
    }

    fn test_runner_batch_manifest_path(project_root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let harness_root = project_root.join("target/incan_tests");
        let manifests = std::fs::read_dir(&harness_root)
            .map_err(|err| {
                format!(
                    "failed reading generated test harness root {}: {err}",
                    harness_root.display()
                )
            })?
            .filter_map(Result::ok)
            .map(|entry| entry.path().join("Cargo.toml"))
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        match manifests.as_slice() {
            [manifest] => Ok(manifest.clone()),
            _ => Err(format!(
                "expected exactly one generated test manifest below {}, found {}",
                harness_root.display(),
                manifests.len()
            )
            .into()),
        }
    }

    fn run_build_lib(project_root: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        let mut command = super::incan_command();
        command
            .args(["build", "--lib"])
            .current_dir(project_root)
            .env("CARGO_NET_OFFLINE", "true");
        run_timed_incan_command("incan build --lib", command)
    }

    /// Publish one project's completed output and sealed dependency collection.
    ///
    /// Normal `build`, `run`, and `test` remain consumers. They cannot create this package handoff implicitly,
    /// including when the project itself depends on a separately baked public provider.
    fn bake_project(project_root: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        bake_project_with_package_features(project_root, &[])
    }

    /// Write an unrelated Rust provider whose generic data wrapper admits either owned handles or mutable references.
    fn write_foreign_reference_provider(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let provider_root = root.join("foreign_reference_provider");
        std::fs::create_dir_all(provider_root.join("src"))?;
        std::fs::write(
            provider_root.join("Cargo.toml"),
            "[package]\nname = \"foreign_reference_provider\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        std::fs::write(
            provider_root.join("src/lib.rs"),
            r#"use core::marker::PhantomData;

pub trait QueryData {}
pub trait Component {}

pub struct FooBar<T: QueryData>(PhantomData<T>);

impl<T: QueryData> FooBar<T> {
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

pub struct Widget;
pub struct Gadget;
pub struct Entity;

impl Component for Widget {}
impl Component for Gadget {}
impl<T: Component> QueryData for &mut T {}
impl<A: QueryData, B: QueryData> QueryData for (A, B) {}
impl QueryData for Entity {}
"#,
        )?;
        Ok(())
    }

    /// Write a provider whose derive macro deliberately differs between the compiler probe and a field-bearing type.
    fn write_shape_sensitive_reference_provider(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let derive_root = root.join("shape_sensitive_derive");
        std::fs::create_dir_all(derive_root.join("src"))?;
        std::fs::write(
            derive_root.join("Cargo.toml"),
            "[package]\nname = \"shape_sensitive_derive\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nproc-macro = true\n",
        )?;
        std::fs::write(
            derive_root.join("src/lib.rs"),
            r#"use proc_macro::TokenStream;

#[proc_macro_derive(Component)]
pub fn component(input: TokenStream) -> TokenStream {
    let source = input.to_string();
    let name = source
        .split_whitespace()
        .nth(1)
        .expect("derive input should contain a type name")
        .trim_end_matches(';');
    if !name.starts_with("__IncanDeriveProbe") {
        return TokenStream::new();
    }
    format!("impl shape_sensitive_provider::Component for {name} {{}}")
        .parse()
        .expect("generated Component implementation should parse")
}
"#,
        )?;

        let provider_root = root.join("shape_sensitive_provider");
        std::fs::create_dir_all(provider_root.join("src"))?;
        std::fs::write(
            provider_root.join("Cargo.toml"),
            "[package]\nname = \"shape_sensitive_provider\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nshape_sensitive_derive = { path = \"../shape_sensitive_derive\" }\n",
        )?;
        std::fs::write(
            provider_root.join("src/lib.rs"),
            r#"use core::marker::PhantomData;

pub use shape_sensitive_derive::Component;

pub trait Component {}
pub trait QueryData {}

pub struct FooBar<T: QueryData>(PhantomData<T>);

impl<T: QueryData> FooBar<T> {
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

impl<T: Component> QueryData for &mut T {}
"#,
        )?;
        Ok(())
    }

    /// Publish one project for an explicit package-feature selection.
    fn bake_project_with_package_features(
        project_root: &Path,
        package_feature_args: &[&str],
    ) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        let mut command = super::incan_command();
        command
            .args(["oven", "bake", "--project", "."])
            .args(package_feature_args)
            .current_dir(project_root)
            .env("CARGO_NET_OFFLINE", "true");
        support::configure_explicit_oven_bake_command(&mut command)?;
        run_timed_incan_command("incan oven bake --project", command)
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

    /// Publish a public-library provider before a separate consumer imports its package Loaf.
    fn bake_library_provider(project_root: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        bake_project(project_root)
    }

    /// Publish one source-backed provider whose checked vocabulary metadata and desugarer are part of the bake.
    fn write_and_bake_source_vocab_fixture_provider(
        root: &Path,
        dependency_key: &str,
        project_name: &str,
        source: &str,
        companion_package: &str,
        companion_source: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let provider_root = root.join("deps").join(dependency_key);
        std::fs::create_dir_all(provider_root.join("src"))?;
        std::fs::write(
            provider_root.join("loaf.toml"),
            format!(
                "[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n"
            ),
        )?;
        std::fs::write(provider_root.join("src/lib.incn"), source)?;
        write_vocab_companion_crate_with_source(
            &provider_root,
            "vocab_companion",
            companion_package,
            companion_source,
        )?;
        let provider_bake = bake_library_provider(&provider_root)?;
        assert!(
            provider_bake.status.success(),
            "expected source-backed {dependency_key} vocabulary fixture provider to bake.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&provider_bake.stdout),
            String::from_utf8_lossy(&provider_bake.stderr)
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

    fn filterkit_vocab_companion_source() -> &'static str {
        r#"use incan_vocab::{DesugarError, DesugarOutput, HelperBinding, IncanExpr, KeywordActivation, KeywordPlacement, KeywordRegistration, KeywordSpec, KeywordSurfaceKind, LibraryManifest, VocabDesugarer, VocabRegistration, VocabSyntaxNode};

#[derive(Default)]
struct FilterkitDesugarer;

impl VocabDesugarer for FilterkitDesugarer {
    fn desugar(&self, _node: &VocabSyntaxNode) -> Result<DesugarOutput, DesugarError> {
        Ok(DesugarOutput::Expression(IncanExpr::Call {
            callee: Box::new(IncanExpr::Helper("filter".to_string())),
            args: vec![IncanExpr::Int(1)],
        }))
    }
}

pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_keyword_registration(KeywordRegistration {
            activation: KeywordActivation::OnImport { namespace: "filterkit.dsl".to_string() },
            keywords: vec![KeywordSpec {
                name: "where".to_string(),
                surface_kind: KeywordSurfaceKind::BlockDeclaration,
                compound_tokens: Vec::new(),
                placement: KeywordPlacement::TopLevel,
            }],
            valid_decorators: Vec::new(),
        })
        .with_library_manifest(LibraryManifest {
            helper_bindings: vec![HelperBinding { key: "filter".to_string(), exported_name: "filter".to_string() }],
            ..LibraryManifest::default()
        })
        .with_desugarer(FilterkitDesugarer)
}

incan_vocab::export_wasm_desugarer!(FilterkitDesugarer);
"#
    }

    fn helperkit_vocab_companion_source() -> &'static str {
        r#"use incan_vocab::{DesugarError, DesugarOutput, HelperBinding, IncanExpr, KeywordActivation, KeywordPlacement, KeywordRegistration, KeywordSpec, KeywordSurfaceKind, LibraryManifest, VocabDesugarer, VocabRegistration, VocabSyntaxNode};

#[derive(Default)]
struct HelperkitDesugarer;

impl VocabDesugarer for HelperkitDesugarer {
    fn desugar(&self, _node: &VocabSyntaxNode) -> Result<DesugarOutput, DesugarError> {
        Ok(DesugarOutput::Expression(IncanExpr::Call {
            callee: Box::new(IncanExpr::Helper("aggregate_as".to_string())),
            args: vec![
                IncanExpr::Call {
                    callee: Box::new(IncanExpr::Helper("lit".to_string())),
                    args: vec![IncanExpr::Int(5)],
                },
                IncanExpr::Str("total".to_string()),
            ],
        }))
    }
}

pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_keyword_registration(KeywordRegistration {
            activation: KeywordActivation::OnImport { namespace: "helperkit.dsl".to_string() },
            keywords: vec![KeywordSpec {
                name: "where".to_string(),
                surface_kind: KeywordSurfaceKind::BlockDeclaration,
                compound_tokens: Vec::new(),
                placement: KeywordPlacement::TopLevel,
            }],
            valid_decorators: Vec::new(),
        })
        .with_library_manifest(LibraryManifest {
            helper_bindings: vec![
                HelperBinding { key: "lit".to_string(), exported_name: "lit".to_string() },
                HelperBinding { key: "aggregate_as".to_string(), exported_name: "aggregate_as".to_string() },
            ],
            ..LibraryManifest::default()
        })
        .with_desugarer(HelperkitDesugarer)
}

incan_vocab::export_wasm_desugarer!(HelperkitDesugarer);
"#
    }

    fn quality_vocab_companion_source() -> &'static str {
        r#"use incan_vocab::{ClauseSurface, DeclarationSurface, DesugarError, DesugarOutput, DslSurface, IncanExpr, KeywordActivation, KeywordRegistration, KeywordSpec, LibraryManifest, ScopedSurfaceDescriptor, ScopedSurfaceReceiver, VocabBodyItem, VocabDesugarer, VocabRegistration, VocabSyntaxNode};

#[derive(Default)]
struct QualityDesugarer;

impl VocabDesugarer for QualityDesugarer {
    fn desugar(&self, node: &VocabSyntaxNode) -> Result<DesugarOutput, DesugarError> {
        let VocabSyntaxNode::Declaration(declaration) = node else {
            return Err(DesugarError::new("quality expects a declaration"));
        };
        let clauses = declaration
            .body
            .iter()
            .filter_map(|item| match item {
                VocabBodyItem::Clause(clause) => Some(clause),
                _ => None,
            })
            .collect::<Vec<_>>();
        for (keyword, compound_tokens) in [
            ("FROM", Vec::<String>::new()),
            ("REQUIRE", Vec::new()),
            ("GROUP", vec!["BY".to_string()]),
            ("EXPECT", Vec::new()),
        ] {
            if !clauses
                .iter()
                .any(|clause| clause.keyword == keyword && clause.compound_tokens == compound_tokens)
            {
                return Err(DesugarError::new(format!("quality request is missing {keyword}")));
            }
        }
        Ok(DesugarOutput::Expression(IncanExpr::Int(7)))
    }
}

pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_keyword_registration(KeywordRegistration {
            activation: KeywordActivation::OnImport { namespace: "querykit.query".to_string() },
            keywords: vec![
                KeywordSpec::block("quality"),
                KeywordSpec::block("FROM").in_block("quality"),
                KeywordSpec::block("REQUIRE").in_block("quality"),
                KeywordSpec::block("GROUP").with_compound_tokens(["BY"]).in_block("quality"),
                KeywordSpec::block("EXPECT").in_block("quality"),
            ],
            valid_decorators: Vec::new(),
        })
        .with_surface(
            DslSurface::on_import("querykit.query")
                .with_declaration(
                    DeclarationSurface::named("quality")
                        .with_mixed_body()
                        .desugars_to_expression()
                        .with_clauses([
                            ClauseSurface::expr("FROM").optional(),
                            ClauseSurface::expr_list("GROUP BY").repeating().after("FROM"),
                            ClauseSurface::expr_list("EXPECT").repeating().after("FROM"),
                            ClauseSurface::expr_list("REQUIRE").repeating().after("FROM"),
                        ]),
                )
                .with_scoped_surface(
                    ScopedSurfaceDescriptor::leading_dot_path("quality.group.field")
                        .in_clause_body("quality", "GROUP")
                        .with_receiver(ScopedSurfaceReceiver::clause("FROM")),
                ),
        )
        .with_library_manifest(LibraryManifest::default())
        .with_desugarer(QualityDesugarer)
}

incan_vocab::export_wasm_desugarer!(QualityDesugarer);
"#
    }

    fn querykit_expression_clause_vocab_companion_source() -> &'static str {
        r#"use incan_vocab::{ClauseSurface, DeclarationSurface, DesugarError, DesugarOutput, DslSurface, IncanExpr, VocabBodyItem, VocabDesugarer, VocabRegistration, VocabSyntaxNode};

#[derive(Default)]
struct QuerykitExpressionClauseDesugarer;

impl VocabDesugarer for QuerykitExpressionClauseDesugarer {
    fn desugar(&self, node: &VocabSyntaxNode) -> Result<DesugarOutput, DesugarError> {
        let VocabSyntaxNode::Declaration(declaration) = node else {
            return Err(DesugarError::new("query expects a declaration"));
        };
        if !declaration.body.iter().any(|item| {
            matches!(item, VocabBodyItem::Clause(clause) if clause.keyword == "SELECT")
        }) {
            return Err(DesugarError::new("missing SELECT clause payload"));
        }
        Ok(DesugarOutput::Expression(IncanExpr::Int(7)))
    }
}

pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_surface(
            DslSurface::on_import("querykit.query").with_declaration(
                DeclarationSurface::named("query")
                    .with_clause_body()
                    .desugars_to_expression()
                    .with_clauses([
                        ClauseSurface::expr("FROM").required(),
                        ClauseSurface::expr_list("GROUP BY").optional(),
                        ClauseSurface::expr_list("SELECT").required(),
                        ClauseSurface::expr_list("ORDER BY").optional(),
                        ClauseSurface::nested_items("WINDOW BY").optional(),
                    ]),
            ),
        )
        .with_desugarer(QuerykitExpressionClauseDesugarer)
}

incan_vocab::export_wasm_desugarer!(QuerykitExpressionClauseDesugarer);
"#
    }

    fn querykit_helper_vocab_companion_source() -> &'static str {
        r#"use incan_vocab::{DesugarError, DesugarOutput, HelperBinding, IncanExpr, KeywordActivation, KeywordPlacement, KeywordRegistration, KeywordSpec, KeywordSurfaceKind, LibraryManifest, VocabDesugarer, VocabRegistration, VocabSyntaxNode};

#[derive(Default)]
struct QuerykitHelperDesugarer;

impl VocabDesugarer for QuerykitHelperDesugarer {
    fn desugar(&self, _node: &VocabSyntaxNode) -> Result<DesugarOutput, DesugarError> {
        Ok(DesugarOutput::Expression(IncanExpr::Tuple(vec![
            IncanExpr::Call {
                callee: Box::new(IncanExpr::Helper("aggregate_as".to_string())),
                args: vec![
                    IncanExpr::Call {
                        callee: Box::new(IncanExpr::Helper("lit".to_string())),
                        args: vec![IncanExpr::Int(5)],
                    },
                    IncanExpr::Str("adjusted".to_string()),
                ],
            },
            IncanExpr::Call {
                callee: Box::new(IncanExpr::Helper("aggregate_as".to_string())),
                args: vec![
                    IncanExpr::Call {
                        callee: Box::new(IncanExpr::Helper("count".to_string())),
                        args: Vec::new(),
                    },
                    IncanExpr::Str("order_count".to_string()),
                ],
            },
        ])))
    }
}

pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_keyword_registration(KeywordRegistration {
            activation: KeywordActivation::OnImport { namespace: "querykit.dsl".to_string() },
            keywords: vec![KeywordSpec {
                name: "where".to_string(),
                surface_kind: KeywordSurfaceKind::BlockDeclaration,
                compound_tokens: Vec::new(),
                placement: KeywordPlacement::TopLevel,
            }],
            valid_decorators: Vec::new(),
        })
        .with_library_manifest(LibraryManifest {
            helper_bindings: vec![
                HelperBinding { key: "lit".to_string(), exported_name: "lit".to_string() },
                HelperBinding { key: "count".to_string(), exported_name: "count".to_string() },
                HelperBinding { key: "aggregate_as".to_string(), exported_name: "aggregate_as".to_string() },
            ],
            ..LibraryManifest::default()
        })
        .with_desugarer(QuerykitHelperDesugarer)
}

incan_vocab::export_wasm_desugarer!(QuerykitHelperDesugarer);
"#
    }

    /// Run one single-library build with the machine-readable phase report used by Oven timing checks.
    fn run_profiled_build_lib(
        label: &str,
        project_root: &Path,
    ) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        let mut command = super::incan_command();
        command
            .args(["build", "--lib"])
            .current_dir(project_root)
            .env("CARGO_NET_OFFLINE", "true");
        run_profiled_build_command(label, command)
    }

    /// Run a normal Oven route with a failing Cargo binary first on PATH.
    ///
    /// This is a behavioural boundary: a successful command proves that its completed-Loaf materialization used only
    /// the selected direct-rustc closure rather than merely avoiding Cargo in an outer command.
    #[cfg(unix)]
    fn run_incan_with_failing_cargo_guard(
        project_root: &Path,
        guard_root: &Path,
        cargo_marker: &Path,
        args: &[&str],
    ) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        std::fs::create_dir_all(guard_root)?;
        let cargo_guard = guard_root.join("cargo");
        std::fs::write(
            &cargo_guard,
            format!("#!/bin/sh\nprintf cargo > \"{}\"\nexit 97\n", cargo_marker.display()),
        )?;
        let mut permissions = std::fs::metadata(&cargo_guard)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&cargo_guard, permissions)?;
        let mut paths = vec![guard_root.to_path_buf()];
        if let Some(inherited) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&inherited));
        }
        Ok(super::incan_command()
            .args(args)
            .current_dir(project_root)
            .env("CARGO_NET_OFFLINE", "true")
            .env("PATH", std::env::join_paths(paths)?)
            .output()?)
    }

    fn run_build_lib_artifact_only(project_root: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        let mut command = super::incan_command();
        command
            .args(["build", "--lib"])
            .current_dir(project_root)
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_INTERNAL_LIBRARY_ARTIFACT_ONLY", "1");
        run_timed_incan_command("incan build --lib (artifact only)", command)
    }

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
from pub::boundarykit import col as __incan_vocab_helper_boundarykit_col
from pub::boundarykit import desc as __incan_vocab_helper_boundarykit_desc

def main() -> None:
  ordered: Frame = frame().order_by([
    __incan_vocab_helper_boundarykit_desc(__incan_vocab_helper_boundarykit_col("amount"))
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

    #[test]
    fn build_keeps_return_context_string_literal_union_arg_as_union_value() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("return_context_union_arg");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"return_context_union_arg\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            project_root.join("src/projection_builders.incn"),
            r#"pub model ColumnRefExpr:
    pub column_name: str

pub model StringLiteralExpr:
    pub value: str

pub model FloatLiteralExpr:
    pub value: float

pub model EqExpr:
    pub arguments: list[ColumnExpr]

pub type ColumnExpr = Union[ColumnRefExpr, StringLiteralExpr, FloatLiteralExpr, EqExpr]

pub def col(name: str) -> ColumnExpr:
    return ColumnRefExpr(column_name=name)

pub def str_expr(value: str) -> ColumnExpr:
    return StringLiteralExpr(value=value)

pub def float_expr(value: float) -> ColumnExpr:
    return FloatLiteralExpr(value=value)

pub def lit(value: Union[int, float, str, bool]) -> ColumnExpr:
    match value:
        float(number) => return float_expr(number)
        str(text) => return str_expr(text)
        bool(flag) => return str_expr("bool")
        int(number) => return str_expr("int")

pub def eq(left: ColumnExpr, right: ColumnExpr) -> ColumnExpr:
    return EqExpr(arguments=[left, right])
"#,
        )?;
        std::fs::write(
            project_root.join("src/functions.incn"),
            "from projection_builders import col as col_builder, eq as eq_builder, lit as lit_builder\n\npub col = alias col_builder\npub lit = alias lit_builder\npub eq = alias eq_builder\n",
        )?;
        std::fs::write(
            project_root.join("src/dataset.incn"),
            r#"from projection_builders import ColumnExpr

pub class LazyFrame[T with Clone]:
    pub rows: list[T]

    def filter(self, predicate: ColumnExpr) -> Self:
        return self
"#,
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            r#"from dataset import LazyFrame
from functions import col, eq, lit

model OrderLine:
    status: str
    discount: float

def repro(lines: LazyFrame[OrderLine]) -> LazyFrame[OrderLine]:
    return lines.filter(eq(col("status"), lit("open"))).filter(eq(col("discount"), lit(0.9)))

def main() -> None:
    lines: LazyFrame[OrderLine] = LazyFrame[OrderLine](rows=[])
    _ = repro(lines)
    println("done")
"#,
        )?;

        let out_dir = project_root.join("out");
        let output = run_build(&main_path, &out_dir)?;
        assert!(
            output.status.success(),
            "expected union literal regression build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let generated_main = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        let normalized: String = generated_main.chars().filter(|c| !c.is_whitespace()).collect();
        // Assert the wrapping, not the callee's spelling. RFC 120 projections emit a linker-visible
        // `__incan_v1_...` name for `lit`, so pinning the source spelling tested the projection rather than the union
        // arm this case exists for. The leading `(` still proves the literal reaches the call as the argument itself.
        assert!(
            normalized.contains("(crate::__IncanUnion43fbd19e99c1db05::V0(\"open\".to_string())"),
            "expected string literal to be wrapped directly as the union string arm, got:\n{generated_main}"
        );
        assert!(
            !normalized.contains("V0(\"open\".to_string()).to_string()"),
            "union wrapper must not receive a post-wrapper string coercion, got:\n{generated_main}"
        );
        Ok(())
    }

    #[test]
    fn std_json_and_generated_runtime_surfaces_share_one_generated_run() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"std_json_runtime_surface_batch\"\nversion = \"0.3.0-dev.1\"\n",
            r#"from std.serde import json
from std.serde.json import Deserialize, Serialize
from std.json import JsonValue

model SerializePayload with Serialize:
  value: int

model HelperPayload with Serialize:
  value: int

@derive(json)
model JsonPayload:
  value: int
  label: str

@derive(Deserialize)
model DirectPayload:
  value: int

@derive(json)
model Envelope:
  status: int
  data: JsonValue

@derive(json)
model Probe:
  name: Option[JsonValue]
  first: Option[JsonValue]
  missing: Option[JsonValue]

const NUMBERS: FrozenList[float] = [3.0, 1.5, 4.25]

def run_explicit_serialize_trait() -> None:
  println(SerializePayload(value=1).to_json())

def run_generated_runtime_helpers() -> None:
  mut xs = [3, 1, 4]
  println(xs.pop())
  println(min(xs))
  println(max(xs))
  println(HelperPayload(value=2).to_json())

def run_std_json_deserialize() -> None:
  match JsonPayload.from_json('{"value":7,"label":"dogfood"}'):
    case Ok(payload):
      println(payload.to_json())
    case Err(err):
      println(err)

def run_direct_deserialize_derive() -> None:
  match DirectPayload.from_json('{"value":7}'):
    case Ok(payload):
      println(f"{payload.value}")
    case Err(err):
      println(err)

def run_json_value_model_field_roundtrip() -> None:
  match Envelope.from_json('{"status":200,"data":{"name":"Ada","items":[1,2]}}'):
    case Ok(envelope):
      match envelope.data["items"]:
        case Some(items):
          let probe = Probe(name=envelope.data["name"], first=items[0], missing=items[9])
          println(probe.to_json())
        case None:
          println("missing items")
    case Err(err):
      println(err)

def run_std_json_value_broad_surface() -> None:
  match JsonValue.parse('{"items":[1,2],"name":"Ada","n":null}'):
    case Ok(data):
      assert data.kind().as_str() == "object"
      assert JsonValue.str("Ada").as_str() == Some("Ada")
      match data.get("n"):
        case Some(value):
          assert value.is_null()
        case None:
          assert false
      match data.get("missing"):
        case Some(_):
          assert false
        case None:
          pass
      match data["items"]:
        case Some(items):
          match items[0]:
            case Some(first):
              match first.expect_int():
                case Ok(n):
                  assert n == 1
                case Err(_):
                  assert false
            case None:
              assert false
          match items[-1]:
            case Some(_):
              assert false
            case None:
              pass
        case None:
          assert false
      mut target = JsonValue.object({"a": JsonValue.int(1)})
      match target.merge(JsonValue.object({"a": JsonValue.int(2), "b": JsonValue.str("bee")})):
        case Ok(_):
          assert target.contains_key("b")
          match target.require("a"):
            case Ok(value):
              assert value.as_int() == Some(2)
            case Err(_):
              assert false
        case Err(_):
          assert false
    case Err(err):
      println(err.message())
      assert false

def run_frozen_float_helpers() -> None:
  println(min(NUMBERS))
  println(max(NUMBERS))

def main() -> None:
  run_explicit_serialize_trait()
  run_generated_runtime_helpers()
  run_std_json_deserialize()
  run_direct_deserialize_derive()
  run_json_value_model_field_roundtrip()
  run_std_json_value_broad_surface()
  run_frozen_float_helpers()
"#,
        )?;

        let output = super::incan_command()
            .arg("run")
            .arg(&main_path)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "expected std/json and generated runtime surface batch to run successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            stdout.lines().collect::<Vec<_>>(),
            vec![
                "{\"value\":1}",
                "4",
                "1",
                "3",
                "{\"value\":2}",
                "{\"value\":7,\"label\":\"dogfood\"}",
                "7",
                "{\"name\":\"Ada\",\"first\":1,\"missing\":null}",
                "1.5",
                "4.25",
            ],
            "expected std/json and generated runtime surface transcript, got:\n{stdout}"
        );
        Ok(())
    }

    fn write_pub_boundary_type_fidelity_library(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let producer_root = root.join("pub_boundary_library");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"pub_boundary_core\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/dataset.incn"),
            r#"pub model SessionError:
  pub kind: str

pub trait DataSet[T]:
  def to_substrait_plan(self) -> int: ...

pub trait BoundedDataSet[T] with DataSet[T]:
  pass

@derive(Clone)
pub class DataFrame[T] with BoundedDataSet:
  pub _type_witness: list[T]

  def to_substrait_plan(self) -> int:
    return 1

@derive(Clone)
pub class LazyFrame[T] with BoundedDataSet:
  pub _type_witness: list[T]

  def to_substrait_plan(self) -> int:
    return 1

  def collect(self) -> Result[DataFrame[T], SessionError]:
    return Ok(DataFrame[T](_type_witness=[]))
"#,
        )?;
        std::fs::write(
            producer_root.join("src/functions.incn"),
            r#"from dataset import DataSet

pub def display[T](data: DataSet[T]) -> None:
  print(data.to_substrait_plan())
"#,
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub from dataset import SessionError, DataSet, BoundedDataSet, DataFrame, LazyFrame\npub from functions import display\n",
        )?;

        let producer_build = bake_library_provider(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected pub-boundary library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        Ok(())
    }

    fn write_minimal_library_crate(artifact_root: &Path, package_name: &str) -> Result<(), Box<dyn std::error::Error>> {
        std::fs::create_dir_all(artifact_root.join("src"))?;
        std::fs::write(
            artifact_root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n"
            ),
        )?;
        std::fs::write(artifact_root.join("src/lib.rs"), "pub fn linked() {}\n")?;
        Ok(())
    }

    fn write_vocab_companion_crate(
        project_root: &Path,
        relative_path: &str,
        package_name: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let crate_root = project_root.join(relative_path);
        std::fs::create_dir_all(crate_root.join("src"))?;
        std::fs::write(
            crate_root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nincan_vocab = {{ path = \"{}\" }}\n\n[lib]\npath = \"src/lib.rs\"\n",
                support::repo_root()
                    .join(oven_model::toolchain_layout::development_support_crate_dir(
                        "incan_vocab"
                    ))
                    .display()
            ),
        )?;
        std::fs::write(
            crate_root.join("src/lib.rs"),
            "pub fn library_vocab() -> incan_vocab::VocabRegistration {\n    incan_vocab::VocabRegistration::new().with_keyword_registration(\n        incan_vocab::KeywordRegistration {\n            activation: incan_vocab::KeywordActivation::OnImport {\n                namespace: \"widgets.dsl\".to_string(),\n            },\n            keywords: vec![incan_vocab::KeywordSpec::new(\n                \"await\",\n                incan_vocab::KeywordSurfaceKind::ControlFlow,\n            )],\n            valid_decorators: vec![\"route\".to_string()],\n        },\n    )\n}\n",
        )?;
        Ok(())
    }

    fn write_vocab_companion_crate_with_assert_keyword(
        project_root: &Path,
        relative_path: &str,
        package_name: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let crate_root = project_root.join(relative_path);
        std::fs::create_dir_all(crate_root.join("src"))?;
        std::fs::write(
            crate_root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nincan_vocab = {{ path = \"{}\" }}\n\n[lib]\npath = \"src/lib.rs\"\n",
                support::repo_root()
                    .join(oven_model::toolchain_layout::development_support_crate_dir(
                        "incan_vocab"
                    ))
                    .display()
            ),
        )?;
        std::fs::write(
            crate_root.join("src/lib.rs"),
            "pub fn library_vocab() -> incan_vocab::VocabRegistration {\n    incan_vocab::VocabRegistration::new().with_keyword_registration(\n        incan_vocab::KeywordRegistration {\n            activation: incan_vocab::KeywordActivation::OnImport {\n                namespace: \"widgets.dsl\".to_string(),\n            },\n            keywords: vec![incan_vocab::KeywordSpec::new(\n                \"assert\",\n                incan_vocab::KeywordSurfaceKind::ControlFlow,\n            )],\n            valid_decorators: vec![\"route\".to_string()],\n        },\n    )\n}\n",
        )?;
        Ok(())
    }

    fn write_vocab_companion_crate_with_source(
        project_root: &Path,
        relative_path: &str,
        package_name: &str,
        lib_source: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let crate_root = project_root.join(relative_path);
        std::fs::create_dir_all(crate_root.join("src"))?;
        std::fs::write(
            crate_root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nincan_vocab = {{ path = \"{}\" }}\n\n[lib]\npath = \"src/lib.rs\"\ncrate-type = [\"rlib\", \"cdylib\"]\n",
                support::repo_root()
                    .join(oven_model::toolchain_layout::development_support_crate_dir(
                        "incan_vocab"
                    ))
                    .display()
            ),
        )?;
        std::fs::write(crate_root.join("src/lib.rs"), lib_source)?;
        Ok(())
    }

    fn wat_bytes_string(bytes: &[u8]) -> String {
        let mut escaped = String::new();
        for byte in bytes {
            escaped.push('\\');
            escaped.push_str(&format!("{byte:02x}"));
        }
        escaped
    }

    fn wat_data_string(text: &str) -> String {
        wat_bytes_string(text.as_bytes())
    }

    fn wat_i32_cell(value: i32) -> String {
        wat_bytes_string(&value.to_le_bytes())
    }

    fn compile_desugarer_wasm(
        status_code: i32,
        output_payload: &str,
        error_payload: &str,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let output_ptr_cell = 0usize;
        let output_len_cell = 4usize;
        let error_ptr_cell = 8usize;
        let error_len_cell = 12usize;
        let input_ptr_cell = 16usize;
        let input_capacity_cell = 20usize;
        let input_len_cell = 24usize;
        let output_offset = 128usize;
        let output_len = output_payload.len();
        let error_offset = output_offset + output_len + 32;
        let input_offset = error_offset + error_payload.len() + 32;
        let input_capacity = 4096usize;
        let wat_source = format!(
            r#"(module
  (memory (export "memory") 1)
  (global $input_ptr_cell (export "__incan_input_ptr") i32 (i32.const {input_ptr_cell}))
  (global (export "__incan_input_capacity") i32 (i32.const {input_capacity_cell}))
  (global $input_len_cell (export "__incan_input_len") i32 (i32.const {input_len_cell}))
  (global (export "__incan_output_ptr") i32 (i32.const {output_ptr_cell}))
  (global (export "__incan_output_len") i32 (i32.const {output_len_cell}))
  (global (export "__incan_error_ptr") i32 (i32.const {error_ptr_cell}))
  (global (export "__incan_error_len") i32 (i32.const {error_len_cell}))
  (data (i32.const {output_ptr_cell}) "{output_ptr_data}")
  (data (i32.const {output_len_cell}) "{output_len_data}")
  (data (i32.const {error_ptr_cell}) "{error_ptr_data}")
  (data (i32.const {error_len_cell}) "{error_len_data}")
  (data (i32.const {input_ptr_cell}) "{input_ptr_data}")
  (data (i32.const {input_capacity_cell}) "{input_capacity_data}")
  (data (i32.const {input_len_cell}) "{input_len_data}")
  (data (i32.const {output_offset}) "{output_data}")
  (data (i32.const {error_offset}) "{error_data}")
  (func (export "__incan_init_desugarer"))
  (func (export "desugar_block") (result i32)
    (i32.const {status_code})
  )
)"#,
            output_ptr_cell = output_ptr_cell,
            output_len_cell = output_len_cell,
            error_ptr_cell = error_ptr_cell,
            error_len_cell = error_len_cell,
            input_ptr_cell = input_ptr_cell,
            input_capacity_cell = input_capacity_cell,
            input_len_cell = input_len_cell,
            output_ptr_data = wat_i32_cell(output_offset as i32),
            output_len_data = wat_i32_cell(output_payload.len() as i32),
            error_ptr_data = wat_i32_cell(error_offset as i32),
            error_len_data = wat_i32_cell(error_payload.len() as i32),
            input_ptr_data = wat_i32_cell(input_offset as i32),
            input_capacity_data = wat_i32_cell(input_capacity as i32),
            input_len_data = wat_i32_cell(0),
            output_data = wat_data_string(output_payload),
            error_data = wat_data_string(error_payload),
        );
        Ok(wat::parse_str(wat_source)?)
    }

    fn compile_desugarer_wasm_requiring_request(
        output_payload: &str,
        error_payload: &str,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let output_ptr_cell = 0usize;
        let output_len_cell = 4usize;
        let error_ptr_cell = 8usize;
        let error_len_cell = 12usize;
        let input_ptr_cell = 16usize;
        let input_capacity_cell = 20usize;
        let input_len_cell = 24usize;
        let output_offset = 128usize;
        let output_len = output_payload.len();
        let error_offset = output_offset + output_len + 32;
        let input_offset = error_offset + error_payload.len() + 32;
        let input_capacity = 4096usize;
        let wat_source = format!(
            r#"(module
  (memory (export "memory") 1)
  (global $input_ptr_cell (export "__incan_input_ptr") i32 (i32.const {input_ptr_cell}))
  (global (export "__incan_input_capacity") i32 (i32.const {input_capacity_cell}))
  (global $input_len_cell (export "__incan_input_len") i32 (i32.const {input_len_cell}))
  (global (export "__incan_output_ptr") i32 (i32.const {output_ptr_cell}))
  (global (export "__incan_output_len") i32 (i32.const {output_len_cell}))
  (global (export "__incan_error_ptr") i32 (i32.const {error_ptr_cell}))
  (global (export "__incan_error_len") i32 (i32.const {error_len_cell}))
  (data (i32.const {output_ptr_cell}) "{output_ptr_data}")
  (data (i32.const {output_len_cell}) "{output_len_data}")
  (data (i32.const {error_ptr_cell}) "{error_ptr_data}")
  (data (i32.const {error_len_cell}) "{error_len_data}")
  (data (i32.const {input_ptr_cell}) "{input_ptr_data}")
  (data (i32.const {input_capacity_cell}) "{input_capacity_data}")
  (data (i32.const {input_len_cell}) "{input_len_data}")
  (data (i32.const {output_offset}) "{output_data}")
  (data (i32.const {error_offset}) "{error_data}")
  (func (export "__incan_init_desugarer"))
  (func (export "desugar_block") (result i32)
    global.get $input_len_cell
    i32.load
    i32.eqz
    if (result i32)
      (i32.const 1)
    else
      global.get $input_ptr_cell
      i32.load
      i32.load8_u
      i32.const 123
      i32.eq
      if (result i32)
        (i32.const 0)
      else
        (i32.const 1)
      end
    end
  )
)"#,
            output_ptr_cell = output_ptr_cell,
            output_len_cell = output_len_cell,
            error_ptr_cell = error_ptr_cell,
            error_len_cell = error_len_cell,
            input_ptr_cell = input_ptr_cell,
            input_capacity_cell = input_capacity_cell,
            input_len_cell = input_len_cell,
            output_ptr_data = wat_i32_cell(output_offset as i32),
            output_len_data = wat_i32_cell(output_payload.len() as i32),
            error_ptr_data = wat_i32_cell(error_offset as i32),
            error_len_data = wat_i32_cell(error_payload.len() as i32),
            input_ptr_data = wat_i32_cell(input_offset as i32),
            input_capacity_data = wat_i32_cell(input_capacity as i32),
            input_len_data = wat_i32_cell(0),
            output_data = wat_data_string(output_payload),
            error_data = wat_data_string(error_payload),
        );
        Ok(wat::parse_str(wat_source)?)
    }

    fn compile_desugarer_wasm_requiring_request_substring(
        output_payload: &str,
        error_payload: &str,
        needle: &str,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let output_ptr_cell = 0usize;
        let output_len_cell = 4usize;
        let error_ptr_cell = 8usize;
        let error_len_cell = 12usize;
        let input_ptr_cell = 16usize;
        let input_capacity_cell = 20usize;
        let input_len_cell = 24usize;
        let output_offset = 128usize;
        let output_len = output_payload.len();
        let error_offset = output_offset + output_len + 32;
        let input_offset = error_offset + error_payload.len() + 32;
        let input_capacity = 16_384usize;
        let needle_offset = input_offset + input_capacity + 32;
        let needle_len = needle.len();
        let wat_source = format!(
            r#"(module
  (memory (export "memory") 1)
  (global $input_ptr_cell (export "__incan_input_ptr") i32 (i32.const {input_ptr_cell}))
  (global (export "__incan_input_capacity") i32 (i32.const {input_capacity_cell}))
  (global $input_len_cell (export "__incan_input_len") i32 (i32.const {input_len_cell}))
  (global (export "__incan_output_ptr") i32 (i32.const {output_ptr_cell}))
  (global (export "__incan_output_len") i32 (i32.const {output_len_cell}))
  (global (export "__incan_error_ptr") i32 (i32.const {error_ptr_cell}))
  (global (export "__incan_error_len") i32 (i32.const {error_len_cell}))
  (data (i32.const {output_ptr_cell}) "{output_ptr_data}")
  (data (i32.const {output_len_cell}) "{output_len_data}")
  (data (i32.const {error_ptr_cell}) "{error_ptr_data}")
  (data (i32.const {error_len_cell}) "{error_len_data}")
  (data (i32.const {input_ptr_cell}) "{input_ptr_data}")
  (data (i32.const {input_capacity_cell}) "{input_capacity_data}")
  (data (i32.const {input_len_cell}) "{input_len_data}")
  (data (i32.const {output_offset}) "{output_data}")
  (data (i32.const {error_offset}) "{error_data}")
  (data (i32.const {needle_offset}) "{needle_data}")
  (func (export "__incan_init_desugarer"))
  (func $matches_at (param $pos i32) (result i32)
    (local $j i32)
    (block $fail
      (loop $scan
        local.get $j
        i32.const {needle_len}
        i32.ge_u
        if
          i32.const 1
          return
        end
        local.get $pos
        local.get $j
        i32.add
        i32.load8_u
        i32.const {needle_offset}
        local.get $j
        i32.add
        i32.load8_u
        i32.ne
        br_if $fail
        local.get $j
        i32.const 1
        i32.add
        local.set $j
        br $scan
      )
    )
    i32.const 0
  )
  (func (export "desugar_block") (result i32)
    (local $input_ptr i32)
    (local $input_len i32)
    (local $end i32)
    (local $i i32)
    global.get $input_ptr_cell
    i32.load
    local.set $input_ptr
    global.get $input_len_cell
    i32.load
    local.set $input_len
    local.get $input_len
    i32.const {needle_len}
    i32.lt_u
    if
      i32.const 1
      return
    end
    local.get $input_ptr
    local.get $input_len
    i32.add
    i32.const {needle_len}
    i32.sub
    i32.const 1
    i32.add
    local.set $end
    local.get $input_ptr
    local.set $i
    (block $not_found
      (loop $search
        local.get $i
        local.get $end
        i32.ge_u
        br_if $not_found
        local.get $i
        call $matches_at
        if
          i32.const 0
          return
        end
        local.get $i
        i32.const 1
        i32.add
        local.set $i
        br $search
      )
    )
    i32.const 1
  )
)"#,
            output_ptr_cell = output_ptr_cell,
            output_len_cell = output_len_cell,
            error_ptr_cell = error_ptr_cell,
            error_len_cell = error_len_cell,
            input_ptr_cell = input_ptr_cell,
            input_capacity_cell = input_capacity_cell,
            input_len_cell = input_len_cell,
            output_ptr_data = wat_i32_cell(output_offset as i32),
            output_len_data = wat_i32_cell(output_payload.len() as i32),
            error_ptr_data = wat_i32_cell(error_offset as i32),
            error_len_data = wat_i32_cell(error_payload.len() as i32),
            input_ptr_data = wat_i32_cell(input_offset as i32),
            input_capacity_data = wat_i32_cell(input_capacity as i32),
            input_len_data = wat_i32_cell(0),
            output_data = wat_data_string(output_payload),
            error_data = wat_data_string(error_payload),
            needle_offset = needle_offset,
            needle_len = needle_len,
            needle_data = wat_data_string(needle),
        );
        Ok(wat::parse_str(wat_source)?)
    }

    fn write_pub_library_with_vocab_desugarer(
        root: &Path,
        dependency_key: &str,
        manifest_name: &str,
        desugarer_bytes: &[u8],
        keyword: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let artifact_root = root.join("deps").join(dependency_key).join("target").join("lib");
        std::fs::create_dir_all(artifact_root.join("desugarers"))?;
        write_minimal_library_crate(&artifact_root, manifest_name)?;
        let desugarer_path = artifact_root.join("desugarers").join("routes_desugarer.wasm");
        std::fs::write(&desugarer_path, desugarer_bytes)?;

        let mut manifest = LibraryManifest::new(manifest_name, "0.1.0");
        manifest.vocab = Some(incan_frontend::library_manifest::VocabExports {
            crate_path: "vocab_companion".to_string(),
            package_name: "vocab_companion".to_string(),
            keyword_registrations: vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: format!("{dependency_key}.dsl"),
                },
                keywords: vec![incan_vocab::KeywordSpec {
                    name: keyword.to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                    compound_tokens: Vec::new(),
                    placement: incan_vocab::KeywordPlacement::TopLevel,
                }],
                valid_decorators: Vec::new(),
            }],
            dsl_surfaces: Vec::new(),
            provider_manifest: incan_vocab::LibraryManifest::default(),
            desugarer_artifact: Some(incan_frontend::library_manifest::VocabDesugarerArtifact {
                artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
                abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
                relative_path: "desugarers/routes_desugarer.wasm".to_string(),
                target: "wasm32-wasip1".to_string(),
                profile: "release".to_string(),
                entrypoint: "desugar_block".to_string(),
                sha256: hex::encode(Sha256::digest(desugarer_bytes)),
            }),
        });
        manifest.write_to_path(&artifact_root.join(format!("{manifest_name}.incnlib")))?;
        Ok(())
    }

    fn write_pub_library_with_querykit_surface_desugarer(
        root: &Path,
        desugarer_bytes: &[u8],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let artifact_root = root.join("deps").join("querykit").join("target").join("lib");
        std::fs::create_dir_all(artifact_root.join("desugarers"))?;
        write_minimal_library_crate(&artifact_root, "querykit_core")?;
        let desugarer_path = artifact_root.join("desugarers").join("querykit_desugarer.wasm");
        std::fs::write(&desugarer_path, desugarer_bytes)?;

        let mut manifest = LibraryManifest::new("querykit_core", "0.1.0");
        manifest.vocab = Some(incan_frontend::library_manifest::VocabExports {
            crate_path: "vocab_companion".to_string(),
            package_name: "vocab_companion".to_string(),
            keyword_registrations: vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "querykit.query".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec {
                    name: "query".to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                    compound_tokens: Vec::new(),
                    placement: incan_vocab::KeywordPlacement::TopLevel,
                }],
                valid_decorators: Vec::new(),
            }],
            dsl_surfaces: vec![
                incan_vocab::DslSurface::on_import("querykit.query")
                    .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                    .with_scoped_surface(
                        incan_vocab::ScopedSurfaceDescriptor::leading_dot_path("query.field")
                            .in_declaration_body("query")
                            .with_receiver(incan_vocab::ScopedSurfaceReceiver::OwningDeclaration),
                    ),
            ],
            provider_manifest: incan_vocab::LibraryManifest::default(),
            desugarer_artifact: Some(incan_frontend::library_manifest::VocabDesugarerArtifact {
                artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
                abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
                relative_path: "desugarers/querykit_desugarer.wasm".to_string(),
                target: "wasm32-wasip1".to_string(),
                profile: "release".to_string(),
                entrypoint: "desugar_block".to_string(),
                sha256: hex::encode(Sha256::digest(desugarer_bytes)),
            }),
        });
        manifest.write_to_path(&artifact_root.join("querykit_core.incnlib"))?;
        Ok(())
    }

    fn write_pub_library_with_querykit_select_desugarer(
        root: &Path,
        desugarer_bytes: &[u8],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let artifact_root = root.join("deps").join("querykit").join("target").join("lib");
        std::fs::create_dir_all(artifact_root.join("desugarers"))?;
        write_minimal_library_crate(&artifact_root, "querykit_core")?;
        let desugarer_path = artifact_root.join("desugarers").join("querykit_desugarer.wasm");
        std::fs::write(&desugarer_path, desugarer_bytes)?;

        let metadata = incan_vocab::VocabRegistration::new()
            .with_surface(
                incan_vocab::DslSurface::on_import("querykit.query").with_declaration(
                    incan_vocab::DeclarationSurface::named("query")
                        .with_clause_body()
                        .desugars_to_expression()
                        .with_clause(
                            incan_vocab::ClauseSurface::expr_list("SELECT")
                                .with_expression_item_modifiers([
                                    incan_vocab::ExpressionItemModifierSurface::expr("for"),
                                    incan_vocab::ExpressionItemModifierSurface::expr("with"),
                                ])
                                .required(),
                        ),
                ),
            )
            .metadata();
        let mut manifest = LibraryManifest::new("querykit_core", "0.1.0");
        manifest.vocab = Some(incan_frontend::library_manifest::VocabExports {
            crate_path: "vocab_companion".to_string(),
            package_name: "vocab_companion".to_string(),
            keyword_registrations: metadata.keyword_registrations,
            dsl_surfaces: metadata.dsl_surfaces,
            provider_manifest: incan_vocab::LibraryManifest::default(),
            desugarer_artifact: Some(incan_frontend::library_manifest::VocabDesugarerArtifact {
                artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
                abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
                relative_path: "desugarers/querykit_desugarer.wasm".to_string(),
                target: "wasm32-wasip1".to_string(),
                profile: "release".to_string(),
                entrypoint: "desugar_block".to_string(),
                sha256: hex::encode(Sha256::digest(desugarer_bytes)),
            }),
        });
        manifest.write_to_path(&artifact_root.join("querykit_core.incnlib"))?;
        Ok(())
    }

    fn write_pub_library_with_querykit_expression_clause_desugarer(
        root: &Path,
        desugarer_bytes: &[u8],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let artifact_root = root.join("deps").join("querykit").join("target").join("lib");
        std::fs::create_dir_all(artifact_root.join("desugarers"))?;
        write_minimal_library_crate(&artifact_root, "querykit_core")?;
        let desugarer_path = artifact_root.join("desugarers").join("querykit_desugarer.wasm");
        std::fs::write(&desugarer_path, desugarer_bytes)?;

        let metadata = incan_vocab::VocabRegistration::new()
            .with_surface(
                incan_vocab::DslSurface::on_import("querykit.query").with_declaration(
                    incan_vocab::DeclarationSurface::named("query")
                        .with_clause_body()
                        .desugars_to_expression()
                        .with_clauses([
                            incan_vocab::ClauseSurface::expr("FROM").required(),
                            incan_vocab::ClauseSurface::expr_list("GROUP BY").optional(),
                            incan_vocab::ClauseSurface::expr_list("SELECT").required(),
                            incan_vocab::ClauseSurface::expr_list("ORDER BY").optional(),
                            incan_vocab::ClauseSurface::nested_items("WINDOW BY").optional(),
                        ]),
                ),
            )
            .metadata();
        let mut manifest = LibraryManifest::new("querykit_core", "0.1.0");
        manifest.vocab = Some(incan_frontend::library_manifest::VocabExports {
            crate_path: "vocab_companion".to_string(),
            package_name: "vocab_companion".to_string(),
            keyword_registrations: metadata.keyword_registrations,
            dsl_surfaces: metadata.dsl_surfaces,
            provider_manifest: incan_vocab::LibraryManifest::default(),
            desugarer_artifact: Some(incan_frontend::library_manifest::VocabDesugarerArtifact {
                artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
                abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
                relative_path: "desugarers/querykit_desugarer.wasm".to_string(),
                target: "wasm32-wasip1".to_string(),
                profile: "release".to_string(),
                entrypoint: "desugar_block".to_string(),
                sha256: hex::encode(Sha256::digest(desugarer_bytes)),
            }),
        });
        manifest.write_to_path(&artifact_root.join("querykit_core.incnlib"))?;
        Ok(())
    }

    fn write_source_pub_library_with_vocab_desugarer_and_query_helpers(
        root: &Path,
        with_vocab: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let producer_root = root.join("deps").join("querykit");

        // ---- Context: source-backed helper library ----
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            if with_vocab {
                "[project]\nname = \"querykit\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n"
            } else {
                "[project]\nname = \"querykit\"\nversion = \"0.1.0\"\n"
            },
        )?;
        std::fs::write(
            producer_root.join("src/helpers.incn"),
            r#"pub model IntLiteralExpr:
  pub value: int

pub model StringLiteralExpr:
  pub value: str

pub type LiteralValue = Union[int, str]
pub type ColumnExpr = Union[IntLiteralExpr, StringLiteralExpr]

pub model AggregateMeasure:
  pub expr: ColumnExpr
  pub label: str

pub const DEFAULT_LABEL: str = "orders"
pub const COUNT_SENTINEL: str = "__querykit_count_no_argument__"

pub def lit(value: LiteralValue) -> ColumnExpr:
  match value:
    int(number) => return IntLiteralExpr(value=number)
    str(text) => return StringLiteralExpr(value=text)

pub def col(name: str) -> ColumnExpr:
  return StringLiteralExpr(value=name)

pub def count(expr: ColumnExpr = col(COUNT_SENTINEL)) -> ColumnExpr:
  return expr

pub def aggregate_as(expr: ColumnExpr, output_name: str) -> AggregateMeasure:
  return AggregateMeasure(expr=expr, label=output_name)

pub def aggregate_default(expr: ColumnExpr, output_name: str = DEFAULT_LABEL) -> AggregateMeasure:
  return AggregateMeasure(expr=expr, label=output_name)
"#,
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub from helpers import IntLiteralExpr, StringLiteralExpr, LiteralValue, ColumnExpr, AggregateMeasure, DEFAULT_LABEL, lit, count, aggregate_as, aggregate_default\n",
        )?;

        if with_vocab {
            write_vocab_companion_crate_with_source(
                &producer_root,
                "vocab_companion",
                "querykit_helper_vocab_companion",
                querykit_helper_vocab_companion_source(),
            )?;
        }

        let producer_build = bake_library_provider(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected querykit producer build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        Ok(())
    }

    fn write_pub_library_with_provider_requirements(
        root: &Path,
        dependency_key: &str,
        manifest_name: &str,
        required_dependencies: Vec<incan_vocab::CargoDependency>,
        required_stdlib_features: Vec<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let artifact_root = root.join("deps").join(dependency_key).join("target").join("lib");
        std::fs::create_dir_all(artifact_root.join("src"))?;
        write_minimal_library_crate(&artifact_root, manifest_name)?;

        let mut manifest = LibraryManifest::new(manifest_name, "0.1.0");
        manifest.vocab = Some(incan_frontend::library_manifest::VocabExports {
            crate_path: format!("{dependency_key}_vocab_companion"),
            package_name: format!("{dependency_key}_vocab_companion"),
            keyword_registrations: Vec::new(),
            dsl_surfaces: Vec::new(),
            provider_manifest: incan_vocab::LibraryManifest {
                required_dependencies,
                required_stdlib_features: required_stdlib_features
                    .into_iter()
                    .map(std::string::ToString::to_string)
                    .collect(),
                ..incan_vocab::LibraryManifest::default()
            },
            desugarer_artifact: None,
        });
        manifest.write_to_path(&artifact_root.join(format!("{manifest_name}.incnlib")))?;
        Ok(())
    }

    fn write_and_bake_source_provider_with_requirements_and_assert_keyword(
        root: &Path,
        axum_requirement: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let provider_root = root.join("deps/widgets");
        std::fs::create_dir_all(provider_root.join("src"))?;
        std::fs::write(
            provider_root.join("loaf.toml"),
            "[project]\nname = \"requirements_widgets_core\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        std::fs::write(
            provider_root.join("src/lib.incn"),
            "pub def requirements_fixture_identity() -> int:\n  return 1\n",
        )?;
        let companion_source = r#"use incan_vocab::{
    CargoDependency, CargoDependencySource, KeywordActivation, KeywordRegistration, KeywordSpec,
    KeywordSurfaceKind, LibraryManifest, VocabRegistration,
};

pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_keyword_registration(KeywordRegistration {
            activation: KeywordActivation::OnImport {
                namespace: "widgets.dsl".to_string(),
            },
            keywords: vec![KeywordSpec::new("assert", KeywordSurfaceKind::ControlFlow)],
            valid_decorators: Vec::new(),
        })
        .with_library_manifest(LibraryManifest {
            required_dependencies: vec![CargoDependency {
                crate_name: "axum".to_string(),
                source: CargoDependencySource::Version("__AXUM_REQUIREMENT__".to_string()),
            }],
            required_stdlib_features: vec!["web".to_string()],
            ..LibraryManifest::default()
        })
}
"#
        .replace("__AXUM_REQUIREMENT__", axum_requirement);
        write_vocab_companion_crate_with_source(
            &provider_root,
            "vocab_companion",
            "requirements_widgets_vocab_companion",
            &companion_source,
        )?;
        let provider_bake = bake_library_provider(&provider_root)?;
        assert!(
            provider_bake.status.success(),
            "expected the widgets provider requirements to bake for axum {axum_requirement}.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&provider_bake.stdout),
            String::from_utf8_lossy(&provider_bake.stderr)
        );
        Ok(())
    }

    /// Publish the root identity a schema-v2 manifest owes for one directly declared model export.
    ///
    /// A v2 identity graph publishes one root entry per raw declaration. A hand-built fixture that pushes a
    /// `ModelExport` without one is rejected while the manifest is written, before the test reaches the diagnostic it
    /// is actually checking.
    fn push_root_model_identity(manifest: &mut LibraryManifest, library: &str, name: &str) {
        let identity = incan_semantics_core::CanonicalSymbolId {
            namespace: incan_semantics_core::SymbolNamespace::OrdinaryLexical,
            origin: incan_semantics_core::SymbolOrigin::Package {
                library: library.to_string(),
                module_path: Vec::new(),
            },
            declaration_name: name.to_string(),
            kind: incan_semantics_core::SemanticSourceTargetKind::Model,
            scope_discriminant: None,
            declaration_span: incan_semantics_core::HirSourceSpan::new(0, 1),
        };
        manifest
            .contract_metadata
            .identity_graph
            .exports
            .push(incan_frontend::library_manifest::ExportIdentity {
                public_name: name.to_string(),
                public_path: vec![library.to_string(), name.to_string()],
                source_path: vec![name.to_string()],
                kind: incan_frontend::library_manifest::ExportIdentityKind::Model,
                projection: incan_frontend::library_manifest::ExportIdentityProjection::Direct,
                canonical: incan_frontend::library_manifest::CanonicalIdentityExport::from_canonical(
                    library, &identity,
                ),
            });
    }

    fn mylib_manifest_with_widget() -> LibraryManifest {
        let mut manifest = LibraryManifest::new("mylib", "0.1.0");
        manifest.exports.models.push(ModelExport {
            name: "Widget".to_string(),
            type_params: Vec::new(),
            traits: Vec::new(),
            trait_adoptions: Vec::new(),
            derives: Vec::new(),
            fields: Vec::new(),
            properties: Vec::new(),
            methods: Vec::new(),
        });
        push_root_model_identity(&mut manifest, "mylib", "Widget");
        manifest
    }

    #[test]
    fn check_reports_unknown_pub_library() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"app\"\n",
            "from pub::missinglib import Widget\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            !output.status.success(),
            "expected check to fail for unknown library, stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&output.stderr));
        assert!(
            stderr.contains("Unknown `pub::` library `missinglib`"),
            "expected unknown-library diagnostic, got:\n{stderr}"
        );
        Ok(())
    }

    #[test]
    fn check_reports_missing_pub_export() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let dep_manifest_path = tmp
            .path()
            .join("deps")
            .join("mylib")
            .join("target")
            .join("lib")
            .join("mylib.incnlib");
        std::fs::create_dir_all(dep_manifest_path.parent().ok_or("missing dependency manifest parent")?)?;
        mylib_manifest_with_widget().write_to_path(&dep_manifest_path)?;
        write_minimal_library_crate(
            dep_manifest_path.parent().ok_or("missing dependency artifact root")?,
            "mylib",
        )?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"app\"\n\n[dependencies]\nmylib = { path = \"deps/mylib\" }\n",
            "from pub::mylib import MissingSymbol\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            !output.status.success(),
            "expected check to fail for missing export, stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&output.stderr));
        assert!(
            stderr.contains("is not exported by `pub::mylib`"),
            "expected missing-export diagnostic, got:\n{stderr}"
        );
        Ok(())
    }

    #[test]
    fn check_reports_pub_manifest_load_failure() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let dep_manifest_path = tmp
            .path()
            .join("deps")
            .join("mylib")
            .join("target")
            .join("lib")
            .join("mylib.incnlib");
        std::fs::create_dir_all(dep_manifest_path.parent().ok_or("missing dependency manifest parent")?)?;
        std::fs::write(&dep_manifest_path, "{ not-json }\n")?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"app\"\n\n[dependencies]\nmylib = { path = \"deps/mylib\" }\n",
            "from pub::mylib import Widget\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            !output.status.success(),
            "expected check to fail for manifest load failure, stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&output.stderr));
        assert!(
            stderr.contains("Failed to load manifest for `pub::mylib`"),
            "expected manifest-load diagnostic, got:\n{stderr}"
        );
        Ok(())
    }

    #[test]
    fn check_passes_for_pub_imported_manifest_type() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let dep_manifest_path = tmp
            .path()
            .join("deps")
            .join("mylib")
            .join("target")
            .join("lib")
            .join("mylib.incnlib");
        std::fs::create_dir_all(dep_manifest_path.parent().ok_or("missing dependency manifest parent")?)?;
        mylib_manifest_with_widget().write_to_path(&dep_manifest_path)?;
        write_minimal_library_crate(
            dep_manifest_path.parent().ok_or("missing dependency artifact root")?,
            "mylib",
        )?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"app\"\n\n[dependencies]\nmylib = { path = \"deps/mylib\" }\n",
            "from pub::mylib import Widget\n\ndef build(x: Widget) -> Widget:\n  return x\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected check to pass for valid pub import, stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn check_reports_missing_pub_library_artifacts() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let dep_manifest_path = tmp
            .path()
            .join("deps")
            .join("mylib")
            .join("target")
            .join("lib")
            .join("mylib.incnlib");
        std::fs::create_dir_all(dep_manifest_path.parent().ok_or("missing dependency manifest parent")?)?;
        mylib_manifest_with_widget().write_to_path(&dep_manifest_path)?;
        // Intentionally do not write Cargo.toml / src/lib.rs to exercise artifact-contract diagnostics.

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"app\"\n\n[dependencies]\nmylib = { path = \"deps/mylib\" }\n",
            "from pub::mylib import Widget\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            !output.status.success(),
            "expected check to fail for missing crate artifacts, stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&output.stderr));
        assert!(
            stderr.contains("Missing generated crate artifacts for `pub::mylib`"),
            "expected missing-artifact diagnostic, got:\n{stderr}"
        );
        Ok(())
    }

    #[test]
    fn check_reports_pub_library_artifact_mismatch() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let dep_artifact_root = tmp.path().join("deps").join("widgets-lib").join("target").join("lib");
        std::fs::create_dir_all(&dep_artifact_root)?;
        let mut manifest = LibraryManifest::new("widgets_core", "0.1.0");
        manifest.exports.models.push(ModelExport {
            name: "Widget".to_string(),
            type_params: Vec::new(),
            traits: Vec::new(),
            trait_adoptions: Vec::new(),
            derives: Vec::new(),
            fields: Vec::new(),
            properties: Vec::new(),
            methods: Vec::new(),
        });
        push_root_model_identity(&mut manifest, "widgets_core", "Widget");
        manifest.write_to_path(&dep_artifact_root.join("widgets_core.incnlib"))?;
        write_minimal_library_crate(&dep_artifact_root, "different_package_name")?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"app\"\n\n[dependencies]\nwidgets = { path = \"deps/widgets-lib\" }\n",
            "from pub::widgets import Widget\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            !output.status.success(),
            "expected check to fail for artifact mismatch, stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&output.stderr));
        assert!(
            stderr.contains("Generated crate metadata mismatch for `pub::widgets`"),
            "expected artifact mismatch diagnostic, got:\n{stderr}"
        );
        Ok(())
    }

    #[test]
    fn build_lib_artifacts_and_consumer_alias_typecheck() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("widgets_core_project");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"widgets_core\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/widgets.incn"),
            "pub model Widget:\n  pub name: str\n\npub def make_widget(name: str) -> Widget:\n  return Widget(name=name)\n",
        )?;
        std::fs::write(
            producer_root.join("src/boxmod.incn"),
            "pub class Box:\n  def get[T with Clone](self, value: T) -> T:\n    return value\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub from boxmod import Box\npub from widgets import Widget, make_widget\n",
        )?;

        let producer_build = bake_library_provider(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected `build --lib` to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        let producer_artifact_root = producer_root.join("target").join("lib");
        assert!(producer_artifact_root.join("Cargo.toml").is_file());
        assert!(producer_artifact_root.join("src/lib.rs").is_file());
        assert!(producer_artifact_root.join("widgets_core.incnlib").is_file());

        let consumer_root = tmp.path().join("consumer_app");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::write(
            consumer_root.join("loaf.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nwidgets = { path = \"../widgets_core_project\" }\n",
        )?;
        let consumer_main = consumer_root.join("src/main.incn");
        std::fs::write(
            &consumer_main,
            "from pub::widgets import Box, Widget as PublicWidget, make_widget\n\ndef main() -> None:\n  w: PublicWidget = make_widget(\"ok\")\n  box: Box = Box()\n  value: int = box.get(1)\n  print(w.name)\n  print(value)\n",
        )?;

        let consumer_check = run_check(&consumer_main)?;
        assert!(
            consumer_check.status.success(),
            "expected consumer check to accept pub:: alias and generic carrier imports.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_check.stdout),
            String::from_utf8_lossy(&consumer_check.stderr)
        );

        Ok(())
    }

    #[test]
    fn compiled_library_preserves_public_computed_property_contract_issue952() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let source_root = support::repo_root();
        let stdlib = source_root.join("loaves/stdlib");
        let producer_root = tmp.path().join("computed_property_provider");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"computed_property_provider\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub class Index:\n  value: int\n\n  pub property dimensions -> int:\n    return self.value\n\npub def make_index() -> Index:\n  return Index(value=2)\n",
        )?;

        let producer_build = super::incan_command()
            .args(["build", "--lib"])
            .current_dir(&producer_root)
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_SOURCE_ROOT", &source_root)
            .env("INCAN_STDLIB", &stdlib)
            .env_remove("INCAN_STDLIB_DIR")
            .output()?;
        assert!(
            producer_build.status.success(),
            "expected producer library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );

        let consumer_root = tmp.path().join("consumer_app");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::write(
            consumer_root.join("loaf.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\ncomputed_property_provider = { path = \"../computed_property_provider\" }\n",
        )?;
        let consumer_main = consumer_root.join("src/main.incn");
        std::fs::write(
            &consumer_main,
            "from pub::computed_property_provider import make_index\n\ndef main() -> None:\n  index = make_index()\n  println(index.dimensions)\n",
        )?;

        let consumer_check = super::incan_command()
            .arg("--check")
            .arg(&consumer_main)
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_SOURCE_ROOT", &source_root)
            .env("INCAN_STDLIB", &stdlib)
            .env_remove("INCAN_STDLIB_DIR")
            .output()?;
        assert!(
            consumer_check.status.success(),
            "expected a compiled-library consumer to access a public computed property.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_check.stdout),
            String::from_utf8_lossy(&consumer_check.stderr)
        );
        Ok(())
    }

    #[test]
    fn build_lib_publishes_checked_modules_and_split_aliases_issues948_892() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("modulelib");
        std::fs::create_dir_all(producer_root.join("src/hyperquant"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"modulelib\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "\"\"\"A module-oriented library.\"\"\"\n\npub from proposals import ConsoleProposal, make_console_proposal\n",
        )?;
        std::fs::write(
            producer_root.join("src/proposals.incn"),
            r#"pub model ConsoleProposal:
  pub title: str

pub def make_console_proposal(title: str) -> ConsoleProposal:
  return ConsoleProposal(title=title)
"#,
        )?;
        std::fs::write(
            producer_root.join("src/hyperquant/mod.incn"),
            "pub def namespace_version() -> int:\n  return 1\n",
        )?;
        std::fs::write(
            producer_root.join("src/hyperquant/index.incn"),
            r#"pub model HyperquantIndex:
  pub size: int

  def doubled(self) -> int:
    return self.size * 2

pub class IndexBuilder:
  pub size: int = 3

  def build(self) -> HyperquantIndex:
    return HyperquantIndex(size=self.size)

pub enum IndexMode:
  Dense
  Sparse

pub type IndexSize = newtype int
pub type IndexList = list[HyperquantIndex]

pub const DEFAULT_SIZE: int = 4

pub def build_index(size: int = DEFAULT_SIZE) -> HyperquantIndex:
  return HyperquantIndex(size=size)

pub default_index = partial build_index(size=DEFAULT_SIZE)

def pack_bits(size: int) -> int:
  return size
"#,
        )?;
        std::fs::write(
            producer_root.join("src/hyperquant/search.incn"),
            r#"from hyperquant.index import HyperquantIndex

pub def search(index: HyperquantIndex) -> int:
  return index.size * 2
"#,
        )?;
        std::fs::write(
            producer_root.join("src/hyperquant/facade.incn"),
            "pub from hyperquant.index import HyperquantIndex as PublicIndex, build_index as make_index\n",
        )?;

        let producer_build = bake_library_provider(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected an unplumbed source directory to publish as a checked namespace.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        assert!(
            producer_root.join("target/lib/modulelib.incnlib").is_file(),
            "expected the combined provider's compiled .incnlib manifest"
        );
        let generated_namespace = std::fs::read_to_string(producer_root.join("target/lib/src/hyperquant/mod.rs"))?;
        assert!(
            generated_namespace.contains("pub use facade::*;")
                && generated_namespace.contains("pub use index::*;")
                && generated_namespace.contains("pub use search::*;"),
            "expected the generated package namespace to re-export its immediate checked source modules.\n\
             generated hyperquant/mod.rs:\n{generated_namespace}"
        );

        let consumer_root = tmp.path().join("consumer");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::write(
            consumer_root.join("loaf.toml"),
            "[project]\nname = \"modulelib_consumer\"\n\n[dependencies]\nmodulelib = { path = \"../modulelib\" }\n",
        )?;
        let main_path = consumer_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            r#"from pub::modulelib import hyperquant
from pub::modulelib import ConsoleProposal as ProviderConsoleProposal, make_console_proposal
from pub::modulelib.hyperquant.facade import PublicIndex as FacadeIndex, make_index
from pub::modulelib.hyperquant import HyperquantIndex as PublicIndex, IndexBuilder, IndexList, IndexMode, IndexSize, search as nested_search

def proposal() -> ProviderConsoleProposal:
  return make_console_proposal("ready")

def main() -> None:
  index: PublicIndex = PublicIndex(size=hyperquant.DEFAULT_SIZE)
  facade_index: FacadeIndex = make_index()
  default_index: PublicIndex = hyperquant.default_index()
  builder = IndexBuilder()
  built: PublicIndex = builder.build()
  indexes: IndexList = [index, facade_index, default_index, built]
  size = IndexSize(index.doubled())
  mode = IndexMode.Dense
  println(nested_search(indexes[0]) + size.0 + hyperquant.namespace_version())
  println(nested_search(facade_index))
  println(nested_search(default_index))
  println(proposal().title)
  match mode:
    IndexMode.Dense => println("dense")
    IndexMode.Sparse => println("sparse")
"#,
        )?;

        let consumer_bake = bake_project(&consumer_root)?;
        assert!(
            consumer_bake.status.success(),
            "expected an explicit Oven bake to import the modulelib package closure.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );
        let out_dir = consumer_root.join("out");
        let consumer_build = run_build(&main_path, &out_dir)?;
        assert!(
            consumer_build.status.success(),
            "expected generated Rust for public module imports to compile.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_build.stdout),
            String::from_utf8_lossy(&consumer_build.stderr)
        );
        let generated_main = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        assert!(
            generated_main.contains("modulelib::ConsoleProposal"),
            "expected generated Rust to retain the combined provider-qualified proposal path.\ngenerated main.rs:\n{generated_main}"
        );
        std::fs::create_dir_all(consumer_root.join("tests"))?;
        std::fs::write(
            consumer_root.join("tests/test_public_module.incn"),
            r#"import pub::modulelib.hyperquant as h

def test_public_module_namespace() -> None:
  index = h.HyperquantIndex(size=h.DEFAULT_SIZE)
  builder = h.IndexBuilder()
  mode = h.IndexMode.Dense
  size = h.IndexSize(index.doubled())
  assert h.search(index) == 8
  assert builder.build().size == 3
  assert size.0 == 8
  assert mode == h.IndexMode.Dense
"#,
        )?;
        std::fs::write(
            consumer_root.join("tests/test_split_alias.incn"),
            r#"from pub::modulelib import ConsoleProposal as SplitConsoleProposal
from pub::modulelib import make_console_proposal

def make_split() -> SplitConsoleProposal:
  return make_console_proposal("split")

def test_split_pub_import_alias() -> None:
  proposal: SplitConsoleProposal = make_split()
  assert proposal.title == "split"
"#,
        )?;
        std::fs::write(
            consumer_root.join("tests/test_same_statement_alias.incn"),
            r#"from pub::modulelib import ConsoleProposal as SameStatementConsoleProposal, make_console_proposal

def make_same_statement() -> SameStatementConsoleProposal:
  return make_console_proposal("same")

def test_same_statement_pub_import_alias() -> None:
  proposal: SameStatementConsoleProposal = make_same_statement()
  assert proposal.title == "same"
"#,
        )?;
        let consumer_tests = run_test(&consumer_root.join("tests"))?;
        assert!(
            consumer_tests.status.success(),
            "expected direct public-module imports to compile and run in a package test batch.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_tests.stdout),
            String::from_utf8_lossy(&consumer_tests.stderr)
        );
        let test_stdout = String::from_utf8_lossy(&consumer_tests.stdout);
        assert!(
            test_stdout.contains("test_public_module.incn::test_public_module_namespace")
                && test_stdout.contains("test_split_alias.incn::test_split_pub_import_alias")
                && test_stdout.contains("test_same_statement_alias.incn::test_same_statement_pub_import_alias"),
            "expected all #948/#892 regressions in the shared test batch.\nstdout:\n{test_stdout}"
        );

        std::fs::write(
            producer_root.join("src/hyperquant.incn"),
            "pub def conflicting_module_file() -> int:\n  return 2\n",
        )?;
        let collision_build = bake_library_provider(&producer_root)?;
        assert!(
            !collision_build.status.success(),
            "a module file and directory entrypoint with the same logical identity must not publish nondeterministically"
        );
        let collision_error = String::from_utf8_lossy(&collision_build.stderr);
        assert!(
            collision_error.contains("both resolve to library module `hyperquant`"),
            "expected the producer collision diagnostic, got:\n{collision_error}"
        );
        Ok(())
    }

    #[test]
    fn build_lib_consumer_preserves_private_class_field_visibility_issue883() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("sealed_class_lib");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"sealed_class_lib\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/vaults.incn"),
            r#"const DEFAULT_PRIVATE_TEXT: str = "authority"

def default_label() -> str:
  return "sealed"

pub class VaultBase:
  private_text: str = DEFAULT_PRIVATE_TEXT
  computed_secret: int = 1 + 2
  pub base_count: int

pub class Vault extends VaultBase:
  secret: str
  pub label: str = default_label()

  def reveal(self) -> str:
    return self.secret

pub def make_vault(secret: str) -> Vault:
  return Vault(base_count=7, secret=secret)
"#,
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub from crate.vaults import Vault as PublicVault, VaultBase, make_vault\n",
        )?;

        let producer_build = bake_library_provider(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected private-field provider library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        let provider_manifest =
            LibraryManifest::read_from_path(&producer_root.join("target/lib/sealed_class_lib.incnlib"))?;
        let vault = provider_manifest
            .contract_metadata
            .api
            .as_ref()
            .and_then(|api| api.modules.iter().find(|module| module.module_path == ["vaults"]))
            .and_then(|module| {
                module.declarations.iter().find_map(|declaration| match declaration {
                    incan_frontend::api_metadata::ApiDeclaration::Class(class) if class.name == "Vault" => Some(class),
                    _ => None,
                })
            })
            .ok_or("expected facade-backed Vault in checked API metadata")?;
        assert_eq!(
            vault.fields.iter().map(|field| field.name.as_str()).collect::<Vec<_>>(),
            vec!["private_text", "computed_secret", "base_count", "secret", "label"],
            "manifest constructor fields must retain the provider's parent-first ABI order"
        );
        let private_text = vault
            .fields
            .iter()
            .find(|field| field.name == "private_text")
            .ok_or("expected inherited private_text field in provider manifest")?;
        let secret = vault
            .fields
            .iter()
            .find(|field| field.name == "secret")
            .ok_or("expected secret field in provider manifest")?;
        let computed_secret = vault
            .fields
            .iter()
            .find(|field| field.name == "computed_secret")
            .ok_or("expected computed_secret field in provider manifest")?;
        let label = vault
            .fields
            .iter()
            .find(|field| field.name == "label")
            .ok_or("expected label field in provider manifest")?;
        assert_eq!(secret.visibility, FieldVisibilityExport::Private);
        assert_eq!(label.visibility, FieldVisibilityExport::Public);
        assert_eq!(private_text.visibility, FieldVisibilityExport::Private);
        assert_eq!(computed_secret.visibility, FieldVisibilityExport::Private);
        assert!(computed_secret.has_default);
        assert_eq!(computed_secret.default, None);
        assert!(private_text.has_default);
        assert_eq!(
            private_text.default,
            Some(incan_frontend::library_manifest::ParamDefaultExport::ConstRef(vec![
                "vaults".to_string(),
                "DEFAULT_PRIVATE_TEXT".to_string(),
            ]))
        );
        assert!(label.has_default);
        assert!(matches!(
            label.default,
            Some(incan_frontend::library_manifest::ParamDefaultExport::Call {
                ref path,
                ref signature,
                ..
            }) if path == &["vaults".to_string(), "default_label".to_string()] && signature.is_some()
        ));

        let decoy_root = tmp.path().join("decoy_class_lib");
        std::fs::create_dir_all(decoy_root.join("src"))?;
        std::fs::write(
            decoy_root.join("loaf.toml"),
            "[project]\nname = \"decoy_class_lib\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            decoy_root.join("src/lib.incn"),
            r#"pub class PublicVault:
  private_text: int = 11
  computed_secret: str = "wrong" + "provider"
  pub base_count: str
  secret: int
  pub label: int = 99
"#,
        )?;
        let decoy_build = bake_library_provider(&decoy_root)?;
        assert!(
            decoy_build.status.success(),
            "expected duplicate-short-name decoy library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&decoy_build.stdout),
            String::from_utf8_lossy(&decoy_build.stderr)
        );

        let consumer_root = tmp.path().join("consumer");
        // Both spellings resolve the same published class. Keep them in one
        // consumer build while preserving the separate negative access check.
        let consumer_main = write_project_files(
            &consumer_root,
            "[project]\nname = \"sealed_class_consumer\"\n\n[dependencies]\nsealed_class_lib = { path = \"../sealed_class_lib\" }\ndecoy_class_lib = { path = \"../decoy_class_lib\" }\n",
            r#"from pub::sealed_class_lib import PublicVault, PublicVault as ConsumerVault

def main() -> None:
  value: PublicVault = PublicVault(base_count=9, secret="authority")
  overridden: PublicVault = PublicVault(computed_secret=4, base_count=10, secret="override")
  aliased: ConsumerVault = ConsumerVault(base_count=11, secret="alias")
  println(value.label)
  println(value.base_count)
  println(overridden.base_count)
  println(aliased.base_count)
"#,
        )?;

        let consumer_bake = bake_project(&consumer_root)?;
        assert!(
            consumer_bake.status.success(),
            "expected an explicit Oven bake to import the sealed-class package closure.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );
        let public_build = run_build(&consumer_main, &consumer_root.join("out"))?;
        assert!(
            public_build.status.success(),
            "expected public and aliased named construction to generate valid Rust.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&public_build.stdout),
            String::from_utf8_lossy(&public_build.stderr)
        );
        std::fs::write(
            &consumer_main,
            r#"from pub::sealed_class_lib import PublicVault as ConsumerVault

def main() -> None:
  value: ConsumerVault = ConsumerVault(base_count=7, secret="authority")
  println(value.secret)
"#,
        )?;
        let private_check = run_check(&consumer_main)?;
        assert!(
            !private_check.status.success(),
            "expected compiled-library private field access to fail Incan typechecking"
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&private_check.stderr));
        assert!(
            stderr.contains("Field 'secret' on 'ConsumerVault' is private"),
            "expected source-level private field diagnostic, got:\n{stderr}"
        );

        Ok(())
    }

    #[test]
    fn private_pub_model_survives_facade_library_and_test_batch_boundaries_issue884()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("sealed_model_lib");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"sealed_model_lib\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/vaults.incn"),
            r#"from std.serde.json import Serialize

pub model Vault with Serialize:
  secret [alias="wire_secret"]: str = "sealed"
  pub label: str

  def reveal(self) -> str:
    return self.secret

  def reflected_field_count(self) -> int:
    return len(self.__field_items__())
"#,
        )?;
        std::fs::write(
            producer_root.join("src/public_api.incn"),
            "pub from crate.vaults import Vault as PublicVault\n",
        )?;
        std::fs::write(
            producer_root.join("src/source_consumer.incn"),
            r#"from crate.vaults import Vault

pub def make_vault(label: str) -> Vault:
  return Vault(label=label)
"#,
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            concat!(
                "pub from crate.public_api import PublicVault as ExportedVault\n",
                "pub from crate.source_consumer import make_vault\n",
            ),
        )?;
        let sibling_leak = producer_root.join("src/sibling_leak.incn");
        std::fs::write(
            &sibling_leak,
            r#"from crate.vaults import Vault

def leak(value: Vault) -> str:
  return value.secret
"#,
        )?;
        let sibling_check = run_check(&sibling_leak)?;
        assert!(
            !sibling_check.status.success(),
            "expected a sibling source module to be outside the private model boundary"
        );
        let sibling_stderr = strip_ansi_escapes(&String::from_utf8_lossy(&sibling_check.stderr));
        assert!(
            sibling_stderr.contains("Field 'secret' on 'Vault' is private"),
            "expected sibling private-field diagnostic, got:\n{sibling_stderr}"
        );
        std::fs::remove_file(&sibling_leak)?;

        let producer_build = bake_library_provider(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected private-model provider library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        let provider_manifest =
            LibraryManifest::read_from_path(&producer_root.join("target/lib/sealed_model_lib.incnlib"))?;
        let vault = provider_manifest
            .contract_metadata
            .api
            .as_ref()
            .and_then(|api| api.modules.iter().find(|module| module.module_path == ["vaults"]))
            .and_then(|module| {
                module.declarations.iter().find_map(|declaration| match declaration {
                    incan_frontend::api_metadata::ApiDeclaration::Model(model) if model.name == "Vault" => Some(model),
                    _ => None,
                })
            })
            .ok_or("expected facade-backed Vault in checked API metadata")?;
        let secret = vault
            .fields
            .iter()
            .find(|field| field.name == "secret")
            .ok_or("expected private secret field in provider manifest")?;
        let label = vault
            .fields
            .iter()
            .find(|field| field.name == "label")
            .ok_or("expected public label field in provider manifest")?;
        assert_eq!(secret.visibility, FieldVisibilityExport::Private);
        assert_eq!(label.visibility, FieldVisibilityExport::Public);

        let consumer_root = tmp.path().join("consumer");
        let consumer_main = write_project_files(
            &consumer_root,
            "[project]\nname = \"sealed_model_consumer\"\nversion = \"0.1.0\"\n\n[dependencies]\nsealed_model_lib = { path = \"../sealed_model_lib\" }\n",
            r#"from pub::sealed_model_lib import ExportedVault as ConsumerVault, make_vault

def main() -> None:
  value = ConsumerVault(label="visible")
  make_vault("sibling")
  println(value.label)
  println(value.reveal())
  fields = value.__fields__()
  println(len(fields))
  println(fields[0].name)
  println(value.reflected_field_count())
"#,
        )?;
        let tests_dir = consumer_root.join("tests");
        std::fs::create_dir_all(&tests_dir)?;
        std::fs::write(
            tests_dir.join("test_private_model.incn"),
            r#"from pub::sealed_model_lib import ExportedVault as TestVault

def test_private_model_provider_bridge_and_reflection() -> None:
  value = TestVault(label="test-batch")
  assert value.label == "test-batch"
  assert value.reveal() == "sealed"
  assert len(value.__fields__()) == 1
  assert value.reflected_field_count() == 1
"#,
        )?;
        let consumer_bake = bake_project(&consumer_root)?;
        assert!(
            consumer_bake.status.success(),
            "expected an explicit Oven bake to import the sealed-model package closure.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );
        let consumer_run = incan_command()
            .current_dir(&consumer_root)
            .args(["run", consumer_main.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            consumer_run.status.success(),
            "expected compiled private-model consumer to run.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_run.stdout),
            String::from_utf8_lossy(&consumer_run.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&consumer_run.stdout).trim(),
            "visible\nsealed\n1\nlabel\n1"
        );
        let test_output = run_test(&tests_dir)?;
        assert!(
            test_output.status.success(),
            "expected compiled private-model test batch to pass.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&test_output.stdout),
            String::from_utf8_lossy(&test_output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&test_output.stdout).contains("test_private_model_provider_bridge_and_reflection"),
            "expected private-model regression test to execute:\n{}",
            String::from_utf8_lossy(&test_output.stdout)
        );

        std::fs::write(
            &consumer_main,
            r#"from pub::sealed_model_lib import ExportedVault as ConsumerVault

def leak(value: ConsumerVault) -> str:
  return value.secret

def construct() -> ConsumerVault:
  return ConsumerVault(wire_secret="leaked", label="outside")

def unpack(value: ConsumerVault) -> str:
  match value:
    ConsumerVault(secret=secret) =>
      return secret
"#,
        )?;
        let private_check = run_check(&consumer_main)?;
        assert!(
            !private_check.status.success(),
            "expected compiled consumer access and construction through private model fields to fail"
        );
        let private_stderr = strip_ansi_escapes(&String::from_utf8_lossy(&private_check.stderr));
        assert!(
            private_stderr.contains("Field 'secret' on 'ConsumerVault' is private")
                && private_stderr.contains("Field 'wire_secret' on 'ConsumerVault' is private"),
            "expected canonical and aliased private-field diagnostics, got:\n{private_stderr}"
        );

        Ok(())
    }

    #[test]
    fn compiled_parent_fields_lower_into_consumer_subclasses_issue885() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let provider_root = tmp.path().join("compiled_parent");
        std::fs::create_dir_all(provider_root.join("src"))?;
        std::fs::write(
            provider_root.join("loaf.toml"),
            "[project]\nname = \"compiled_parent\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            provider_root.join("src/lib.incn"),
            r#"
pub class Base:
  private_text: str = "authority"
  pub base_count: int

pub class Child extends Base:
  pub own_flag: bool
"#,
        )?;
        let provider_build = bake_library_provider(&provider_root)?;
        assert!(
            provider_build.status.success(),
            "expected compiled parent library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&provider_build.stdout),
            String::from_utf8_lossy(&provider_build.stderr)
        );
        assert!(
            provider_root.join("target/lib/compiled_parent.incnlib").is_file(),
            "expected the provider build to publish its compiled library artifact"
        );

        let consumer_root = tmp.path().join("consumer");
        let consumer_main = write_project_files(
            &consumer_root,
            "[project]\nname = \"compiled_parent_consumer\"\nversion = \"0.1.0\"\n\n[dependencies]\ncompiled_parent = { path = \"../compiled_parent\" }\n",
            r#"from pub::compiled_parent import Child

class GrandChild extends Child:
  pub extra: float

def main() -> None:
  value: GrandChild = GrandChild(
    base_count=7,
    own_flag=true,
    extra=1.5,
  )
  println(value.base_count)
"#,
        )?;

        let out_dir = consumer_root.join("out");
        let consumer_bake = bake_project(&consumer_root)?;
        assert!(
            consumer_bake.status.success(),
            "expected an explicit Oven bake to import the compiled-parent package closure.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );
        // One normal consumer build proves the completed result is reusable for the original lowering regression.
        // Lock, package-test batch, transitive provider, and Rust bridge paths
        // each have focused coverage; repeating them here adds cost without
        // extending #885's compiled-parent field-materialization contract.
        let build_output = run_build(&consumer_main, &out_dir)?;
        assert!(
            build_output.status.success(),
            "expected inherited compiled-parent fields to survive generated Rust lowering.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );

        let generated_main = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        let grandchild_start = generated_main
            .find("struct GrandChild")
            .ok_or("expected generated GrandChild struct")?;
        let grandchild_end = generated_main[grandchild_start..]
            .find("\n}")
            .map(|offset| grandchild_start + offset)
            .ok_or("expected generated GrandChild struct body")?;
        let grandchild = &generated_main[grandchild_start..grandchild_end];
        let private_index = grandchild
            .find("private_text: String")
            .ok_or("expected inherited private_text field in generated GrandChild")?;
        assert!(
            !grandchild.contains("pub private_text: String"),
            "expected inherited private_text to preserve private visibility.\ngenerated GrandChild:\n{grandchild}"
        );
        let base_index = grandchild
            .find("pub base_count: i64")
            .ok_or("expected inherited base_count field in generated GrandChild")?;
        let child_index = grandchild
            .find("pub own_flag: bool")
            .ok_or("expected inherited own_flag field in generated GrandChild")?;
        let local_index = grandchild
            .find("pub extra: f64")
            .ok_or("expected local extra field in generated GrandChild")?;
        assert!(
            private_index < base_index && base_index < child_index && child_index < local_index,
            "expected compiled and consumer inheritance fields in parent-first order.\ngenerated GrandChild:\n{grandchild}"
        );

        Ok(())
    }

    #[test]
    fn build_succeeds_for_pub_import_regression_batch() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("pub_import_regression_batch_project");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"pub_import_regression_batch\"\nversion = \"0.1.0\"\n",
        )?;

        let files = [
            (
                "src/session/types.incn",
                r#"pub class Session:
  pub id: int
"#,
            ),
            ("src/session/mod.incn", "pub from crate.session.types import Session\n"),
            (
                "src/session_facade_case.incn",
                r#"from session import Session

pub def run_session_facade() -> None:
  s = Session(id=1)
  print(s.id)
"#,
            ),
            (
                "src/imported_enum_loop_rels.incn",
                r#"@derive(Clone)
pub enum ConformanceRel:
  Read
  Filter
"#,
            ),
            (
                "src/imported_enum_loop_case.incn",
                r#"from imported_enum_loop_rels import ConformanceRel

def relation_kind_name_from_conformance(rel: ConformanceRel) -> str:
  match rel:
    ConformanceRel.Read =>
      return "ReadRel"
    _ =>
      return "Other"

def scenario_matches(required: list[ConformanceRel]) -> bool:
  for expected in required:
    if expected == ConformanceRel.Read:
      if relation_kind_name_from_conformance(expected) == "ReadRel":
        return true
  return false

pub def run_imported_enum_loop() -> None:
  println(scenario_matches([ConformanceRel.Read]))
"#,
            ),
            (
                "src/len_comparison_recursive_case.incn",
                r#"@derive(Clone)
pub enum ExprKind:
  Column
  Add

@derive(Clone)
pub model Expr:
  pub kind: ExprKind
  pub column_name: str
  pub arguments: list[Expr]

pub def lower(expr: Expr) -> int:
  if expr.kind == ExprKind.Column:
    return 0
  if len(expr.arguments) < 2:
    return -1
  return 1

pub def run_len_comparison_recursive() -> None:
  println(lower(Expr(kind=ExprKind.Add, column_name="root", arguments=[])))
"#,
            ),
            (
                "src/loop_helper_shared_string_list_case.incn",
                r#"def match_index(xs: list[str], y: int) -> int:
  mut idx = 0
  while idx < len(xs):
    if len(xs[idx]) == y:
      return idx
    idx = idx + 1
  return -1

def helper_loop(xs: list[str], ys: list[int]) -> list[int]:
  mut out: list[int] = []
  for y in ys:
    out.append(match_index(xs, y))
  return out

pub def run_loop_helper_shared_string_list() -> None:
  helper_loop(["a", "bb", "ccc"], [1, 2])
"#,
            ),
            (
                "src/dict_comp_reuses_noncopy_key_case.incn",
                r#"def lengths(names: list[str]) -> dict[str, int]:
  return {name: len(name) for name in names}

pub def run_dict_comp_reuses_noncopy_key() -> None:
  values = lengths(["alice", "bob"])
  println(values["alice"])
"#,
            ),
            (
                "src/tuple_unpack_enumerate_cases.incn",
                r#"model Binding:
  name: str
  output_index: int
  expr_index: int

def field_ref(index: int) -> int:
  return index

def bind_loop(xs: list[str]) -> list[Binding]:
  mut out: list[Binding] = []
  for idx, name in enumerate(xs):
    out.append(Binding(name=name, output_index=idx, expr_index=field_ref(idx)))
  return out

def bind_comp(xs: list[str]) -> list[Binding]:
  return [Binding(name=name, output_index=idx, expr_index=field_ref(idx)) for idx, name in enumerate(xs)]

pub def run_tuple_unpack_enumerate_cases() -> None:
  bind_loop(["a", "bb"])
  bind_comp(["a", "bb"])
"#,
            ),
            (
                "src/list_str_append_literal_case.incn",
                r#"pub def columns(input_columns: list[str]) -> list[str]:
  mut columns: list[str] = []
  columns.append(input_columns[0])
  columns.append("count")
  return columns

pub def run_list_str_append_literal() -> None:
  columns(["orders_total"])
"#,
            ),
            (
                "src/imported_sum_functions.incn",
                r#"pub model ColumnRef:
  pub name: str

pub model AggregateMeasure:
  pub column_name: str

pub def col(name: str) -> ColumnRef:
  return ColumnRef(name=name)

pub def sum(expr: ColumnRef) -> AggregateMeasure:
  return AggregateMeasure(column_name=expr.name)
"#,
            ),
            (
                "src/imported_sum_shadow_case.incn",
                r#"from imported_sum_functions import col, sum

def selected_column_name() -> str:
  amount = col("amount")
  result = sum(amount)
  return result.column_name

pub def run_imported_sum_shadow() -> None:
  println(selected_column_name())
"#,
            ),
            (
                "src/cross_module_union_producers.incn",
                r#"pub def parse_value(flag: bool) -> int | str:
  if flag:
    return 1
  return "fallback"
"#,
            ),
            (
                "src/cross_module_union_consumers.incn",
                r#"pub def describe(value: int | str) -> str:
  if isinstance(value, int):
    return "number"
  else:
    return value.upper()
"#,
            ),
            (
                "src/cross_module_union_case.incn",
                r#"from cross_module_union_producers import parse_value
from cross_module_union_consumers import describe

pub def run_cross_module_union() -> None:
  println(describe(parse_value(False)))
  println(describe("literal"))
"#,
            ),
            (
                "src/qualified_enum_constructor_match_case.incn",
                r#"pub enum QualifiedConformanceRel:
  Read
  Filter
  Project

pub def relation_kind_name_from_conformance(rel: QualifiedConformanceRel) -> str:
  match rel:
    QualifiedConformanceRel.Read =>
      return "ReadRel"
    QualifiedConformanceRel.Filter =>
      return "FilterRel"
    QualifiedConformanceRel.Project =>
      return "ProjectRel"
    _ =>
      return "UnknownRel"

pub def run_qualified_enum_constructor_match() -> None:
  println(relation_kind_name_from_conformance(QualifiedConformanceRel.Filter))
"#,
            ),
            (
                "src/main.incn",
                r#"from cross_module_union_case import run_cross_module_union
from dict_comp_reuses_noncopy_key_case import run_dict_comp_reuses_noncopy_key
from imported_enum_loop_case import run_imported_enum_loop
from imported_sum_shadow_case import run_imported_sum_shadow
from len_comparison_recursive_case import run_len_comparison_recursive
from list_str_append_literal_case import run_list_str_append_literal
from loop_helper_shared_string_list_case import run_loop_helper_shared_string_list
from qualified_enum_constructor_match_case import run_qualified_enum_constructor_match
from session_facade_case import run_session_facade
from tuple_unpack_enumerate_cases import run_tuple_unpack_enumerate_cases

def main() -> None:
  run_session_facade()
  run_imported_enum_loop()
  run_len_comparison_recursive()
  run_loop_helper_shared_string_list()
  run_dict_comp_reuses_noncopy_key()
  run_tuple_unpack_enumerate_cases()
  run_list_str_append_literal()
  run_imported_sum_shadow()
  run_cross_module_union()
  run_qualified_enum_constructor_match()
"#,
            ),
        ];

        for (relative, source) in files {
            let path = project_root.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, source)?;
        }

        let main_path = project_root.join("src/main.incn");
        let build_output = run_build(&main_path, &project_root.join("out"))?;
        assert!(
            build_output.status.success(),
            "expected pub import regression batch project to build successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );

        Ok(())
    }

    #[test]
    fn build_and_run_iterator_comprehension_and_if_let_scenarios() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"iterator_comprehension_if_let_batch\"\nversion = \"0.1.0\"\n",
            "def is_even(n: int) -> bool:\n  return n % 2 == 0\n\n\
def double(n: int) -> int:\n  return n * 2\n\n\
def maybe_double(opt: Option[int]) -> int:\n  if let Some(value) = opt:\n    return value * 2\n  return 0\n\n\
def next_value(values: list[Option[int]], idx: int) -> Option[int]:\n  if idx < len(values):\n    return values[idx]\n  return None\n\n\
def sum_values(values: list[Option[int]]) -> int:\n  mut idx = 0\n  mut total = 0\n  while let Some(value) = next_value(values, idx):\n    total = total + value\n    idx = idx + 1\n  return total\n\n\
def main() -> None:\n  xs = [1, 2, 3, 4, 5]\n  ys = xs.iter().filter(is_even).map(double).take(2).collect()\n  batches = xs.iter().batch(2).collect()\n  println(len(ys))\n  println(ys[0])\n  println(len(batches))\n  comp_source = [1, 2, 3]\n  comp = [n * 2 for n in comp_source if n > 1]\n  println(len(comp))\n  println(comp[0])\n  println(len(comp_source))\n  println(maybe_double(Some(21)))\n  println(maybe_double(None))\n  println(sum_values([Some(1), Some(2), None, Some(99)]))\n",
        )?;

        let out_dir = tmp.path().join("out");
        let build_output = run_build(&main_path, &out_dir)?;
        assert!(
            build_output.status.success(),
            "expected iterator/comprehension/if-let batch to build successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );

        // The normal build above already proves the compiler route. Execute that exact Oven artifact for the
        // language-runtime assertions instead of recompiling the same source through `incan run`.
        let binary = out_dir.join("oven/release/iterator_comprehension_if_let_batch");
        assert!(
            binary.is_file(),
            "expected Oven to produce the iterator/comprehension executable at {}",
            binary.display()
        );
        let run_output = Command::new(&binary).output()?;
        assert!(
            run_output.status.success(),
            "expected built iterator/comprehension/if-let batch to run successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&run_output.stdout),
            String::from_utf8_lossy(&run_output.stderr)
        );

        let stdout = String::from_utf8_lossy(&run_output.stdout);
        assert_eq!(
            stdout.lines().collect::<Vec<_>>(),
            vec!["2", "4", "3", "2", "4", "3", "42", "0", "3"]
        );

        Ok(())
    }

    #[test]
    fn build_lib_with_vocab_companion_embeds_vocab_payload() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("widgets_vocab_project");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"widgets_vocab_core\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub def make_widget(name: str) -> str:\n  return name\n",
        )?;
        write_vocab_companion_crate(&producer_root, "vocab_companion", "widgets_vocab_companion")?;

        let producer_build = run_profiled_build_lib("incan build --lib vocabulary companion", &producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected `build --lib` with vocab companion to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        assert_library_build_phase_keys(
            &producer_build,
            &[
                "library_codegen_emit_rust",
                "library_codegen_write_project",
                "library_codegen_sync_provider_dependencies",
                "library_oven_prepare_profiles",
                "library_oven_prepare_vocab_context",
                "library_generate_rust",
            ],
        )?;

        let manifest_path = producer_root
            .join("target")
            .join("lib")
            .join("widgets_vocab_core.incnlib");
        let manifest = LibraryManifest::read_from_path(&manifest_path)?;
        let vocab = manifest.vocab.as_ref().ok_or("expected vocab payload in .incnlib")?;
        assert_eq!(vocab.crate_path, "vocab_companion");
        assert_eq!(vocab.package_name, "widgets_vocab_companion");
        assert_eq!(vocab.keyword_registrations.len(), 1);
        assert_eq!(
            manifest.soft_keywords.activations,
            vec![incan_frontend::library_manifest::SoftKeywordActivation {
                namespace: "widgets.dsl".to_string(),
                keyword: "await".to_string(),
            }]
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn normal_oven_build_lib_with_vocab_companion_never_launches_cargo() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("guarded_widgets_vocab_project");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"guarded_widgets_core\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub def make_widget(name: str) -> str:\n  return name\n",
        )?;
        write_vocab_companion_crate(&producer_root, "vocab_companion", "guarded_widgets_vocab_companion")?;
        let cargo_marker = tmp.path().join("cargo-was-started");

        let producer_build = run_incan_with_failing_cargo_guard(
            &producer_root,
            &tmp.path().join("cargo-guard"),
            &cargo_marker,
            &["build", "--lib"],
        )?;
        assert!(
            producer_build.status.success(),
            "expected normal Oven `build --lib` with vocab companion to succeed without Cargo.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        assert!(
            !cargo_marker.exists(),
            "normal Oven vocabulary extraction launched the guarded Cargo binary"
        );

        let manifest = LibraryManifest::read_from_path(&producer_root.join("target/lib/guarded_widgets_core.incnlib"))?;
        let vocab = manifest.vocab.as_ref().ok_or("expected vocab payload in .incnlib")?;
        assert_eq!(vocab.package_name, "guarded_widgets_vocab_companion");
        assert_eq!(vocab.keyword_registrations.len(), 1);
        Ok(())
    }

    #[test]
    fn build_lib_preserves_ordinal_map_metadata_for_consumer_check() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("ordinal_keys_lib");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"ordinal_keys_core\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/status.incn"),
            r#"import std.collections as collections
from std.collections import OrdinalKey as Key, OrdinalMap, OrdinalMapError

pub enum Status(str):
    Open = "open"
    Paid = "paid"
    Cancelled = "cancelled"


@derive(Clone, Eq)
pub trait StableKey with Key:
    def stable_marker(self) -> int: ...


@derive(Clone, Eq)
pub model SmallKey with StableKey:
    pub value: int

    @staticmethod
    def ordinal_encoding() -> str:
        return "ordinal-keys-core:small-key-v1"

    @staticmethod
    def from_ordinal_bytes(data: bytes) -> Result[Self, OrdinalMapError]:
        if len(data) != 1:
            return Err(OrdinalMapError.invalid_key_record("SmallKey requires one byte"))
        return Ok(SmallKey(value=int(data[0])))

    def ordinal_bytes(self) -> bytes:
        value: u8 = self.value.wrapping_resize()
        return [value]

    def ordinal_hash(self) -> int:
        return 10_000 + self.value

    def stable_marker(self) -> int:
        return self.value


pub def echo_key[T with Key](value: T) -> T:
    return value


pub def status_map_bytes() -> bytes:
    statuses: list[Status] = [Status.Open, Status.Paid, Status.Cancelled]
    match OrdinalMap.from_keys(statuses):
        Ok(columns) => return columns.to_bytes()
        Err(_) => return b""


pub def small_key_map_bytes() -> bytes:
    alpha = SmallKey(value=1)
    beta = SmallKey(value=2)
    gamma = SmallKey(value=3)
    match OrdinalMap.from_pairs([(alpha, 10), (beta, 20), (gamma, 30)]):
        Ok(columns) => return columns.to_bytes()
        Err(_) => return b""
"#,
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub from status import SmallKey, StableKey as PublicStableKey, Status, echo_key, small_key_map_bytes, status_map_bytes\n",
        )?;

        let producer_build = bake_library_provider(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected `build --lib` to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        assert!(
            producer_root
                .join("target")
                .join("lib")
                .join("ordinal_keys_core.incnlib")
                .is_file()
        );
        let manifest = LibraryManifest::read_from_path(
            &producer_root
                .join("target")
                .join("lib")
                .join("ordinal_keys_core.incnlib"),
        )?;
        let stable_key = manifest
            .exports
            .traits
            .iter()
            .find(|trait_export| trait_export.name == "PublicStableKey")
            .ok_or("expected aliased StableKey export")?;
        assert_eq!(stable_key.source_name.as_deref(), Some("StableKey"));
        assert_eq!(stable_key.supertraits[0].name, "Key");
        assert_eq!(stable_key.supertraits[0].source_name.as_deref(), Some("OrdinalKey"));
        let status = manifest
            .exports
            .enums
            .iter()
            .find(|enum_export| enum_export.name == "Status")
            .ok_or("expected Status value enum export")?;
        assert_eq!(
            status.ordinal_type_identity.as_deref(),
            Some("ordinal_keys_core.Status")
        );

        let consumer_root = tmp.path().join("ordinal_keys_consumer");
        let consumer_name = unique_test_project_name("ordinal_keys_consumer");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::write(
            consumer_root.join("loaf.toml"),
            format!(
                "[project]\nname = \"{consumer_name}\"\n\n[dependencies]\nordinal_keys = {{ path = \"../ordinal_keys_lib\" }}\n"
            ),
        )?;
        let consumer_main = consumer_root.join("src/main.incn");
        std::fs::write(
            &consumer_main,
            "from std.collections import OrdinalMap, OrdinalMapError\nfrom pub::ordinal_keys import SmallKey, Status, echo_key, small_key_map_bytes, status_map_bytes\n\ndef run() -> Result[None, OrdinalMapError]:\n  probe = echo_key(\"probe\")\n  if len(probe) == 0:\n    print(probe)\n  status_map: OrdinalMap[Status] = OrdinalMap.from_bytes(status_map_bytes())?\n  small_key_map: OrdinalMap[SmallKey] = OrdinalMap.from_bytes(small_key_map_bytes())?\n  print(status_map.require(Status.Paid)?)\n  print(small_key_map.require(SmallKey(value=2))?)\n  return Ok(None)\n\ndef main() -> None:\n  match run():\n    Ok(_) => pass\n    Err(err) => print(err.message())\n",
        )?;

        let consumer_check = run_check(&consumer_main)?;
        assert!(
            consumer_check.status.success(),
            "expected consumer check to accept imported OrdinalMap metadata.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_check.stdout),
            String::from_utf8_lossy(&consumer_check.stderr)
        );
        Ok(())
    }

    #[test]
    fn build_lib_preserves_std_environ_typed_reads_for_consumers() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("environ_types_provider");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"environ_types_core\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/types.incn"),
            r#"from std.traits.convert import TryFrom

pub model EnvToken with TryFrom[str]:
  pub value: str

  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    if len(value) == 0:
      return Err("token must not be empty")
    return Ok(EnvToken(value=value))

pub model MultiToken with TryFrom[str], TryFrom[int]:
  pub value: str

  @classmethod
  def try_from(cls, value: str) for TryFrom[str] -> Result[Self, str]:
    if len(value) == 0:
      return Err("token must not be empty")
    return Ok(MultiToken(value=value))

  @classmethod
  def try_from(cls, value: int) for TryFrom[int] -> Result[Self, str]:
    return Ok(MultiToken(value=f"{value}"))

pub trait EnvReadable[T] with TryFrom[T]:
  def source_name(self) -> str: ...

pub model TraitToken with EnvReadable[str]:
  pub value: str

  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    return Ok(TraitToken(value=value))

  def source_name(self) -> str:
    return "environment"

pub class EnvClass with TryFrom[str]:
  pub value: str

  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    return Ok(EnvClass(value=value))

pub enum EnvMode with TryFrom[str]:
  Dev
  Prod

  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    if value == "prod":
      return Ok(EnvMode.Prod)
    return Ok(EnvMode.Dev)

pub type ExplicitLabel = newtype str with TryFrom[str]:
  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    return Ok(ExplicitLabel(value))

pub type Port = newtype int:
  def from_underlying(value: int) -> Result[Self, ValidationError]:
    if value < 1 or value > 65535:
      return Err(ValidationError("port out of range"))
    return Ok(Port(value))

pub type WrappedPort = newtype Port
pub type Positive = newtype int[gt=0]
pub type PositiveBox = newtype Positive
pub type Ratio = newtype float[ge=0, le=1]
pub type Boxed[T] = newtype T
pub type ClonedBox[T with Clone] = newtype T

@no_implicit_coercion
pub type StrictPort = newtype int
"#,
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub from types import Boxed as PublicBoxed, ClonedBox, EnvClass, EnvMode, EnvReadable, EnvToken, ExplicitLabel, MultiToken, Port, Port as PublicPort, Positive as PublicPositive, PositiveBox as PublicPositiveBox, Ratio, StrictPort, TraitToken, WrappedPort\n",
        )?;

        let producer_build = bake_library_provider(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected std.environ type provider to build.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );

        let provider_manifest = LibraryManifest::read_from_path(
            &producer_root
                .join("target")
                .join("lib")
                .join("environ_types_core.incnlib"),
        )?;
        let positive = provider_manifest
            .exports
            .newtypes
            .iter()
            .find(|newtype| newtype.name == "PublicPositive")
            .ok_or("missing aliased constrained newtype export")?;
        assert_eq!(positive.constraints.len(), 1);
        assert_eq!(positive.constraints[0].value, 0);
        let positive_box = provider_manifest
            .exports
            .newtypes
            .iter()
            .find(|newtype| newtype.name == "PublicPositiveBox")
            .ok_or("missing aliased composed newtype export")?;
        assert_eq!(
            positive_box.underlying,
            TypeRef::Named {
                origin: None,
                name: "PublicPositive".to_string()
            }
        );
        let boxed = provider_manifest
            .exports
            .newtypes
            .iter()
            .find(|newtype| newtype.name == "PublicBoxed")
            .ok_or("missing aliased generic newtype export")?;
        assert_eq!(boxed.type_params.len(), 1);
        let strict = provider_manifest
            .exports
            .newtypes
            .iter()
            .find(|newtype| newtype.name == "StrictPort")
            .ok_or("missing no-implicit-coercion newtype export")?;
        assert!(!strict.implicit_coercion_enabled);

        let consumer_root = tmp.path().join("environ_types_consumer");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::write(
            consumer_root.join("loaf.toml"),
            "[project]\nname = \"environ_types_consumer\"\n\n[dependencies]\nenviron_types = { path = \"../environ_types_provider\" }\n",
        )?;
        let consumer_main = consumer_root.join("src/main.incn");
        std::fs::write(
            &consumer_main,
            r#"from std.environ import EnvironError, get_as
from std.traits.convert import TryFrom
from pub::environ_types import ClonedBox, EnvClass, EnvMode, EnvToken, ExplicitLabel, MultiToken, Port, PublicBoxed, PublicPort, PublicPositive, PublicPositiveBox, Ratio, StrictPort, TraitToken, WrappedPort

def require[T with TryFrom[str]](key: str) -> Result[T, EnvironError]:
  match get_as[T](key)?:
    Some(value) => return Ok(value)
    None => return Err(EnvironError.missing(key))

def mode_name(mode: EnvMode) -> str:
  match mode:
    EnvMode.Dev => return "Dev"
    EnvMode.Prod => return "Prod"

def read_values() -> Result[None, EnvironError]:
  token = require[EnvToken]("INCAN_PACKAGE_ENV_TOKEN")?
  trait_token = require[TraitToken]("INCAN_PACKAGE_ENV_TRAIT_TOKEN")?
  env_class = require[EnvClass]("INCAN_PACKAGE_ENV_CLASS")?
  env_mode = require[EnvMode]("INCAN_PACKAGE_ENV_MODE")?
  explicit_label = require[ExplicitLabel]("INCAN_PACKAGE_ENV_EXPLICIT_LABEL")?
  port = require[Port]("INCAN_PACKAGE_ENV_PORT")?
  public_port = require[PublicPort]("INCAN_PACKAGE_ENV_PUBLIC_PORT")?
  wrapped = require[WrappedPort]("INCAN_PACKAGE_ENV_WRAPPED")?
  multi = require[MultiToken]("INCAN_PACKAGE_ENV_MULTI")?
  positive = require[PublicPositive]("INCAN_PACKAGE_ENV_POSITIVE")?
  positive_box = require[PublicPositiveBox]("INCAN_PACKAGE_ENV_POSITIVE_BOX")?
  boxed = require[PublicBoxed[int]]("INCAN_PACKAGE_ENV_BOXED")?
  cloned = require[ClonedBox[str]]("INCAN_PACKAGE_ENV_CLONED")?
  ratio = require[Ratio]("INCAN_PACKAGE_ENV_RATIO")?
  strict = require[StrictPort]("INCAN_PACKAGE_ENV_STRICT")?
  unwrapped = wrapped.0
  positive_inner = positive_box.0
  defaulted = get_as[Port]("INCAN_PACKAGE_ENV_MISSING", default=8080)?
  println(f"{token.value}:{trait_token.value}:{env_class.value}:{mode_name(env_mode)}:{explicit_label.0}:{multi.value}:{port.0}:{public_port.0}:{unwrapped.0}:{positive.0}:{positive_inner.0}:{boxed.0}:{cloned.0}:{ratio.0}:{strict.0}:{defaulted.0}")
  return Ok(None)

def main() -> None:
  match read_values():
    Ok(_) => pass
    Err(error) => println(f"unexpected:{error.kind_name()}")
  match get_as[Port]("INCAN_PACKAGE_ENV_BAD_PORT"):
    Ok(_) => println("invalid:unexpected")
    Err(error) => println(f"invalid:{error.kind_name()}")
  match get_as[PublicPositive]("INCAN_PACKAGE_ENV_NON_POSITIVE"):
    Ok(_) => println("constrained:unexpected")
    Err(error) => println(f"constrained:{error.kind_name()}")
  match get_as[Ratio]("INCAN_PACKAGE_ENV_RATIO_HIGH"):
    Ok(_) => println("ratio-high:unexpected")
    Err(error) => println(f"ratio-high:{error.kind_name()}")
"#,
        )?;

        let consumer_bake = bake_project(&consumer_root)?;
        assert!(
            consumer_bake.status.success(),
            "expected an explicit Oven bake to import the typed-environment package closure.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );
        let consumer_run = super::incan_command()
            .args(["run", consumer_main.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_PACKAGE_ENV_TOKEN", "secret-token")
            .env("INCAN_PACKAGE_ENV_TRAIT_TOKEN", "trait-token")
            .env("INCAN_PACKAGE_ENV_CLASS", "class-token")
            .env("INCAN_PACKAGE_ENV_MODE", "prod")
            .env("INCAN_PACKAGE_ENV_EXPLICIT_LABEL", "explicit")
            .env("INCAN_PACKAGE_ENV_PORT", "5432")
            .env("INCAN_PACKAGE_ENV_PUBLIC_PORT", "5433")
            .env("INCAN_PACKAGE_ENV_WRAPPED", "6543")
            .env("INCAN_PACKAGE_ENV_MULTI", "multi-token")
            .env("INCAN_PACKAGE_ENV_POSITIVE", "12")
            .env("INCAN_PACKAGE_ENV_POSITIVE_BOX", "13")
            .env("INCAN_PACKAGE_ENV_BOXED", "88")
            .env("INCAN_PACKAGE_ENV_CLONED", "cloned")
            .env("INCAN_PACKAGE_ENV_RATIO", "0.25")
            .env("INCAN_PACKAGE_ENV_STRICT", "9090")
            .env("INCAN_PACKAGE_ENV_BAD_PORT", "70000")
            .env("INCAN_PACKAGE_ENV_NON_POSITIVE", "0")
            .env("INCAN_PACKAGE_ENV_RATIO_HIGH", "1.5")
            .env_remove("INCAN_PACKAGE_ENV_MISSING")
            .output()?;
        assert!(
            consumer_run.status.success(),
            "expected package typed environment reads to run.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_run.stdout),
            String::from_utf8_lossy(&consumer_run.stderr)
        );
        assert_eq!(
            String::from_utf8(consumer_run.stdout)?,
            concat!(
                "secret-token:trait-token:class-token:Prod:explicit:multi-token:5432:5433:6543:12:13:88:cloned:0.25:9090:8080\n",
                "invalid:invalid_value\n",
                "constrained:invalid_value\n",
                "ratio-high:invalid_value\n",
            )
        );

        let strict_default = consumer_root.join("src/strict_default.incn");
        std::fs::write(
            &strict_default,
            r#"from std.environ import get_as
from pub::environ_types import StrictPort

def main() -> None:
  get_as[StrictPort]("INCAN_PACKAGE_ENV_STRICT_DEFAULT", 8080)
"#,
        )?;
        let strict_check = run_check(&strict_default)?;
        assert!(
            !strict_check.status.success(),
            "expected package no-implicit-coercion policy to reject underlying default"
        );
        assert!(
            String::from_utf8_lossy(&strict_check.stderr)
                .contains("Implicit coercion into newtype 'StrictPort' is disabled"),
            "expected package coercion diagnostic, got:\n{}",
            String::from_utf8_lossy(&strict_check.stderr)
        );
        Ok(())
    }

    #[test]
    fn std_environ_typed_reads_survive_facades_and_test_batches() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let project_name = unique_test_project_name("environ_facade_test_batch");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::create_dir_all(project_root.join("tests"))?;
        std::fs::write(
            project_root.join("loaf.toml"),
            format!("[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n"),
        )?;
        std::fs::write(
            project_root.join("src/env_types.incn"),
            r#"pub type Port = newtype int:
  def from_underlying(value: int) -> Result[Self, ValidationError]:
    if value < 1 or value > 65535:
      return Err(ValidationError("port out of range"))
    return Ok(Port(value))
"#,
        )?;
        std::fs::write(
            project_root.join("src/environ_facade.incn"),
            "pub from env_types import Port\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            r#"from environ_facade import Port
from std.environ import get_as

def main() -> None:
  match get_as[Port]("INCAN_ENVIRON_FACADE_MISSING", default=8080):
    Ok(port) => println(port.0)
    Err(error) => println(error.kind_name())
"#,
        )?;
        std::fs::write(
            project_root.join("tests/test_environ.incn"),
            r#"from environ_facade import Port
from std.environ import get_as
from std.testing import assert_eq, fail

def test_defaulted_port_through_facade() -> None:
  match get_as[Port]("INCAN_ENVIRON_TEST_BATCH_MISSING", 9090):
    Ok(port) => assert_eq(port.0, 9090)
    Err(error) => fail(f"expected defaulted Port, got {error.kind_name()}")
  match get_as[Port]("INCAN_ENVIRON_TEST_BATCH_INVALID"):
    Ok(_) => fail("expected invalid facade Port to fail validation")
    Err(error) => assert_eq(error.kind_name(), "invalid_value")
"#,
        )?;

        let run_output = super::incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .env_remove("INCAN_ENVIRON_FACADE_MISSING")
            .output()?;
        assert!(
            run_output.status.success(),
            "expected facade typed environment read to run.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&run_output.stdout),
            String::from_utf8_lossy(&run_output.stderr)
        );
        assert_eq!(String::from_utf8(run_output.stdout)?, "8080\n");

        let test_output = super::incan_command()
            .args(["test", project_root.join("tests").to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_TEST_SHARED_TARGET_DIR", shared_test_runner_target_dir())
            .env("INCAN_ENVIRON_TEST_BATCH_INVALID", "70000")
            .env_remove("INCAN_ENVIRON_TEST_BATCH_MISSING")
            .output()?;
        assert!(
            test_output.status.success(),
            "expected test batch typed environment read to pass.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&test_output.stdout),
            String::from_utf8_lossy(&test_output.stderr)
        );
        Ok(())
    }

    #[test]
    fn std_environ_module_qualified_overloads_run() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let project_name = unique_test_project_name("environ_module_qualified");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("loaf.toml"),
            format!("[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n"),
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            r#"import std.environ as environ
from std.environ import EnvironError


def read_values() -> Result[None, EnvironError]:
  present = environ.get_as[int]("INCAN_ENVIRON_QUALIFIED_PRESENT")?.unwrap_or(0)
  fallback = environ.get_as[int]("INCAN_ENVIRON_QUALIFIED_MISSING", default=8080)?
  println(f"{present}:{fallback}")
  return Ok(None)


def main() -> None:
  match read_values():
    Ok(_) => pass
    Err(error) => println(error.kind_name())
"#,
        )?;

        let output = super::incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_ENVIRON_QUALIFIED_PRESENT", "42")
            .env_remove("INCAN_ENVIRON_QUALIFIED_MISSING")
            .output()?;
        assert!(
            output.status.success(),
            "expected module-qualified std.environ overloads to run.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout)?, "42:8080\n");
        Ok(())
    }

    #[test]
    fn std_environ_overloads_run_through_source_facade() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let project_name = unique_test_project_name("environ_function_facade");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("loaf.toml"),
            format!("[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n"),
        )?;
        std::fs::write(
            project_root.join("src/environ_facade.incn"),
            "pub from std.environ import EnvironError, get_as\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            r#"from environ_facade import EnvironError, get_as


def read_values() -> Result[None, EnvironError]:
  present = get_as[int]("INCAN_ENVIRON_FACADE_PRESENT")?.unwrap_or(0)
  fallback = get_as[int]("INCAN_ENVIRON_FACADE_MISSING", 9090)?
  println(f"{present}:{fallback}")
  return Ok(None)


def main() -> None:
  match read_values():
    Ok(_) => pass
    Err(error) => println(error.kind_name())
"#,
        )?;

        let output = super::incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_ENVIRON_FACADE_PRESENT", "73")
            .env_remove("INCAN_ENVIRON_FACADE_MISSING")
            .output()?;
        assert!(
            output.status.success(),
            "expected std.environ overloads re-exported by a source facade to run.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout)?, "73:9090\n");
        Ok(())
    }

    #[test]
    fn check_pub_boundary_preserves_consumer_type_fidelity_cases() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_pub_boundary_type_fidelity_library(tmp.path())?;

        let cases = [
            (
                "question_mark_result",
                "`lazy.collect()?` across pub boundary",
                r#"from pub::pubdemo import LazyFrame, SessionError

model Row:
  value: int

def main() -> Result[None, SessionError]:
  lazy = LazyFrame[Row](_type_witness=[])
  df = lazy.collect()?
  print(df.to_substrait_plan())
  return Ok(None)
"#,
            ),
            (
                "derived_method_chain",
                "`lazy.clone().collect()?` across pub boundary",
                r#"from pub::pubdemo import LazyFrame, SessionError

model Row:
  value: int

def main() -> Result[None, SessionError]:
  lazy = LazyFrame[Row](_type_witness=[])
  df = lazy.clone().collect()?
  print(df.to_substrait_plan())
  return Ok(None)
"#,
            ),
            (
                "trait_supertype",
                "`DataFrame[T]` satisfying `DataSet[T]` across pub boundary",
                r#"from pub::pubdemo import DataFrame, SessionError, display

model Row:
  value: int

def main() -> Result[None, SessionError]:
  df = DataFrame[Row](_type_witness=[])
  display(df)
  return Ok(None)
"#,
            ),
        ];

        for (name, description, source) in cases {
            let case_root = tmp.path().join(name);
            let main_path = write_project_files(
                &case_root,
                "[project]\nname = \"consumer\"\n\n[dependencies]\npubdemo = { path = \"../pub_boundary_library\" }\n",
                source,
            )?;

            let output = run_check(&main_path)?;
            assert!(
                output.status.success(),
                "expected {description} to typecheck.\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    #[test]
    fn build_lib_fails_early_for_invalid_helper_binding_manifest() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("invalid_helper_vocab_project");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"widgets_core\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub def make_widget(name: str) -> str:\n  return name\n",
        )?;
        write_vocab_companion_crate_with_source(
            &producer_root,
            "vocab_companion",
            "widgets_vocab_companion",
            "use incan_vocab::{HelperBinding, LibraryManifest, VocabRegistration};\n\npub fn library_vocab() -> VocabRegistration {\n    VocabRegistration::new().with_library_manifest(LibraryManifest {\n        helper_bindings: vec![HelperBinding {\n            key: \"filter\".to_string(),\n            exported_name: \"filter\".to_string(),\n        }],\n        ..LibraryManifest::default()\n    })\n}\n",
        )?;

        let producer_build = bake_library_provider(&producer_root)?;
        assert!(
            !producer_build.status.success(),
            "expected `build --lib` to fail for invalid helper binding.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&producer_build.stderr));
        assert!(
            stderr.contains("unknown exported symbol `filter`"),
            "expected helper-binding validation failure, got:\n{stderr}"
        );
        Ok(())
    }

    #[test]
    fn consumer_check_uses_serialized_vocab_metadata_for_keyword_activation() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("widgets_assert_vocab_project");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"widgets_assert_core\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub def make_widget(name: str) -> str:\n  return name\n",
        )?;
        write_vocab_companion_crate_with_assert_keyword(&producer_root, "vocab_companion", "widgets_vocab_companion")?;

        let producer_build = run_build_lib(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected `build --lib` with assert vocab companion to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );

        let consumer_root = tmp.path().join("consumer_with_vocab_keyword");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::write(
            consumer_root.join("loaf.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nwidgets = { path = \"../widgets_assert_vocab_project\" }\n",
        )?;
        let consumer_main = consumer_root.join("src/main.incn");
        std::fs::write(
            &consumer_main,
            "import pub::widgets\n\ndef main() -> None:\n  assert true\n",
        )?;

        let check_output = run_check(&consumer_main)?;
        assert!(
            check_output.status.success(),
            "expected consumer check to parse/typecheck assert keyword from serialized vocab metadata.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&check_output.stdout),
            String::from_utf8_lossy(&check_output.stderr)
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_activates_standard_checked_c_vocab() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let fixture_header = tmp.path().join("fixture.h");
        std::fs::write(
            &fixture_header,
            "typedef struct fixture_pair { int left; int right; } fixture_pair;\n#define FIXTURE_OK 0\nint abs(int value);\n",
        )?;
        let source = format!(
            "from std.interop import c\n\nbinding Fixture:\n    header = \"{}\"\n    link = c.system_library(\"c\")\n\n    symbol absolute(value: c.i32) -> c.i32:\n        native = \"abs\"\n\n    enum Status:\n        OK: c.i32 = FIXTURE_OK\n\n    struct Pair:\n        native = \"fixture_pair\"\n        left: c.i32 = left\n        right: c.i32 = right\n\ndef absolute(value: int) -> int:\n    unsafe:\n        return Fixture.absolute(value)\n\ndef main() -> None:\n    assert Fixture.Status.OK == 0\n    assert absolute(-7) == 7\n",
            fixture_header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"standard_interop_vocab_consumer\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");

        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            output.status.success(),
            "expected the standard C binding vocabulary to lower and typecheck.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let checkout = support::repo_root();
        let run_output = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["run", main_path.to_string_lossy().as_ref()])
            .output()?;
        assert!(
            run_output.status.success(),
            "expected a checked C call to compile and run.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&run_output.stdout),
            String::from_utf8_lossy(&run_output.stderr)
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_resolves_a_manifest_declared_package_relative_c_header() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let header = tmp.path().join("interop/include/fixture.h");
        std::fs::create_dir_all(header.parent().ok_or("fixture header has no parent")?)?;
        std::fs::write(&header, "int fixture_abs(int value);\n")?;
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"package_relative_c_header\"\n\n[sdk]\nprofile = \"minimal\"\n\n[interop.c]\nschema = 1\n\n[[interop.c.targets]]\ntarget = \"aarch64-apple-darwin\"\nheaders = [\"interop/include/fixture.h\"]\n",
            "from std.interop import c\n\nbinding Fixture:\n    header = \"interop/include/fixture.h\"\n    link = c.system_library(\"c\")\n\n    symbol absolute(value: c.i32) -> c.i32:\n        native = \"fixture_abs\"\n\ndef main() -> None:\n    pass\n",
        )?;
        let output = run_check_against_checkout_sdk(&main_path, &tmp.path().join("generated-cargo-target"))?;
        assert!(
            output.status.success(),
            "expected a package-relative checked C header to resolve through [interop.c].\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_verifies_checked_c_resource_and_output_contracts() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let fixture_header = tmp.path().join("fixture.h");
        std::fs::write(
            &fixture_header,
            "typedef unsigned long size_t;\nvoid free(void *);\nint posix_memalign(void **, size_t, size_t);\nunsigned int rand_r(unsigned int *);\n#define FIXTURE_OK 0\n",
        )?;
        let source = format!(
            "from std.interop import c\n\nbinding Fixture:\n    header = \"{}\"\n    link = c.system_library(\"c\")\n\n    resource Memory:\n        native = \"void\"\n        release = close\n\n    symbol close(handle: c.Owned[Memory]) -> None:\n        native = \"free\"\n\n    enum Status:\n        OK: c.i32 = FIXTURE_OK\n\n    symbol open(output: c.Out[c.Owned[Memory]], alignment: c.Size, size: c.Size) -> c.i32:\n        native = \"posix_memalign\"\n\n        outcome Status.OK:\n            initializes = [output]\n\n    symbol random(seed: c.InOut[c.u32]) -> c.u32:\n        native = \"rand_r\"\n\ndef main() -> None:\n    unsafe:\n        handle = c.out[c.Owned[Memory]]()\n        status = Fixture.open(handle, 8, 8)\n        if status == Fixture.Status.OK:\n            resource = handle.take()\n            Fixture.close(resource)\n\n        seed_value: u32 = 7\n        seed = c.inout(seed_value)\n        Fixture.random(seed)\n        seed.take()\n",
            fixture_header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"checked_c_resource_contracts\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");

        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            output.status.success(),
            "expected checked C resource and output contracts to lower and verify.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let checkout = support::repo_root();
        let run_output = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["run", main_path.to_string_lossy().as_ref()])
            .output()?;
        assert!(
            run_output.status.success(),
            "expected checked C resources and output slots to compile, run, and release.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&run_output.stdout),
            String::from_utf8_lossy(&run_output.stderr)
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_run_passes_checked_c_string_to_a_pointer_parameter() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let fixture_header = tmp.path().join("checked_c_string_fixture.h");
        std::fs::write(
            &fixture_header,
            "typedef unsigned long size_t;\nsize_t strlen(const char *value);\n",
        )?;
        let source = format!(
            r#"from std.interop import c

binding LibC:
    header = "{}"
    link = c.system_library("c")

    symbol string_length(value: c.ConstPtr[c.c_char]) -> c.Size:
        native = "strlen"

def checked_length(value: str) -> Result[usize, str]:
    text = c.cstr(value)?
    unsafe:
        return Ok(LibC.string_length(text.as_const_ptr()))

def main() -> Result[None, str]:
    assert checked_length("incan")? == 5
    match c.cstr("not{nul}allowed"):
        Err(_) => return Ok(None)
        Ok(_) => return Err("expected c.cstr to reject an interior terminator")
"#,
            fixture_header.display(),
            nul = '\0',
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"checked_c_string\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");

        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            output.status.success(),
            "expected a checked C string to typecheck.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let checkout = support::repo_root();
        let run_output = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["run", main_path.to_string_lossy().as_ref()])
            .output()?;
        assert!(
            run_output.status.success(),
            "expected a checked C string to compile and run.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&run_output.stdout),
            String::from_utf8_lossy(&run_output.stderr)
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_hides_owned_c_resources_behind_an_incan_facade() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let fixture_header = tmp.path().join("fixture.h");
        std::fs::write(
            &fixture_header,
            r#"typedef struct fixture_file FILE;
int fclose(FILE *);
FILE *tmpfile(void);
int fflush(FILE *);
int fileno(FILE *);
int close(int);
"#,
        )?;
        let source = format!(
            r#"from std.interop import c

binding CFile:
    header = "{}"
    link = c.system_library("c")

    resource File:
        native = "FILE"
        release = close

    symbol close(file: c.Owned[File]) -> c.i32:
        native = "fclose"

    symbol open() -> Option[c.Owned[File]]:
        native = "tmpfile"

    symbol flush(file: c.BorrowedMut[File]) -> c.i32:
        native = "fflush"

    symbol descriptor(file: c.Borrowed[File]) -> c.i32:
        native = "fileno"

    symbol close_descriptor(descriptor: c.i32) -> c.i32:
        native = "close"

def temporary_descriptor() -> int:
    unsafe:
        if let Some(open_file) = CFile.open():
            mut file = open_file
            if CFile.flush(file) != 0:
                return -1
            return CFile.descriptor(file)
        return -1

def temporary_file_is_released() -> bool:
    descriptor = temporary_descriptor()
    if descriptor < 0:
        return False
    unsafe:
        return CFile.close_descriptor(descriptor) == -1

def main() -> None:
    assert temporary_file_is_released()
"#,
            fixture_header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"checked_c_resource_facade\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");

        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            output.status.success(),
            "expected a same-module Incan façade to hide an owned C resource.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let checkout = support::repo_root();
        let run_output = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["run", main_path.to_string_lossy().as_ref()])
            .output()?;
        assert!(
            run_output.status.success(),
            concat!(
                "expected the Incan façade to borrow, flush, and release its encapsulated C resource.\n",
                "stdout:\n{}\nstderr:\n{}",
            ),
            String::from_utf8_lossy(&run_output.stdout),
            String::from_utf8_lossy(&run_output.stderr)
        );
        Ok(())
    }

    fn sqlite_checked_c_source(sqlite_header: &Path) -> String {
        format!(
            r#"from std.interop import c

binding SQLite:
    header = "{}"
    link = c.system_library("sqlite3")

    resource Database:
        native = "sqlite3"
        release = close

    enum Status:
        OK: c.i32 = SQLITE_OK

    symbol close(database: c.Owned[Database]) -> c.i32:
        native = "sqlite3_close"

    symbol open(path: c.ConstPtr[c.c_char], database: c.Out[c.Owned[Database]]) -> c.i32:
        native = "sqlite3_open"

        outcome Status.OK:
            initializes = [database]

    symbol error_message(database: c.Borrowed[Database]) -> c.ConstPtr[c.c_char]:
        native = "sqlite3_errmsg"

def memory_database_error_is_owned_text() -> Result[str, str]:
    path = c.cstr(":memory:")?
    unsafe:
        database_slot = c.out[c.Owned[Database]]()
        status = SQLite.open(path.as_const_ptr(), database_slot)
        if status == SQLite.Status.OK:
            database = database_slot.take()
            view = SQLite.error_message(database)
            text = view.copy_utf8(max_bytes=4096)?
            SQLite.close(database)
            return Ok(text)
        return Err("sqlite open failed")

def main() -> Result[None, str]:
    assert memory_database_error_is_owned_text()? != ""
"#,
            sqlite_header.display()
        )
    }

    fn sqlite_header_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
        [
            "/usr/include/sqlite3.h",
            "/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk/usr/include/sqlite3.h",
            "/opt/homebrew/include/sqlite3.h",
            "/usr/local/include/sqlite3.h",
        ]
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .ok_or_else(|| "SQLite acceptance requires a system sqlite3.h header".into())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_copies_a_sqlite_error_view_with_an_explicit_bound() -> Result<(), Box<dyn std::error::Error>> {
        let sqlite_header = sqlite_header_path()?;
        let tmp = tempfile::tempdir()?;
        let source = sqlite_checked_c_source(&sqlite_header);
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"sqlite_checked_c_facade\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");
        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            output.status.success(),
            "expected SQLite to use an owned output handle, a borrowed error view, and bounded copied text.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let checkout = support::repo_root();
        let emitted = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["--emit-rust", main_path.to_string_lossy().as_ref(), "--strict"])
            .output()?;
        assert!(
            emitted.status.success(),
            "expected the bounded SQLite view bridge to lower and emit strict Rust.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&emitted.stdout),
            String::from_utf8_lossy(&emitted.stderr)
        );
        let generated = String::from_utf8_lossy(&emitted.stdout);
        assert!(
            generated.contains("__incan_checked_c_copy_utf8"),
            "expected the generated SQLite façade to use the compiler-private bounded copy helper:\n{generated}"
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_models_a_sqlite_caller_owned_byte_buffer_through_a_declared_shim()
    -> Result<(), Box<dyn std::error::Error>> {
        let sqlite_header = sqlite_header_path()?;
        let tmp = tempfile::tempdir()?;
        let shim_header = tmp.path().join("sqlite_span_bridge.h");
        std::fs::write(
            &shim_header,
            format!(
                "#include \"{}\"\n#include <stddef.h>\n#include <stdint.h>\nsize_t incan_sqlite_random_bytes(uint8_t *destination, size_t destination_capacity);\n",
                sqlite_header.display()
            ),
        )?;
        let source = format!(
            r#"from std.interop import c

binding SQLiteBuffer:
    header = "{}"
    link = c.system_library("sqlite3")

    symbol random_bytes(destination: c.MutPtr[c.u8], destination_capacity: c.Size) -> c.Size:
        native = "incan_sqlite_random_bytes"
        bounds = {{ destination: destination_capacity }}

def random_bytes() -> Result[bytes, str]:
    mut destination = c.mutable_bytes_span(b"\0\0\0\0\0\0\0\0")
    unsafe:
        written = SQLiteBuffer.random_bytes(destination.as_mut_ptr(), destination.byte_capacity())
        return destination.into_bytes(written)
"#,
            shim_header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"sqlite_checked_c_span\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");

        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            output.status.success(),
            "expected the SQLite shim boundary to retain one checked byte buffer contract.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let checkout = support::repo_root();
        let emitted = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["--emit-rust", main_path.to_string_lossy().as_ref(), "--strict"])
            .output()?;
        assert!(
            emitted.status.success(),
            "expected the SQLite caller-owned buffer bridge to emit strict Rust.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&emitted.stdout),
            String::from_utf8_lossy(&emitted.stderr)
        );
        let generated = String::from_utf8_lossy(&emitted.stdout);
        assert!(
            generated.contains("__incan_checked_c_finish_span")
                && generated.contains("*mut u8")
                && generated.contains("-> usize"),
            "expected a bounded byte buffer and exact c.Size carrier, got:\n{generated}"
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_models_an_accelerate_f32_span_through_a_declared_framework_shim()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let shim_header = tmp.path().join("accelerate_span_bridge.h");
        std::fs::write(
            &shim_header,
            "#include <stddef.h>\n#define INCAN_ACCELERATE_OK 0\nint incan_accelerate_sum_f32(const float *values, size_t value_count, float *output);\n",
        )?;
        let source = format!(
            r#"from std.interop import c

binding Accelerate:
    header = "{}"
    link = c.framework("Accelerate")

    enum Status:
        OK: c.i32 = INCAN_ACCELERATE_OK

    symbol sum(values: c.ConstPtr[c.f32], value_count: c.Size, output: c.Out[c.f32]) -> c.i32:
        native = "incan_accelerate_sum_f32"
        bounds = {{ values: value_count }}

        outcome Status.OK:
            initializes = [output]

def sum(values: list[f32]) -> Result[f32, str]:
    source = c.f32_span(values)
    unsafe:
        output = c.out[c.f32]()
        status = Accelerate.sum(source.as_const_ptr(), source.element_count(), output)
        if status == Accelerate.Status.OK:
            return Ok(output.take())
        return Err("Accelerate sum failed")
"#,
            shim_header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"accelerate_checked_c_span\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");

        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            output.status.success(),
            "expected the Accelerate-shaped shim boundary to retain one checked f32 span and output contract.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let checkout = support::repo_root();
        let emitted = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["--emit-rust", main_path.to_string_lossy().as_ref(), "--strict"])
            .output()?;
        assert!(
            emitted.status.success(),
            "expected the Accelerate f32 span bridge to emit strict Rust.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&emitted.stdout),
            String::from_utf8_lossy(&emitted.stderr)
        );
        let generated = String::from_utf8_lossy(&emitted.stdout);
        assert!(
            generated.contains("*const f32")
                && generated.contains("fn from_incan_value(value: f32)")
                && generated.contains("fn take(self) -> f32")
                && generated.contains("#[link(name = \"Accelerate\", kind = \"framework\")]")
                && !generated.contains("i64::try_from(value)"),
            "expected an exact f32 span/output carrier and source-selected framework link without i64 normalization, got:\n{generated}"
        );

        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_supports_checked_byte_spans_and_caller_owned_buffers() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let header = tmp.path().join("span_fixture.h");
        std::fs::write(
            &header,
            "#include <stddef.h>\n#include <stdint.h>\nsize_t fixture_copy_prefix(const uint8_t *source, size_t source_length, uint8_t *destination, size_t destination_capacity);\n",
        )?;
        let source = format!(
            r#"from std.interop import c

binding Fixture:
    header = "{}"
    link = c.system_library("c")

    symbol copy_prefix(
        source: c.ConstPtr[c.u8],
        source_length: c.Size,
        destination: c.MutPtr[c.u8],
        destination_capacity: c.Size,
    ) -> c.Size:
        native = "fixture_copy_prefix"
        bounds = {{
            source: source_length,
            destination: destination_capacity,
        }}

def copy_bounded(data: bytes) -> Result[bytes, str]:
    source = c.bytes_span(data)
    mut destination = c.mutable_bytes_span(b"\0\0\0\0")
    unsafe:
        written = Fixture.copy_prefix(
            source.as_const_ptr(),
            source.byte_length(),
            destination.as_mut_ptr(),
            destination.byte_capacity(),
        )
        return Ok(destination.into_bytes(written)?)

def main() -> Result[None, str]:
    assert copy_bounded(b"abc")? == b"abc"
    return Ok(None)
"#,
            header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"checked_c_spans\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");

        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            output.status.success(),
            "expected bounded byte spans and a caller-owned output buffer to typecheck.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let checkout = support::repo_root();
        let emitted = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["--emit-rust", main_path.to_string_lossy().as_ref(), "--strict"])
            .output()?;
        assert!(
            emitted.status.success(),
            "expected bounded byte spans and caller-owned buffer finishing to emit strict Rust.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&emitted.stdout),
            String::from_utf8_lossy(&emitted.stderr)
        );
        let generated = String::from_utf8_lossy(&emitted.stdout);
        assert!(
            generated.contains("__incan_checked_c_finish_span"),
            "expected generated Rust to validate the returned count before returning caller-owned storage:\n{generated}"
        );
        assert!(
            generated.contains(".as_mut_ptr()") && generated.contains(".as_ptr()"),
            "expected generated Rust to extract only the checked pointer forms:\n{generated}"
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_rejects_unpaired_or_immutable_checked_byte_buffer_arguments()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let header = tmp.path().join("span_fixture.h");
        std::fs::write(
            &header,
            "#include <stddef.h>\n#include <stdint.h>\nsize_t fixture_copy_prefix(const uint8_t *source, size_t source_length, uint8_t *destination, size_t destination_capacity);\n",
        )?;
        let source = format!(
            r#"from std.interop import c

binding Fixture:
    header = "{}"
    link = c.system_library("c")

    symbol copy_prefix(
        source: c.ConstPtr[c.u8],
        source_length: c.Size,
        destination: c.MutPtr[c.u8],
        destination_capacity: c.Size,
    ) -> c.Size:
        native = "fixture_copy_prefix"
        bounds = {{
            source: source_length,
            destination: destination_capacity,
        }}

def reject_unchecked_pair(data: bytes) -> None:
    source = c.bytes_span(data)
    other = c.bytes_span(b"not the same owner")
    destination = c.mutable_bytes_span(b"\0\0\0\0")
    unsafe:
        Fixture.copy_prefix(
            source.as_const_ptr(),
            other.byte_length(),
            destination.as_mut_ptr(),
            destination.byte_capacity(),
        )
"#,
            header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"checked_c_span_rejections\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");
        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            !output.status.success(),
            "expected invalid checked byte-span source to be rejected.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let diagnostics = String::from_utf8_lossy(&output.stderr);
        assert!(
            diagnostics.contains("to come from the same checked immutable byte span"),
            "expected the declared pointer/length relationship diagnostic, got:\n{diagnostics}"
        );
        assert!(
            diagnostics.contains("requires a mutable borrow of 'destination'"),
            "expected an immutable caller-owned buffer to be rejected, got:\n{diagnostics}"
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_rejects_checked_byte_span_escape_and_reuse() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let source = r#"from std.interop import c

def reject_escape_and_reuse(data: bytes) -> Result[None, str]:
    source = c.bytes_span(data)
    escaped = [source]
    mut destination = c.mutable_bytes_span(b"\0\0\0\0")
    unsafe:
        completed = destination.into_bytes(0)?
        destination.into_bytes(0)?
    return Ok(None)
"#;
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"checked_c_span_escape_rejections\"\n\n[sdk]\nprofile = \"minimal\"\n",
            source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");
        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            !output.status.success(),
            "expected an escaped or reused checked byte carrier to be rejected.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let diagnostics = String::from_utf8_lossy(&output.stderr);
        assert!(
            diagnostics.contains("has no ordinary value surface"),
            "expected a checked byte-carrier escape diagnostic, got:\n{diagnostics}"
        );
        assert!(
            diagnostics.contains("was already consumed"),
            "expected a checked mutable byte-buffer consumption diagnostic, got:\n{diagnostics}"
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_preserves_checked_c_f32_scalar_identity() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let header = tmp.path().join("float_fixture.h");
        std::fs::write(&header, "float fixture_absolute_f32(float value);\n")?;
        let source = format!(
            r#"from std.interop import c

binding FloatFixture:
    header = "{}"
    link = c.system_library("m")

    symbol absolute(value: c.f32) -> c.f32:
        native = "fixture_absolute_f32"

def absolute(value: f32) -> f32:
    unsafe:
        return FloatFixture.absolute(value)

def main() -> None:
    value: f32 = 1.5
    assert absolute(value) == value
"#,
            header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"checked_c_f32_scalar\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");
        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            output.status.success(),
            "expected an exact checked c.f32 signature to typecheck.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let checkout = support::repo_root();
        let emitted = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["--emit-rust", main_path.to_string_lossy().as_ref(), "--strict"])
            .output()?;
        assert!(
            emitted.status.success(),
            "expected an exact checked c.f32 signature to emit strict Rust.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&emitted.stdout),
            String::from_utf8_lossy(&emitted.stderr)
        );
        assert!(
            String::from_utf8_lossy(&emitted.stdout).contains("f32"),
            "expected generated Rust to retain the f32 ABI carrier"
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_preserves_exact_checked_c_scalar_carriers() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let header = tmp.path().join("scalar_fixture.h");
        std::fs::write(
            &header,
            "#include <stddef.h>\n#include <stdint.h>\nint8_t fixture_i8(int8_t value);\nuint8_t fixture_u8(uint8_t value);\nint16_t fixture_i16(int16_t value);\nuint16_t fixture_u16(uint16_t value);\nint32_t fixture_i32(int32_t value);\nuint32_t fixture_u32(uint32_t value);\nint64_t fixture_i64(int64_t value);\nuint64_t fixture_u64(uint64_t value);\n__int128 fixture_i128(__int128 value);\nunsigned __int128 fixture_u128(unsigned __int128 value);\nfloat fixture_f32(float value);\ndouble fixture_f64(double value);\nsize_t fixture_size(size_t value);\n",
        )?;
        let source = format!(
            r#"from std.interop import c

binding Scalars:
    header = "{}"
    link = c.system_library("c")

    symbol i8(value: c.i8) -> c.i8:
        native = "fixture_i8"
    symbol u8(value: c.u8) -> c.u8:
        native = "fixture_u8"
    symbol i16(value: c.i16) -> c.i16:
        native = "fixture_i16"
    symbol u16(value: c.u16) -> c.u16:
        native = "fixture_u16"
    symbol i32(value: c.i32) -> c.i32:
        native = "fixture_i32"
    symbol u32(value: c.u32) -> c.u32:
        native = "fixture_u32"
    symbol i64(value: c.i64) -> c.i64:
        native = "fixture_i64"
    symbol u64(value: c.u64) -> c.u64:
        native = "fixture_u64"
    symbol i128(value: c.i128) -> c.i128:
        native = "fixture_i128"
    symbol u128(value: c.u128) -> c.u128:
        native = "fixture_u128"
    symbol f32(value: c.f32) -> c.f32:
        native = "fixture_f32"
    symbol f64(value: c.f64) -> c.f64:
        native = "fixture_f64"
    symbol size(value: c.Size) -> c.Size:
        native = "fixture_size"

def exact_i8(value: i8) -> i8:
    unsafe:
        return Scalars.i8(value)

def exact_u8(value: u8) -> u8:
    unsafe:
        return Scalars.u8(value)

def exact_i16(value: i16) -> i16:
    unsafe:
        return Scalars.i16(value)

def exact_u16(value: u16) -> u16:
    unsafe:
        return Scalars.u16(value)

def exact_i32(value: i32) -> i32:
    unsafe:
        return Scalars.i32(value)

def exact_u32(value: u32) -> u32:
    unsafe:
        return Scalars.u32(value)

def exact_i64(value: i64) -> i64:
    unsafe:
        return Scalars.i64(value)

def exact_u64(value: u64) -> u64:
    unsafe:
        return Scalars.u64(value)

def exact_i128(value: i128) -> i128:
    unsafe:
        return Scalars.i128(value)

def exact_u128(value: u128) -> u128:
    unsafe:
        return Scalars.u128(value)

def exact_f32(value: f32) -> f32:
    unsafe:
        return Scalars.f32(value)

def exact_f64(value: f64) -> f64:
    unsafe:
        return Scalars.f64(value)

def exact_size(value: usize) -> usize:
    unsafe:
        return Scalars.size(value)
"#,
            header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"checked_c_exact_scalars\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");
        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            output.status.success(),
            "expected checked fixed-width C carriers to retain exact Incan representations.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let checkout = support::repo_root();
        let emitted = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["--emit-rust", main_path.to_string_lossy().as_ref(), "--strict"])
            .output()?;
        assert!(
            emitted.status.success(),
            "expected exact checked C carriers to emit strict Rust.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&emitted.stdout),
            String::from_utf8_lossy(&emitted.stderr)
        );
        let generated = String::from_utf8_lossy(&emitted.stdout);
        for rust_type in [
            "i8", "u8", "i16", "u16", "i32", "u32", "i64", "u64", "i128", "u128", "f32", "f64", "usize",
        ] {
            assert!(
                generated.contains(&format!("__incan_arg_0: {rust_type}")),
                "expected generated Rust to preserve exact {rust_type} checked-C carrier, got:\n{generated}"
            );
        }
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_supports_checked_f32_spans_for_paired_numeric_pointers() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let header = tmp.path().join("float_span_fixture.h");
        std::fs::write(
            &header,
            "#include <stddef.h>\nfloat fixture_sum_f32(const float *values, size_t value_count);\nsize_t fixture_fill_f32(float *destination, size_t destination_capacity);\n",
        )?;
        let source = format!(
            r#"from std.interop import c

binding FloatSpanFixture:
    header = "{}"
    link = c.system_library("m")

    symbol sum(values: c.ConstPtr[c.f32], value_count: c.Size) -> c.f32:
        native = "fixture_sum_f32"
        bounds = {{ values: value_count }}

    symbol fill(destination: c.MutPtr[c.f32], destination_capacity: c.Size) -> c.Size:
        native = "fixture_fill_f32"
        bounds = {{ destination: destination_capacity }}

def sum(values: list[f32]) -> f32:
    source = c.f32_span(values)
    unsafe:
        return FloatSpanFixture.sum(source.as_const_ptr(), source.element_count())

def fill() -> Result[list[f32], str]:
    mut destination = c.mutable_f32_span([0.0, 0.0, 0.0])
    unsafe:
        written = FloatSpanFixture.fill(destination.as_mut_ptr(), destination.element_capacity())
        return destination.into_f32s(written)
"#,
            header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"checked_c_f32_spans\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");
        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            output.status.success(),
            "expected paired checked f32 span calls to typecheck.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let checkout = support::repo_root();
        let bindings = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args([
                "inspect",
                "bindings",
                main_path.to_string_lossy().as_ref(),
                "--format",
                "json",
            ])
            .output()?;
        assert!(
            bindings.status.success(),
            "expected binding inspection to project the checked f32 span association.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&bindings.stdout),
            String::from_utf8_lossy(&bindings.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&bindings.stdout)?;
        let sum = report["bindings"][0]["symbols"]
            .as_array()
            .and_then(|symbols| symbols.iter().find(|symbol| symbol["name"] == serde_json::json!("sum")))
            .ok_or("binding inspection did not retain FloatSpanFixture.sum")?;
        assert_eq!(
            sum["buffers"],
            serde_json::json!([{
                "pointer_parameter": "values",
                "length_parameter": "value_count",
                "element": "c.f32",
            }]),
            "binding inspection must project the descriptor-owned span association"
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_rejects_unpaired_checked_f32_span_arguments() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let header = tmp.path().join("float_span_fixture.h");
        std::fs::write(
            &header,
            "#include <stddef.h>\nfloat fixture_sum_f32(const float *values, size_t value_count);\n",
        )?;
        let source = format!(
            r#"from std.interop import c

binding FloatSpanFixture:
    header = "{}"
    link = c.system_library("m")

    symbol sum(values: c.ConstPtr[c.f32], value_count: c.Size) -> c.f32:
        native = "fixture_sum_f32"
        bounds = {{ values: value_count }}

def reject_mismatched_owner(left: list[f32], right: list[f32]) -> f32:
    source = c.f32_span(left)
    other = c.f32_span(right)
    unsafe:
        return FloatSpanFixture.sum(source.as_const_ptr(), other.element_count())
"#,
            header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"checked_c_f32_span_rejection\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");
        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            !output.status.success(),
            "expected independently owned f32 pointer/count arguments to be rejected.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("same checked immutable f32 span"),
            "expected the declared f32 pointer/count relationship diagnostic, got:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn sqlite_checked_c_tooling_projects_the_shared_descriptor() -> Result<(), Box<dyn std::error::Error>> {
        let sqlite_header = sqlite_header_path()?;
        let tmp = tempfile::tempdir()?;
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"sqlite_checked_c_tooling\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &sqlite_checked_c_source(&sqlite_header),
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");
        let checkout = support::repo_root();
        let entry_path = main_path.to_string_lossy();

        let bindings = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["inspect", "bindings", entry_path.as_ref(), "--format", "json"])
            .output()?;
        assert!(
            bindings.status.success(),
            "expected binding inspection to reuse the checked SQLite descriptor.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&bindings.stdout),
            String::from_utf8_lossy(&bindings.stderr)
        );
        let binding_report: serde_json::Value = serde_json::from_slice(&bindings.stdout)?;
        let sqlite = binding_report["bindings"]
            .as_array()
            .and_then(|bindings| {
                bindings
                    .iter()
                    .find(|binding| binding["name"] == serde_json::json!("SQLite"))
            })
            .ok_or("binding inspection did not retain the SQLite descriptor")?;
        assert_eq!(sqlite["link_capability"], serde_json::json!("system_library"));
        assert_eq!(sqlite["resources"][0]["name"], serde_json::json!("Database"));
        assert_eq!(sqlite["resources"][0]["release"], serde_json::json!("close"));
        let error_message = sqlite["symbols"]
            .as_array()
            .and_then(|symbols| {
                symbols
                    .iter()
                    .find(|symbol| symbol["name"] == serde_json::json!("error_message"))
            })
            .ok_or("binding inspection did not retain SQLite.error_message")?;
        assert_eq!(
            error_message["parameters"][0]["type"]["access"],
            serde_json::json!("borrowed")
        );
        assert_eq!(error_message["return_type"]["kind"], serde_json::json!("pointer"));
        assert_eq!(error_message["return_type"]["mutable"], serde_json::json!(false));
        assert_eq!(
            error_message["return_type"]["pointee"]["spelling"],
            serde_json::json!("c.c_char")
        );

        let codegraph = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["inspect", "codegraph", entry_path.as_ref(), "--format", "jsonl"])
            .output()?;
        assert!(
            codegraph.status.success(),
            "expected codegraph to reuse the checked SQLite descriptor.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&codegraph.stdout),
            String::from_utf8_lossy(&codegraph.stderr)
        );
        let records = String::from_utf8(codegraph.stdout)?
            .lines()
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<Result<Vec<_>, _>>()?;
        let binding = records
            .iter()
            .find(|record| {
                record["record"] == serde_json::json!("c_binding") && record["name"] == serde_json::json!("SQLite")
            })
            .ok_or("codegraph did not retain the checked SQLite binding")?;
        assert_eq!(binding["provenance"], serde_json::json!("checked"));
        let error_call = records
            .iter()
            .find(|record| {
                record["record"] == serde_json::json!("c_binding_call")
                    && record["binding"] == serde_json::json!("SQLite")
                    && record["symbol"] == serde_json::json!("error_message")
            })
            .ok_or("codegraph did not retain the checked SQLite.error_message call")?;
        assert_eq!(error_call["binding_id"], binding["id"]);
        assert_eq!(error_call["unsafe_acknowledged"], serde_json::json!(true));
        assert_eq!(error_call["provenance"], serde_json::json!("checked"));
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_reports_checked_c_signature_mismatch_at_the_binding() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let fixture_header = tmp.path().join("fixture.h");
        std::fs::write(&fixture_header, "long fixture_abs(int value);\n")?;
        let source = format!(
            "from std.interop import c\n\nbinding Fixture:\n    header = \"{}\"\n    link = c.system_library(\"c\")\n\n    symbol absolute(value: c.i32) -> c.i32:\n        native = \"fixture_abs\"\n\ndef main() -> None:\n    pass\n",
            fixture_header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"checked_c_signature_mismatch\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");

        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            !output.status.success(),
            "expected a mismatched C signature to fail before code generation.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let diagnostics = strip_ansi_escapes(&String::from_utf8_lossy(&output.stderr));
        assert!(
            diagnostics.contains("C binding `Fixture` verification failed"),
            "expected a binding-anchored verifier diagnostic, got:\n{diagnostics}"
        );
        assert!(
            diagnostics.contains("Incan C signature mismatch"),
            "expected the Clang signature assertion detail, got:\n{diagnostics}"
        );
        Ok(())
    }

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
            !generated_main_rs.contains("__incan_vocab_helper_filterkit_filter"),
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
                    "__incan_vocab_helper_helperkit_aggregate_as(__incan_vocab_helper_helperkit_lit(5),\"total\".to_string()"
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
                || generated_main_rs.contains("__incan_vocab_helper_querykit_count("),
            "expected omitted count() argument to be filled from the helper's default expression, got:\n{generated_main_rs}"
        );
        assert!(
            !generated_main_rs.contains("__incan_vocab_helper_querykit_count()"),
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
                || generated_main_rs.contains("__incan_vocab_helper_querykit_count("),
            "expected omitted count() argument to be filled from the helper's default expression, got:\n{generated_main_rs}"
        );
        assert!(
            !generated_main_rs.contains("querykit::count()")
                && !generated_main_rs.contains("__incan_vocab_helper_querykit_count()"),
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

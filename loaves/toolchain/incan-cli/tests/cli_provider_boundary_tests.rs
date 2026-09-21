//! Compiled SDK providers, package boundaries, and Oven-baked provider composition.
//!
//! One of nine roots split out of the former `tests/cli_integration.rs`. The shared command surface lives in
//! `incan_test_support::cli_project`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use incan_test_support as support;

use incan_test_support::cli_project;

use cli_project::*;

#[cfg(unix)]
#[test]
fn scheduler_nested_build_and_run_fail_closed_when_the_immutable_native_plan_is_absent()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let source = tmp.path().join("scheduler-miss.incn");
    fs::write(&source, "def main() -> None:\n    pass\n")?;
    let scheduler_data_root = tmp.path().join("scheduler-toolchain");
    fs::create_dir_all(scheduler_data_root.join("share/incan/oven/loafs"))?;
    let incan_home = tmp.path().join("scheduler-home");
    let guard_dir = tmp.path().join("cargo-guard");
    let marker = tmp.path().join("cargo-was-started");
    let source_arg = source.to_string_lossy().into_owned();
    let envs = [
        ("INCAN_INTERNAL_OVEN_LOAF_EXECUTION", Path::new("1")),
        ("INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT", scheduler_data_root.as_path()),
        ("INCAN_HOME", incan_home.as_path()),
    ];

    for command in ["build", "run"] {
        let output = run_incan_with_failing_cargo_guard_and_env(
            tmp.path(),
            &[command, source_arg.as_str()],
            &guard_dir,
            &marker,
            &envs,
        )?;
        assert_failure(&output, &format!("scheduler nested {command} native-plan miss"));
        let diagnostics = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            diagnostics.contains("dependencies have not been compiled yet")
                && diagnostics.contains("will not compile them for you"),
            "scheduler nested {command} did not fail closed:\n{diagnostics}"
        );
    }
    assert!(
        !marker.exists(),
        "scheduler-native build/run miss launched the guarded Cargo executable"
    );
    let entries = incan_home.join("oven/store/v2/entries");
    assert!(
        !entries.exists() || fs::read_dir(&entries)?.next().is_none(),
        "scheduler-native build/run miss materialized a caller-owned store entry at {}",
        entries.display()
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn concurrent_normal_checks_reuse_sealed_sdk_inventory_without_mutable_publication()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "concurrent_sdk_provider_publication", "")?;
    let store = tmp.path().join("provider-store");
    let generated_target = tmp.path().join("generated-target");
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;
    let store_arg = store.to_str().ok_or("provider-store path was not valid UTF-8")?;

    let mut first = configured_incan_command(tmp.path(), &["check", main_arg]);
    first
        .env("INCAN_INTERNAL_SDK_PROVIDER_STORE", store_arg)
        .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_target)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut second = configured_incan_command(tmp.path(), &["check", main_arg]);
    second
        .env("INCAN_INTERNAL_SDK_PROVIDER_STORE", store_arg)
        .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_target)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let first = first.spawn()?;
    let second = second.spawn()?;
    let first = first.wait_with_output()?;
    let second = second.wait_with_output()?;
    assert_success(&first, "first concurrent SDK provider publication");
    assert_success(&second, "second concurrent SDK provider publication");

    assert!(
        !store.exists(),
        "normal checks must reuse their sealed SDK inventory instead of publishing a mutable per-fixture provider \
         store: {}",
        store.display()
    );
    let inventory_path = std::env::var_os("INCAN_SDK_INVENTORY")
        .map(PathBuf::from)
        .ok_or("normal Oven check has no sealed SDK inventory")?;
    assert!(
        inventory_path.is_file(),
        "normal Oven SDK inventory is not a regular file: {}",
        inventory_path.display()
    );
    let artifact_root = inventory_path
        .parent()
        .ok_or("normal Oven SDK inventory has no immutable provider root")?
        .to_path_buf();
    let inventory = incan_provider::SdkInventory::read_from_path(&inventory_path)?;
    inventory.validate_compiler_version(incan_lang::version::INCAN_VERSION)?;
    assert!(
        inventory.components.values().all(|component| component.available),
        "the reused full-profile provider identity must contain every component"
    );
    let workspace_lock: toml::Value = toml::from_str(&fs::read_to_string(support::repo_root().join("Cargo.lock"))?)?;
    let locked_packages = workspace_lock
        .get("package")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|package| package.get("name").and_then(toml::Value::as_str))
        .collect::<std::collections::HashSet<_>>();
    for component_id in inventory.components.keys() {
        let manifest_path = artifact_root.join("components").join(component_id).join("Cargo.toml");
        let manifest: toml::Value = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
        let package = manifest.get("package").and_then(toml::Value::as_table).ok_or_else(|| {
            format!(
                "SDK provider manifest {} has no [package] table",
                manifest_path.display()
            )
        })?;
        assert_eq!(
            package.get("license").and_then(toml::Value::as_str),
            Some("Apache-2.0"),
            "official SDK provider `{component_id}` must preserve its source-owned SPDX license: {}",
            manifest_path.display()
        );
        assert!(
            package.get("license-file").is_none(),
            "SPDX-licensed SDK provider `{component_id}` must not invent a Cargo license-file: {}",
            manifest_path.display()
        );
        for (dependency_name, dependency) in manifest
            .get("dependencies")
            .and_then(toml::Value::as_table)
            .into_iter()
            .flatten()
        {
            let dependency_table = dependency.as_table();
            if dependency_table.is_some_and(|dependency| dependency.contains_key("path")) {
                continue;
            }
            let package_name = dependency_table
                .and_then(|dependency| dependency.get("package"))
                .and_then(toml::Value::as_str)
                .unwrap_or(dependency_name);
            assert!(
                locked_packages.contains(package_name),
                "SDK provider `{component_id}` registry dependency `{package_name}` must be anchored in the workspace \
                 lock so offline integration shards can resolve it: {}",
                manifest_path.display()
            );
        }
    }
    Ok(())
}

#[test]
fn compiled_sdk_providers_replace_consumer_fs_source_closure() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "compiled_sdk_provider_glob", "")?;
    fs::write(
        &main_path,
        r#"from std.fs import IoError
from std.fs.glob import matches
from std.fs.locking import try_exclusive
from std.fs.path import Path


def read_chunks(target: Path) -> Result[None, IoError]:
  input = target.open("rb", -1, None, None, None)?
  for chunk in input.chunks(4)?:
    assert len(chunk) > 0
  return Ok(None)


def main() -> None:
  payload = b"artifact"
  target = Path("target/compiled-stdlib-artifact.bin")
  match target.write_bytes(payload):
    Ok(_) => pass
    Err(_) => pass
  match target.read_bytes():
    Ok(data) => assert data == payload
    Err(_) => pass
  match try_exclusive("target/compiled-stdlib-artifact.bin"):
    Ok(_) => pass
    Err(_) => pass
  match read_chunks(target):
    Ok(_) => pass
    Err(_) => pass
  println(matches("routes/users.incn", "routes/*.incn"))
  println(Path("routes/users.incn").name())
"#,
    )?;
    let output_dir = tmp.path().join("generated");
    let main_arg = main_path.to_string_lossy();
    let output_arg = output_dir.to_string_lossy();
    let output = run_incan(tmp.path(), &["build", &main_arg, &output_arg])?;
    assert_success(&output, "incan build with compiled std.fs artifact");

    let cargo_toml = fs::read_to_string(output_dir.join("Cargo.toml"))?;
    assert!(
        cargo_toml.contains("[dependencies.incan_stdlib_system]")
            && cargo_toml.contains("[dependencies.incan_stdlib_core]"),
        "consumer must directly link every semantic SDK owner named by generated Rust; the std.io fallible stream \
         protocol is core-owned:\n\
         {cargo_toml}"
    );
    assert!(
        !cargo_toml.contains("[dependencies.incan_stdlib_data]")
            && !cargo_toml.contains("[dependencies.incan_stdlib_web]"),
        "filesystem-only consumers must not link unrelated SDK providers:\n{cargo_toml}"
    );
    assert!(
        !output_dir.join("src/__incan_std").exists(),
        "migrated std.fs source closure must not be materialized into the consumer"
    );
    let main_rust = read_generated_rust(&output_dir.join("src/main.rs"))?;
    assert!(
        main_rust.contains("pub use incan_stdlib_system::__incan_std::*;")
            && main_rust.contains("pub use crate::__incan_std::fs::glob::matches;"),
        "generated consumer must route the stable std.fs facade through its compiled provider:\n{main_rust}"
    );
    assert!(
        main_rust.contains("pub use crate::__incan_std::fs::path::Path;"),
        "generated consumer must construct types through the stable provider facade:\n{main_rust}"
    );
    assert!(
        main_rust.contains("crate::__incan_std::fs::locking::try_exclusive"),
        "manifest-discovered stdlib modules must call through the stable provider facade:\n{main_rust}"
    );
    assert!(
        main_rust.contains("target.write_bytes(payload.clone())"),
        "compiled newtype method metadata must preserve Incan ownership semantics:\n{main_rust}"
    );

    let codegraph = run_incan(tmp.path(), &["inspect", "codegraph", &main_arg, "--format", "jsonl"])?;
    assert_success(&codegraph, "incan inspect codegraph with compiled std.fs metadata");
    Ok(())
}

#[test]
fn compiled_sdk_providers_preserve_facade_imports() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "compiled_sdk_provider_web_facade", "")?;
    fs::write(
        &main_path,
        r#"from std.fs import Path as FsPath
from std.telemetry import TraceId
from std.traits import TryFrom
from std.web import App, route, Response, Json, GET

def main() -> None:
  pass
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &["check", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&output, "incan check with compiled stdlib facade metadata");
    Ok(())
}

#[test]
fn fallible_iterator_adapters_compile_in_test_batch() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    write_minimal_project(tmp.path(), "fallible_iterator_test_batch", "")?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        tests_dir.join("test_fallible_iterator.incn"),
        r#"from std.derives.collection import FallibleIterator
from std.testing import assert_eq, assert_true

model NumberStream with FallibleIterator[int, str]:
    values: list[int]
    index: int

    def __next__(mut self) -> Result[Option[int], str]:
        if self.index >= len(self.values):
            return Ok(None)
        value = self.values[self.index]
        self.index += 1
        return Ok(Some(value))


def double(value: int) -> int:
    return value * 2


def test_fallible_adapter_batch() -> None:
    match NumberStream(values=[1, 2], index=0).map(double).collect():
        Ok(values) =>
            assert_eq(len(values), 2)
            assert_eq(values[0], 2)
            assert_eq(values[1], 4)
        Err(error) => assert_true(false, error)
"#,
    )?;

    let output = run_incan(tmp.path(), &["test", "tests"])?;
    assert_success(&output, "incan test batch with FallibleIterator adapters");
    Ok(())
}

#[test]
fn fallible_iterator_defaults_cross_compiled_package_boundary() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let producer_root = tmp.path().join("fallible_streams");
    let producer_src = producer_root.join("src");
    fs::create_dir_all(&producer_src)?;
    fs::write(
        producer_root.join("loaf.toml"),
        "[project]\nname = \"fallible_streams\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        producer_src.join("streams.incn"),
        r#"from std.derives.collection import FallibleIterator

@derive(Clone)
pub enum StreamError:
    Fetch(str)


pub model NumberStream with FallibleIterator[int, str]:
    pub values: list[int]
    pub index: int

    def __next__(mut self) -> Result[Option[int], str]:
        if self.index >= len(self.values):
            return Ok(None)
        value = self.values[self.index]
        self.index += 1
        return Ok(Some(value))


pub def numbers() -> NumberStream:
    return NumberStream(values=[1, 2, 3], index=0)
"#,
    )?;
    fs::write(
        producer_src.join("facade.incn"),
        "pub from streams import NumberStream, StreamError, numbers\n",
    )?;
    fs::write(
        producer_src.join("lib.incn"),
        "pub from facade import NumberStream, StreamError, numbers\n",
    )?;
    let producer_build = run_explicit_oven_bake(&producer_root)?;
    assert_success(
        &producer_build,
        "explicit Oven bake for fallible iterator package boundary",
    );

    let consumer_root = tmp.path().join("fallible_consumer");
    let consumer_main = write_minimal_project(
        &consumer_root,
        "fallible_consumer",
        r#"
[dependencies]
fallible_streams = { path = "../fallible_streams" }
"#,
    )?;
    fs::write(consumer_root.join("sample.bin"), b"abcde")?;
    fs::write(
        &consumer_main,
        r#"from pub::fallible_streams import StreamError, numbers
from std.fs import IoError, Path


def double(value: int) -> int:
    return value * 2


def read_file_chunks() -> Result[None, IoError]:
    input = Path("sample.bin").open("rb", -1, None, None, None)?
    for chunk in input.chunks(2)?:
        println(f"chunk:{len(chunk)}")
    return Ok(None)


def main() -> None:
    match numbers().map(double).map_err(StreamError.Fetch).collect():
        Ok(values) => println(f"values:{values[0]}:{values[1]}:{values[2]}")
        Err(StreamError.Fetch(detail)) => println(f"error:{detail}")
    match read_file_chunks():
        Ok(_) => pass
        Err(error) => println(error.message())
"#,
    )?;
    let consumer_bake = run_explicit_oven_bake(&consumer_root)?;
    assert_success(
        &consumer_bake,
        "explicit Oven bake for the fallible iterator package consumer",
    );
    let consumer_run = run_incan(&consumer_root, &["run"])?;
    assert_success(
        &consumer_run,
        "compiled package consumer with fallible defaults and File.chunks",
    );
    assert_eq!(
        String::from_utf8_lossy(&consumer_run.stdout)
            .lines()
            .collect::<Vec<_>>(),
        vec!["values:2:4:6", "chunk:2", "chunk:2", "chunk:1"]
    );
    Ok(())
}

#[test]
fn set_constructor_survives_facade_package_and_test_batch_issue951() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let producer_root = tmp.path().join("set_library");
    let producer_src = producer_root.join("src");
    fs::create_dir_all(&producer_src)?;
    fs::write(
        producer_root.join("loaf.toml"),
        "[project]\nname = \"set_library\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        producer_src.join("sets.incn"),
        r#""""Publish a collection helper that exercises canonical Set construction."""


pub def unique(values: List[str]) -> Set[str]:
    """Return the distinct values from one source list."""
    return set(values)
"#,
    )?;
    fs::write(
        producer_src.join("facade.incn"),
        r#""""Re-export the public set helper through an intermediate facade."""

pub from sets import unique
"#,
    )?;
    fs::write(
        producer_src.join("lib.incn"),
        r#""""Publish the package's stable public facade."""

pub from facade import unique
"#,
    )?;

    let producer_build = run_explicit_oven_bake(&producer_root)?;
    assert_success(
        &producer_build,
        "explicit Oven bake for Set constructor package boundary",
    );

    let consumer_root = tmp.path().join("set_consumer");
    let consumer_main = write_minimal_project(
        &consumer_root,
        "set_consumer",
        r#"
[dependencies]
set_library = { path = "../set_library" }
"#,
    )?;
    fs::write(
        &consumer_main,
        r#""""Consume a compiled helper that constructs a Set behind a facade."""

from pub::set_library import unique


def main() -> None:
    """Print the cardinality returned by the compiled package."""
    println(len(unique(["beta", "alpha", "beta"])))
"#,
    )?;
    let tests_dir = consumer_root.join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        tests_dir.join("test_sets.incn"),
        r#""""Exercise compiled and local Set construction in one generated test batch."""

from pub::set_library import unique
from std.testing import assert_eq


def test_set_constructor_boundaries() -> None:
    """Verify the provider facade and test-batch lowering routes."""
    assert_eq(len(unique(["beta", "alpha", "beta"])), 2)
    assert_eq(len(set(["gamma", "gamma", "delta"])), 2)
"#,
    )?;

    let consumer_bake = run_explicit_oven_bake(&consumer_root)?;
    assert_success(
        &consumer_bake,
        "explicit Oven bake for the Set constructor package consumer",
    );

    let consumer_run = run_incan(&consumer_root, &["run"])?;
    assert_success(
        &consumer_run,
        "compiled package consumer with a facade-exported Set constructor",
    );
    assert_eq!(String::from_utf8(consumer_run.stdout)?, "2\n");

    let consumer_tests = run_incan(&consumer_root, &["test", "tests"])?;
    assert_success(
        &consumer_tests,
        "compiled package and local Set constructors in a generated test batch",
    );
    Ok(())
}

#[test]
fn compiled_sdk_providers_keep_serde_trait_imports_out_of_consumers() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "compiled_sdk_provider_serde", "")?;
    fs::write(
        &main_path,
        r#"from std.serde.json import Serialize

def main() -> None:
  println("serde trait metadata is available")
"#,
    )?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        tests_dir.join("test_artifact.incn"),
        r#"from std.fs.locking import try_exclusive
from std.fs.path import Path
from std.testing import assert_eq

def test_artifact_path() -> None:
  assert_eq(Path("routes/users.incn").name(), "users.incn")
  match try_exclusive("target/test-artifact.lock"):
    Ok(_) => pass
    Err(_) => pass
"#,
    )?;
    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(
        &bake_output,
        "explicit Oven bake for compiled SDK provider test projection",
    );
    let output_dir = tmp.path().join("generated");
    let main_arg = main_path.to_string_lossy();
    let output_arg = output_dir.to_string_lossy();
    let output = run_incan(tmp.path(), &["build", &main_arg, &output_arg])?;
    assert_success(&output, "incan build with compiled std.serde.json artifact");

    assert!(
        !output_dir.join("src/__incan_std").exists(),
        "compiled std.serde.json must not be materialized into the consumer"
    );
    let cargo_toml = fs::read_to_string(output_dir.join("Cargo.toml"))?;
    assert!(
        cargo_toml.contains("[dependencies.incan_stdlib_data]"),
        "consumer must link the compiled stdlib data provider:\n{cargo_toml}"
    );

    let test_output = run_incan(tmp.path(), &["test"])?;
    assert_success(&test_output, "incan test with compiled std.fs artifact");
    let mut generated_test_harnesses = 0;
    for entry in fs::read_dir(tmp.path().join("target/incan_tests"))? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            generated_test_harnesses += 1;
            assert!(
                !entry.path().join("src/__incan_std").exists(),
                "migrated std.fs source closure must not be materialized into a generated test harness: {}",
                entry.path().display()
            );
        }
    }
    assert!(
        generated_test_harnesses > 0,
        "incan test did not create a generated test harness"
    );
    Ok(())
}

#[test]
fn compiled_json_trait_owner_crosses_library_boundaries_issue946() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let provider_root = tmp.path().join("json_provider");
    let _provider_main = write_minimal_project(
        &provider_root,
        "json_provider",
        "\n[sdk]\nprofile = \"minimal\"\ncomponents = [\"stdlib-data\"]\n",
    )?;
    fs::write(
        provider_root.join("src/lib.incn"),
        "pub from crate.codec import encode_item\npub from crate.models import Item\n",
    )?;
    fs::write(
        provider_root.join("src/models.incn"),
        r#"from std.serde import json

@derive(json)
pub model Item:
  pub value: str
"#,
    )?;
    fs::write(
        provider_root.join("src/codec.incn"),
        r#"from std.serde.json import Serialize
from crate.models import Item

pub def encode_item(item: Item) -> str:
  return item.to_json()
"#,
    )?;

    let provider_build = run_explicit_oven_bake(&provider_root)?;
    assert_success(&provider_build, "explicit Oven bake for the multi-module JSON provider");
    let generated_encoder = fs::read_to_string(provider_root.join("target/lib/src/codec.rs"))?;
    assert!(
        generated_encoder.contains("__incan_std::serde::json::Serialize::to_json(&item)"),
        "generated encoder must retain the canonical source trait owner:\n{generated_encoder}"
    );
    assert!(
        !generated_encoder.contains("return json::Serialize::to_json(&item)"),
        "generated encoder cannot rely on another source module's `json` import:\n{generated_encoder}"
    );

    let consumer_root = tmp.path().join("consumer");
    let consumer_main = write_minimal_project(
        &consumer_root,
        "json_consumer",
        "[dependencies]\njson_provider = { path = \"../json_provider\" }\n",
    )?;
    fs::write(
        &consumer_main,
        r#"from pub::json_provider import Item, encode_item

def main() -> None:
  println(encode_item(Item(value="ok")))
"#,
    )?;
    let tests_dir = consumer_root.join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        tests_dir.join("test_json_provider.incn"),
        r#"from pub::json_provider import Item, encode_item
from std.testing import assert_eq

def test_compiled_json_provider() -> None:
  assert_eq(encode_item(Item(value="ok")), "{\"value\":\"ok\"}")
"#,
    )?;

    let consumer_bake = run_explicit_oven_bake(&consumer_root)?;
    assert_success(
        &consumer_bake,
        "explicit Oven bake for the multi-module JSON package consumer",
    );

    let consumer_run = run_incan(&consumer_root, &["run"])?;
    assert_success(&consumer_run, "consumer of the compiled multi-module JSON provider");
    assert!(
        String::from_utf8_lossy(&consumer_run.stdout).contains("{\"value\":\"ok\"}"),
        "unexpected compiled JSON provider output:\n{}",
        String::from_utf8_lossy(&consumer_run.stdout)
    );
    let consumer_test = run_incan(&consumer_root, &["test"])?;
    assert_success(
        &consumer_test,
        "package test batch consuming the compiled multi-module JSON provider",
    );
    Ok(())
}

#[test]
fn data_component_owns_hashing_without_linking_the_codecs_provider() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "data_component_hashing",
        "\n\n[sdk]\nprofile = \"minimal\"\ncomponents = [\"stdlib-data\"]\n",
    )?;
    fs::write(
        &main_path,
        r#"from std.collections import OrdinalMap

def main() -> None:
  println("data provider linked")
"#,
    )?;
    let output_dir = tmp.path().join("generated");
    let main_arg = main_path.to_string_lossy();
    let output_arg = output_dir.to_string_lossy();
    let build = run_incan(tmp.path(), &["build", &main_arg, &output_arg])?;
    assert_success(&build, "data-only SDK component generated-Rust build");

    let cargo_toml = fs::read_to_string(output_dir.join("Cargo.toml"))?;
    assert!(cargo_toml.contains("[dependencies.incan_stdlib_data]"));
    assert!(
        !cargo_toml.contains("[dependencies.incan_stdlib_codecs]"),
        "the data provider must not link compression dependencies through the codecs provider:\n{cargo_toml}"
    );
    assert!(
        !output_dir.join("src/__incan_std").exists(),
        "data-only consumers must use the compiled provider without materializing stdlib source"
    );

    let hash_probe = tmp.path().join("src/hash_probe.incn");
    fs::write(
        &hash_probe,
        r#"from std.hash import sha256

def main() -> None:
  pass
"#,
    )?;
    let probe = run_incan(
        tmp.path(),
        &[
            "check",
            hash_probe.to_str().ok_or("hash probe path was not valid UTF-8")?,
        ],
    )?;
    assert_success(&probe, "public std.hash import from the enabled data component");

    let compression_probe = tmp.path().join("src/compression_probe.incn");
    fs::write(
        &compression_probe,
        r#"from std.compression import gzip

def main() -> None:
  pass
"#,
    )?;
    let compression = run_incan(
        tmp.path(),
        &[
            "check",
            compression_probe
                .to_str()
                .ok_or("compression probe path was not valid UTF-8")?,
        ],
    )?;
    assert_failure(&compression, "public std.compression import with codecs disabled");
    let diagnostic = format!(
        "{}\n{}",
        String::from_utf8_lossy(&compression.stdout),
        String::from_utf8_lossy(&compression.stderr)
    );
    assert!(
        diagnostic.contains("stdlib-compression") && diagnostic.contains("disabled"),
        "disabled public compression imports must identify the component selection remedy:\n{diagnostic}"
    );
    Ok(())
}

#[test]
fn build_lib_materializes_oven_artifacts_without_a_generated_cargo_preheat() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let helper_dir = tmp.path().join("library_preheat_helper");
    fs::create_dir_all(helper_dir.join("src"))?;
    fs::write(
        helper_dir.join("Cargo.toml"),
        "[package]\nname = \"library_preheat_helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(helper_dir.join("src").join("lib.rs"), "pub fn value() -> i64 { 7 }\n")?;

    let _main_path = write_minimal_project(
        tmp.path(),
        "cli_library_preheat_project",
        r#"
[rust-dependencies.library_preheat_helper]
path = "library_preheat_helper"
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("lib.incn"),
        r#"from rust::library_preheat_helper import value

pub def exported_value() -> int:
  return value()
"#,
    )?;

    let bake = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake, "explicit Oven bake for library direct-rustc materialization");
    assert!(
        tmp.path()
            .join("target/lib/oven/debug/libcli_library_preheat_project.rlib")
            .is_file(),
        "explicit Oven bake must materialize a caller-owned direct-rustc debug artifact"
    );
    assert!(
        tmp.path()
            .join("target/lib/oven/release/libcli_library_preheat_project.rlib")
            .is_file(),
        "explicit Oven bake must materialize a caller-owned direct-rustc release artifact"
    );
    assert!(
        tmp.path().join("oven.lock").is_file(),
        "explicit Oven bake must publish the canonical project lock"
    );
    assert!(
        !tmp.path().join("target/incan_lock/Cargo.toml").exists(),
        "explicit Oven bake must not leave a generated Cargo workspace in the compiler-owned lock directory"
    );

    // Remove only caller-owned projections, then prove one normal locked command can restore both profiles from the
    // completed project Loafs. A separate normal build and lock walk would retrace the publication path needlessly.
    let debug_artifact = tmp
        .path()
        .join("target/lib/oven/debug/libcli_library_preheat_project.rlib");
    let release_artifact = tmp
        .path()
        .join("target/lib/oven/release/libcli_library_preheat_project.rlib");
    fs::remove_file(&debug_artifact)?;
    fs::remove_file(&release_artifact)?;
    let lock_projection = tmp.path().join("target/incan_lock");
    if lock_projection.exists() {
        fs::remove_dir_all(lock_projection)?;
    }

    let locked_build = run_incan(tmp.path(), &["build", "--lib", "--locked"])?;
    assert_success(
        &locked_build,
        "normal locked build --lib should restore direct-rustc artifacts from completed project Loafs",
    );
    assert!(
        debug_artifact.is_file(),
        "the normal locked replay must recreate the debug library artifact"
    );
    assert!(
        release_artifact.is_file(),
        "the normal locked replay must recreate the release library artifact"
    );
    assert!(
        !tmp.path().join("target/incan_lock/Cargo.toml").exists(),
        "locked Oven builds must not create a Cargo workspace in the compiler-owned lock directory"
    );

    Ok(())
}

#[test]
fn build_public_alias_of_imported_item_reexports_original_path_issue617() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "public_alias_import_reexport", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    fs::write(
        src_dir.join("helper.incn"),
        r#"pub def target(value: int) -> int:
    """Return one incremented value."""
    return value + 1
"#,
    )?;
    fs::write(
        &main_path,
        r#"from helper import target as target_builder


pub public_target = alias target_builder


def main() -> None:
    """Exercise public alias re-export of an imported public function."""
    assert public_target(1) == 2
"#,
    )?;

    let output_dir = tmp.path().join("out");
    let build_output = run_incan(
        tmp.path(),
        &[
            "build",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
            output_dir.to_str().ok_or("output path was not valid UTF-8")?,
        ],
    )?;
    assert_success(&build_output, "public alias of imported item build");

    let generated_main = read_generated_rust(&output_dir.join("src/main.rs"))?;
    assert!(
        !generated_main.contains("pub use target_builder as public_target;"),
        "public alias should not re-export the private local import binding, got:\n{generated_main}"
    );
    assert!(
        generated_main.contains("pub use crate::helper::target as public_target;")
            || generated_main.contains("pub use helper::target as public_target;"),
        "public alias should re-export the original imported path, got:\n{generated_main}"
    );
    Ok(())
}

#[test]
fn build_pub_consumer_imports_public_alias_of_imported_item_issue617() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let producer_root = tmp.path().join("alias_lib");
    let producer_src = producer_root.join("src");
    fs::create_dir_all(&producer_src)?;
    fs::write(
        producer_root.join("loaf.toml"),
        r#"[project]
name = "alias_lib"
version = "0.1.0"
"#,
    )?;
    fs::write(
        producer_src.join("helper.incn"),
        r#"pub def target(value: int) -> int:
    return value + 1
"#,
    )?;
    fs::write(
        producer_src.join("functions.incn"),
        r#"from helper import target as target_impl

pub public_target = alias target_impl
"#,
    )?;
    fs::write(
        producer_src.join("lib.incn"),
        r#"pub from functions import public_target
"#,
    )?;

    let producer_build = run_incan(&producer_root, &["build", "--lib"])?;
    assert_success(&producer_build, "producer build --lib for public alias issue617");

    let manifest_path = producer_root.join("target").join("lib").join("alias_lib.incnlib");
    let manifest: serde_json::Value = serde_json::from_str(&fs::read_to_string(&manifest_path)?)?;
    assert!(
        manifest.pointer("/exports/aliases/0/projected_function").is_some(),
        "callable alias export should include function projection metadata, got:\n{manifest}"
    );

    let consumer_root = tmp.path().join("alias_consumer");
    let consumer_main = write_minimal_project(
        &consumer_root,
        "alias_consumer",
        r#"
[dependencies]
alias_lib = { path = "../alias_lib" }
"#,
    )?;
    fs::write(
        &consumer_main,
        r#"from pub::alias_lib import public_target


def main() -> None:
    assert public_target(1) == 2
"#,
    )?;

    let consumer_check = run_incan(
        &consumer_root,
        &[
            "--check",
            consumer_main.to_str().ok_or("consumer main path was not valid UTF-8")?,
        ],
    )?;
    assert_success(&consumer_check, "pub consumer check for public alias issue617");
    Ok(())
}

#[test]
fn test_accepts_public_alias_of_imported_item_issue631() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "public_alias_test_reexport", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        src_dir.join("helper.incn"),
        r#"pub def target() -> int:
    return 1
"#,
    )?;
    fs::write(
        src_dir.join("functions.incn"),
        r#"from helper import target as target_builder

pub public_target = alias target_builder
"#,
    )?;
    fs::write(
        &main_path,
        r#"from functions import public_target


def main() -> None:
    assert public_target() == 1
"#,
    )?;
    fs::write(
        tests_dir.join("test_alias.incn"),
        r#"from functions import public_target


def test_alias() -> None:
    assert public_target() == 1
"#,
    )?;

    let test_path = tests_dir.join("test_alias.incn");
    let test_output = run_incan(
        tmp.path(),
        &["test", test_path.to_str().ok_or("test path was not valid UTF-8")?],
    )?;
    assert_success(&test_output, "incan test for public alias issue631");
    Ok(())
}

#[test]
fn test_std_registry_runs_in_a_compiled_test_batch() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "std_registry_test_batch", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        src_dir.join("feature.incn"),
        r#"from std.registry import Registry, SubjectKind, describe

@derive(Clone, Eq)
pub type FunctionId = newtype str

@derive(Descriptor)
pub model FunctionSpec:
    pub summary: str

pub static functions: Registry[FunctionId, FunctionSpec] = Registry.define(
    subjects=[SubjectKind.Function],
)

@describe(functions, FunctionId("normalize"), FunctionSpec(summary="Normalize text"))
pub def normalize(value: str) -> str:
    return value
"#,
    )?;
    fs::write(
        tests_dir.join("test_std_registry_batch.incn"),
        r#"from std.testing import assert_eq
from feature import FunctionId, functions, normalize

def test_loaded_entries_keep_checked_description_shape() -> None:
    assert_eq(normalize("value"), "value")
    entries = functions.loaded_entries()
    assert_eq(len(entries), 1)
    assert_eq(entries[0].key, FunctionId("normalize"))
    assert_eq(entries[0].descriptor.summary, "Normalize text")
    assert_eq(entries[0].subject.qualified_name, "feature.normalize")
"#,
    )?;

    let test_path = tests_dir.join("test_std_registry_batch.incn");
    let output = run_incan(
        tmp.path(),
        &["test", test_path.to_str().ok_or("test path was not valid UTF-8")?],
    )?;
    assert_success(&output, "compiled test batch for std.registry");
    Ok(())
}

#[test]
fn imported_registry_descriptions_keep_the_catalog_as_canonical_authority_issue1004()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "imported_registry_description", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        src_dir.join("catalog.incn"),
        r#"from std.registry import Registry, SubjectKind

@derive(Clone, Eq, Descriptor)
pub model FunctionKey:
    pub name: str

@derive(Clone, Descriptor)
pub model FunctionDescriptor:
    pub deterministic: bool

pub static functions: Registry[FunctionKey, FunctionDescriptor] = Registry.define(
    subjects=[SubjectKind.Function],
)
"#,
    )?;
    fs::write(
        src_dir.join("normalize.incn"),
        r#"from std.registry import describe
from catalog import FunctionDescriptor, FunctionKey, functions

@describe(
    functions,
    FunctionKey(name="normalize"),
    FunctionDescriptor(deterministic=true)
)
pub def normalize(value: str) -> str:
    return value
"#,
    )?;
    fs::write(
        src_dir.join("lib.incn"),
        r#"pub from catalog import FunctionDescriptor, FunctionKey, functions
pub from normalize import normalize
"#,
    )?;
    fs::write(
        tests_dir.join("test_registry.incn"),
        r#"from std.testing import assert_eq
from catalog import functions
from normalize import normalize

def test_imported_registry_description() -> None:
    assert_eq(normalize("value"), "value", "imported described function should execute")
    entries = functions.loaded_entries()
    assert_eq(
        len(entries),
        1,
        f"the imported canonical registry should receive one description, got {len(entries)}",
    )
    assert_eq(entries[0].key.name, "normalize", "registry key should survive imported registration")
    assert_eq(entries[0].descriptor.deterministic, true, "registry descriptor should survive imported registration")
    assert_eq(
        entries[0].subject.qualified_name,
        "normalize.normalize",
        "registry subject should retain the contributing module identity",
    )
"#,
    )?;

    let library_build = run_incan(tmp.path(), &["build", "--lib"])?;
    assert_success(&library_build, "library build with an imported canonical registry");

    let test_path = tests_dir.join("test_registry.incn");
    let test_output = run_incan(
        tmp.path(),
        &["test", test_path.to_str().ok_or("test path was not valid UTF-8")?],
    )?;
    assert_success(&test_output, "compiled test using the imported canonical registry");
    Ok(())
}

/// RFC 113: the mandatory core provider must supply the Incan-authored registry implementation, including the
/// compiler-reserved helper boundary, to a minimal-profile consumer.
#[test]
fn build_std_registry_consumer_uses_compiled_core_provider() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "std_registry_provider_build",
        "\n\n[sdk]\nprofile = \"minimal\"\n",
    )?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    fs::write(
        src_dir.join("feature.incn"),
        r#"from std.registry import Registry, SubjectKind, describe

@derive(Clone, Eq)
pub type FunctionId = newtype str

@derive(Descriptor)
pub model FunctionSpec:
    pub target: Type[int]

pub static functions: Registry[FunctionId, FunctionSpec] = Registry.define(
    subjects=[SubjectKind.Function],
)

@describe(functions, FunctionId("normalize"), FunctionSpec(target=int))
pub def normalize(value: int) -> int:
    return value
"#,
    )?;
    fs::write(
        &main_path,
        r#"from feature import normalize

def main() -> None:
    println(normalize(1))
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &output,
        "minimal-profile build using std.registry from the compiled core provider",
    );
    Ok(())
}

#[test]
fn oven_baked_public_direct_rust_provider_composes_into_consumer_issue1053() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let provider_root = tmp.path().join("uuid_provider");
    fs::create_dir_all(provider_root.join("src"))?;
    fs::write(
        provider_root.join("loaf.toml"),
        r#"[project]
name = "uuid_provider"
version = "0.1.0"

[rust-dependencies.uuid]
version = "1"
features = ["v4"]
"#,
    )?;
    fs::write(
        provider_root.join("src/lib.incn"),
        r#"from rust::uuid import Uuid


pub def provider_token() -> str:
    return Uuid.new_v4().to_string()
"#,
    )?;
    let provider_bake = run_explicit_oven_bake(&provider_root)?;
    assert_success(
        &provider_bake,
        "explicit Oven bake for the public direct-Rust UUID provider",
    );

    let consumer_root = tmp.path().join("consumer");
    fs::create_dir_all(consumer_root.join("src"))?;
    fs::write(
        consumer_root.join("loaf.toml"),
        "[project]\nname = \"consumer\"\n\n[dependencies]\nuuid_provider = { path = \"../uuid_provider\" }\n",
    )?;
    fs::write(
        consumer_root.join("src/main.incn"),
        r#"from pub::uuid_provider import provider_token


def main() -> None:
    assert len(provider_token()) == 36
"#,
    )?;
    let consumer_bake = run_explicit_oven_bake(&consumer_root)?;
    assert_success(
        &consumer_bake,
        "explicit Oven bake for a consumer of the public direct-Rust UUID provider",
    );
    Ok(())
}

/// An explicit consumer bake owns its direct registry closure even when it imports a separately baked provider.
///
/// The provider's package Loaf remains a receipt-checked input, but it cannot become the registry authority for the
/// consumer's independently declared `itoa` root. A later locked run proves the selected consumer Loaf remains
/// sufficient after the explicit publisher has completed. `itoa` is deliberately the same zero-dependency registry
/// root already used by the other direct-dependency tests in this file (see e.g.
/// `workspace_lock_is_published_once_at_the_root_from_any_member`): any external crate proves the registry-authority
/// behavior under test, so picking one already covered by the Oven Loaf dependency prefetch manifest keeps this
/// test's own registry closure from growing independently.
#[test]
fn oven_baked_provider_and_direct_registry_consumer_bake_issue1054() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let provider_root = tmp.path().join("provider");
    fs::create_dir_all(provider_root.join("src"))?;
    fs::write(
        provider_root.join("loaf.toml"),
        "[project]\nname = \"provider\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        provider_root.join("src/lib.incn"),
        "pub def provided() -> int:\n  return 7\n",
    )?;
    let provider_bake = run_explicit_oven_bake(&provider_root)?;
    assert_success(&provider_bake, "explicit Oven bake for #1054 provider");

    let consumer_root = tmp.path().join("consumer");
    let consumer_main = write_minimal_project(
        &consumer_root,
        "consumer",
        r#"
[dependencies]
provider = { path = "../provider" }

[rust-dependencies]
itoa = "1"
"#,
    )?;
    fs::write(
        &consumer_main,
        r#"from pub::provider import provided
from rust::itoa import Buffer


def main() -> None:
  assert provided() == 7
  println("provider and direct registry closure")
"#,
    )?;
    fs::write(
        consumer_root.join("src/lib.incn"),
        r#"from pub::provider import provided
from rust::itoa import Buffer


pub def provider_value() -> int:
  return provided()
"#,
    )?;

    let consumer_bake = run_explicit_oven_bake(&consumer_root)?;
    assert_success(
        &consumer_bake,
        "explicit Oven bake for #1054 provider plus direct itoa consumer",
    );

    let consumer_run = run_incan(&consumer_root, &["run", "--locked"])?;
    assert_success(
        &consumer_run,
        "locked Oven run for #1054 provider plus direct itoa consumer",
    );
    assert_eq!(
        String::from_utf8(consumer_run.stdout)?,
        "provider and direct registry closure\n"
    );
    Ok(())
}

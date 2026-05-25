use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn incan_binary() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_incan") {
        return PathBuf::from(path);
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        let path = PathBuf::from(target_dir).join("debug").join("incan");
        if path.exists() {
            return path;
        }
    }

    manifest_dir.join("target").join("debug").join("incan")
}

fn run_incan(current_dir: &Path, args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
    Ok(Command::new(incan_binary())
        .args(args)
        .current_dir(current_dir)
        .env("CARGO_NET_OFFLINE", "true")
        .env("INCAN_NO_BANNER", "1")
        .env(
            "INCAN_GENERATED_CARGO_TARGET_DIR",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("target/incan_generated_shared_target"),
        )
        .output()?)
}

fn assert_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_failure(output: &Output, context: &str) {
    assert!(
        !output.status.success(),
        "{context} unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_minimal_project(root: &Path, name: &str, extra_manifest: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        root.join("incan.toml"),
        format!(
            r#"[project]
name = "{name}"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"
{extra_manifest}"#
        ),
    )?;

    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"def main() -> None:
  println("cli lifecycle ok")
"#,
    )?;
    Ok(main_path)
}

#[test]
fn requires_incan_allows_compatible_project_commands() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
        r#"[project]
name = "compatible_toolchain_guard"
version = "0.1.0"
requires-incan = ">=0.3,<0.4"

[project.scripts]
main = "src/main.incn"
"#,
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"def main() -> None:
  println("cli lifecycle ok")
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&output, "incan lock with compatible requires-incan");

    Ok(())
}

#[test]
fn requires_incan_rejects_project_aware_commands() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_root = tmp.path();
    let src_dir = project_root.join("src");
    let tests_dir = project_root.join("tests");
    fs::create_dir_all(&src_dir)?;
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        project_root.join("incan.toml"),
        r#"[project]
name = "toolchain_guard"
version = "0.1.0"
requires-incan = ">999.0.0"

[project.scripts]
main = "src/main.incn"
"#,
    )?;
    fs::write(
        src_dir.join("main.incn"),
        r#"def main() -> None:
  println("should not run")
"#,
    )?;
    fs::write(
        tests_dir.join("test_main.incn"),
        r#"from std.testing import test

@test
def test_guard() -> None:
  assert True
"#,
    )?;

    let cases = vec![
        (vec!["lock"], "incan lock"),
        (vec!["build", "src/main.incn"], "incan build"),
        (vec!["run"], "incan run"),
        (vec!["test"], "incan test"),
    ];

    for (args, context) in cases {
        let output = run_incan(project_root, &args)?;
        assert_failure(&output, context);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("does not satisfy requires-incan"),
            "{context} should reject incompatible requires-incan, got:\n{stderr}"
        );
        assert!(
            stderr.contains("project.requires-incan"),
            "{context} should name the project constraint layer, got:\n{stderr}"
        );
    }

    Ok(())
}

#[test]
fn env_requires_incan_is_reported_and_enforced_for_env_run() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_root = tmp.path();
    fs::write(
        project_root.join("incan.toml"),
        r#"[project]
name = "env_toolchain_guard"
version = "0.1.0"

[tool.incan.envs.release]
requires-incan = ">999.0.0"

[tool.incan.envs.release.scripts]
probe = ["incan", "--version"]
"#,
    )?;

    let show_output = run_incan(project_root, &["env", "show", "release"])?;
    assert_success(&show_output, "incan env show release");
    let show_stdout = String::from_utf8_lossy(&show_output.stdout);
    assert!(
        show_stdout.contains("requires-incan: >999.0.0"),
        "env show should report effective constraint, got:\n{show_stdout}"
    );
    assert!(
        show_stdout.contains("unsatisfied"),
        "env show should report compatibility state, got:\n{show_stdout}"
    );

    let dry_run_output = run_incan(project_root, &["env", "run", "release", "probe", "--dry-run"])?;
    assert_success(&dry_run_output, "incan env run release probe --dry-run");
    let dry_run_stdout = String::from_utf8_lossy(&dry_run_output.stdout);
    assert!(
        dry_run_stdout.contains("active Incan:") && dry_run_stdout.contains("unsatisfied"),
        "env dry-run should surface unsatisfied compatibility without spawning, got:\n{dry_run_stdout}"
    );

    let run_output = run_incan(project_root, &["env", "run", "release", "probe"])?;
    assert_failure(&run_output, "incan env run release probe");
    let stderr = String::from_utf8_lossy(&run_output.stderr);
    assert!(
        stderr.contains("env.release.requires-incan"),
        "env run should name the env constraint layer, got:\n{stderr}"
    );

    Ok(())
}

#[test]
fn init_creates_project_scaffold_with_expected_content() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_dir = tmp.path().join("generated_app");

    let output = run_incan(
        tmp.path(),
        &[
            "init",
            project_dir.to_str().ok_or("project path was not valid UTF-8")?,
            "--name",
            "cli_init_app",
            "--description",
            "Generated by CLI integration test",
            "--author",
            "CLI Tester <cli@example.com>",
            "--license",
            "MIT",
            "-y",
        ],
    )?;

    assert_success(&output, "incan init");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Created project 'cli_init_app'"),
        "init summary should name the created project, got:\n{stdout}"
    );

    let manifest = fs::read_to_string(project_dir.join("incan.toml"))?;
    assert!(
        manifest.contains(r#"name = "cli_init_app""#),
        "manifest should include explicit project name"
    );
    assert!(
        manifest.contains(r#"version = "0.1.0""#),
        "manifest should include default version"
    );
    assert!(
        manifest.contains(r#"description = "Generated by CLI integration test""#),
        "manifest should include explicit description"
    );
    assert!(
        manifest.contains(r#"authors = ["CLI Tester <cli@example.com>"]"#),
        "manifest should include explicit author"
    );
    assert!(
        manifest.contains(r#"license = "MIT""#),
        "manifest should include explicit license"
    );
    assert!(
        manifest.contains(r#"main = "src/main.incn""#),
        "manifest should include main script"
    );

    let main = fs::read_to_string(project_dir.join("src").join("main.incn"))?;
    assert!(
        main.contains("Hello from cli_init_app!"),
        "starter main should use the project name"
    );
    assert!(project_dir.join("tests").join("test_main.incn").exists());
    assert!(project_dir.join("README.md").exists());
    assert!(project_dir.join(".gitignore").exists());
    Ok(())
}

#[test]
fn lock_generates_lockfile_for_manifest_project() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "cli_lock_project", "")?;

    let output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;

    assert_success(&output, "incan lock");
    let lock = fs::read_to_string(tmp.path().join("incan.lock"))?;
    assert!(lock.contains("# Auto-generated by Incan - do not edit manually"));
    assert!(lock.contains("[incan]"));
    assert!(
        !lock.contains("generated ="),
        "incan.lock must not include volatile generation timestamps"
    );
    assert!(lock.contains("deps-fingerprint = \"sha256:"));
    assert!(lock.contains("[cargo]"));
    assert!(lock.contains("[[package]]"));

    let second_output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&second_output, "second incan lock");
    let second_lock = fs::read_to_string(tmp.path().join("incan.lock"))?;
    assert_eq!(lock, second_lock, "relocking unchanged inputs must be deterministic");
    Ok(())
}

#[test]
fn lock_preheats_dependency_graph_for_path_dependencies() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let helper_dir = tmp.path().join("preheat_helper");
    fs::create_dir_all(helper_dir.join("src"))?;
    fs::write(
        helper_dir.join("Cargo.toml"),
        "[package]\nname = \"preheat_helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(helper_dir.join("src").join("lib.rs"), "pub fn value() -> i64 { 1 }\n")?;

    let main_path = write_minimal_project(
        tmp.path(),
        "cli_lock_preheat_project",
        r#"
[rust-dependencies.preheat_helper]
path = "preheat_helper"
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::preheat_helper import value

def main() -> None:
  println(str(value()))
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;

    assert_success(&output, "incan lock with dependency preheat");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("preheating Cargo dependencies for generated test harnesses"),
        "lock should explain dependency preheat work, got:\n{stderr}"
    );
    assert!(
        tmp.path()
            .join("target/incan_lock/.incan_dependency_preheat_fingerprint")
            .is_file(),
        "dependency preheat should write a fingerprint stamp"
    );
    Ok(())
}

fn stale_lockfile_without_changing_cargo_payload(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let lock_path = root.join("incan.lock");
    let original = fs::read_to_string(&lock_path)?;
    let stale = original.replace("deps-fingerprint = \"sha256:", "deps-fingerprint = \"sha256:stale");
    fs::write(lock_path, &stale)?;
    Ok(stale)
}

#[test]
fn build_reuses_stale_lockfile_without_rewriting_by_default() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "cli_default_stale_lock_build_project", "")?;

    let lock_output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&lock_output, "incan lock before default build");
    let stale_lock = stale_lockfile_without_changing_cargo_payload(tmp.path())?;

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;

    assert_success(&build_output, "incan build with stale lockfile by default");
    let stderr = String::from_utf8_lossy(&build_output.stderr);
    assert!(
        stderr.contains("warning: incan.lock is out of date; using the existing lock payload without rewriting it"),
        "default build should warn instead of silently refreshing the stale lockfile, got:\n{stderr}"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("incan.lock"))?,
        stale_lock,
        "default build must not rewrite an existing stale incan.lock"
    );
    Ok(())
}

#[test]
fn test_reuses_stale_lockfile_without_rewriting_by_default() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "cli_default_stale_lock_test_project", "")?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        tests_dir.join("test_main.incn"),
        r#"from std.testing import assert_eq

def test_smoke() -> None:
  assert_eq(1, 1)
"#,
    )?;

    let lock_output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&lock_output, "incan lock before default test");
    let stale_lock = stale_lockfile_without_changing_cargo_payload(tmp.path())?;

    let test_output = run_incan(tmp.path(), &["test"])?;

    assert_success(&test_output, "incan test with stale lockfile by default");
    let stderr = String::from_utf8_lossy(&test_output.stderr);
    assert!(
        stderr.contains("warning: incan.lock is out of date; using the existing lock payload without rewriting it"),
        "default test should warn instead of silently refreshing the stale lockfile, got:\n{stderr}"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("incan.lock"))?,
        stale_lock,
        "default test must not rewrite an existing stale incan.lock"
    );
    Ok(())
}

#[test]
fn test_lock_with_path_rust_dependency_stays_fresh_for_test_issue505() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "cli_lock_path_dep_fresh_for_test_project",
        r#"

[rust-dependencies]
tiny_helper = { path = "rust/tiny_helper" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::tiny_helper import plus_one

pub def value() -> int:
  return plus_one(0)

def main() -> None:
  println(value())
"#,
    )?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        tests_dir.join("test_main.incn"),
        r#"from std.testing import assert_eq
from crate.main import value

def test_value() -> None:
  assert_eq(value(), 1)
"#,
    )?;
    let helper_src = tmp.path().join("rust").join("tiny_helper").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("helper src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "tiny_helper"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        "pub fn plus_one(value: i64) -> i64 { value + 1 }\n",
    )?;

    let lock_output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&lock_output, "incan lock with path Rust dependency");

    let test_output = run_incan(tmp.path(), &["test"])?;

    assert_success(&test_output, "incan test after lock with path Rust dependency");
    let stderr = String::from_utf8_lossy(&test_output.stderr);
    assert!(
        !stderr.contains("incan.lock is out of date"),
        "fresh lock should not warn as stale for path Rust dependencies, got:\n{stderr}"
    );
    Ok(())
}

#[test]
fn multi_entrypoint_lock_covers_project_scripts_and_tests_issue505() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "cli_multi_entry_lock_freshness_project",
        r#"
extra = "src/extra.incn"

[rust-dependencies]
tiny_helper = { path = "rust/tiny_helper" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"pub def value() -> int:
  return 1

def main() -> None:
  println(value())
"#,
    )?;
    let extra_path = tmp.path().join("src").join("extra.incn");
    fs::write(
        &extra_path,
        r#"from rust::tiny_helper import plus_one

def main() -> None:
  println(plus_one(1))
"#,
    )?;

    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        tests_dir.join("test_main.incn"),
        r#"from std.serde.json import Serialize
from std.testing import assert_eq
from crate.main import value

model Event with Serialize:
  id: int

def test_value() -> None:
  event = Event(id=1)
  assert_eq(event.to_json(), "{\"id\":1}")
  assert_eq(value(), 1)
"#,
    )?;

    let helper_src = tmp.path().join("rust").join("tiny_helper").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("helper src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "tiny_helper"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        "pub fn plus_one(value: i64) -> i64 { value + 1 }\n",
    )?;

    let assert_no_stale_warning = |output: &Output, context: &str| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("incan.lock is out of date"),
            "{context} should not warn that incan.lock is stale, got:\n{stderr}"
        );
    };

    let default_lock_output = run_incan(tmp.path(), &["lock"])?;
    assert_success(&default_lock_output, "default incan lock");

    let test_after_default_lock = run_incan(tmp.path(), &["test"])?;
    assert_success(&test_after_default_lock, "incan test after default lock");
    assert_no_stale_warning(&test_after_default_lock, "incan test after default lock");

    let extra_after_default_lock = run_incan(
        tmp.path(),
        &["run", extra_path.to_str().ok_or("extra path was not valid UTF-8")?],
    )?;
    assert_success(&extra_after_default_lock, "incan run extra after default lock");
    assert_no_stale_warning(&extra_after_default_lock, "incan run extra after default lock");

    let extra_lock_output = run_incan(
        tmp.path(),
        &["lock", extra_path.to_str().ok_or("extra path was not valid UTF-8")?],
    )?;
    assert_success(&extra_lock_output, "incan lock extra");

    let extra_after_extra_lock = run_incan(
        tmp.path(),
        &["run", extra_path.to_str().ok_or("extra path was not valid UTF-8")?],
    )?;
    assert_success(&extra_after_extra_lock, "incan run extra after extra lock");
    assert_no_stale_warning(&extra_after_extra_lock, "incan run extra after extra lock");

    let test_after_extra_lock = run_incan(tmp.path(), &["test"])?;
    assert_success(&test_after_extra_lock, "incan test after extra lock");
    assert_no_stale_warning(&test_after_extra_lock, "incan test after extra lock");

    Ok(())
}

#[test]
fn run_accepts_generic_rust_param_scenarios_share_one_generated_project() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "cli_generic_rust_param_scenarios",
        r#"

[rust-dependencies]
borrow_helper = { path = "rust/borrow_helper" }
decode_helper = { path = "rust/decode_helper" }
decode_trait_helper = { path = "rust/decode_trait_helper" }
prost = { path = "rust/prost" }
prost-types = { path = "rust/prost-types" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from borrowed_generic import borrowed_generic_case
from by_value_decode import by_value_decode_case
from cross_crate_decode import cross_crate_decode_case
from trait_by_value_decode import trait_by_value_decode_case

def main() -> None:
  println(borrowed_generic_case())
  println(by_value_decode_case())
  println(trait_by_value_decode_case())
  println(cross_crate_decode_case())
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("borrowed_generic.incn"),
        r#"from rust::borrow_helper import takes_ref

model Payload:
  name: str

pub def borrowed_generic_case() -> str:
  payload = Payload(name="demo")
  return f"borrowed:{takes_ref(payload)}"
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("by_value_decode.incn"),
        r#"from rust::decode_helper import FileDescriptorSet
from rust::std::io import Cursor

pub def by_value_decode_case() -> str:
  mut cursor = Cursor.new(b"abc")
  match FileDescriptorSet.decode(cursor):
    Ok(_) => return "by_value:ok"
    Err(_) => return "by_value:err"
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("trait_by_value_decode.incn"),
        r#"from rust::decode_trait_helper import FileDescriptorSet, Message

pub def trait_by_value_decode_case() -> str:
  encoded = b"abc"
  match FileDescriptorSet.decode(encoded.as_slice()):
    Ok(_) => return "trait_by_value:ok"
    Err(_) => return "trait_by_value:err"
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("cross_crate_decode.incn"),
        r#"from rust::prost import Message
from rust::prost_types import FileDescriptorSet, ProducerPlan

pub def cross_crate_decode_case() -> str:
  producer = ProducerPlan.new()
  encoded = producer.encode_to_vec()
  match FileDescriptorSet.decode(encoded.as_slice()):
    Ok(_) => return "cross_crate:ok"
    Err(_) => return "cross_crate:err"
"#,
    )?;

    let helper_src = tmp.path().join("rust").join("borrow_helper").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("helper src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "borrow_helper"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        "pub fn takes_ref<TValue>(_value: &TValue) -> i64 { 1 }\n",
    )?;
    let helper_src = tmp.path().join("rust").join("decode_helper").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("helper src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "decode_helper"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        r#"pub trait DecodeBuf {}

impl DecodeBuf for std::io::Cursor<Vec<u8>> {}

pub struct DecodeError;

pub struct FileDescriptorSet;

impl FileDescriptorSet {
    pub fn decode<T: DecodeBuf>(_buf: T) -> Result<Self, DecodeError> {
        Ok(Self)
    }
}
"#,
    )?;
    let helper_src = tmp.path().join("rust").join("decode_trait_helper").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("helper src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "decode_trait_helper"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        r#"pub trait DecodeBuf {}

impl DecodeBuf for &[u8] {}

pub struct DecodeError;

pub struct FileDescriptorSet;

pub trait Message: Sized {
    fn decode(_buf: impl DecodeBuf) -> Result<Self, DecodeError>;
}

impl Message for FileDescriptorSet {
    fn decode(_buf: impl DecodeBuf) -> Result<Self, DecodeError> {
        Ok(Self)
    }
}
"#,
    )?;
    let prost_src = tmp.path().join("rust").join("prost").join("src");
    fs::create_dir_all(&prost_src)?;
    fs::write(
        prost_src.parent().ok_or("prost src has no parent")?.join("Cargo.toml"),
        r#"[package]
name = "prost"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        prost_src.join("lib.rs"),
        r#"pub trait Buf {}

impl Buf for &[u8] {}

pub struct DecodeError;

pub trait Message: Sized {
    fn decode(_buf: impl Buf) -> Result<Self, DecodeError>;
}
"#,
    )?;
    let prost_types_src = tmp.path().join("rust").join("prost-types").join("src");
    fs::create_dir_all(&prost_types_src)?;
    fs::write(
        prost_types_src
            .parent()
            .ok_or("prost-types src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "prost-types"
version = "0.1.0"
edition = "2021"

[dependencies]
prost = { path = "../prost" }
"#,
    )?;
    fs::write(
        prost_types_src.join("lib.rs"),
        r#"pub struct ProducerPlan;

impl ProducerPlan {
    pub fn new() -> Self {
        Self
    }

    pub fn encode_to_vec(&self) -> Vec<u8> {
        b"abc".to_vec()
    }
}

pub struct FileDescriptorSet;

impl prost::Message for FileDescriptorSet {
    fn decode(_buf: impl prost::Buf) -> Result<Self, prost::DecodeError> {
        Ok(Self)
    }
}
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &["run", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;

    assert_success(&output, "incan run with batched generic Rust param scenarios");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim(),
        "borrowed:1\nby_value:ok\ntrait_by_value:ok\ncross_crate:ok",
        "expected batched generic Rust param output, got:\n{stdout}"
    );
    Ok(())
}

#[test]
fn build_locked_rejects_stale_lockfile() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "cli_locked_project", "")?;

    let lock_output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&lock_output, "incan lock before locked build");

    fs::write(
        tmp.path().join("incan.toml"),
        r#"[project]
name = "cli_locked_project"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"

[rust-dependencies.serde]
version = "1.0"
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::serde import Serialize

def main() -> None:
  println("cli lifecycle ok")
"#,
    )?;

    let build_output = run_incan(
        tmp.path(),
        &[
            "build",
            "--locked",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
        ],
    )?;

    assert_failure(&build_output, "incan build --locked with stale lockfile");
    let stderr = String::from_utf8_lossy(&build_output.stderr);
    assert!(
        stderr.contains("incan.lock is out of date"),
        "locked build should report stale lockfile, got:\n{stderr}"
    );
    assert!(
        stderr.contains("incan lock"),
        "locked build should tell users how to refresh the lockfile"
    );
    Ok(())
}

#[test]
fn build_frozen_rejects_missing_lockfile() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "cli_frozen_project", "")?;

    let build_output = run_incan(
        tmp.path(),
        &[
            "build",
            "--frozen",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
        ],
    )?;

    assert_failure(&build_output, "incan build --frozen without lockfile");
    let stderr = String::from_utf8_lossy(&build_output.stderr);
    assert!(
        stderr.contains("incan.lock is missing; run `incan lock`"),
        "frozen build should report missing lockfile, got:\n{stderr}"
    );
    assert!(
        !tmp.path().join("incan.lock").exists(),
        "frozen build must not create incan.lock after rejecting a missing lockfile"
    );
    Ok(())
}

#[test]
fn tools_doctor_reports_text_and_json() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;

    let text_output = run_incan(tmp.path(), &["tools", "doctor"])?;
    assert_success(&text_output, "incan tools doctor");
    let text = String::from_utf8_lossy(&text_output.stdout);
    assert!(
        text.contains("Incan tools doctor"),
        "text report should include command heading, got:\n{text}"
    );
    assert!(
        text.contains("PATH incan") && text.contains("PATH incan-lsp"),
        "text report should include PATH resolution sections, got:\n{text}"
    );
    assert!(
        text.contains("editor setup"),
        "text report should include editor recovery guidance, got:\n{text}"
    );
    assert!(
        text.contains("offline readiness"),
        "text report should include offline-readiness diagnostics, got:\n{text}"
    );
    assert!(
        text.contains("advisory local signals only"),
        "offline-readiness text should avoid guaranteeing offline success, got:\n{text}"
    );

    let json_output = run_incan(tmp.path(), &["tools", "doctor", "--format", "json"])?;
    assert_success(&json_output, "incan tools doctor --format json");
    let json: serde_json::Value = serde_json::from_slice(&json_output.stdout)?;
    assert_eq!(
        json.get("version").and_then(serde_json::Value::as_str),
        Some(env!("CARGO_PKG_VERSION"))
    );
    assert!(
        json.get("current_exe").and_then(serde_json::Value::as_str).is_some(),
        "doctor JSON should include current_exe: {json}"
    );
    assert!(
        json.pointer("/path/incan")
            .and_then(serde_json::Value::as_object)
            .is_some(),
        "doctor JSON should include path.incan: {json}"
    );
    assert!(
        json.pointer("/path/incan_lsp")
            .and_then(serde_json::Value::as_object)
            .is_some(),
        "doctor JSON should include path.incan_lsp: {json}"
    );
    assert!(
        json.pointer("/cargo_bin/incan")
            .and_then(serde_json::Value::as_object)
            .is_some(),
        "doctor JSON should include cargo_bin.incan: {json}"
    );
    assert_eq!(
        json.pointer("/editor_setup/literal_path_settings")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        json.pointer("/editor_setup/reload_after_rebuild")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        json.pointer("/offline_readiness/advisory_only")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        json.pointer("/offline_readiness/source_of_truth")
            .and_then(serde_json::Value::as_str),
        Some("Cargo and RFC 020 policy flags")
    );
    assert!(
        matches!(
            json.pointer("/offline_readiness/status")
                .and_then(serde_json::Value::as_str),
            Some("present" | "missing" | "unknown")
        ),
        "doctor JSON should include stable offline-readiness status: {json}"
    );
    assert!(
        json.pointer("/offline_readiness/cargo/available")
            .and_then(serde_json::Value::as_bool)
            .is_some(),
        "doctor JSON should include cargo availability: {json}"
    );
    assert!(
        json.pointer("/offline_readiness/cargo_home/source")
            .and_then(serde_json::Value::as_str)
            .is_some(),
        "doctor JSON should include effective Cargo home source: {json}"
    );
    assert!(
        json.pointer("/offline_readiness/caches/registry_cache/exists")
            .and_then(serde_json::Value::as_bool)
            .is_some(),
        "doctor JSON should include registry cache hints: {json}"
    );
    assert!(
        json.pointer("/offline_readiness/cargo_config/source_replacement_detected")
            .and_then(serde_json::Value::as_bool)
            .is_some(),
        "doctor JSON should include Cargo config source replacement hints: {json}"
    );
    assert!(
        json.pointer("/offline_readiness/next_steps")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|steps| !steps.is_empty()),
        "doctor JSON should include concrete next steps: {json}"
    );
    Ok(())
}

#[test]
fn tools_metadata_api_reports_checked_json() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_dir = tmp.path().join("metadata_app");
    let main_path = write_minimal_project(&project_dir, "metadata_app", "")?;
    fs::write(
        &main_path,
        r#"
pub const LABEL = "metadata"

pub def label() -> str:
    """
    Return the label.

    Returns:
        str: Label text.
    """
    return LABEL
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &[
            "tools",
            "metadata",
            "api",
            project_dir.to_str().ok_or("project path was not valid UTF-8")?,
            "--format",
            "json",
        ],
    )?;
    assert_success(&output, "incan tools metadata api --format json");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        json.pointer("/schema_version").and_then(serde_json::Value::as_u64),
        Some(1)
    );
    assert_eq!(
        json.pointer("/package/name").and_then(serde_json::Value::as_str),
        Some("metadata_app")
    );
    assert_eq!(
        json.pointer("/package/version").and_then(serde_json::Value::as_str),
        Some("0.1.0")
    );
    assert_eq!(
        json.pointer("/modules/0/module_path/0")
            .and_then(serde_json::Value::as_str),
        Some("main")
    );
    assert!(
        json.pointer("/modules/0/declarations")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|decls| decls.len() == 2),
        "expected const and function declarations in metadata JSON: {json}"
    );
    assert_eq!(
        json.pointer("/modules/0/declarations/1/docstring_sections/summary")
            .and_then(serde_json::Value::as_str),
        Some("Return the label.")
    );
    assert_eq!(
        json.pointer("/modules/0/declarations/1/docstring_sections/returns/ty")
            .and_then(serde_json::Value::as_str),
        Some("str")
    );
    Ok(())
}

#[test]
fn tools_metadata_api_reports_docstring_drift() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_dir = tmp.path().join("metadata_docstring_drift_app");
    let src_dir = project_dir.join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        project_dir.join("incan.toml"),
        r#"[project]
name = "metadata_docstring_drift_app"
version = "0.1.0"
"#,
    )?;
    fs::write(
        src_dir.join("metrics.incn"),
        r#"
pub def avg(values: List[float]) -> float:
    """
    Return the arithmetic mean.

    Args:
        missing: Stale argument.

    Returns:
        str: Wrong return type.

    Aliases:
        MissingAvg: Stale public alias.
    """
    return 0.0
"#,
    )?;
    fs::write(
        src_dir.join("lib.incn"),
        r#"
pub from crate.metrics import avg as PublicAvg
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &[
            "tools",
            "metadata",
            "api",
            project_dir.to_str().ok_or("project path was not valid UTF-8")?,
            "--format",
            "json",
        ],
    )?;
    assert_failure(&output, "incan tools metadata api with docstring drift");
    assert!(
        output.stdout.is_empty(),
        "metadata JSON should not be printed when docstring validation fails"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("API docstring drift for `avg`"),
        "expected docstring drift diagnostic heading, got:\n{stderr}"
    );
    assert!(
        stderr.contains("documented parameter `missing` does not exist"),
        "expected stale parameter diagnostic, got:\n{stderr}"
    );
    assert!(
        stderr.contains("documented return type `str` does not match checked return type `float`"),
        "expected return type diagnostic, got:\n{stderr}"
    );
    assert!(
        stderr.contains("documented alias `MissingAvg` does not exist"),
        "expected stale alias diagnostic, got:\n{stderr}"
    );
    Ok(())
}

#[test]
fn tools_metadata_api_reports_public_import_aliases() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_dir = tmp.path().join("metadata_alias_app");
    let src_dir = project_dir.join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        project_dir.join("incan.toml"),
        r#"[project]
name = "metadata_alias_app"
version = "0.1.0"
"#,
    )?;
    fs::write(
        src_dir.join("widgets.incn"),
        r#"
pub model Widget:
    """
    Widget contract.

    Aliases:
        PublicWidget: Re-exported package surface.
    """
    name: str
"#,
    )?;
    fs::write(
        src_dir.join("lib.incn"),
        r#"
pub from crate.widgets import Widget as PublicWidget
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &[
            "tools",
            "metadata",
            "api",
            project_dir.to_str().ok_or("project path was not valid UTF-8")?,
            "--format",
            "json",
        ],
    )?;
    assert_success(&output, "incan tools metadata api --format json");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let declarations = json
        .pointer("/modules")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|module| module.pointer("/declarations").and_then(serde_json::Value::as_array))
        .flatten();
    let alias = declarations
        .filter(|declaration| declaration.pointer("/kind").and_then(serde_json::Value::as_str) == Some("alias"))
        .find(|declaration| declaration.pointer("/name").and_then(serde_json::Value::as_str) == Some("PublicWidget"))
        .ok_or_else(|| format!("expected PublicWidget alias declaration in metadata JSON: {json}"))?;
    assert_eq!(
        alias
            .pointer("/target_path")
            .and_then(serde_json::Value::as_array)
            .map(|segments| segments
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>()),
        Some(vec!["crate", "widgets", "Widget"])
    );
    Ok(())
}

fn write_order_summary_bundle(project_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let contract_dir = project_dir.join("contracts");
    fs::create_dir_all(&contract_dir)?;
    fs::write(
        contract_dir.join("order_summary.json"),
        r#"{
  "schema_version": 1,
  "stable_model_id": "orders.summary",
  "logical_type_name": "OrderSummary",
  "publishable": true,
  "fields": [
    {
      "name": "order_id",
      "type": "str",
      "alias": "orderId",
      "description": "Stable order identifier"
    },
    {
      "name": "total_cents",
      "type": "int"
    },
    {
      "name": "coupon_code",
      "type": "str",
      "nullable": true
    }
  ]
}
"#,
    )?;
    Ok(())
}

#[test]
fn tools_metadata_model_emits_project_contract_model() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_dir = tmp.path().join("contract_model_app");
    write_minimal_project(
        &project_dir,
        "contract_model_app",
        r#"
[tool.incan.metadata]
model-bundles = ["contracts/order_summary.json"]
"#,
    )?;
    write_order_summary_bundle(&project_dir)?;

    let output = run_incan(
        tmp.path(),
        &[
            "tools",
            "metadata",
            "model",
            project_dir.to_str().ok_or("project path was not valid UTF-8")?,
            "OrderSummary",
            "--format",
            "incan",
        ],
    )?;
    assert_success(&output, "incan tools metadata model --format incan");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("pub model OrderSummary:"),
        "expected emitted model, got:\n{stdout}"
    );
    assert!(
        stdout.contains("order_id [alias=\"orderId\", description=\"Stable order identifier\"]: str"),
        "expected field metadata in emitted model, got:\n{stdout}"
    );
    assert!(
        stdout.contains("coupon_code: Option[str]"),
        "expected nullable field projection, got:\n{stdout}"
    );
    Ok(())
}

#[test]
fn tools_metadata_model_materializes_project_bundle_for_run() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_dir = tmp.path().join("contract_model_run_app");
    let main_path = write_minimal_project(
        &project_dir,
        "contract_model_run_app",
        r#"
[tool.incan.metadata]
model-bundles = ["contracts/order_summary.json"]
"#,
    )?;
    write_order_summary_bundle(&project_dir)?;
    fs::write(
        project_dir.join("src").join("orders.incn"),
        r#"
pub def make_order() -> OrderSummary:
    return OrderSummary(order_id="o-1", total_cents=1250, coupon_code=None)

pub def order_wire_name() -> str:
    let row = make_order()
    for info in row.__fields__():
        if info.name == "order_id":
            return str(info.wire_name)
    return ""

pub def order_description() -> str:
    let row = make_order()
    for info in row.__fields__():
        if info.name == "order_id":
            match info.description:
                Some(description) => return str(description)
                None => return ""
    return ""
"#,
    )?;
    fs::write(
        &main_path,
        r#"
from crate.orders import make_order, order_description, order_wire_name

def main() -> None:
    let row = make_order()
    println(row.order_id)
    println(order_wire_name())
    println(order_description())
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &["run", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&output, "incan run with contract-backed model");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("o-1"),
        "expected materialized model value at runtime, got:\n{stdout}"
    );
    assert!(
        stdout.contains("orderId"),
        "expected RFC 021 alias reflection parity for materialized model, got:\n{stdout}"
    );
    assert!(
        stdout.contains("Stable order identifier"),
        "expected RFC 021 description reflection parity for materialized model, got:\n{stdout}"
    );
    Ok(())
}

#[test]
fn tools_metadata_model_reads_built_library_artifact() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_dir = tmp.path().join("contract_model_lib");
    let src_dir = project_dir.join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        project_dir.join("incan.toml"),
        r#"[project]
name = "contract_model_lib"
version = "0.1.0"

[tool.incan.metadata]
model-bundles = ["contracts/order_summary.json"]
"#,
    )?;
    fs::write(
        src_dir.join("lib.incn"),
        r#"
pub def ping() -> str:
    return "pong"
"#,
    )?;
    write_order_summary_bundle(&project_dir)?;

    let build_output = run_incan(&project_dir, &["build", "--lib"])?;
    assert_success(&build_output, "incan build --lib");

    let artifact_path = project_dir
        .join("target")
        .join("lib")
        .join("contract_model_lib.incnlib");
    let output = run_incan(
        tmp.path(),
        &[
            "tools",
            "metadata",
            "model",
            artifact_path.to_str().ok_or("artifact path was not valid UTF-8")?,
            "orders.summary",
            "--format",
            "incan",
        ],
    )?;
    assert_success(&output, "incan tools metadata model from .incnlib");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("pub model OrderSummary:"),
        "expected artifact-backed model, got:\n{stdout}"
    );
    Ok(())
}

#[test]
fn tools_metadata_model_reports_non_introspectable_artifact() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_dir = tmp.path().join("contract_model_lib_without_models");
    let src_dir = project_dir.join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        project_dir.join("incan.toml"),
        r#"[project]
name = "contract_model_lib_without_models"
version = "0.1.0"
"#,
    )?;
    fs::write(
        src_dir.join("lib.incn"),
        r#"
pub def ping() -> str:
    return "pong"
"#,
    )?;

    let build_output = run_incan(&project_dir, &["build", "--lib"])?;
    assert_success(&build_output, "incan build --lib without model metadata");

    let artifact_path = project_dir
        .join("target")
        .join("lib")
        .join("contract_model_lib_without_models.incnlib");
    let output = run_incan(
        tmp.path(),
        &[
            "tools",
            "metadata",
            "model",
            artifact_path.to_str().ok_or("artifact path was not valid UTF-8")?,
            "Missing",
            "--format",
            "incan",
        ],
    )?;
    assert_failure(&output, "incan tools metadata model from non-introspectable .incnlib");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does not carry checked model metadata"),
        "expected non-introspectable artifact diagnostic, got:\n{stderr}"
    );
    Ok(())
}

#[test]
fn fmt_tuple_target_list_comprehension_remains_buildable() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "fmt_tuple_target_list_comp", "")?;
    fs::write(
        &main_path,
        r#"def main() -> None:
  values = ["alpha", "beta"]
  labels: list[str] = [f"{idx}:{value}" for idx, value in enumerate(values)]
"#,
    )?;

    let fmt_output = run_incan(
        tmp.path(),
        &["fmt", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&fmt_output, "incan fmt tuple-target list comprehension");

    let formatted = fs::read_to_string(&main_path)?;
    assert!(
        formatted.contains("for idx, value in enumerate(values)"),
        "formatter should keep tuple comprehension targets unparenthesized, got:\n{formatted}"
    );
    assert!(
        !formatted.contains("for (idx, value) in enumerate(values)"),
        "formatter emitted parser-invalid tuple target parentheses, got:\n{formatted}"
    );

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "incan build after formatting tuple-target list comprehension",
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

    let generated_main = fs::read_to_string(output_dir.join("src/main.rs"))?;
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
        producer_root.join("incan.toml"),
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
fn build_lib_materializes_facade_decorator_metadata_projection_issue695() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let producer_root = tmp.path().join("metadata_registry");
    let src = producer_root.join("src");
    let operators = src.join("functions").join("operators");
    fs::create_dir_all(&operators)?;
    fs::write(
        producer_root.join("incan.toml"),
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

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&build_output, "incan build for public alias issue631");

    let test_path = tests_dir.join("test_alias.incn");
    let test_output = run_incan(
        tmp.path(),
        &["test", test_path.to_str().ok_or("test path was not valid UTF-8")?],
    )?;
    assert_success(&test_output, "incan test for public alias issue631");
    Ok(())
}

#[test]
fn build_frozen_uses_existing_lockfile_without_network() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "cli_frozen_existing_lock_project", "")?;

    let lock_output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&lock_output, "incan lock before frozen build");

    let build_output = run_incan(
        tmp.path(),
        &[
            "build",
            "--frozen",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
        ],
    )?;

    assert_success(&build_output, "incan build --frozen with existing lockfile");
    let stdout = String::from_utf8_lossy(&build_output.stdout);
    assert!(
        stdout.contains("Build successful"),
        "frozen build should complete with the existing lockfile, got:\n{stdout}"
    );
    Ok(())
}

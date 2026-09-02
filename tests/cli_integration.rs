use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

mod support;

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
    run_incan_with_env(current_dir, args, &[])
}

/// Publish a public-library provider before a separate consumer selects its package Loaf. This is intentionally
/// distinct from normal `build --lib`: only the explicit Oven command may create the provider handoff.
fn run_explicit_oven_bake(current_dir: &Path) -> Result<Output, Box<dyn std::error::Error>> {
    run_explicit_oven_bake_with_home(current_dir, None)
}

/// Bake one project while sharing a caller-selected standalone Oven home with its later workspace replay.
fn run_explicit_oven_bake_with_home(
    current_dir: &Path,
    standalone_incan_home: Option<&Path>,
) -> Result<Output, Box<dyn std::error::Error>> {
    let mut command = configured_incan_command(current_dir, &["oven", "bake", "--project", "."]);
    if !support::oven_compiler_suite_is_active()
        && let Some(incan_home) = standalone_incan_home
    {
        command.env("INCAN_HOME", incan_home);
    }
    support::configure_explicit_oven_bake_command(&mut command)?;
    let timing = support::command_timing_started();
    let output = command.output()?;
    support::report_command_timing("incan oven bake --project .", timing);
    Ok(output)
}

/// Copy one checked package handoff without following a symlink outside its fixture. The relocation test needs both the
/// public library output and its immutable package Loaf collection.
fn copy_fixture_directory(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if file_type.is_symlink() {
            return Err(format!("fixture handoff contains symlink {}", entry.path().display()).into());
        }
        if file_type.is_dir() {
            copy_fixture_directory(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), target)?;
        } else {
            return Err(format!("fixture handoff contains unsupported path {}", entry.path().display()).into());
        }
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn fixture_handoff_copy_rejects_symlinks() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    let outside = temp.path().join("outside");
    let destination = temp.path().join("destination");
    fs::create_dir_all(&source)?;
    fs::create_dir_all(&outside)?;
    fs::write(outside.join("artifact"), "must not be copied")?;
    symlink(&outside, source.join("linked-handoff"))?;

    let result = copy_fixture_directory(&source, &destination);
    let Err(error) = result else {
        return Err("fixture handoff copy followed a symlink".into());
    };
    assert!(
        error.to_string().contains("fixture handoff contains symlink"),
        "unexpected symlink rejection: {error}"
    );
    assert!(
        !destination.join("linked-handoff").exists(),
        "fixture handoff copy materialized a symlink target"
    );
    Ok(())
}

/// Run a CLI command with a Cargo executable that records and rejects any launch.
#[cfg(unix)]
fn run_incan_with_failing_cargo_guard(
    current_dir: &Path,
    args: &[&str],
    guard_dir: &Path,
    marker: &Path,
) -> Result<Output, Box<dyn std::error::Error>> {
    run_incan_with_failing_cargo_guard_and_env(current_dir, args, guard_dir, marker, &[])
}

/// Install one Cargo executable that records and rejects any launch.
#[cfg(unix)]
fn install_failing_cargo_guard(guard_dir: &Path, marker: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;

    fs::create_dir_all(guard_dir)?;
    let guard = guard_dir.join("cargo");
    fs::write(
        &guard,
        format!("#!/bin/sh\nprintf cargo > \"{}\"\nexit 97\n", marker.display()),
    )?;
    let mut permissions = fs::metadata(&guard)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&guard, permissions)?;
    Ok(guard)
}

/// Run a guarded CLI command with explicit child-only environment handoffs.
#[cfg(unix)]
fn run_incan_with_failing_cargo_guard_and_env(
    current_dir: &Path,
    args: &[&str],
    guard_dir: &Path,
    marker: &Path,
    envs: &[(&str, &Path)],
) -> Result<Output, Box<dyn std::error::Error>> {
    let _guard = install_failing_cargo_guard(guard_dir, marker)?;
    let mut paths = vec![guard_dir.to_path_buf()];
    if let Some(inherited) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&inherited));
    }
    let mut command = configured_incan_command(current_dir, args);
    command.env("PATH", std::env::join_paths(paths)?);
    for (key, value) in envs {
        command.env(*key, *value);
    }
    let timing = support::command_timing_started();
    let output = command.output()?;
    support::report_command_timing(&format!("incan {} (Cargo guard)", args.join(" ")), timing);
    Ok(output)
}

fn run_incan_with_env(
    current_dir: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
) -> Result<Output, Box<dyn std::error::Error>> {
    run_incan_with_env_and_removed(current_dir, args, envs, &[])
}

fn run_incan_with_env_and_removed(
    current_dir: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
    removed_envs: &[&str],
) -> Result<Output, Box<dyn std::error::Error>> {
    let mut command = configured_incan_command(current_dir, args);
    for key in removed_envs {
        command.env_remove(key);
    }
    let timing = support::command_timing_started();
    let output = command.envs(envs.iter().copied()).output()?;
    support::report_command_timing(&format!("incan {}", args.join(" ")), timing);
    Ok(output)
}

fn configured_incan_command(current_dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(incan_binary());
    command
        .args(args)
        .current_dir(current_dir)
        .env("CARGO_NET_OFFLINE", "true")
        .env("INCAN_NO_BANNER", "1")
        .env(
            "INCAN_STDLIB",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/incan_stdlib/stdlib"),
        )
        .env(
            "INCAN_STDLIB_DIR",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/incan_stdlib/stdlib"),
        );
    if !support::oven_compiler_suite_is_active() {
        command
            .env(
                "INCAN_GENERATED_CARGO_TARGET_DIR",
                support::generated_cargo_target_dir(),
            )
            .env("INCAN_INTERNAL_SDK_PROVIDER_STORE", support::sdk_provider_store())
            // Explicit provider bakes must not contend with a developer's ambient store when this test binary runs
            // outside the suite.
            .env("INCAN_HOME", current_dir.join(".incan-test"));
    }
    command
}

/// Return a Clang executable suitable for a header-only C ABI verifier fixture, when this host has one.
fn c_abi_test_clang() -> Option<String> {
    if let Some(executable) = std::env::var_os("INCAN_C_ABI_CLANG").filter(|value| !value.is_empty()) {
        return Some(executable.to_string_lossy().into_owned());
    }
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("xcrun").args(["--find", "clang"]).output().ok()?;
        if !output.status.success() {
            return None;
        }
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!path.is_empty()).then_some(path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let status = Command::new("clang").arg("--version").status().ok()?;
        status.success().then_some("clang".to_string())
    }
}

/// Run one Unix CLI probe in its own process group so a timed-out recursive subprocess tree can be terminated
/// together. Callers configure the command first, which keeps the watchdog independent of a fixture's environment.
#[cfg(unix)]
fn run_command_with_timeout(
    mut command: Command,
    label: &str,
    timeout: std::time::Duration,
) -> Result<(Output, bool), Box<dyn std::error::Error>> {
    use std::os::unix::process::CommandExt;

    command.process_group(0).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let started = std::time::Instant::now();
    let timing = support::command_timing_started();
    loop {
        if child.try_wait()?.is_some() {
            let output = child.wait_with_output()?;
            support::report_command_timing(&format!("{label} (timeout supervised)"), timing);
            return Ok((output, false));
        }
        if started.elapsed() >= timeout {
            // TERM is best-effort because the group can disappear between the timeout check and this signal. The
            // group-wide KILL below is the authoritative cleanup before any output pipe is reaped.
            let _ = signal_process_group(child.id(), libc::SIGTERM);
            let grace_started = std::time::Instant::now();
            while grace_started.elapsed() < std::time::Duration::from_secs(2) {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            // Always address the full group with SIGKILL after the grace window. The group may contain a descendant
            // that retained the output pipes after the leader exited or ignored SIGTERM.
            if let Err(error) = signal_process_group(child.id(), libc::SIGKILL) {
                let leader_kill = child.kill();
                let leader_wait = child.wait();
                if let Err(kill_error) = leader_kill {
                    return Err(std::io::Error::other(format!(
                        "process-group SIGKILL failed ({error}); leader kill also failed ({kill_error})"
                    ))
                    .into());
                }
                leader_wait?;
                return Err(error.into());
            }
            let kill_started = std::time::Instant::now();
            while kill_started.elapsed() < std::time::Duration::from_secs(2) {
                if child.try_wait()?.is_some() {
                    let output = child.wait_with_output()?;
                    support::report_command_timing(&format!("{label} (timeout supervised)"), timing);
                    return Ok((output, true));
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            return Err("timed-out Incan process group did not exit after SIGKILL".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

/// Send one signal to the complete Unix process group owned by a bounded CLI probe.
#[cfg(unix)]
fn signal_process_group(child_id: u32, signal: libc::c_int) -> std::io::Result<()> {
    let process_group = i32::try_from(child_id).map_err(|error| std::io::Error::other(error.to_string()))?;
    // SAFETY: The child was spawned with its PID as its process-group ID, and negating that validated positive ID
    // targets only the task-owned group. `signal` is one of libc's SIGTERM/SIGKILL constants supplied above.
    let result = unsafe { libc::kill(-process_group, signal) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

#[cfg(unix)]
fn run_incan_with_os_env(
    current_dir: &Path,
    args: &[&str],
    key: &str,
    value: std::ffi::OsString,
) -> Result<Output, Box<dyn std::error::Error>> {
    Ok(configured_incan_command(current_dir, args).env(key, value).output()?)
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

/// Write the canonical interop-only lock projection needed by `inspect interop-plan` without compiling SDK providers.
///
/// Interop-plan inspection re-hashes declared package inputs itself. The command tests below therefore need a valid
/// canonical semantic projection, not the unrelated provider-install work performed by `incan lock`.
fn write_locked_oven_interop_plan(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let manifest = incan::manifest::ProjectManifest::discover(root)?.ok_or("interop fixture manifest was missing")?;
    let interop = incan::oven_interop::locked_oven_interop_targets(&manifest)?;
    let lock = incan::lockfile::IncanLock::new_with_semantic(
        "fixture".to_string(),
        incan::lockfile::CargoFeatureSelection::default(),
        incan::lockfile::SemanticLockState {
            oven: Some(incan::lockfile::LockedOvenState { interop }),
            ..Default::default()
        },
        String::new(),
    );
    lock.write(&root.join("incan.lock"))?;
    Ok(())
}

/// Write one workspace-root interop projection for a selected member without materializing SDK providers.
fn write_locked_workspace_oven_interop_plan(
    workspace_root: &Path,
    member_root: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let manifest = incan::manifest::ProjectManifest::discover(member_root)?
        .ok_or("workspace interop fixture member manifest was missing")?;
    let interop = incan::oven_interop::locked_oven_interop_targets(&manifest)?;
    let member_root = member_root
        .strip_prefix(workspace_root)?
        .to_string_lossy()
        .replace('\\', "/");
    let lock = incan::lockfile::IncanLock::new_with_semantic(
        "fixture".to_string(),
        incan::lockfile::CargoFeatureSelection::default(),
        incan::lockfile::SemanticLockState {
            workspace_members: vec![incan::lockfile::LockedWorkspaceMember {
                member_root,
                sdk: None,
                packages: Vec::new(),
                feature_edges: Vec::new(),
                providers: Vec::new(),
                oven: Some(incan::lockfile::LockedOvenState { interop }),
            }],
            ..Default::default()
        },
        String::new(),
    );
    lock.write(&workspace_root.join("incan.lock"))?;
    Ok(())
}

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

#[test]
fn build_typed_web_extractors_and_scalar_captures_issue867() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "typed_web_extractors", "")?;
    fs::write(
        &main_path,
        r#"import api::routes
from std.web import App

def main() -> None:
  App.run(host="127.0.0.1", port=0)
"#,
    )?;
    let api_dir = tmp.path().join("src/api");
    fs::create_dir_all(&api_dir)?;
    fs::write(
        api_dir.join("routes.incn"),
        r#"from std.web import route, Json, Query, Path, GET, POST
from std.serde import json
import std.async

@derive(json)
model Search:
  q: str

@derive(json)
model Update:
  name: str

@derive(json)
model Reply:
  value: str

@route("/search", methods=[GET])
async def search(query: Query[Search]) -> Json[Reply]:
  return Json(Reply(value=query.q))

@route("/json", methods=[POST])
async def create(body: Json[Update]) -> Json[Reply]:
  return Json(Reply(value=body.name))

@route("/typed/{id}", methods=[GET])
async def typed_path(_: Path[int]) -> Json[Reply]:
  return Json(Reply(value="typed"))

@route("/scalar/{id}", methods=[GET])
async def scalar_path(id: int) -> Json[Reply]:
  return Json(Reply(value=f"{id}"))

@route("/multi/{year}/{month}", methods=[GET])
async def multiple_paths(year: int, month: int) -> Json[Reply]:
  return Json(Reply(value=f"{year}-{month}"))

@route("/mixed/{id}", methods=[POST])
async def mixed(id: int, _query: Query[Search], _body: Json[Update]) -> Json[Reply]:
  return Json(Reply(value=f"{id}"))

@route("/methods", methods=[GET, POST])
async def multiple_methods() -> Json[Reply]:
  return Json(Reply(value="methods"))
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &["build", main_path.to_string_lossy().as_ref(), "--offline"],
    )?;
    assert_success(&output, "typed web extractor build");

    let generated_root = tmp.path().join("target/incan/typed_web_extractors/src");
    let generated_main = fs::read_to_string(generated_root.join("main.rs"))?;
    let generated_routes = fs::read_to_string(generated_root.join("api/routes.rs"))?;
    let generated = format!("{generated_main}\n{generated_routes}");
    let compact_generated = generated
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    assert!(
        generated.contains("\"/typed/{id}\""),
        "generated route must retain Axum 0.8 capture syntax"
    );
    assert!(
        compact_generated.contains("Query<Search>") && compact_generated.contains("Json<Update>"),
        "generated typed request extractors must retain their concrete types"
    );
    assert!(
        generated.contains("\"/multi/{year}/{month}\"")
            && !compact_generated.contains("Query<_>")
            && !compact_generated.contains("Json<_>"),
        "generated multiple captures must retain Axum 0.8 syntax without inferred item signatures"
    );
    Ok(())
}

#[test]
fn run_synchronous_result_main_issue843() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "synchronous_result_main", "")?;
    fs::write(
        &main_path,
        r#"def run() -> Result[None, str]:
  println("fallible entrypoint")
  return Ok(None)

def main() -> Result[None, str]:
  run()?
  return Ok(None)
"#,
    )?;

    let output = run_incan(tmp.path(), &["run", main_path.to_str().ok_or("non-utf8 main path")?])?;
    assert_success(&output, "incan run with a synchronous Result-returning main");
    assert_eq!(String::from_utf8(output.stdout)?, "fallible entrypoint\n");
    Ok(())
}

#[test]
fn rust_std_io_trait_interop_borrows_receivers_and_propagates_results_issues878_888()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "rust_std_io_trait_interop",
        r#"

[rust-dependencies]
console_interop = { path = "rust/console_interop" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::std::io import Error as IoError, Read, stdin, stdout
from rust::console_interop import EnterAlternateScreen, ExecutableCommand

def enter_alternate_screen() -> Result[None, IoError]:
    mut output = stdout()
    _ = output.execute(EnterAlternateScreen)?
    return Ok(None)

def inspect_alternate_screen_result() -> None:
    mut output = stdout()
    match output.execute(EnterAlternateScreen):
        Ok(_) => pass
        Err(_) => pass

def main() -> None:
    mut input = stdin()
    _ = Read.by_ref(input)
    inspect_alternate_screen_result()
    match enter_alternate_screen():
        Ok(_) => pass
        Err(_) => pass
"#,
    )?;
    let helper_src = tmp.path().join("rust/console_interop/src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("console interop source directory had no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "console_interop"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        r#"use std::io::{self, Write};

pub struct EnterAlternateScreen;

pub trait Command {}

impl Command for EnterAlternateScreen {}

pub trait ExecutableCommand {
    fn execute(&mut self, command: impl Command) -> io::Result<&mut Self>;
}

impl<W: Write + ?Sized> ExecutableCommand for W {
    fn execute(&mut self, _command: impl Command) -> io::Result<&mut Self> {
        Ok(self)
    }
}
"#,
    )?;
    let main_arg = main_path.to_str().ok_or("non-utf8 main path")?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake_output, "explicit Oven bake for direct std-I/O trait interop");
    let build_output = run_incan(tmp.path(), &["build", main_arg, "--locked"])?;
    assert_success(&build_output, "incan build for direct std-I/O trait interop");

    let generated = fs::read_to_string(tmp.path().join("target/incan/rust_std_io_trait_interop/src/main.rs"))?;
    assert!(
        generated.contains("Read::by_ref(&mut input)"),
        "owned Rust trait receiver must be borrowed without dereference:\n{generated}"
    );
    assert!(
        !generated.contains("Read::by_ref(&mut *input)"),
        "owned Rust trait receiver must not use the guard reborrow shape:\n{generated}"
    );
    let compact = generated.split_whitespace().collect::<String>();
    assert!(
        compact.contains("output.execute(EnterAlternateScreen)?"),
        "generated Rust must preserve the fallible extension-trait call:\n{generated}"
    );
    assert!(
        compact.contains("matchoutput.execute(EnterAlternateScreen)"),
        "generated Rust must preserve direct Result inspection for the extension-trait call:\n{generated}"
    );
    Ok(())
}

#[test]
fn rust_trait_object_method_arguments_borrow_by_metadata_issue832() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "rust_trait_object_borrow_arguments",
        r#"

[rust-dependencies]
duck_adapter = { path = "rust/duck_adapter" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::duck_adapter import InterleavedOwned, Processor

def main() -> None:
  mut processor = Processor.new()
  input_frames: usize = 3
  empty_frames: usize = 0
  input = InterleavedOwned.new(input_frames)
  mut output = InterleavedOwned.new(empty_frames)
  println(processor.process_into_buffer(input, output))
"#,
    )?;

    let helper_src = tmp.path().join("rust").join("duck_adapter").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("duck adapter source directory had no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "duck_adapter"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        r#"pub trait Adapter {
    fn frames(&self) -> usize;
}

pub trait AdapterMut: Adapter {
    fn set_frames(&mut self, frames: usize);
}

pub struct InterleavedOwned {
    frames: usize,
}

impl InterleavedOwned {
    pub fn new(frames: usize) -> Self {
        Self { frames }
    }
}

impl Adapter for InterleavedOwned {
    fn frames(&self) -> usize {
        self.frames
    }
}

impl AdapterMut for InterleavedOwned {
    fn set_frames(&mut self, frames: usize) {
        self.frames = frames;
    }
}

pub struct Processor;

impl Processor {
    pub fn new() -> Self {
        Self
    }

    pub fn process_into_buffer(&mut self, input: &dyn Adapter, output: &mut dyn AdapterMut) -> usize {
        output.set_frames(input.frames());
        output.frames()
    }
}
"#,
    )?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(
        &bake_output,
        "explicit Oven bake for Rust trait-object method argument borrowing",
    );
    let output = run_incan(tmp.path(), &["run"])?;
    assert_success(&output, "Rust trait-object method argument borrowing");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "3");

    let generated = fs::read_to_string(
        tmp.path()
            .join("target/incan/rust_trait_object_borrow_arguments/src/main.rs"),
    )?;
    let compact_generated: String = generated.chars().filter(|ch| !ch.is_whitespace()).collect();
    assert!(
        compact_generated.contains("process_into_buffer(&input,&mutoutput)"),
        "trait-object argument borrows must survive generated Rust:\n{generated}"
    );
    Ok(())
}

#[test]
fn rust_concrete_reference_arguments_borrow_by_metadata_issue861() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "rust_concrete_reference_arguments",
        r#"

[rust-dependencies]
mut_ref_probe = { path = "rust/mut_ref_probe" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::mut_ref_probe import Header, Writer

def main() -> None:
  mut writer = Writer.new()
  mut header = Header.new()
  println(writer.mutate(header, 1))
  println(writer.view_value(header))
"#,
    )?;

    let helper_src = tmp.path().join("rust").join("mut_ref_probe").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("mutable-reference probe source directory had no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "mut_ref_probe"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        r#"pub struct Header {
    value: usize,
}

impl Header {
    pub fn new() -> Self {
        Self { value: 0 }
    }
}

pub mod writer {
    use super::Header;

    pub struct Writer;

    impl Writer {
        pub fn new() -> Self {
            Self
        }

        pub fn mutate<T>(&mut self, header: &mut Header, _value: T) -> usize {
            header.value += 1;
            header.value
        }

        pub fn view_value(&self, header: &Header) -> usize {
            header.value
        }
    }
}

pub use writer::Writer;
"#,
    )?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(
        &bake_output,
        "explicit Oven bake for Rust concrete-reference argument borrowing",
    );
    let output = run_incan(tmp.path(), &["run"])?;
    assert_success(&output, "Rust concrete-reference argument borrowing");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "1\n1\n");

    let generated = fs::read_to_string(
        tmp.path()
            .join("target/incan/rust_concrete_reference_arguments/src/main.rs"),
    )?;
    let compact_generated: String = generated.chars().filter(|ch| !ch.is_whitespace()).collect();
    assert!(
        compact_generated.contains("writer.mutate(&mutheader,1)"),
        "generic method concrete mutable-reference argument must preserve its generated Rust borrow:\n{generated}"
    );
    assert!(
        compact_generated.contains("writer.view_value(&header)"),
        "concrete shared-reference argument must preserve its generated Rust borrow:\n{generated}"
    );
    Ok(())
}

fn parse_json_stdout(output: &Output) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn parse_jsonl_stdout(output: &Output) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error>> {
    let stdout = String::from_utf8(output.stdout.clone())?;
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| Ok(serde_json::from_str(line)?))
        .collect()
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
    let inventory = incan::provider::SdkInventory::read_from_path(&inventory_path)?;
    inventory.validate_compiler_version(incan::version::INCAN_VERSION)?;
    assert!(
        inventory.components.values().all(|component| component.available),
        "the reused full-profile provider identity must contain every component"
    );
    let workspace_lock: toml::Value = toml::from_str(&fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock"),
    )?)?;
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
    let main_rust = fs::read_to_string(output_dir.join("src/main.rs"))?;
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
        producer_root.join("incan.toml"),
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
        producer_root.join("incan.toml"),
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
fn workspace_inspect_reports_deterministic_scope_and_stale_member_locks() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("incan.toml"),
        r#"
[project]
name = "root"

[workspace]
members = ["packages/*"]
default-members = ["zebra", "alpha"]
"#,
    )?;
    for name in ["alpha", "zebra"] {
        let member_root = root.path().join("packages").join(name);
        fs::create_dir_all(member_root.join("src"))?;
        fs::write(
            member_root.join("incan.toml"),
            format!("[project]\nname = \"{name}\"\n"),
        )?;
    }
    fs::write(root.path().join("packages/zebra/incan.lock"), "obsolete member lock")?;

    let default_output = run_incan(root.path(), &["workspace", "inspect", "--format", "json"])?;
    assert_success(&default_output, "workspace inspect from root");
    let default_report = parse_json_stdout(&default_output)?;
    assert_eq!(default_report["schema_version"], 1);
    assert_eq!(default_report["selected_scope"]["origin"], "default_members");
    assert_eq!(default_report["selected_scope"]["members"][0]["name"], "alpha");
    assert_eq!(default_report["selected_scope"]["members"][1]["name"], "zebra");
    assert_eq!(
        default_report["lock"]["stale_member_local_locks"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    let current_member_output = run_incan(
        &root.path().join("packages/zebra/src"),
        &["workspace", "inspect", "--format", "json"],
    )?;
    assert_success(&current_member_output, "workspace inspect from member");
    let current_member_report = parse_json_stdout(&current_member_output)?;
    assert_eq!(current_member_report["selected_scope"]["origin"], "current_member");
    assert_eq!(current_member_report["selected_scope"]["members"][0]["name"], "zebra");

    let all_output = run_incan(
        root.path(),
        &["workspace", "inspect", "--format", "json", "--workspace"],
    )?;
    assert_success(&all_output, "workspace inspect --workspace");
    let all_report = parse_json_stdout(&all_output)?;
    assert_eq!(all_report["selected_scope"]["origin"], "workspace");
    assert_eq!(
        all_report["selected_scope"]["members"].as_array().map(Vec::len),
        Some(3)
    );
    Ok(())
}

#[test]
fn workspace_lock_is_published_once_at_the_root_from_any_member() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("incan.toml"),
        r#"
[workspace]
members = ["packages/*"]

[workspace.rust-dependencies]
itoa = "1"
"#,
    )?;
    for (name, version) in [("alpha", "1.2.3"), ("zebra", "4.5.6")] {
        let member_root = root.path().join("packages").join(name);
        fs::create_dir_all(member_root.join("src"))?;
        fs::write(
            member_root.join("incan.toml"),
            format!(
                "[project]\nname = \"{name}\"\nversion = \"{version}\"\n\n[project.scripts]\nmain = \"src/main.incn\"\n\n[project.features]\ndefault = [\"{name}\"]\n{name} = []\n{}",
                if name == "alpha" {
                    "\n[rust-dependencies]\nitoa = { workspace = true }\n"
                } else {
                    ""
                },
            ),
        )?;
        fs::write(
            member_root.join("src/main.incn"),
            if name == "alpha" {
                "from rust::itoa import Buffer\n\ndef main() -> None:\n  println(\"workspace lock\")\n"
            } else {
                "def main() -> None:\n  println(\"workspace lock\")\n"
            },
        )?;
        fs::create_dir_all(member_root.join("tests"))?;
        fs::write(
            member_root.join("tests/test_member.incn"),
            format!("from std.testing import test\n\n@test\ndef test_{name}() -> None:\n  assert True\n"),
        )?;
    }

    let output = run_incan(&root.path().join("packages/alpha"), &["lock"])?;
    assert_success(&output, "incan lock from workspace member");
    let root_lock = root.path().join("incan.lock");
    assert!(root_lock.is_file(), "workspace root lock was not written");
    let lock = incan::lockfile::IncanLock::load(&root_lock)?;
    assert_eq!(
        lock.cargo_lock_payload, "version = 4\n",
        "normal Oven lock publication must retain only the inert legacy Cargo payload"
    );
    let member_roots = lock
        .semantic
        .workspace_members
        .iter()
        .map(|member| member.member_root.as_str())
        .collect::<Vec<_>>();
    assert_eq!(member_roots, vec!["packages/alpha", "packages/zebra"]);
    let member_features = lock
        .semantic
        .workspace_members
        .iter()
        .map(|member| {
            member
                .packages
                .first()
                .map(|package| package.active_features.iter().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        member_features,
        vec![
            vec!["alpha".to_string(), "default".to_string()],
            vec!["default".to_string(), "zebra".to_string()]
        ]
    );
    let inspect_output = run_incan(
        &root.path().join("packages/alpha"),
        &["workspace", "inspect", "--format", "json", "--workspace"],
    )?;
    assert_success(&inspect_output, "workspace inspect after semantic lock publication");
    let inspect_report = parse_json_stdout(&inspect_output)?;
    assert_eq!(
        inspect_report["lock"]["state"]["semantic"]["workspace_members"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert!(
        !root.path().join("packages/alpha/incan.lock").exists()
            && !root.path().join("packages/zebra/incan.lock").exists(),
        "workspace members must not receive authoritative lockfiles"
    );
    for (name, _) in [("alpha", "1.2.3"), ("zebra", "4.5.6")] {
        let member_root = root.path().join("packages").join(name);
        let bake_output = run_explicit_oven_bake(&member_root)?;
        assert_success(&bake_output, &format!("explicit Oven bake for workspace member {name}"));
        let build_output = run_incan(&member_root, &["build", "--locked"])?;
        assert_success(
            &build_output,
            &format!("incan build --locked from workspace member {name}"),
        );
        assert!(
            root.path()
                .join("packages")
                .join(name)
                .join("target/incan")
                .join(name)
                .join("oven/release")
                .join(name)
                .is_file(),
            "workspace member {name} did not emit its caller-owned Oven binary"
        );
    }

    // The member lock, explicit member builds, and aggregate workspace build
    // all operate on this one canonical topology. Retain each command mode,
    // but do not recreate the same members solely to inspect the JSON fan-out.
    let aggregate_output = run_incan(root.path(), &["build", "--workspace", "--report", "json"])?;
    assert_success(&aggregate_output, "workspace build --workspace --report json");
    let aggregate_report = parse_json_stdout(&aggregate_output)?;
    assert_eq!(aggregate_report["schema_version"], "incan.workspace.build.v1");
    assert_eq!(aggregate_report["ok"], true);
    assert_eq!(aggregate_report["workspace"]["selected_scope"]["origin"], "workspace");
    assert_eq!(aggregate_report["results"][0]["member"]["name"], "alpha");
    assert_eq!(aggregate_report["results"][1]["member"]["name"], "zebra");
    assert_eq!(
        aggregate_report["results"][0]["report"]["workspace"]["member_name"],
        "alpha"
    );
    assert_eq!(
        aggregate_report["results"][1]["report"]["workspace"]["member_name"],
        "zebra"
    );
    assert!(
        aggregate_report["results"][0]["report"]["dependencies"]["rust"]
            .as_array()
            .is_some_and(|dependencies| dependencies.iter().any(|dependency| dependency["crate_name"] == "itoa"))
    );

    let test_output = run_incan(root.path(), &["test", "--workspace", "--format", "json"])?;
    assert_success(&test_output, "workspace test --workspace --format json");
    let test_records = parse_jsonl_stdout(&test_output)?;
    assert_eq!(test_records[0]["event"], "workspace_scope");
    assert_eq!(test_records[0]["workspace"]["selected_scope"]["origin"], "workspace");
    let tested_members = test_records
        .iter()
        .filter(|record| record.get("test_id").is_some())
        .map(|record| record["workspace"]["member"]["name"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(tested_members, vec!["alpha", "zebra"]);
    assert!(
        test_records
            .iter()
            .filter(|record| record.get("summary").is_some())
            .all(|record| record["workspace"]["root"].is_string())
    );
    Ok(())
}

#[test]
fn workspace_root_library_without_a_script_publishes_the_canonical_lock_issue997()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(
        root.path().join("incan.toml"),
        r#"[project]
name = "root-library"
version = "0.1.0"

[workspace]
members = ["packages/member"]
"#,
    )?;
    fs::write(
        root.path().join("src/lib.incn"),
        "pub def answer() -> int:\n  return 42\n",
    )?;

    let member = root.path().join("packages/member");
    fs::create_dir_all(member.join("src"))?;
    fs::write(
        member.join("incan.toml"),
        "[project]\nname = \"member-library\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        member.join("src/lib.incn"),
        "pub def member_answer() -> int:\n  return 7\n",
    )?;

    let output = run_incan(root.path(), &["lock"])?;
    assert_success(&output, "rooted workspace lock without scripts");

    let root_lock = root.path().join("incan.lock");
    assert!(root_lock.is_file(), "rooted workspace lock was not written");
    let lock = incan::lockfile::IncanLock::load(&root_lock)?;
    let roots = lock
        .semantic
        .workspace_members
        .iter()
        .map(|member| member.member_root.as_str())
        .collect::<Vec<_>>();
    assert_eq!(roots, vec!["", "packages/member"]);
    assert!(
        !member.join("incan.lock").exists(),
        "a workspace member must not receive a second authoritative lock"
    );
    Ok(())
}

#[test]
fn rooted_workspace_semantic_lock_is_relocation_stable_issue906() -> Result<(), Box<dyn std::error::Error>> {
    fn create_locked_workspace(
        root: &Path,
        prebuilt_artifact: &Path,
    ) -> Result<incan::lockfile::IncanLock, Box<dyn std::error::Error>> {
        fs::create_dir_all(root.join("src"))?;
        fs::write(
            root.join("incan.toml"),
            r#"[project]
name = "root_lib"
version = "0.1.0"

[workspace]
members = ["consumer"]
default-members = ["root_lib", "consumer"]

[workspace.dependencies]
root_lib = { path = "." }
"#,
        )?;
        fs::write(root.join("src/lib.incn"), "pub def answer() -> int:\n  return 42\n")?;
        let artifact = root.join("target/lib");
        fs::create_dir_all(artifact.join("src"))?;
        for relative in ["Cargo.toml", "root_lib.incnlib", "src/lib.rs"] {
            fs::copy(prebuilt_artifact.join(relative), artifact.join(relative))?;
        }
        copy_fixture_directory(&prebuilt_artifact.join("oven"), &artifact.join("oven"))?;
        let consumer = root.join("consumer");
        fs::create_dir_all(consumer.join("src"))?;
        fs::write(
            consumer.join("incan.toml"),
            r#"[project]
name = "consumer"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"

[dependencies]
root_lib = { workspace = true }
"#,
        )?;
        fs::write(
            consumer.join("src/main.incn"),
            "from pub::root_lib import answer\n\n\ndef main() -> None:\n  println(answer())\n",
        )?;

        let lock_output = run_incan(root, &["lock"])?;
        assert_success(&lock_output, "rooted workspace lock generation");
        Ok(incan::lockfile::IncanLock::load(&root.join("incan.lock"))?)
    }

    let temp = tempfile::tempdir()?;
    let producer = temp.path().join("prebuilt/root_lib");
    fs::create_dir_all(producer.join("src"))?;
    fs::write(
        producer.join("incan.toml"),
        r#"[project]
name = "root_lib"
version = "0.1.0"

"#,
    )?;
    fs::write(producer.join("src/lib.incn"), "pub def answer() -> int:\n  return 42\n")?;
    let library_output = run_explicit_oven_bake(&producer)?;
    assert_success(
        &library_output,
        "standalone root library explicit Oven bake before workspace activation",
    );

    let first = create_locked_workspace(&temp.path().join("first/root_lib"), &producer.join("target/lib"))?;
    let second = create_locked_workspace(&temp.path().join("relocated/root_lib"), &producer.join("target/lib"))?;

    assert_eq!(first.semantic, second.semantic);
    assert_eq!(first.deps_fingerprint, second.deps_fingerprint);
    let consumer = first
        .semantic
        .workspace_members
        .iter()
        .find(|member| member.member_root == "consumer")
        .ok_or("consumer semantic graph missing")?;
    assert!(
        consumer
            .packages
            .iter()
            .any(|package| package.package == "root_lib" && package.project_root.is_empty()),
        "the root package should use the workspace-root coordinate"
    );
    assert!(
        consumer
            .packages
            .iter()
            .any(|package| package.package == "consumer" && package.project_root == "consumer"),
        "the selected member package should use its workspace-relative coordinate"
    );
    assert!(
        consumer
            .feature_edges
            .iter()
            .any(|edge| edge.from == "consumer" && edge.to.is_empty()),
        "the member-to-root dependency edge should be workspace-relative"
    );
    Ok(())
}

#[test]
fn rooted_workspace_member_build_uses_direct_rust_dependencies_issue907() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(
        root.path().join("incan.toml"),
        r#"[project]
name = "root_lib"
version = "0.1.0"

[workspace]
members = ["consumer"]
default-members = ["consumer"]
"#,
    )?;
    fs::write(
        root.path().join("src/lib.incn"),
        "pub def root_marker() -> None:\n  pass\n",
    )?;

    let consumer = root.path().join("consumer");
    fs::create_dir_all(consumer.join("src"))?;
    fs::write(
        consumer.join("incan.toml"),
        r#"[project]
name = "consumer"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"

[rust-dependencies]
itoa = "1"
"#,
    )?;
    fs::write(
        consumer.join("src/main.incn"),
        "from rust::itoa import Buffer\n\n\ndef main() -> None:\n  println(\"direct dependency\")\n",
    )?;

    let bake_output = run_explicit_oven_bake(&consumer)?;
    assert_success(
        &bake_output,
        "explicit Oven bake for rooted workspace member with a direct Rust dependency",
    );
    let output = run_incan(&consumer, &["build", "--no-locked"])?;
    assert_success(&output, "rooted workspace member build with a direct Rust dependency");
    Ok(())
}

#[cfg(unix)]
#[test]
fn rooted_workspace_cold_lock_and_selected_member_preserve_identity_issues908_909_931()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(
        root.path().join("src/lib.incn"),
        "from std.json import JsonValue\n\n\npub def answer() -> int:\n  return 42\n",
    )?;
    fs::write(
        root.path().join("src/main.incn"),
        "def main() -> None:\n  println(\"root executable\")\n",
    )?;

    fs::write(
        root.path().join("incan.toml"),
        r#"[project]
name = "root_lib"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"

[workspace]
members = ["consumer"]
default-members = ["root_lib", "consumer"]

[workspace.dependencies]
root_lib = { path = "." }

[workspace.rust-dependencies]
itoa = "1"
"#,
    )?;
    let consumer = root.path().join("consumer");
    fs::create_dir_all(consumer.join("src"))?;
    fs::write(
        consumer.join("incan.toml"),
        r#"[project]
name = "consumer"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"

[dependencies]
root_lib = { workspace = true }

[rust-dependencies]
regex = "1"
itoa = { workspace = true }
"#,
    )?;
    fs::write(
        consumer.join("src/main.incn"),
        "from pub::root_lib import answer\nfrom rust::regex import Regex\n\n\ndef main() -> None:\n  println(answer())\n",
    )?;
    fs::create_dir_all(consumer.join("tests"))?;
    fs::write(
        consumer.join("tests/test_workspace_rust_dependency.incn"),
        r#"from rust::itoa import Buffer
from std.testing import test


@test
def test_workspace_rust_dependency_is_available() -> None:
    assert True
"#,
    )?;

    let source_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let stdlib = source_root.join("crates/incan_stdlib/stdlib");
    let toolchain_crates = source_root.join("crates");
    let incan_home = root.path().join(".incan-home");
    let provider_store = support::cold_sdk_provider_store_or(&incan_home.join("cache/providers/sdk-v2"));
    let generated_target = support::generated_cargo_target_dir_or(&incan_home.join("generated-target"));
    let configure = |cwd: &Path, args: &[&str]| -> Result<Command, Box<dyn std::error::Error>> {
        let mut command = Command::new(incan_binary());
        command
            .args(args)
            .current_dir(cwd)
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_NO_BANNER", "1")
            .env("INCAN_LOCK_PREHEAT", "1")
            .env("INCAN_SOURCE_ROOT", source_root)
            .env("INCAN_STDLIB", &stdlib)
            .env("INCAN_STDLIB_DIR", &stdlib)
            .env("INCAN_TOOLCHAIN_CRATES_DIR", &toolchain_crates)
            .env("INCAN_HOME", &incan_home)
            .env("INCAN_INTERNAL_SDK_PROVIDER_STORE", &provider_store)
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_target);
        if !support::oven_compiler_suite_is_active() {
            command
                .env_remove("INCAN_SDK_INVENTORY")
                .env_remove("INCAN_INTERNAL_SDK_PROVIDER_PATH_FILE");
        }
        if args == ["oven", "bake", "--project", "."] {
            support::configure_explicit_oven_bake_command(&mut command)?;
        }
        Ok(command)
    };
    let run = |cwd: &Path, args: &[&str]| -> Result<Output, Box<dyn std::error::Error>> {
        Ok(configure(cwd, args)?.output()?)
    };

    assert!(!root.path().join("target").exists());
    assert!(!incan_home.exists());
    assert!(!root.path().join("incan.lock").exists());

    // The same cold fixture covers both #908/#909's selected-root artifact and #931's bounded Oven
    // admission/fixed-point contract. The explicit package bake is the only permitted provider publication step.
    // The one cold publication now covers both library and executable profiles. Keep a bounded watchdog, but give
    // sealed or low-core runners enough headroom that the deliberately consolidated journey is not killed between
    // its library and executable halves.
    let artifact_preparation_timeout = std::thread::available_parallelism()
        .map(|parallelism| {
            if support::oven_compiler_suite_is_active() || parallelism.get() <= 4 {
                std::time::Duration::from_secs(6 * 60)
            } else {
                std::time::Duration::from_secs(3 * 60)
            }
        })
        .unwrap_or_else(|_| std::time::Duration::from_secs(6 * 60));
    let (provider_bake_output, timed_out) = run_command_with_timeout(
        configure(root.path(), &["oven", "bake", "--project", "."])?,
        "rooted workspace explicit provider bake",
        artifact_preparation_timeout,
    )?;
    assert!(
        !timed_out,
        "rooted workspace provider bake exceeded its bounded preparation window\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&provider_bake_output.stdout),
        String::from_utf8_lossy(&provider_bake_output.stderr)
    );
    assert_success(&provider_bake_output, "cold rooted workspace provider publication");

    let lock_path = root.path().join("incan.lock");
    assert!(
        lock_path.is_file(),
        "the explicit project bake must publish the canonical workspace lock before sealing its completed Loaf"
    );
    let first_lock = fs::read(&lock_path)?;
    let parsed = incan::lockfile::IncanLock::load(&lock_path)?;
    assert!(!parsed.deps_fingerprint.is_empty());

    let (first_lock_output, timed_out) = run_command_with_timeout(
        configure(root.path(), &["lock"])?,
        "cold rooted workspace lock",
        artifact_preparation_timeout,
    )?;
    assert!(
        !timed_out,
        "rooted workspace lock exceeded its bounded artifact-preparation window\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&first_lock_output.stdout),
        String::from_utf8_lossy(&first_lock_output.stderr)
    );
    assert_success(
        &first_lock_output,
        "rooted workspace lock fixed point after explicit bake",
    );
    assert_eq!(first_lock, fs::read(&lock_path)?);
    assert!(
        root.path().join("target/lib/root_lib.incnlib").is_file(),
        "the explicit provider bake must materialize the selected root library artifact"
    );
    if support::oven_compiler_suite_is_active() {
        assert!(
            !provider_store.exists(),
            "sealed compiler-suite execution must not publish a mutable per-fixture provider store: {}",
            provider_store.display()
        );
        let inventory_path = std::env::var_os("INCAN_SDK_INVENTORY")
            .map(PathBuf::from)
            .ok_or("compiler-suite workspace lock has no sealed SDK inventory")?;
        assert!(
            inventory_path.is_file(),
            "compiler-suite SDK inventory is not a regular file: {}",
            inventory_path.display()
        );
    } else {
        assert!(
            !provider_store.exists(),
            "normal Oven lock must not publish a mutable SDK provider store: {}",
            provider_store.display()
        );
        assert!(
            incan_home.join("oven/store/v2").is_dir(),
            "cold normal Oven lock did not materialize its selected Loaf into the bounded Oven store"
        );
    }

    let second_lock_output = run(root.path(), &["lock"])?;
    assert_success(&second_lock_output, "second rooted workspace lock fixed point");
    assert_eq!(first_lock, fs::read(&lock_path)?);

    let cargo_guard_dir = root.path().join("cargo-reuse-guard");
    let cargo_marker = root.path().join("cargo-reuse-invoked");
    let cargo_guard = install_failing_cargo_guard(&cargo_guard_dir, &cargo_marker)?;
    for projection in [
        root.path().join("target/lib/oven/package-loafs.json"),
        root.path().join("target/lib/oven/debug/libroot_lib.rlib"),
        root.path().join("target/lib/oven/release/libroot_lib.rlib"),
        root.path().join("target/lib/src/lib.rs"),
        root.path().join("target/incan/root_lib/oven/debug/root_lib"),
        root.path().join("target/incan/root_lib/oven/release/root_lib"),
        root.path().join("target/incan/root_lib/src/main.rs"),
    ] {
        fs::remove_file(&projection)?;
    }
    let mut second_bake = configure(root.path(), &["oven", "bake", "--project", "."])?;
    let mut guarded_path = vec![cargo_guard_dir];
    if let Some(inherited) = std::env::var_os("PATH") {
        guarded_path.extend(std::env::split_paths(&inherited));
    }
    second_bake
        .env("CARGO", &cargo_guard)
        .env("PATH", std::env::join_paths(&guarded_path)?)
        .env("INCAN_SDK_INVENTORY", root.path().join("missing-sdk-inventory.json"))
        .env_remove("INCAN_INTERNAL_SDK_PROVIDER_PATH_FILE");
    let (second_bake_output, timed_out) = run_command_with_timeout(
        second_bake,
        "rooted workspace unchanged explicit bake reuse",
        artifact_preparation_timeout,
    )?;
    assert!(
        !timed_out,
        "unchanged rooted workspace bake exceeded its reuse window\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&second_bake_output.stdout),
        String::from_utf8_lossy(&second_bake_output.stderr)
    );
    assert_success(&second_bake_output, "unchanged rooted workspace project-bake reuse");
    let second_bake_stdout = String::from_utf8_lossy(&second_bake_output.stdout);
    assert_eq!(
        second_bake_stdout.matches("Reused Oven library").count(),
        2,
        "unchanged debug and release profiles must both reuse their completed Loafs:\n{second_bake_stdout}"
    );
    assert_eq!(
        second_bake_stdout.matches("Reused Oven executable").count(),
        2,
        "unchanged mixed projects must reuse both executable profiles without frontend work:\n{second_bake_stdout}"
    );
    for restored in [
        root.path().join("target/lib/oven/package-loafs.json"),
        root.path().join("target/lib/oven/debug/libroot_lib.rlib"),
        root.path().join("target/lib/oven/release/libroot_lib.rlib"),
        root.path().join("target/lib/src/lib.rs"),
        root.path().join("target/incan/root_lib/oven/debug/root_lib"),
        root.path().join("target/incan/root_lib/oven/release/root_lib"),
        root.path().join("target/incan/root_lib/src/main.rs"),
    ] {
        assert!(
            restored.is_file(),
            "unchanged bake did not restore {}",
            restored.display()
        );
    }
    assert!(
        !cargo_marker.exists(),
        "unchanged explicit project bake launched Cargo instead of reusing its sealed Loafs"
    );
    assert_eq!(first_lock, fs::read(&lock_path)?);

    let library_output = run(root.path(), &["build", "--lib", "--member", "root_lib"])?;
    assert_success(&library_output, "rooted workspace library build after lock publication");
    assert_eq!(first_lock, fs::read(&lock_path)?);

    let strict_output = run(root.path(), &["build", "--lib", "--member", "root_lib", "--locked"])?;
    assert_success(&strict_output, "strict rooted workspace library build");
    assert_eq!(first_lock, fs::read(&lock_path)?);
    assert!(
        root.path().join("target/lib/oven/release/libroot_lib.rlib").is_file(),
        "the selected root must materialize a caller-owned direct-rustc library"
    );
    assert!(root.path().join("target/lib/root_lib.incnlib").is_file());

    let consumer_bake_output = run(&consumer, &["oven", "bake", "--project", "."])?;
    assert_success(
        &consumer_bake_output,
        "explicit Oven bake for rooted workspace consumer",
    );
    let consumer_output = run(&consumer, &["run", "src/main.incn", "--locked"])?;
    assert_success(&consumer_output, "consumer of the freshly rebuilt root library");
    assert_eq!(first_lock, fs::read(&lock_path)?);

    // #907 uses the same rooted workspace and canonical lock as #908/#909/#931.
    // Keep its distinct selected-member test command, but do not build a second
    // identical workspace merely to prove inherited Rust dependency selection.
    let inherited_dependency_test = run(
        root.path(),
        &[
            "test",
            "--member",
            "consumer",
            "--locked",
            "--fail-on-empty",
            "tests/test_workspace_rust_dependency.incn",
        ],
    )?;
    assert_success(
        &inherited_dependency_test,
        "rooted workspace selected-member test with an inherited Rust dependency",
    );
    assert_eq!(first_lock, fs::read(&lock_path)?);

    let mut rejected_features = configure(
        root.path(),
        &["build", "--lib", "--member", "root_lib", "--cargo-features", "sentinel"],
    )?;
    rejected_features
        .env("CARGO", &cargo_guard)
        .env("PATH", std::env::join_paths(&guarded_path)?);
    let rejected_features = rejected_features.output()?;
    assert_failure(
        &rejected_features,
        "completed library output with unsupported Cargo feature controls",
    );
    assert!(
        String::from_utf8_lossy(&rejected_features.stderr).contains("do not accept Cargo feature controls"),
        "completed-output selection bypassed the normal Cargo-feature rejection:\n{}",
        String::from_utf8_lossy(&rejected_features.stderr)
    );
    assert!(!cargo_marker.exists());

    let fresh_lock = fs::read_to_string(&lock_path)?;
    let stale_lock = fresh_lock.replace("deps-fingerprint = \"sha256:", "deps-fingerprint = \"sha256:stale");
    assert_ne!(
        fresh_lock, stale_lock,
        "the regression must corrupt canonical lock authority"
    );
    fs::write(&lock_path, &stale_lock)?;
    for (description, args) in [
        (
            "strict completed library build",
            vec!["build", "--lib", "--member", "root_lib", "--locked"],
        ),
        (
            "strict completed library JSON report",
            vec!["build", "--lib", "--member", "root_lib", "--locked", "--report", "json"],
        ),
        (
            "strict completed executable run",
            vec!["run", "src/main.incn", "--member", "root_lib", "--locked"],
        ),
    ] {
        let mut command = configure(root.path(), &args)?;
        command
            .env("CARGO", &cargo_guard)
            .env("PATH", std::env::join_paths(&guarded_path)?);
        let output = command.output()?;
        assert_failure(&output, description);
        let diagnostic = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            diagnostic.contains("workspace incan.lock is out of date"),
            "{description} did not preserve strict-lock diagnostic precedence:\n{diagnostic}"
        );
        assert!(
            !diagnostic.contains("no receipt-compatible Loaf"),
            "{description} inspected stale completed outputs before strict lock authority:\n{diagnostic}"
        );
        assert_eq!(fs::read_to_string(&lock_path)?, stale_lock);
        assert!(!cargo_marker.exists(), "{description} launched Cargo");
    }

    let mut non_strict = configure(root.path(), &["build", "--lib", "--member", "root_lib"])?;
    non_strict
        .env("CARGO", &cargo_guard)
        .env("PATH", std::env::join_paths(&guarded_path)?);
    let non_strict = non_strict.output()?;
    assert_success(
        &non_strict,
        "non-strict completed library build with stale canonical lock",
    );
    let non_strict_diagnostic = format!(
        "{}\n{}",
        String::from_utf8_lossy(&non_strict.stdout),
        String::from_utf8_lossy(&non_strict.stderr)
    );
    assert!(
        non_strict_diagnostic.contains("workspace incan.lock is out of date; continuing without using it"),
        "non-strict stale-lock build did not expose its tolerated-stale authority decision:\n{non_strict_diagnostic}"
    );
    assert_eq!(fs::read_to_string(&lock_path)?, stale_lock);
    assert!(!cargo_marker.exists(), "non-strict stale-lock build launched Cargo");
    Ok(())
}

#[test]
fn locked_build_synthesizes_unreferenced_selected_workspace_member_cargo_root() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(
        root.path().join("incan.toml"),
        r#"[project]
name = "root_lib"
version = "0.1.0"

[workspace]
members = ["leaf", "sibling"]
default-members = ["root_lib", "leaf", "sibling"]
"#,
    )?;
    fs::write(
        root.path().join("src/lib.incn"),
        "pub def root_value() -> int:\n  return 1\n",
    )?;

    let vendor = root.path().join("vendor");
    fs::create_dir_all(&vendor)?;
    fs::write(
        vendor.join("Cargo.toml"),
        "[workspace]\nmembers = [\"foo-v1\", \"foo-v2\"]\n\n[workspace.package]\nversion = \"1.0.0\"\n",
    )?;
    for (directory, version) in [("foo-v1", "1.0.0"), ("foo-v2", "2.0.0")] {
        let package = vendor.join(directory);
        fs::create_dir_all(package.join("src"))?;
        let version = if directory == "foo-v1" {
            "version.workspace = true".to_string()
        } else {
            format!("version = \"{version}\"")
        };
        fs::write(
            package.join("Cargo.toml"),
            format!("[package]\nname = \"foo\"\n{version}\nedition = \"2021\"\n"),
        )?;
        fs::write(package.join("src/lib.rs"), "pub fn value() -> i64 { 1 }\n")?;
    }

    let leaf = root.path().join("leaf");
    fs::create_dir_all(leaf.join("src"))?;
    fs::write(
        leaf.join("incan.toml"),
        r#"[project]
name = "leaf"
version = "0.2.0"

[rust-dependencies.json_alias]
package = "serde_json"
version = "1"

[rust-dependencies.old_flags]
package = "bitflags"
version = "=1.3.2"

[rust-dependencies.foo_old]
package = "foo"
path = "../vendor/foo-v1"
"#,
    )?;
    fs::write(
        leaf.join("src/lib.incn"),
        "from std.json import JsonValue\nfrom rust::json_alias import Value\nfrom rust::old_flags import bitflags\nfrom rust::foo_old import value\n\n\npub def leaf_value() -> int:\n  return 2\n",
    )?;

    let sibling = root.path().join("sibling");
    fs::create_dir_all(sibling.join("src"))?;
    fs::write(
        sibling.join("incan.toml"),
        r#"[project]
name = "sibling"
version = "0.3.0"

[rust-dependencies.new_flags]
package = "bitflags"
version = "=2.11.0"

[rust-dependencies.foo_new]
package = "foo"
path = "../vendor/foo-v2"
"#,
    )?;
    fs::write(
        sibling.join("src/lib.incn"),
        "from std.regex import Regex as StdRegex\nfrom rust::new_flags import bitflags\nfrom rust::foo_new import value\n\n\npub def sibling_value() -> int:\n  return 3\n",
    )?;

    let bake_output = run_explicit_oven_bake_with_home(&leaf, Some(&root.path().join(".incan-test")))?;
    assert_success(
        &bake_output,
        "explicit Oven bake for the selected unreferenced workspace member",
    );
    let canonical = incan::lockfile::IncanLock::load(&root.path().join("incan.lock"))?;
    let member_roots = canonical
        .semantic
        .workspace_members
        .iter()
        .map(|member| member.member_root.as_str())
        .collect::<Vec<_>>();
    assert!(member_roots.contains(&"leaf"));
    assert!(member_roots.contains(&"sibling"));
    let _ = fs::remove_dir_all(root.path().join("target"));
    let _ = fs::remove_dir_all(leaf.join("target"));
    let locked_build = run_incan(root.path(), &["build", "--lib", "--member", "leaf", "--locked"])?;
    assert_success(
        &locked_build,
        "target-free locked build of an unreferenced selected workspace member",
    );

    let inspect = run_incan(&leaf, &["workspace", "inspect", "--format", "json"])?;
    assert_success(&inspect, "inspect selected member dependency authority");
    let inspect = parse_json_stdout(&inspect)?;
    let leaf_member = inspect["members"]
        .as_array()
        .and_then(|members| members.iter().find(|member| member["name"] == "leaf"))
        .ok_or("workspace inspection omitted the selected leaf member")?;
    let rust_dependencies = leaf_member["effective_dependencies"]["rust"]
        .as_object()
        .ok_or("selected member inspection had no effective Rust dependency map")?;
    assert!(rust_dependencies.contains_key("json_alias"));
    assert!(rust_dependencies.contains_key("old_flags"));
    assert!(rust_dependencies.contains_key("foo_old"));
    assert!(
        !rust_dependencies.contains_key("new_flags"),
        "the sibling's direct Rust dependency leaked into selected-member authority"
    );
    assert!(leaf.join("target/lib/oven/release/libleaf.rlib").is_file());
    Ok(())
}

#[cfg(unix)]
#[test]
fn workspace_lock_concurrent_publishers_leave_one_parseable_root_lock() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join(".gitignore"), "target/\n.incan-home/\n")?;
    fs::write(
        root.path().join("incan.toml"),
        "[workspace]\nmembers = [\"packages/*\"]\n",
    )?;
    for name in ["alpha", "zebra"] {
        let member_root = root.path().join("packages").join(name);
        fs::create_dir_all(member_root.join("src"))?;
        fs::write(
            member_root.join("incan.toml"),
            format!(
                "[project]\nname = \"{name}\"\nversion = \"0.1.0\"\n\n[project.scripts]\nmain = \"src/main.incn\"\n"
            ),
        )?;
        fs::write(member_root.join("src/main.incn"), "def main() -> None:\n  pass\n")?;
    }

    let stdlib = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/incan_stdlib/stdlib");
    let generated_target = support::generated_cargo_target_dir();
    let spawn_lock = |member: &str| -> Result<std::process::Child, Box<dyn std::error::Error>> {
        Ok(Command::new(incan_binary())
            .arg("lock")
            .current_dir(root.path().join("packages").join(member))
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_NO_BANNER", "1")
            .env("INCAN_STDLIB", &stdlib)
            .env("INCAN_STDLIB_DIR", &stdlib)
            .env("INCAN_HOME", root.path().join(".incan-home"))
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_target)
            .spawn()?)
    };

    let baseline_output = spawn_lock("alpha")?.wait_with_output()?;
    assert_success(&baseline_output, "baseline workspace lock publisher");
    let run_git = |args: &[&str]| -> Result<Output, Box<dyn std::error::Error>> {
        Ok(Command::new("git").args(args).current_dir(root.path()).output()?)
    };
    let init_output = run_git(&["init", "--quiet"])?;
    assert_success(&init_output, "initialize workspace hygiene fixture");
    let add_output = run_git(&["add", "."])?;
    assert_success(&add_output, "stage workspace hygiene fixture");
    let commit_output = run_git(&[
        "-c",
        "user.name=Incan Tests",
        "-c",
        "user.email=tests@incan.invalid",
        "commit",
        "--quiet",
        "-m",
        "baseline",
    ])?;
    assert_success(&commit_output, "commit workspace hygiene fixture");
    let status_before = run_git(&["status", "--short"])?;
    assert_success(&status_before, "inspect clean workspace fixture before publication");
    assert!(
        status_before.stdout.is_empty(),
        "workspace fixture was not clean before lock publication: {}",
        String::from_utf8_lossy(&status_before.stdout)
    );

    let left = spawn_lock("alpha")?;
    let right = spawn_lock("zebra")?;
    let left_output = left.wait_with_output()?;
    let right_output = right.wait_with_output()?;
    assert_success(&left_output, "first concurrent workspace lock publisher");
    assert_success(&right_output, "second concurrent workspace lock publisher");

    let lock_path = root.path().join("incan.lock");
    let lock = incan::lockfile::IncanLock::load(&lock_path)?;
    assert!(!lock.deps_fingerprint.is_empty());
    assert!(
        root.path()
            .join("target/incan_lock/.incan.lock.publication.lock")
            .is_file(),
        "concurrent publishers must share one stable compiler-owned publication lock"
    );
    assert!(
        !root.path().join(".incan.lock.incan.lock").exists(),
        "concurrent lock publication must not leave a persistent project-root sidecar"
    );
    assert!(
        fs::read_dir(root.path())?
            .filter_map(Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().contains(".incan-stage-")),
        "failed workspace lock publication left a private staging file behind"
    );
    let status_after = run_git(&["status", "--short"])?;
    assert_success(&status_after, "inspect workspace fixture after publication");
    assert!(
        status_after.stdout.is_empty(),
        "incan lock dirtied a clean Git checkout:\n{}",
        String::from_utf8_lossy(&status_after.stdout)
    );
    Ok(())
}

#[test]
fn workspace_fmt_fans_out_in_member_order_without_changing_single_project_semantics()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("incan.toml"),
        "[workspace]\nmembers = [\"packages/*\"]\n",
    )?;
    for name in ["zebra", "alpha"] {
        let member_root = root.path().join("packages").join(name);
        fs::create_dir_all(member_root.join("src"))?;
        fs::write(
            member_root.join("incan.toml"),
            format!("[project]\nname = \"{name}\"\n"),
        )?;
        fs::write(
            member_root.join("src/main.incn"),
            "def main() -> None:\n  println(\"formatted\")\n",
        )?;
    }

    let output = run_incan(root.path(), &["fmt", "--workspace"])?;
    assert_success(&output, "workspace fmt --workspace");
    let stdout = String::from_utf8(output.stdout)?;
    let alpha = stdout
        .find("workspace member alpha")
        .ok_or("alpha formatting output missing")?;
    let zebra = stdout
        .find("workspace member zebra")
        .ok_or("zebra formatting output missing")?;
    assert!(
        alpha < zebra,
        "workspace formatter did not use deterministic member order:\n{stdout}"
    );
    Ok(())
}

#[test]
fn workspace_check_fans_out_with_one_member_scoped_json_report() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("incan.toml"),
        "[workspace]\nmembers = [\"packages/*\"]\n",
    )?;
    for (name, source) in [
        ("zebra", "def main() -> None:\n  println(\"zebra\")\n"),
        ("alpha", "def main() -> None:\n  println(\"alpha\")\n"),
    ] {
        let member_root = root.path().join("packages").join(name);
        fs::create_dir_all(member_root.join("src"))?;
        fs::write(
            member_root.join("incan.toml"),
            format!("[project]\nname = \"{name}\"\n\n[project.scripts]\nmain = \"src/main.incn\"\n"),
        )?;
        fs::write(member_root.join("src/main.incn"), source)?;
    }

    let output = run_incan(root.path(), &["check", "--workspace", "--format", "json"])?;
    assert_success(&output, "workspace check --workspace");
    let report = parse_json_stdout(&output)?;
    assert_eq!(report["schema_version"], "incan.workspace.check.v1");
    assert_eq!(report["ok"], true);
    assert_eq!(report["workspace"]["selected_scope"]["origin"], "workspace");
    assert_eq!(report["results"][0]["member"]["name"], "alpha");
    assert_eq!(report["results"][1]["member"]["name"], "zebra");
    assert_eq!(report["results"][0]["report"]["ok"], true);
    assert_eq!(report["results"][1]["report"]["ok"], true);

    let member_output = run_incan(root.path(), &["check", "--member", "zebra", "--format", "json"])?;
    assert_success(&member_output, "workspace check --member");
    let member_report = parse_json_stdout(&member_output)?;
    assert_eq!(
        member_report["workspace"]["selected_scope"]["origin"],
        "explicit_members"
    );
    assert_eq!(member_report["results"].as_array().map(Vec::len), Some(1));
    assert_eq!(member_report["results"][0]["member"]["name"], "zebra");
    Ok(())
}

#[test]
fn workspace_run_and_version_require_one_explicit_member() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("incan.toml"),
        "[workspace]\nmembers = [\"packages/*\"]\n",
    )?;
    for name in ["zebra", "alpha"] {
        let member_root = root.path().join("packages").join(name);
        fs::create_dir_all(member_root.join("src"))?;
        fs::write(
            member_root.join("incan.toml"),
            format!(
                "[project]\nname = \"{name}\"\nversion = \"0.1.0\"\n\n[project.scripts]\nmain = \"src/main.incn\"\n"
            ),
        )?;
        fs::write(
            member_root.join("src/main.incn"),
            format!("def main() -> None:\n  println(\"{name}\")\n"),
        )?;
    }

    let run_output = run_incan(root.path(), &["run", "--member", "alpha"])?;
    assert_success(&run_output, "workspace run --member alpha");
    assert_eq!(String::from_utf8(run_output.stdout)?, "alpha\n");

    let multi_run = run_incan(root.path(), &["run", "--workspace"])?;
    assert!(
        !multi_run.status.success(),
        "workspace run unexpectedly accepted multiple members"
    );
    assert!(
        String::from_utf8(multi_run.stderr)?.contains("incan run requires exactly one workspace member"),
        "workspace run did not explain the one-member requirement"
    );

    let version_output = run_incan(root.path(), &["version", "patch", "--member", "alpha"])?;
    assert_success(&version_output, "workspace version --member alpha");
    let alpha_manifest = fs::read_to_string(root.path().join("packages/alpha/incan.toml"))?;
    let zebra_manifest = fs::read_to_string(root.path().join("packages/zebra/incan.toml"))?;
    assert!(alpha_manifest.contains("version = \"0.1.1\""));
    assert!(zebra_manifest.contains("version = \"0.1.0\""));
    Ok(())
}

#[test]
fn workspace_env_fragments_are_inherited_only_through_explicit_member_extends() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("incan.toml"),
        r#"
[workspace]
members = ["packages/member"]

[workspace.envs.ci]
env-vars = { ROOT = "1", SHARED = "workspace" }

[workspace.envs.ci.scripts]
test = ["incan", "test"]
"#,
    )?;
    let member_root = root.path().join("packages/member");
    fs::create_dir_all(member_root.join("src"))?;
    fs::write(
        member_root.join("incan.toml"),
        r#"
[project]
name = "member"

[tool.incan.envs.ci]
extends = ["workspace:ci"]
env-vars = { SHARED = "member", MEMBER = "1" }
"#,
    )?;

    let output = run_incan(&member_root, &["env", "show", "ci", "--format", "json"])?;
    assert_success(&output, "workspace env inheritance");
    let report = parse_json_stdout(&output)?;
    assert_eq!(
        report["overlay_chain"],
        serde_json::json!(["project", "default", "workspace:ci", "ci"])
    );
    assert_eq!(report["env_vars"]["ROOT"], "1");
    assert_eq!(report["env_vars"]["SHARED"], "member");
    assert_eq!(report["env_vars"]["MEMBER"], "1");
    assert_eq!(report["scripts"]["test"], serde_json::json!(["incan", "test"]));
    Ok(())
}

/// One normal command covers the passing `std.environ` access matrix.
///
/// These formerly independent fixtures each created a fresh project and
/// repeated the same direct-rustc journey. Their accessor contracts are
/// orthogonal but composable, so failures still name the precise assertion
/// without paying for four copies of normal-command setup.
#[test]
fn run_std_environ_passing_accessors_share_one_program_issues557_rfc089() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "std_environ_access_matrix", "")?;
    fs::write(
        &main_path,
        r#"from std.environ import EnvironError, get, get_as, get_optional, get_or
from std.traits.convert import TryFrom

model EnvPort with TryFrom[str]:
  value: int

  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    port = int(value)
    if port < 1 or port > 65535:
      return Err("port out of range")
    return Ok(EnvPort(value=port))


class EnvLabel with TryFrom[str]:
  pub value: str

  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    if len(value) == 0:
      return Err("label must not be empty")
    return Ok(EnvLabel(value=value))


enum EnvMode with TryFrom[str]:
  Dev
  Prod

  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    if value == "dev":
      return Ok(EnvMode.Dev)
    elif value == "prod":
      return Ok(EnvMode.Prod)
    return Err("unknown mode")


def print_port(label: str, result: Result[Option[EnvPort], EnvironError]) -> None:
  match result:
    Ok(value) =>
      match value:
        Some(port) => println(f"{label}:{port.value}")
        None => println(f"{label}:missing")
    Err(err) => println(f"{label}:{err.kind_name()}:{err.key}")

def require[T with TryFrom[str]](key: str) -> Result[T, EnvironError]:
  match get_as[T](key)?:
    Some(value) => return Ok(value)
    None => return Err(EnvironError.missing(key))

def read_primitive_values() -> Result[None, EnvironError]:
  integer = get_as[int]("INCAN_ENVIRON_INTEGER")?.unwrap_or(0)
  floating = get_as[float]("INCAN_ENVIRON_FLOAT")?.unwrap_or(0.0)
  flag = get_as[bool]("INCAN_ENVIRON_BOOL")?.unwrap_or(false)
  text = get_as[str]("INCAN_ENVIRON_TEXT")?.unwrap_or("missing")
  println(f"{integer}:{floating}:{flag}:{text}")

  match get_as[i8]("INCAN_ENVIRON_I8")?:
    Some(narrow) => println(narrow)
    None => println("missing-i8")

  match get_as[f32]("INCAN_ENVIRON_F32")?:
    Some(narrow_float) => println(f"f32:{narrow_float}")
    None => println("missing-f32")

  i16_value = require[i16]("INCAN_ENVIRON_I16")?
  i32_value = require[i32]("INCAN_ENVIRON_I32")?
  i64_value = require[i64]("INCAN_ENVIRON_I64")?
  i128_value = require[i128]("INCAN_ENVIRON_I128")?
  isize_value = require[isize]("INCAN_ENVIRON_ISIZE")?
  u8_value = require[u8]("INCAN_ENVIRON_U8")?
  u16_value = require[u16]("INCAN_ENVIRON_U16")?
  u32_value = require[u32]("INCAN_ENVIRON_U32")?
  u64_value = require[u64]("INCAN_ENVIRON_U64")?
  u128_value = require[u128]("INCAN_ENVIRON_U128")?
  usize_value = require[usize]("INCAN_ENVIRON_USIZE")?
  f64_value = require[f64]("INCAN_ENVIRON_F64")?
  println(f"widths:{i16_value}:{i32_value}:{i64_value}:{i128_value}:{isize_value}:{u8_value}:{u16_value}:{u32_value}:{u64_value}:{u128_value}:{usize_value}:{f64_value}")

  match get_as[bool]("INCAN_ENVIRON_FALSE")?:
    Some(false) => println("bool:false")
    _ => println("bool:unexpected")

  match get_as[i8]("INCAN_ENVIRON_I8_OVERFLOW"):
    Ok(_) => println("i8-overflow:unexpected")
    Err(error) => println(f"i8-overflow:{error.kind_name()}")

  match get_as[u8]("INCAN_ENVIRON_U8_NEGATIVE"):
    Ok(_) => println("u8-negative:unexpected")
    Err(error) => println(f"u8-negative:{error.kind_name()}")

  match get_as[f64]("INCAN_ENVIRON_BAD_F64"):
    Ok(_) => println("f64-invalid:unexpected")
    Err(error) => println(error.message())

  match get_as[bool]("INCAN_ENVIRON_BAD_BOOL"):
    Ok(_) => println("bool-invalid:unexpected")
    Err(error) => println(f"bool-invalid:{error.kind_name()}")

  match get_as[int]("INCAN_ENVIRON_BAD_INTEGER"):
    Ok(_) => println("unexpected-valid")
    Err(error) => println(f"{error.kind_name()}:{error.key}")

  match get_as[int]("INCAN_ENVIRON_REDACTED"):
    Ok(_) => println("redaction:unexpected")
    Err(error) => println(error.message())

  match get_as[int]("INCAN_ENVIRON_MISSING_INTEGER"):
    Ok(None) => println("missing")
    Ok(Some(_)) => println("unexpected-present")
    Err(_) => println("unexpected-error")
  return Ok(None)

type DefaultPort = newtype int:
  def from_underlying(value: int) -> Result[Self, ValidationError]:
    if value < 1 or value > 65535:
      return Err(ValidationError("port out of range"))
    return Ok(DefaultPort(value))

def main() -> None:
  match get("INCAN_ENVIRON_PRESENT"):
    Ok(value) => println(value)
    Err(err) => println(err.kind_name())
  match get("INCAN_ENVIRON_MISSING_TEST"):
    Ok(value) => println(value)
    Err(err) => println(f"{err.kind_name()}:{err.key}")
  match get_optional("INCAN_ENVIRON_MISSING_TEST"):
    Some(value) => println(value)
    None => println("optional-missing")
  match get_optional("INCAN_ENVIRON_PRESENT"):
    Some(value) => println(f"optional:{value}")
    None => println("optional:unexpected")
  println(get_or("INCAN_ENVIRON_MISSING_TEST", "fallback"))
  println(get_or("INCAN_ENVIRON_PRESENT", "unexpected"))
  match get_optional(""):
    Some(value) => println(value)
    None => println("optional-invalid")
  println(get_or("", "invalid-fallback"))
  match get(""):
    Ok(value) => println(value)
    Err(err) => println(err.kind_name())
  match get("A=B"):
    Ok(value) => println(value)
    Err(err) => println(f"{err.kind_name()}:{err.detail}")
  match get("A\0B"):
    Ok(value) => println(value)
    Err(err) => println(f"nul:{err.kind_name()}")

  print_port("present", get_as[EnvPort]("INCAN_ENVIRON_PORT"))
  print_port("missing", get_as[EnvPort]("INCAN_ENVIRON_MISSING_PORT"))
  print_port("invalid", get_as[EnvPort]("INCAN_ENVIRON_PORT_BAD"))
  print_port("empty", get_as[EnvPort](""))
  match get_as[EnvLabel]("INCAN_ENVIRON_LABEL"):
    Ok(Some(label)) => println(f"label:{label.value}")
    _ => println("label:unexpected")
  match get_as[EnvMode]("INCAN_ENVIRON_MODE"):
    Ok(Some(EnvMode.Prod)) => println("mode:prod")
    _ => println("mode:unexpected")

  match read_primitive_values():
    Ok(_) => pass
    Err(error) => println(error.message())

  match get_as[int]("INCAN_ENVIRON_DEFAULT_MISSING", 8080):
    Ok(value) => println(f"positional:{value}")
    Err(error) => println(f"positional:{error.kind_name()}")

  match get_as[int]("INCAN_ENVIRON_DEFAULT_PRESENT", default=8080):
    Ok(value) => println(f"keyword:{value}")
    Err(error) => println(f"keyword:{error.kind_name()}")

  match get_as[int]("INCAN_ENVIRON_DEFAULT_INVALID", 8080):
    Ok(value) => println(f"invalid:unexpected:{value}")
    Err(error) => println(f"invalid:{error.kind_name()}")

  match get_as[DefaultPort]("INCAN_ENVIRON_DEFAULT_PORT", 5432):
    Ok(port) => println(f"newtype:{port.0}")
    Err(error) => println(f"newtype:{error.kind_name()}")
"#,
    )?;

    let run_output = run_incan_with_env_and_removed(
        tmp.path(),
        &["run", main_path.to_str().ok_or("non-utf8 main path")?],
        &[
            ("INCAN_ENVIRON_PRESENT", "present-value"),
            ("INCAN_ENVIRON_PORT", "5432"),
            ("INCAN_ENVIRON_PORT_BAD", "70000"),
            ("INCAN_ENVIRON_LABEL", "worker"),
            ("INCAN_ENVIRON_MODE", "prod"),
            ("INCAN_ENVIRON_INTEGER", "42"),
            ("INCAN_ENVIRON_FLOAT", "3.5"),
            ("INCAN_ENVIRON_BOOL", "true"),
            ("INCAN_ENVIRON_TEXT", "hello"),
            ("INCAN_ENVIRON_I8", "-8"),
            ("INCAN_ENVIRON_F32", "1.25"),
            ("INCAN_ENVIRON_I16", "-16"),
            ("INCAN_ENVIRON_I32", "-32"),
            ("INCAN_ENVIRON_I64", "-64"),
            ("INCAN_ENVIRON_I128", "-128"),
            ("INCAN_ENVIRON_ISIZE", "-7"),
            ("INCAN_ENVIRON_U8", "8"),
            ("INCAN_ENVIRON_U16", "16"),
            ("INCAN_ENVIRON_U32", "32"),
            ("INCAN_ENVIRON_U64", "64"),
            ("INCAN_ENVIRON_U128", "128"),
            ("INCAN_ENVIRON_USIZE", "7"),
            ("INCAN_ENVIRON_F64", "6.25"),
            ("INCAN_ENVIRON_FALSE", "false"),
            ("INCAN_ENVIRON_I8_OVERFLOW", "128"),
            ("INCAN_ENVIRON_U8_NEGATIVE", "-1"),
            ("INCAN_ENVIRON_BAD_F64", "not-a-float"),
            ("INCAN_ENVIRON_BAD_BOOL", "yes"),
            ("INCAN_ENVIRON_BAD_INTEGER", "not-an-int"),
            ("INCAN_ENVIRON_REDACTED", "secret-value-must-not-appear"),
            ("INCAN_ENVIRON_DEFAULT_PRESENT", "9090"),
            ("INCAN_ENVIRON_DEFAULT_INVALID", "not-an-int"),
        ],
        &[
            "INCAN_ENVIRON_MISSING_TEST",
            "INCAN_ENVIRON_MISSING_PORT",
            "INCAN_ENVIRON_MISSING_INTEGER",
            "INCAN_ENVIRON_DEFAULT_MISSING",
            "INCAN_ENVIRON_DEFAULT_PORT",
        ],
    )?;
    assert_success(&run_output, "incan run for the std.environ passing access matrix");

    assert_eq!(
        String::from_utf8_lossy(&run_output.stdout),
        concat!(
            "present-value\n",
            "missing:INCAN_ENVIRON_MISSING_TEST\n",
            "optional-missing\n",
            "optional:present-value\n",
            "fallback\n",
            "present-value\n",
            "optional-invalid\n",
            "invalid-fallback\n",
            "invalid_key\n",
            "invalid_key:environment variable key must not be empty or contain `=` or NUL\n",
            "nul:invalid_key\n",
            "present:5432\n",
            "missing:missing\n",
            "invalid:invalid_value:INCAN_ENVIRON_PORT_BAD\n",
            "empty:invalid_key:\n",
            "label:worker\n",
            "mode:prod\n",
            "42:3.5:true:hello\n",
            "-8\n",
            "f32:1.25\n",
            "widths:-16:-32:-64:-128:-7:8:16:32:64:128:7:6.25\n",
            "bool:false\n",
            "i8-overflow:invalid_value\n",
            "u8-negative:invalid_value\n",
            "environment variable `INCAN_ENVIRON_BAD_F64` could not be parsed or validated as `the requested type`\n",
            "bool-invalid:invalid_value\n",
            "invalid_value:INCAN_ENVIRON_BAD_INTEGER\n",
            "environment variable `INCAN_ENVIRON_REDACTED` could not be parsed or validated as `the requested type`\n",
            "missing\n",
            "positional:8080\n",
            "keyword:9090\n",
            "invalid:invalid_value\n",
            "newtype:5432\n",
        ),
    );
    assert!(!String::from_utf8_lossy(&run_output.stdout).contains("secret-value-must-not-appear"));
    assert!(!String::from_utf8_lossy(&run_output.stderr).contains("secret-value-must-not-appear"));

    Ok(())
}

#[test]
fn run_std_environ_validated_newtype_accessors_rfc089() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "std_environ_validated_newtypes", "")?;
    fs::write(
        &main_path,
        r#"from std.environ import EnvironError, get_as

type Port = newtype int:
  def from_underlying(value: int) -> Result[Self, ValidationError]:
    if value < 1 or value > 65535:
      return Err(ValidationError("port out of range"))
    return Ok(Port(value))

type Label = newtype str
type Positive = newtype int[gt=0]
type Ratio = newtype float[ge=0, le=1]
type Boxed[T] = newtype T
type BinaryBlob = newtype bytes

trait TryFrom[T]:
  @classmethod
  def try_from(cls, value: T) -> Result[Self, str]: ...

model LocalToken with TryFrom[str]:
  value: str

  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    return Ok(LocalToken(value=value))

def print_port(label: str, result: Result[Option[Port], EnvironError]) -> None:
  match result:
    Ok(Some(port)) => println(f"{label}:{port.0}")
    Ok(None) => println(f"{label}:missing")
    Err(error) => println(f"{label}:{error.kind_name()}:{error.key}")

def main() -> None:
  local = LocalToken(value="local")
  println(local.value)
  print_port("valid", get_as[Port]("INCAN_ENVIRON_VALID_PORT"))
  print_port("invalid", get_as[Port]("INCAN_ENVIRON_INVALID_PORT"))
  print_port("malformed", get_as[Port]("INCAN_ENVIRON_MALFORMED_PORT"))

  match get_as[Label]("INCAN_ENVIRON_LABEL"):
    Ok(Some(label)) => println(f"label:{label.0}")
    Ok(None) => println("label:missing")
    Err(error) => println(f"label:{error.kind_name()}")

  match get_as[Positive]("INCAN_ENVIRON_POSITIVE"):
    Ok(Some(value)) => println(f"positive:{value.0}")
    Ok(None) => println("positive:missing")
    Err(error) => println(f"positive:{error.kind_name()}")

  match get_as[Positive]("INCAN_ENVIRON_NON_POSITIVE"):
    Ok(_) => println("non-positive:unexpected-valid")
    Err(error) => println(f"non-positive:{error.kind_name()}")

  match get_as[Boxed[int]]("INCAN_ENVIRON_BOXED"):
    Ok(Some(value)) => println(f"boxed:{value.0}")
    Ok(None) => println("boxed:missing")
    Err(error) => println(f"boxed:{error.kind_name()}")

  match get_as[Ratio]("INCAN_ENVIRON_RATIO"):
    Ok(Some(value)) => println(f"ratio:{value.0}")
    _ => println("ratio:unexpected")

  match get_as[Ratio]("INCAN_ENVIRON_RATIO_HIGH"):
    Ok(_) => println("ratio-high:unexpected")
    Err(error) => println(f"ratio-high:{error.kind_name()}")
"#,
    )?;

    let run_output = run_incan_with_env_and_removed(
        tmp.path(),
        &["run", main_path.to_str().ok_or("non-utf8 main path")?],
        &[
            ("INCAN_ENVIRON_VALID_PORT", "5432"),
            ("INCAN_ENVIRON_INVALID_PORT", "70000"),
            ("INCAN_ENVIRON_MALFORMED_PORT", "not-a-port"),
            ("INCAN_ENVIRON_LABEL", "service"),
            ("INCAN_ENVIRON_POSITIVE", "9"),
            ("INCAN_ENVIRON_NON_POSITIVE", "0"),
            ("INCAN_ENVIRON_BOXED", "77"),
            ("INCAN_ENVIRON_RATIO", "0.5"),
            ("INCAN_ENVIRON_RATIO_HIGH", "1.5"),
        ],
        &[],
    )?;
    assert_success(&run_output, "incan run for std.environ validated newtypes");

    assert_eq!(
        String::from_utf8(run_output.stdout)?,
        concat!(
            "local\n",
            "valid:5432\n",
            "invalid:invalid_value:INCAN_ENVIRON_INVALID_PORT\n",
            "malformed:invalid_value:INCAN_ENVIRON_MALFORMED_PORT\n",
            "label:service\n",
            "positive:9\n",
            "non-positive:invalid_value\n",
            "boxed:77\n",
            "ratio:0.5\n",
            "ratio-high:invalid_value\n",
        ),
    );

    Ok(())
}

#[test]
fn run_std_environ_invalid_newtype_default_uses_checked_construction() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "std_environ_invalid_newtype_default", "")?;
    fs::write(
        &main_path,
        r#"from std.environ import get_as

type Port = newtype int:
  def from_underlying(value: int) -> Result[Self, ValidationError]:
    if value < 1 or value > 65535:
      return Err(ValidationError("port out of range"))
    return Ok(Port(value))

def main() -> None:
  get_as[Port]("INCAN_ENVIRON_INVALID_DEFAULT_MISSING", default=70000)
"#,
    )?;

    let output = run_incan_with_env_and_removed(
        tmp.path(),
        &["run", main_path.to_str().ok_or("non-utf8 main path")?],
        &[],
        &["INCAN_ENVIRON_INVALID_DEFAULT_MISSING"],
    )?;
    assert_failure(&output, "invalid newtype default checked construction");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validated newtype construction failed") || stderr.contains("port out of range"),
        "expected ordinary checked-construction failure, got:\n{stderr}"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn run_std_environ_reports_non_unicode_through_public_api() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::ffi::OsStringExt;

    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "std_environ_non_unicode", "")?;
    fs::write(
        &main_path,
        r#"from std.environ import get

def main() -> None:
  match get("INCAN_ENVIRON_NON_UNICODE"):
    Ok(_) => println("unexpected")
    Err(error) => println(f"{error.kind_name()}:{error.key}")
"#,
    )?;

    let output = run_incan_with_os_env(
        tmp.path(),
        &["run", main_path.to_str().ok_or("non-utf8 main path")?],
        "INCAN_ENVIRON_NON_UNICODE",
        std::ffi::OsString::from_vec(vec![0xff, b's', b'e', b'c', b'r', b'e', b't']),
    )?;
    assert_success(&output, "non-Unicode std.environ public API probe");
    assert_eq!(
        String::from_utf8(output.stdout)?,
        "not_unicode:INCAN_ENVIRON_NON_UNICODE\n"
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("secret"),
        "non-Unicode environment values must not appear in diagnostics:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn assert_codegraph_record_contract(records: &[serde_json::Value]) {
    assert!(!records.is_empty(), "codegraph export should include a header record");
    assert_eq!(records[0]["record"], serde_json::json!("header"));
    assert_eq!(records[0]["schema_version"], serde_json::json!(7));
    assert_eq!(records[0]["languages"], serde_json::json!(["incan"]));
    assert!(
        records[0]["degraded"].is_boolean(),
        "codegraph snapshot metadata should carry degraded state: {}",
        records[0]
    );

    for record in records.iter().skip(1) {
        assert_eq!(
            record["language"],
            serde_json::json!("incan"),
            "v0.5 codegraph fact records should be explicitly Incan-language facts: {record}"
        );
        assert!(
            record["provenance"].is_string(),
            "codegraph fact records should carry provenance: {record}"
        );
        assert!(
            record["degraded"].is_boolean(),
            "codegraph fact records should carry degraded state: {record}"
        );

        if let Some(span) = record.get("span").filter(|span| span.is_object()) {
            assert_source_span_shape(span, record);
        }
        if let Some(span) = record.get("primary_span").filter(|span| span.is_object()) {
            assert_source_span_shape(span, record);
        }
    }

    assert!(
        records
            .iter()
            .skip(1)
            .all(|record| record["language"] != serde_json::json!("rust")),
        "v0.4 should not emit Rust codegraph facts before first-class Rust support lands"
    );
}

fn assert_source_span_shape(span: &serde_json::Value, record: &serde_json::Value) {
    assert!(
        span["file"].is_string()
            && span["start"].is_number()
            && span["end"].is_number()
            && span["start_line"].is_number()
            && span["start_column"].is_number()
            && span["end_line"].is_number()
            && span["end_column"].is_number(),
        "source-backed codegraph records should carry stable file and span identity: {record}"
    );
}

#[test]
fn semantic_inspection_surfaces_share_project_identity() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
        r#"[project]
name = "semantic_probe"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"
"#,
    )?;
    fs::write(
        src_dir.join("helpers.incn"),
        r#"pub model Widget:
    """A documented value that should appear in reports and graph facts."""
    pub value: int

pub def make_widget(value: int) -> Widget:
    """Create a value for semantic inspection smoke tests."""
    return Widget(value=value)
"#,
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"from helpers import make_widget

pub def entrypoint() -> int:
    return make_widget(42).value

def main() -> None:
    println(f"semantic {entrypoint()}")
"#,
    )?;

    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;
    let check = run_incan(tmp.path(), &["check", main_arg, "--format", "json"])?;
    assert_success(&check, "incan check --format json semantic inspection fixture");
    let check_json = parse_json_stdout(&check)?;
    assert_eq!(check_json["schema_version"], serde_json::json!(2));
    assert_eq!(check_json["ok"], serde_json::json!(true));
    assert_eq!(check_json["diagnostics"], serde_json::json!([]));

    let build = run_incan(tmp.path(), &["build", main_arg, "--offline", "--report", "json"])?;
    assert_success(&build, "incan build --report json semantic inspection fixture");
    let build_json = parse_json_stdout(&build)?;
    // Each report is independently versioned; this test asserts shared *project identity*, not a shared schema
    // number. The check report moved to v2 when it began carrying warnings, while build reports stayed at v1.
    assert_eq!(build_json["schema_version"], serde_json::json!(1));
    assert_eq!(build_json["project"]["name"], serde_json::json!("semantic_probe"));
    assert_source_files_include(&build_json, &["src/main.incn", "src/helpers.incn"])?;

    let inspect = run_incan(tmp.path(), &["inspect", "rust", main_arg, "--format", "json"])?;
    assert_success(&inspect, "incan inspect rust --format json semantic inspection fixture");
    let inspect_json = parse_json_stdout(&inspect)?;
    assert_eq!(inspect_json["schema_version"], build_json["schema_version"]);
    assert_eq!(inspect_json["compiler_version"], build_json["compiler_version"]);
    assert_eq!(
        inspect_json["generated"]["project_path"],
        build_json["generated"]["project_path"]
    );
    assert_eq!(
        inspect_json["generated"]["manifest_path"],
        build_json["generated"]["manifest_path"]
    );
    assert_eq!(
        inspect_json["generated"]["crate_root"],
        build_json["generated"]["crate_root"]
    );
    assert_source_files_include(&inspect_json, &["src/main.incn", "src/helpers.incn"])?;

    let codegraph = run_incan(tmp.path(), &["inspect", "codegraph", main_arg, "--format", "jsonl"])?;
    assert_success(
        &codegraph,
        "incan inspect codegraph --format jsonl semantic inspection fixture",
    );
    let records = parse_jsonl_stdout(&codegraph)?;
    assert_codegraph_record_contract(&records);
    // Codegraph is a separate versioned projection. RFC 113 adds checked registry records under codegraph schema v2,
    // while build reports retain their independently versioned schema.
    assert_eq!(build_json["schema_version"], serde_json::json!(1));
    assert_eq!(records[0]["compiler_version"], build_json["compiler_version"]);
    assert_eq!(records[0]["package"]["name"], serde_json::json!("semantic_probe"));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("file")
            && record["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("src/main.incn"))
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("file")
            && record["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("src/helpers.incn"))
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("declaration")
            && record["kind"] == serde_json::json!("function")
            && record["name"] == serde_json::json!("entrypoint")
            && record["visibility"] == serde_json::json!("public")
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("entrypoint")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["callee"] == serde_json::json!("make_widget")
            && record["provenance"] == serde_json::json!("checked")
    }));

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
fn inspect_bindings_projects_checked_declaration_facts() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
        r#"[project]
name = "binding_inspection"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"

[oven.interop]
schema = 1

[[oven.interop.targets]]
target = "aarch64-apple-darwin"
headers = ["fixture.h"]

[[oven.interop.targets.artifacts]]
name = "fixture"
kind = "system"
capability = "system.fixture"

[[oven.interop.targets.bindings]]
module = ["fixture"]
name = "Fixture"
artifacts = ["fixture"]
"#,
    )?;
    fs::write(
        tmp.path().join("fixture.h"),
        "typedef struct fixture_handle fixture_handle;\ntypedef struct fixture_pair { int left; int right; } fixture_pair;\n#define FIXTURE_OK 0\nint fixture_add(int left, int right);\nvoid fixture_close(fixture_handle *handle);\nint fixture_inspect(fixture_handle *handle);\nint fixture_open(fixture_handle **output, int *attempts);\n",
    )?;
    fs::write(
        src_dir.join("fixture.incn"),
        r#"from std.interop import c

binding Fixture:
    header = "fixture.h"
    link = c.system_library("fixture")

    resource Handle:
        native = "fixture_handle"
        release = close

    symbol close(handle: c.Owned[Handle]) -> None:
        native = "fixture_close"

    symbol inspect(handle: c.Borrowed[Handle]) -> c.i32:
        native = "fixture_inspect"

    symbol add(left: c.i32, right: c.i32) -> c.i32:
        native = "fixture_add"

    enum Status:
        OK: c.i32 = FIXTURE_OK

    symbol open(output: c.Out[c.Owned[Handle]], attempts: c.InOut[c.i32]) -> c.i32:
        native = "fixture_open"

        outcome Status.OK:
            initializes = [output]
            updates = [attempts]

    struct Pair:
        native = "fixture_pair"
        left: c.i32 = left
        right: c.i32 = right

pub def fixture_name() -> str:
    return "fixture"

def private_bridge() -> c.i32:
    left: i32 = 2
    right: i32 = 3
    unsafe:
        return Fixture.add(left, right)

pub def checked_sum() -> c.i32:
    return private_bridge()
"#,
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"from fixture import fixture_name

def main() -> None:
    println(fixture_name())
"#,
    )?;

    let project = tmp.path().to_str().ok_or("project path was not valid UTF-8")?;
    let output = run_incan(tmp.path(), &["inspect", "bindings", project, "--format", "json"])?;
    assert_success(&output, "inspect checked C binding declarations as JSON");
    let report = parse_json_stdout(&output)?;
    assert_eq!(report["schema_version"], serde_json::json!(2));
    let bindings = report["bindings"]
        .as_array()
        .ok_or("binding report did not contain bindings")?;
    assert_eq!(bindings.len(), 1);
    let fixture = &bindings[0];
    assert_eq!(fixture["name"], serde_json::json!("Fixture"));
    assert_eq!(fixture["module"], serde_json::json!(["fixture"]));
    assert_eq!(fixture["header"], serde_json::json!("fixture.h"));
    assert_eq!(fixture["system_library"], serde_json::json!("fixture"));
    assert!(
        fixture["identity"]
            .as_str()
            .is_some_and(|identity| identity.starts_with("sha256:")),
        "checked binding inspection must publish the compiler-owned portable descriptor identity: {fixture}"
    );
    assert!(
        fixture["source"]["file"]
            .as_str()
            .is_some_and(|path| path.ends_with("src/fixture.incn"))
    );
    assert_eq!(fixture["source"]["start_line"], serde_json::json!(3));
    assert_eq!(fixture["source"]["start_column"], serde_json::json!(1));
    assert!(fixture["source"]["end_column"].is_number());
    assert_eq!(fixture["resources"][0]["name"], serde_json::json!("Handle"));
    assert_eq!(fixture["resources"][0]["native"], serde_json::json!("fixture_handle"));
    assert_eq!(fixture["resources"][0]["release"], serde_json::json!("close"));
    assert_eq!(fixture["symbols"][0]["name"], serde_json::json!("close"));
    assert_eq!(fixture["symbols"][0]["native"], serde_json::json!("fixture_close"));
    assert_eq!(
        fixture["symbols"][0]["parameters"][0]["type"]["kind"],
        serde_json::json!("resource")
    );
    assert_eq!(
        fixture["symbols"][0]["parameters"][0]["type"]["access"],
        serde_json::json!("owned")
    );
    assert_eq!(
        fixture["symbols"][1]["parameters"][0]["type"]["access"],
        serde_json::json!("borrowed")
    );
    assert_eq!(
        fixture["symbols"][2]["parameters"][0]["type"]["spelling"],
        serde_json::json!("c.i32")
    );
    assert_eq!(
        fixture["symbols"][2]["return_type"]["spelling"],
        serde_json::json!("c.i32")
    );
    assert_eq!(fixture["symbols"][3]["name"], serde_json::json!("open"));
    assert_eq!(
        fixture["symbols"][3]["parameters"][0]["type"]["kind"],
        serde_json::json!("output")
    );
    assert_eq!(
        fixture["symbols"][3]["parameters"][0]["type"]["mode"],
        serde_json::json!("out")
    );
    assert_eq!(
        fixture["symbols"][3]["parameters"][0]["type"]["value"]["kind"],
        serde_json::json!("resource")
    );
    assert_eq!(
        fixture["symbols"][3]["parameters"][1]["type"]["mode"],
        serde_json::json!("in_out")
    );
    assert_eq!(
        fixture["symbols"][3]["outcomes"][0]["result"],
        serde_json::json!("Status.OK")
    );
    assert_eq!(
        fixture["symbols"][3]["outcomes"][0]["initializes"],
        serde_json::json!(["output"])
    );
    assert_eq!(
        fixture["symbols"][3]["outcomes"][0]["updates"],
        serde_json::json!(["attempts"])
    );
    assert_eq!(fixture["enums"][0]["name"], serde_json::json!("Status"));
    assert_eq!(fixture["enums"][0]["carrier"], serde_json::json!("c.i32"));
    assert_eq!(
        fixture["enums"][0]["variants"][0]["native"],
        serde_json::json!("FIXTURE_OK")
    );
    assert_eq!(fixture["structs"][0]["name"], serde_json::json!("Pair"));
    assert_eq!(fixture["structs"][0]["native"], serde_json::json!("fixture_pair"));
    assert_eq!(fixture["structs"][0]["fields"][1]["name"], serde_json::json!("right"));

    let text = run_incan(tmp.path(), &["inspect", "bindings", project, "--format", "text"])?;
    assert_success(&text, "inspect checked C binding declarations as text");
    let text = String::from_utf8(text.stdout)?;
    assert!(
        text.contains("Binding Fixture (fixture)"),
        "unexpected binding report:\n{text}"
    );
    assert!(text.contains("fixture.h"), "unexpected binding report:\n{text}");
    assert!(text.contains("identity: sha256:"), "unexpected binding report:\n{text}");
    assert!(text.contains("fixture_add"), "unexpected binding report:\n{text}");
    assert!(
        text.contains("resource Handle [native: fixture_handle, release: close]"),
        "unexpected binding report:\n{text}"
    );
    assert!(
        text.contains("outcome Status.OK [initializes: output, updates: attempts, invalidates: -]"),
        "unexpected binding report:\n{text}"
    );

    write_locked_oven_interop_plan(tmp.path())?;
    let receipt = run_incan(
        tmp.path(),
        &[
            "inspect",
            "bindings",
            project,
            "--format",
            "receipt",
            "--target",
            "aarch64-apple-darwin",
        ],
    )?;
    assert_success(&receipt, "inspect redacted checked C binding usage receipt");
    let receipt_stdout = String::from_utf8(receipt.stdout)?;
    let receipt: serde_json::Value = serde_json::from_str(&receipt_stdout)?;
    assert_eq!(receipt["schema_version"], serde_json::json!(2));
    assert_eq!(
        receipt["compatibility"]["binding_contract"],
        serde_json::json!("exact_descriptor_identity")
    );
    assert_eq!(
        receipt["compatibility"]["target_contract"],
        serde_json::json!("exact_locked_target_identity")
    );
    assert_eq!(receipt["target"]["target"], serde_json::json!("aarch64-apple-darwin"));
    assert!(
        receipt["target"]["locked_target_identity"]
            .as_str()
            .is_some_and(|identity| identity.starts_with("sha256:")),
        "binding usage receipt must join the compiler-owned locked target identity: {receipt}"
    );
    assert!(
        receipt["target"].get("selected_execution_identity").is_none(),
        "a receipt must omit an execution identity when no selected interop execution receipt exists: {receipt}"
    );
    assert_eq!(receipt["bindings"].as_array().map(Vec::len), Some(1));
    assert_eq!(receipt["bindings"][0]["name"], serde_json::json!("Fixture"));
    assert_eq!(
        receipt["bindings"][0]["target_artifacts"],
        serde_json::json!(["fixture"])
    );
    assert!(
        receipt["bindings"][0]["identity"]
            .as_str()
            .is_some_and(|identity| identity.starts_with("sha256:")),
        "binding usage receipt must retain the checked descriptor identity: {receipt}"
    );
    assert_eq!(receipt["calls"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        receipt["calls"][0]["binding_identity"], receipt["bindings"][0]["identity"],
        "raw-call usage must join its checked binding by compiler-owned identity"
    );
    assert_eq!(receipt["calls"][0]["symbol"], serde_json::json!("add"));
    assert_eq!(
        receipt["calls"][0]["owner"]["name"],
        serde_json::json!("private_bridge")
    );
    assert_eq!(receipt["calls"][0]["owner"]["visibility"], serde_json::json!("private"));
    assert_eq!(receipt["facades"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        receipt["facades"][0]["facade"]["name"],
        serde_json::json!("checked_sum")
    );
    assert_eq!(
        receipt["facades"][0]["facade"]["visibility"],
        serde_json::json!("public")
    );
    assert_eq!(
        receipt["facades"][0]["bridge"]["name"],
        serde_json::json!("private_bridge")
    );
    assert_eq!(
        receipt["facades"][0]["bridge"]["visibility"],
        serde_json::json!("private")
    );
    assert_eq!(
        receipt["facades"][0]["calls"][0]["binding_identity"], receipt["bindings"][0]["identity"],
        "the facade receipt must link its bridge raw call through the exact descriptor identity"
    );
    assert_eq!(receipt["facades"][0]["calls"][0]["symbol"], serde_json::json!("add"));
    assert!(
        !receipt_stdout.contains(&tmp.path().to_string_lossy().to_string())
            && !receipt_stdout.contains("fixture.h")
            && !receipt_stdout.contains("source"),
        "binding usage receipt must not retain local paths, header declarations, or source spans: {receipt_stdout}"
    );

    let relocated_temp = tempfile::tempdir()?;
    let relocated_root = relocated_temp.path().join("binding-inspection-relocated");
    fs::create_dir_all(relocated_root.join("src"))?;
    for relative_path in [
        "incan.toml",
        "incan.lock",
        "fixture.h",
        "src/fixture.incn",
        "src/main.incn",
    ] {
        fs::copy(tmp.path().join(relative_path), relocated_root.join(relative_path))?;
    }
    let relocated_project = relocated_root
        .to_str()
        .ok_or("relocated project path was not valid UTF-8")?;
    let relocated_receipt = run_incan(
        &relocated_root,
        &[
            "inspect",
            "bindings",
            relocated_project,
            "--format",
            "receipt",
            "--target",
            "aarch64-apple-darwin",
        ],
    )?;
    assert_success(&relocated_receipt, "relocated redacted checked C binding usage receipt");
    assert_eq!(
        receipt_stdout.as_bytes(),
        relocated_receipt.stdout.as_slice(),
        "a relocated locked package changed its redacted binding receipt"
    );

    let manifest_path = tmp.path().join("incan.toml");
    let manifest = fs::read_to_string(&manifest_path)?;
    let dangling_manifest = manifest.replacen(
        "module = [\"fixture\"]\nname = \"Fixture\"",
        "module = [\"fixture\"]\nname = \"MissingFixture\"",
        1,
    );
    assert_ne!(
        manifest, dangling_manifest,
        "fixture manifest must contain the declared binding relation"
    );
    fs::write(&manifest_path, dangling_manifest)?;
    write_locked_oven_interop_plan(tmp.path())?;
    let dangling = run_incan(
        tmp.path(),
        &[
            "inspect",
            "bindings",
            project,
            "--format",
            "receipt",
            "--target",
            "aarch64-apple-darwin",
        ],
    )?;
    assert_failure(&dangling, "dangling target binding-artifact correspondence");
    assert!(
        String::from_utf8_lossy(&dangling.stderr).contains("did not produce that checked binding"),
        "a receipt must reject an authored correspondence without a compiler-produced binding:\n{}",
        String::from_utf8_lossy(&dangling.stderr)
    );

    let ignored_target = run_incan(
        tmp.path(),
        &[
            "inspect",
            "bindings",
            project,
            "--format",
            "json",
            "--target",
            "aarch64-apple-darwin",
        ],
    )?;
    assert_failure(&ignored_target, "binding target outside receipt mode");
    assert!(
        String::from_utf8_lossy(&ignored_target.stderr).contains("requires `--format receipt`"),
        "a target outside receipt mode must fail instead of being silently ignored:\n{}",
        String::from_utf8_lossy(&ignored_target.stderr)
    );

    let broken_path = src_dir.join("broken.incn");
    fs::write(
        &broken_path,
        r#"from std.interop import c

@c.binding(header="fixture.h", link=c.system_library("fixture"))
class Broken:
    value: int
"#,
    )?;
    let broken = run_incan(
        tmp.path(),
        &[
            "inspect",
            "bindings",
            broken_path.to_str().ok_or("broken path was not valid UTF-8")?,
            "--format",
            "json",
        ],
    )?;
    assert_failure(&broken, "invalid checked C binding inspection");
    assert!(
        broken.stdout.is_empty(),
        "strict binding inspection must not emit partial JSON:\n{}",
        String::from_utf8_lossy(&broken.stdout)
    );
    assert!(
        String::from_utf8_lossy(&broken.stderr).contains("must extend BindingDeclaration"),
        "binding inspection should preserve the compiler diagnostic:\n{}",
        String::from_utf8_lossy(&broken.stderr)
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
fn codegraph_projects_checked_c_bindings_and_explicit_unsafe_calls() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let source_path = tmp.path().join("main.incn");
    let header_path = tmp.path().join("fixture.h");
    fs::write(
        &header_path,
        concat!(
            "typedef struct fixture_handle fixture_handle;\n",
            "#define FIXTURE_OK 0\n",
            "void fixture_close(fixture_handle *handle);\n",
            "int fixture_open(fixture_handle **output, int *attempts);\n",
            "unsigned int fixture_random(unsigned int *seed);\n",
        ),
    )?;
    fs::write(
        &source_path,
        format!(
            r#"from std.interop import c

binding Fixture:
    header = "{}"
    link = c.system_library("c")

    resource Handle:
        native = "fixture_handle"
        release = close

    symbol close(handle: c.Owned[Handle]) -> None:
        native = "fixture_close"

    enum Status:
        OK: c.i32 = FIXTURE_OK

    symbol open(output: c.Out[c.Owned[Handle]], attempts: c.InOut[c.i32]) -> c.i32:
        native = "fixture_open"

        outcome Status.OK:
            initializes = [output]
            updates = [attempts]

    symbol random(seed: c.InOut[c.u32]) -> c.u32:
        native = "fixture_random"

def inspect_contract() -> None:
    unsafe:
        handle = c.out[c.Owned[Handle]]()
        attempts_value: i32 = 0
        attempts = c.inout(attempts_value)
        status = Fixture.open(handle, attempts)
        if status == Fixture.Status.OK:
            resource = handle.take()
            Fixture.close(resource)

        seed_value: u32 = 7
        seed = c.inout(seed_value)
        Fixture.random(seed)
        seed.take()

pub def public_facade() -> None:
    inspect_contract()
"#,
            header_path.display()
        ),
    )?;

    let source = source_path.to_str().ok_or("source path was not valid UTF-8")?;
    let first = run_incan(tmp.path(), &["inspect", "codegraph", source, "--format", "jsonl"])?;
    assert_success(&first, "codegraph checked C binding projection");
    let second = run_incan(tmp.path(), &["inspect", "codegraph", source, "--format", "jsonl"])?;
    assert_success(&second, "second codegraph checked C binding projection");
    assert_eq!(
        first.stdout, second.stdout,
        "checked C codegraph records must be deterministic"
    );

    let records = parse_jsonl_stdout(&first)?;
    assert_codegraph_record_contract(&records);
    let binding = records
        .iter()
        .find(|record| record["record"] == serde_json::json!("c_binding"))
        .ok_or("codegraph did not emit the checked C binding record")?;
    assert_eq!(binding["name"], serde_json::json!("Fixture"));
    assert_eq!(binding["header"], serde_json::json!(header_path.to_string_lossy()));
    assert_eq!(binding["system_library"], serde_json::json!("c"));
    assert!(
        binding["binding_identity"]
            .as_str()
            .is_some_and(|identity| identity.starts_with("sha256:")),
        "checked C codegraph record must publish the portable descriptor identity: {binding}"
    );
    assert_eq!(binding["provenance"], serde_json::json!("checked"));
    let declaration_id = binding["declaration_id"]
        .as_str()
        .ok_or("checked C binding did not link to its class declaration record")?;
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("declaration")
            && record["id"] == serde_json::json!(declaration_id)
            && record["name"] == serde_json::json!("Fixture")
    }));
    assert_eq!(binding["resources"][0]["name"], serde_json::json!("Handle"));
    assert_eq!(binding["resources"][0]["release"], serde_json::json!("close"));
    assert_eq!(
        binding["symbols"][1]["parameters"][0]["type"]["kind"],
        serde_json::json!("output")
    );
    assert_eq!(
        binding["symbols"][1]["parameters"][0]["type"]["mode"],
        serde_json::json!("out")
    );
    assert_eq!(
        binding["symbols"][1]["parameters"][1]["type"]["mode"],
        serde_json::json!("in_out")
    );
    assert_eq!(
        binding["symbols"][1]["outcomes"][0]["initializes"],
        serde_json::json!(["output"])
    );
    assert_eq!(binding["enums"][0]["carrier"], serde_json::json!("c.i32"));

    let raw_call = records
        .iter()
        .find(|record| {
            record["record"] == serde_json::json!("c_binding_call")
                && record["binding"] == serde_json::json!("Fixture")
                && record["symbol"] == serde_json::json!("open")
        })
        .ok_or("codegraph did not emit the checked raw C call record")?;
    assert_eq!(raw_call["binding_id"], binding["id"]);
    assert_eq!(raw_call["binding_identity"], binding["binding_identity"]);
    assert_eq!(raw_call["owner_visibility"], serde_json::json!("private"));
    let owner_declaration_id = raw_call["owner_declaration_id"]
        .as_str()
        .ok_or("checked raw C call did not retain its compiler-known owning bridge")?;
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("declaration")
            && record["id"] == serde_json::json!(owner_declaration_id)
            && record["name"] == serde_json::json!("inspect_contract")
    }));
    let facade_declaration_id = records
        .iter()
        .find(|record| {
            record["record"] == serde_json::json!("declaration")
                && record["kind"] == serde_json::json!("function")
                && record["name"] == serde_json::json!("public_facade")
                && record["visibility"] == serde_json::json!("public")
        })
        .and_then(|record| record["id"].as_str())
        .ok_or("codegraph did not emit the public C-ABI facade declaration")?;
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["callee"] == serde_json::json!("inspect_contract")
            && record["owner_id"] == serde_json::json!(facade_declaration_id)
            && record["target_id"] == serde_json::json!(owner_declaration_id)
            && record["provenance"] == serde_json::json!("checked")
    }));
    let facade = records
        .iter()
        .find(|record| record["record"] == serde_json::json!("c_binding_facade"))
        .ok_or("codegraph did not emit the compiler-proven C-ABI facade relation")?;
    assert_eq!(
        facade["facade_declaration_id"],
        serde_json::json!(facade_declaration_id)
    );
    assert_eq!(facade["bridge_declaration_id"], serde_json::json!(owner_declaration_id));
    assert_eq!(facade["provenance"], serde_json::json!("checked"));
    assert_eq!(facade["degraded"], serde_json::json!(false));
    assert!(
        facade["call_id"].is_string(),
        "facade relation must link to its compiler-proven ordinary call: {facade}"
    );
    assert!(
        facade["raw_call_ids"].as_array().is_some_and(|ids| !ids.is_empty()),
        "facade relation must link the private bridge to its direct raw calls: {facade}"
    );
    assert_eq!(raw_call["unsafe_acknowledged"], serde_json::json!(true));
    assert_eq!(raw_call["provenance"], serde_json::json!("checked"));
    let call_id = raw_call["call_id"]
        .as_str()
        .ok_or("checked raw C call did not link to its generic call record")?;
    assert!(
        records.iter().any(|record| {
            record["record"] == serde_json::json!("call") && record["id"] == serde_json::json!(call_id)
        })
    );

    let raw_call_count = records
        .iter()
        .filter(|record| record["record"] == serde_json::json!("c_binding_call"))
        .count();
    assert_eq!(
        raw_call_count, 3,
        "only direct native calls should receive C binding call records"
    );

    Ok(())
}

fn assert_source_files_include(
    report: &serde_json::Value,
    suffixes: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    let files = report["source_files"]
        .as_array()
        .ok_or_else(|| format!("report source_files should be an array: {report}"))?;
    for suffix in suffixes {
        if !files
            .iter()
            .any(|file| file["path"].as_str().is_some_and(|path| path.ends_with(suffix)))
        {
            return Err(format!("expected report to include source file ending with {suffix}: {report}").into());
        }
    }
    Ok(())
}

#[test]
fn check_json_reports_parser_diagnostics() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let source_path = tmp.path().join("broken.incn");
    fs::write(&source_path, "def broken(:\n")?;

    let output = run_incan(
        tmp.path(),
        &[
            "check",
            source_path.to_str().ok_or("source path was not valid UTF-8")?,
            "--format",
            "json",
        ],
    )?;
    assert_failure(&output, "incan check --format json parser diagnostic");
    let json = parse_json_stdout(&output)?;
    assert_eq!(json["schema_version"], serde_json::json!(2));
    assert_eq!(json["ok"], serde_json::json!(false));
    assert_eq!(json["diagnostics"][0]["code"], serde_json::json!("INCAN-P0001"));
    assert_eq!(json["diagnostics"][0]["phase"], serde_json::json!("parse"));
    assert_eq!(
        json["diagnostics"][0]["primary_span"]["start"]["line"],
        serde_json::json!(1)
    );

    Ok(())
}

#[test]
fn check_json_reports_typechecker_diagnostics() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let source_path = tmp.path().join("main.incn");
    fs::write(
        &source_path,
        r#"def main() -> None:
    missing()
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &[
            "check",
            source_path.to_str().ok_or("source path was not valid UTF-8")?,
            "--format",
            "json",
        ],
    )?;
    assert_failure(&output, "incan check --format json typechecker diagnostic");
    let json = parse_json_stdout(&output)?;
    assert_eq!(json["diagnostics"][0]["code"], serde_json::json!("INCAN-T0001"));
    assert_eq!(json["diagnostics"][0]["phase"], serde_json::json!("typecheck"));
    assert_eq!(
        json["diagnostics"][0]["message"],
        serde_json::json!("Unknown symbol 'missing'")
    );
    assert_eq!(
        json["diagnostics"][0]["explain"],
        serde_json::json!("incan explain INCAN-T0001")
    );

    let legacy_output = run_incan(
        tmp.path(),
        &[
            "--check",
            source_path.to_str().ok_or("source path was not valid UTF-8")?,
            "--format",
            "json",
        ],
    )?;
    assert_failure(&legacy_output, "incan --check --format json typechecker diagnostic");
    let legacy_json = parse_json_stdout(&legacy_output)?;
    assert_eq!(legacy_json["diagnostics"][0]["code"], serde_json::json!("INCAN-T0001"));

    Ok(())
}

#[test]
fn diagnostic_facts_keep_related_spans_and_type_payloads_across_cli_and_codegraph()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let source_path = tmp.path().join("main.incn");
    fs::write(
        &source_path,
        r#"model Wire:
    id [alias="wire"]: int
    label [alias="wire"]: str

def accept(value: int) -> None:
    pass

def main() -> None:
    accept(value=1, value=2)
    accept("text")
"#,
    )?;
    let source_arg = source_path.to_str().ok_or("source path was not valid UTF-8")?;

    let check = run_incan(tmp.path(), &["check", source_arg, "--format", "json"])?;
    assert_failure(&check, "incan check should emit diagnostic facts");
    let check_json = parse_json_stdout(&check)?;
    let cli_diagnostics = check_json["diagnostics"]
        .as_array()
        .ok_or("expected CLI diagnostics array")?;
    let cli_alias = cli_diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("Duplicate alias"))
        })
        .ok_or("expected duplicate alias diagnostic")?;
    let cli_duplicate_arg = cli_diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("Duplicate argument"))
        })
        .ok_or("expected duplicate argument diagnostic")?;
    let cli_mismatch = cli_diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("Argument 'value' of 'accept'"))
        })
        .ok_or("expected call argument type mismatch diagnostic")?;

    assert_eq!(cli_alias["origin"], serde_json::json!("typechecker"));
    assert_eq!(
        cli_alias["related_spans"][0]["label"],
        serde_json::json!("First field alias 'wire'")
    );
    assert_eq!(
        cli_duplicate_arg["related_spans"][0]["label"],
        serde_json::json!("First argument named 'value'")
    );
    assert_eq!(cli_mismatch["expected"], serde_json::json!("int"));
    assert_eq!(cli_mismatch["actual"], serde_json::json!("str"));

    let codegraph = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            source_arg,
            "--format",
            "jsonl",
            "--allow-errors",
        ],
    )?;
    assert_success(&codegraph, "tolerant codegraph should project diagnostic facts");
    let records = parse_jsonl_stdout(&codegraph)?;
    let graph_alias = records
        .iter()
        .find(|record| record["record"] == serde_json::json!("diagnostic") && record["message"] == cli_alias["message"])
        .ok_or("expected duplicate alias codegraph diagnostic")?;
    let graph_duplicate_arg = records
        .iter()
        .find(|record| {
            record["record"] == serde_json::json!("diagnostic") && record["message"] == cli_duplicate_arg["message"]
        })
        .ok_or("expected duplicate argument codegraph diagnostic")?;
    let graph_mismatch = records
        .iter()
        .find(|record| {
            record["record"] == serde_json::json!("diagnostic") && record["message"] == cli_mismatch["message"]
        })
        .ok_or("expected type mismatch codegraph diagnostic")?;

    assert_eq!(graph_alias["origin"], cli_alias["origin"]);
    assert_eq!(
        graph_alias["related_spans"][0]["label"],
        cli_alias["related_spans"][0]["label"]
    );
    assert_eq!(
        graph_alias["related_spans"][0]["span"]["start"],
        cli_alias["related_spans"][0]["span"]["start"]["offset"]
    );
    assert_eq!(
        graph_duplicate_arg["related_spans"][0]["label"],
        cli_duplicate_arg["related_spans"][0]["label"]
    );
    assert_eq!(graph_mismatch["expected"], cli_mismatch["expected"]);
    assert_eq!(graph_mismatch["actual"], cli_mismatch["actual"]);

    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn rust_std_result_and_contextual_f32_interop_compile_together_issues801_802() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
        r#"[project]
name = "result_interop_probe"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"
"#,
    )?;
    let source_path = src_dir.join("main.incn");
    fs::write(
        &source_path,
        r#"from rust::std::fs import metadata
from rust::std::io import Error as IoError
from rust::std::path import Path as RustPath

pub def file_len(path: str) -> Result[int, IoError]:
  meta = metadata(RustPath.new(path))?
  return Ok(int(meta.len()))

def accepts_f32(value: f32) -> None:
  print("ok")

def main() -> None:
  result = file_len("incan.toml")
  zero: f32 = 0.0
  accepts_f32(1.5)
  print("checked")
"#,
    )?;
    let source_arg = source_path.to_str().ok_or("source path was not valid UTF-8")?;

    let bake = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake, "explicit Oven bake for std Result and contextual f32 interop");
    let build = run_incan(tmp.path(), &["build", source_arg, "--offline"])?;
    assert_success(
        &build,
        "incan build should emit Rust for std::fs::metadata try operator and contextual f32 literals",
    );
    let generated = fs::read_to_string(tmp.path().join("target/incan/result_interop_probe/src/main.rs"))?;
    assert!(
        !generated.contains("0f64") && !generated.contains("1.5f64"),
        "contextual float literals should not be hard-suffixed as f64:\n{generated}"
    );

    Ok(())
}

#[test]
fn check_json_reports_tooling_diagnostics() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let missing_path = tmp.path().join("missing.incn");

    let output = run_incan(
        tmp.path(),
        &[
            "check",
            missing_path.to_str().ok_or("missing path was not valid UTF-8")?,
            "--format",
            "json",
        ],
    )?;
    assert_failure(&output, "incan check --format json tooling diagnostic");
    let json = parse_json_stdout(&output)?;
    assert_eq!(json["diagnostics"][0]["code"], serde_json::json!("INCAN-C0001"));
    assert_eq!(json["diagnostics"][0]["phase"], serde_json::json!("tooling"));
    assert!(
        json["diagnostics"][0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("Cannot access file")),
        "expected missing file diagnostic, got:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );

    Ok(())
}

#[test]
fn check_json_reports_import_diagnostics() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
        r#"[project]
name = "diag_import"
version = "0.1.0"
"#,
    )?;
    let source_path = src_dir.join("main.incn");
    fs::write(
        &source_path,
        r#"from pub::missinglib import Widget

def main() -> None:
    return
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &[
            "check",
            source_path.to_str().ok_or("source path was not valid UTF-8")?,
            "--format",
            "json",
        ],
    )?;
    assert_failure(&output, "incan check --format json import diagnostic");
    let json = parse_json_stdout(&output)?;
    assert_eq!(json["diagnostics"][0]["code"], serde_json::json!("INCAN-I0001"));
    assert_eq!(json["diagnostics"][0]["phase"], serde_json::json!("import"));
    assert!(
        json["diagnostics"][0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("Unknown `pub::` library")),
        "expected pub library import diagnostic, got:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );

    Ok(())
}

#[test]
fn explain_reports_known_and_unknown_diagnostic_codes() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;

    let known = run_incan(tmp.path(), &["explain", "INCAN-P0001", "--format", "json"])?;
    assert_success(&known, "incan explain known code json");
    let known_json = parse_json_stdout(&known)?;
    assert_eq!(known_json["schema_version"], serde_json::json!(1));
    assert_eq!(known_json["found"], serde_json::json!(true));
    assert_eq!(known_json["entry"]["code"], serde_json::json!("INCAN-P0001"));

    let unknown = run_incan(tmp.path(), &["explain", "INCAN-NOPE", "--format", "json"])?;
    assert_failure(&unknown, "incan explain unknown code json");
    let unknown_json = parse_json_stdout(&unknown)?;
    assert_eq!(unknown_json["found"], serde_json::json!(false));
    assert_eq!(unknown_json["entry"]["code"], serde_json::json!("INCAN-U0001"));

    Ok(())
}

#[test]
fn build_report_json_describes_executable_build() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let source_path = tmp.path().join("main.incn");
    fs::write(
        &source_path,
        r#"def main() -> None:
    println("report ok")
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &[
            "build",
            source_path.to_str().ok_or("source path was not valid UTF-8")?,
            "--offline",
            "--report",
            "json",
        ],
    )?;
    assert_success(&output, "incan build --report json executable");
    let report = parse_json_stdout(&output)?;
    assert_eq!(report["schema_version"], serde_json::json!(1));
    assert_eq!(report["status"], serde_json::json!("success"));
    assert_eq!(report["mode"], serde_json::json!("executable"));
    assert_eq!(report["profile"], serde_json::json!("release"));
    assert!(
        report["generated"]["project_path"]
            .as_str()
            .is_some_and(|path| path.contains("target/incan"))
    );
    assert!(report["generated"]["manifest_path"].is_null());
    assert!(
        report["generated"]["oven_output_dir"]
            .as_str()
            .is_some_and(|path| path.ends_with("target/incan/main/oven"))
    );
    assert!(report["source_files"].as_array().is_some_and(|files| {
        files.iter().any(|file| {
            file["path"].as_str().is_some_and(|path| path.ends_with("main.incn"))
                && file["module_path"]
                    .as_array()
                    .is_some_and(|segments| segments.as_slice() == [serde_json::json!("main")])
        })
    }));
    assert!(report["cargo"].is_null());
    assert!(report["oven"]["receipt_identity"].is_string());
    assert!(report["oven"]["build_unit_identity"].is_string());
    assert!(report["oven"]["plan_identity"].is_string());
    assert!(report["semantic"]["packages"].as_array().is_some());
    assert!(report["semantic"]["feature_edges"].as_array().is_some());
    assert!(report["semantic"]["providers"].as_array().is_some_and(|providers| {
        !providers.is_empty()
            && providers.iter().all(|provider| {
                provider["identity"].is_string()
                    && provider["participation"].is_string()
                    && provider["provenance"].is_object()
                    && provider["implementation_facets"].is_array()
                    && provider["backend_requirements"].is_array()
            })
    }));
    assert!(report["artifacts"].as_array().is_some_and(|artifacts| {
        artifacts.iter().any(|artifact| {
            artifact["kind"] == serde_json::json!("binary") && artifact["exists"] == serde_json::json!(true)
        })
    }));
    assert!(report["timings_ms"]["total"].as_u64().is_some());
    assert!(report["notes"].as_array().is_some_and(|notes| {
        notes
            .iter()
            .any(|note| note.as_str().is_some_and(|text| text.contains("direct-rustc plan")))
    }));

    Ok(())
}

#[test]
fn build_report_output_file_describes_library_build() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
        r#"[project]
name = "report_lib"
version = "0.1.0"
"#,
    )?;
    fs::write(
        src_dir.join("lib.incn"),
        r#"pub def answer() -> int:
    return 42
"#,
    )?;
    let report_path = tmp.path().join("target").join("build-report.json");
    let output = run_incan(
        tmp.path(),
        &[
            "build",
            "--lib",
            "--offline",
            "--report",
            "json",
            "--report-output",
            report_path.to_str().ok_or("report path was not valid UTF-8")?,
        ],
    )?;
    assert_success(&output, "incan build --lib --report-output");
    assert!(
        output.stdout.is_empty(),
        "report-output should keep machine JSON out of stdout, got:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let report: serde_json::Value = serde_json::from_str(&fs::read_to_string(&report_path)?)?;
    assert_eq!(report["mode"], serde_json::json!("library"));
    assert_eq!(report["project"]["name"], serde_json::json!("report_lib"));
    assert_eq!(
        report["entrypoint"].as_str().map(|path| path.ends_with("src/lib.incn")),
        Some(true)
    );
    assert!(report["generated"]["cargo_target_dir"].is_null());
    assert!(
        report["generated"]["oven_output_dir"]
            .as_str()
            .is_some_and(|path| path.ends_with("target/lib/oven"))
    );
    assert!(report["cargo"].is_null());
    assert!(report["oven"]["plan_identity"].is_string());
    assert!(report["source_files"].as_array().is_some_and(|files| {
        files
            .iter()
            .any(|file| file["path"].as_str().is_some_and(|path| path.ends_with("src/lib.incn")))
    }));
    assert!(report["artifacts"].as_array().is_some_and(|artifacts| {
        artifacts.iter().any(|artifact| {
            artifact["kind"] == serde_json::json!("incan_library_manifest")
                && artifact["exists"] == serde_json::json!(true)
        })
    }));
    assert!(report["artifacts"].as_array().is_some_and(|artifacts| {
        artifacts.iter().any(|artifact| {
            artifact["kind"] == serde_json::json!("rust_library_debug") && artifact["exists"] == serde_json::json!(true)
        })
    }));
    assert!(report["artifacts"].as_array().is_some_and(|artifacts| {
        artifacts.iter().any(|artifact| {
            artifact["kind"] == serde_json::json!("rust_library_release")
                && artifact["exists"] == serde_json::json!(true)
        })
    }));
    assert!(report["timings_ms"]["library_load_sources"].as_u64().is_some());
    assert!(
        report["timings_ms"]["library_collect_vocab_metadata"]
            .as_u64()
            .is_some()
    );
    assert!(report["timings_ms"]["library_prepare_total"].as_u64().is_some());
    assert!(report["timings_ms"]["oven_build"].as_u64().is_some());
    assert!(report["timings_ms"]["total"].as_u64().is_some());

    Ok(())
}

#[test]
fn hyphenated_library_package_preserves_identity_and_emits_a_valid_rust_target_issue995()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(
        root.path().join("incan.toml"),
        "[project]\nname = \"hyphenated-library\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        root.path().join("src/lib.incn"),
        "pub def answer() -> int:\n  return 42\n",
    )?;

    let output = run_incan(root.path(), &["build", "--lib"])?;
    assert_success(&output, "hyphenated library build");

    let cargo_toml = fs::read_to_string(root.path().join("target/lib/Cargo.toml"))?;
    let manifest: toml::Value = toml::from_str(&cargo_toml)?;
    assert_eq!(manifest["package"]["name"].as_str(), Some("hyphenated-library"));
    assert_eq!(manifest["lib"]["name"].as_str(), Some("hyphenated_library"));
    Ok(())
}

#[test]
fn inspect_rust_reports_current_generated_rust_files() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let source_path = tmp.path().join("main.incn");
    fs::write(
        &source_path,
        r#"def main() -> None:
    println("inspect ok")
"#,
    )?;
    let executable = run_incan(
        tmp.path(),
        &[
            "inspect",
            "rust",
            source_path.to_str().ok_or("source path was not valid UTF-8")?,
            "--format",
            "json",
        ],
    )?;
    assert_success(&executable, "incan inspect rust executable");
    let executable_report = parse_json_stdout(&executable)?;
    assert_eq!(executable_report["mode"], serde_json::json!("executable"));
    assert!(executable_report["source_files"].as_array().is_some_and(|files| {
        files
            .iter()
            .any(|file| file["path"].as_str().is_some_and(|path| path.ends_with("main.incn")))
    }));
    assert!(
        executable_report["rust_files"]
            .as_array()
            .is_some_and(|files| { files.iter().any(|file| file["crate_root"] == serde_json::json!(true)) })
    );

    let project = tempfile::tempdir()?;
    let src_dir = project.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        project.path().join("incan.toml"),
        r#"[project]
name = "inspect_lib"
version = "0.1.0"
"#,
    )?;
    fs::write(
        src_dir.join("lib.incn"),
        r#"pub model Widget:
    """Widget docs survive into generated Rust."""
    pub value: int

pub def answer() -> int:
    """Answer docs survive into generated Rust."""
    return 42
"#,
    )?;
    let library = run_incan(
        project.path(),
        &[
            "inspect",
            "rust",
            project.path().to_str().ok_or("project path was not valid UTF-8")?,
            "--lib",
            "--format",
            "json",
        ],
    )?;
    assert_success(&library, "incan inspect rust --lib");
    let library_report = parse_json_stdout(&library)?;
    assert_eq!(library_report["mode"], serde_json::json!("library"));
    assert!(library_report["source_files"].as_array().is_some_and(|files| {
        files
            .iter()
            .any(|file| file["path"].as_str().is_some_and(|path| path.ends_with("src/lib.incn")))
    }));
    assert!(
        library_report["generated"]["project_path"]
            .as_str()
            .is_some_and(|path| path.ends_with("target/lib"))
    );
    assert!(
        library_report["rust_files"]
            .as_array()
            .is_some_and(|files| { files.iter().any(|file| file["crate_root"] == serde_json::json!(true)) })
    );
    let crate_root_path = library_report["rust_files"]
        .as_array()
        .and_then(|files| files.iter().find(|file| file["crate_root"] == serde_json::json!(true)))
        .and_then(|file| file["path"].as_str())
        .ok_or("library inspection report did not include a crate root file")?;
    let crate_root = fs::read_to_string(crate_root_path)?;
    assert!(
        crate_root.contains(r#"#[doc = "Widget docs survive into generated Rust."]"#)
            || crate_root.contains("/// Widget docs survive into generated Rust."),
        "expected generated Rust to include public model docs, got:\n{crate_root}"
    );
    assert!(
        crate_root.contains(r#"#[doc = "Answer docs survive into generated Rust."]"#)
            || crate_root.contains("/// Answer docs survive into generated Rust."),
        "expected generated Rust to include public function docs, got:\n{crate_root}"
    );

    Ok(())
}

#[test]
fn inspect_codegraph_exports_multifile_imports_and_public_symbols() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
        r#"[project]
name = "graph_demo"
version = "0.1.0"
"#,
    )?;
    fs::write(
        src_dir.join("helpers.incn"),
        r#"pub model Widget:
    pub value: int

pub def make_widget(value: int) -> Widget:
    return Widget(value=value)
"#,
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"import helpers
from helpers import make_widget

enum Signal:
    Ready

def local_value() -> int:
    return 3

def qualified_value() -> int:
    return helpers.make_widget(std.builtins.len([1, 2])).value

def ready() -> Signal:
    return Signal.Ready()

pub def entrypoint() -> int:
    return make_widget(local_value()).value
"#,
    )?;

    let first = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&first, "incan inspect codegraph");
    let second = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&second, "second incan inspect codegraph");
    assert_eq!(first.stdout, second.stdout, "codegraph JSONL should be deterministic");

    let records = parse_jsonl_stdout(&first)?;
    assert_codegraph_record_contract(&records);
    assert_eq!(records[0]["record"], serde_json::json!("header"));
    assert_eq!(records[0]["package"]["name"], serde_json::json!("graph_demo"));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("import")
            && record["path"] == serde_json::json!("helpers")
            && record["items"].as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item.as_str().is_some_and(|value| value == "make_widget"))
            })
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("declaration")
            && record["kind"] == serde_json::json!("function")
            && record["name"] == serde_json::json!("entrypoint")
            && record["visibility"] == serde_json::json!("public")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("export")
            && record["name"] == serde_json::json!("entrypoint")
            && record["kind"] == serde_json::json!("declaration")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("containment")
            && record["kind"] == serde_json::json!("module_contains_declaration")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["kind"] == serde_json::json!("function")
            && record["callee"] == serde_json::json!("make_widget")
            && record["argument_count"] == serde_json::json!(1)
            && record["target_id"].as_str().is_some_and(|target_id| {
                records.iter().any(|candidate| {
                    candidate["record"] == serde_json::json!("declaration")
                        && candidate["id"] == serde_json::json!(target_id)
                        && candidate["name"] == serde_json::json!("make_widget")
                })
            })
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("make_widget")
            && record["canonical_identity"]["origin"]["kind"] == serde_json::json!("module")
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["callee"] == serde_json::json!("helpers.make_widget")
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("make_widget")
            && record["canonical_identity"]["origin"]["kind"] == serde_json::json!("module")
            && record["target_id"].as_str().is_some_and(|target_id| {
                records.iter().any(|candidate| {
                    candidate["record"] == serde_json::json!("declaration")
                        && candidate["id"] == serde_json::json!(target_id)
                        && candidate["name"] == serde_json::json!("make_widget")
                })
            })
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["callee"] == serde_json::json!("std.builtins.len")
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("len")
            && record["canonical_identity"]["origin"]["kind"] == serde_json::json!("builtin")
            && record["target_id"] == serde_json::Value::Null
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["callee"] == serde_json::json!("Signal.Ready")
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("Ready")
            && record["canonical_identity"]["kind"] == serde_json::json!("variant")
            && record["canonical_identity"]["origin"]["kind"] == serde_json::json!("module")
            && record["target_id"] == serde_json::Value::Null
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["kind"] == serde_json::json!("function")
            && record["callee"] == serde_json::json!("local_value")
            && record["argument_count"] == serde_json::json!(0)
            && record["target_id"].as_str().is_some_and(|target_id| {
                records.iter().any(|candidate| {
                    candidate["record"] == serde_json::json!("declaration")
                        && candidate["id"] == serde_json::json!(target_id)
                        && candidate["name"] == serde_json::json!("local_value")
                })
            })
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("local_value")
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("reference")
            && record["kind"] == serde_json::json!("identifier")
            && record["name"] == serde_json::json!("make_widget")
            && record["target_id"].as_str().is_some_and(|target_id| {
                records.iter().any(|candidate| {
                    candidate["record"] == serde_json::json!("declaration")
                        && candidate["id"] == serde_json::json!(target_id)
                        && candidate["name"] == serde_json::json!("make_widget")
                })
            })
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("make_widget")
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("reference")
            && record["kind"] == serde_json::json!("identifier")
            && record["name"] == serde_json::json!("local_value")
            && record["target_id"].as_str().is_some_and(|target_id| {
                records.iter().any(|candidate| {
                    candidate["record"] == serde_json::json!("declaration")
                        && candidate["id"] == serde_json::json!(target_id)
                        && candidate["name"] == serde_json::json!("local_value")
                })
            })
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("local_value")
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("reference")
            && record["kind"] == serde_json::json!("field")
            && record["name"] == serde_json::json!("value")
            && record["target_id"] == serde_json::Value::Null
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("value")
            && record["canonical_identity"]["namespace"] == serde_json::json!("member")
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("containment")
            && record["kind"] == serde_json::json!("declaration_contains_call")
    }));

    let directory = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            src_dir.to_str().ok_or("src directory path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&directory, "directory incan inspect codegraph");
    let directory_records = parse_jsonl_stdout(&directory)?;
    assert!(directory_records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["callee"] == serde_json::json!("local_value")
            && record["target_id"].as_str().is_some_and(|target_id| {
                directory_records.iter().any(|candidate| {
                    candidate["record"] == serde_json::json!("declaration")
                        && candidate["id"] == serde_json::json!(target_id)
                        && candidate["name"] == serde_json::json!("local_value")
                })
            })
            && record["provenance"] == serde_json::json!("checked")
    }));

    Ok(())
}

#[test]
fn inspect_codegraph_keeps_one_identity_through_alias_reexport_and_without_a_local_record()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
        r#"[project]
name = "identity_graph"
version = "0.1.0"
"#,
    )?;
    fs::write(
        src_dir.join("provider.incn"),
        r#"pub def helper() -> int:
    return 7

pub run = alias helper
"#,
    )?;
    fs::write(
        src_dir.join("facade.incn"),
        r#"pub from provider import run as h
"#,
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"from facade import h as run_helper

def entrypoint() -> int:
    print("identity")
    return (run_helper)()
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            src_dir.to_str().ok_or("source path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&output, "identity-backed incan inspect codegraph");
    let records = parse_jsonl_stdout(&output)?;

    let provider = records
        .iter()
        .find(|record| {
            record["record"] == serde_json::json!("declaration") && record["name"] == serde_json::json!("helper")
        })
        .ok_or("provider declaration was absent")?;
    let provider_identity = &provider["canonical_identity"];
    assert_eq!(provider_identity["declaration_name"], serde_json::json!("helper"));
    assert_eq!(provider["provenance"], serde_json::json!("checked"));

    let declaration_alias = records
        .iter()
        .find(|record| {
            record["record"] == serde_json::json!("declaration") && record["name"] == serde_json::json!("run")
        })
        .ok_or("provider declaration alias was absent")?;
    assert_eq!(&declaration_alias["canonical_identity"], provider_identity);
    assert_ne!(declaration_alias["id"], provider["id"]);

    let reexport = records
        .iter()
        .find(|record| record["record"] == serde_json::json!("export") && record["name"] == serde_json::json!("h"))
        .ok_or("facade re-export record was absent")?;
    assert_eq!(&reexport["canonical_identity"], provider_identity);
    assert_eq!(reexport["provenance"], serde_json::json!("checked"));

    let aliased_import = records
        .iter()
        .find(|record| {
            record["record"] == serde_json::json!("import")
                && record["bindings"].as_array().is_some_and(|bindings| {
                    bindings
                        .iter()
                        .any(|binding| binding["local_name"] == serde_json::json!("run_helper"))
                })
        })
        .ok_or("consumer alias import record was absent")?;
    let aliased_binding = aliased_import["bindings"]
        .as_array()
        .and_then(|bindings| {
            bindings
                .iter()
                .find(|binding| binding["local_name"] == serde_json::json!("run_helper"))
        })
        .ok_or("consumer alias binding was absent")?;
    assert_eq!(&aliased_binding["canonical_identity"], provider_identity);
    assert_eq!(aliased_import["provenance"], serde_json::json!("checked"));

    for record_kind in ["reference", "call"] {
        let aliased = records
            .iter()
            .find(|record| {
                record["record"] == serde_json::json!(record_kind)
                    && (record["name"] == serde_json::json!("run_helper")
                        || record["callee"] == serde_json::json!("run_helper"))
            })
            .ok_or_else(|| format!("aliased {record_kind} record was absent"))?;
        assert_eq!(
            &aliased["canonical_identity"], provider_identity,
            "every spelling must retain the original provider identity"
        );
        assert_eq!(
            aliased["target_id"], provider["id"],
            "graph-local linkage must select the canonical declaration rather than its alias binding"
        );
        assert_eq!(aliased["provenance"], serde_json::json!("checked"));
    }

    let builtin = records
        .iter()
        .find(|record| record["record"] == serde_json::json!("call") && record["callee"] == serde_json::json!("print"))
        .ok_or("builtin call record was absent")?;
    assert_eq!(builtin["target_id"], serde_json::Value::Null);
    assert_eq!(
        builtin["canonical_identity"]["origin"]["kind"],
        serde_json::json!("builtin")
    );
    assert_eq!(
        builtin["canonical_identity"]["declaration_name"],
        serde_json::json!("print")
    );
    assert_eq!(
        builtin["provenance"],
        serde_json::json!("checked"),
        "a missing graph-local declaration must not erase compiler-proven identity"
    );

    Ok(())
}

#[test]
fn inspect_codegraph_exports_checked_registry_facts() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
        r#"[project]
name = "registry_graph"
version = "0.1.0"
"#,
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"from std.registry import Registry, SubjectKind, describe

@derive(Clone, Eq)
type FunctionId = newtype str

@derive(Descriptor)
model FunctionSpec:
    summary: str

pub static functions: Registry[FunctionId, FunctionSpec] = Registry.define(
    subjects=[SubjectKind.Function],
)

@describe(functions, FunctionId("normalize"), FunctionSpec(summary="Normalize text"))
pub def normalize(value: str) -> str:
    return value
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&output, "registry codegraph export");
    let records = parse_jsonl_stdout(&output)?;
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("registry")
            && record["registry_identity"] == serde_json::json!("main::functions")
            && record["registry_public"] == serde_json::json!(true)
            && record["subject_kind"] == serde_json::json!("function")
            && record["subject_identity"] == serde_json::json!("main.normalize")
            && record["key"]["kind"] == serde_json::json!("newtype")
            && record["descriptor"]["kind"] == serde_json::json!("model")
            && record["registration_span"].is_object()
            && record["subject_span"].is_object()
            && record["provenance"] == serde_json::json!("checked")
    }));
    Ok(())
}

#[test]
fn inspect_codegraph_attaches_facade_paths_to_checked_registry_facts() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
        "[project]\nname = \"registry_graph_facade\"\nversion = \"0.1.0\"\n",
    )?;
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
        src_dir.join("main.incn"),
        r#"pub from crate.feature import functions as public_functions
pub from crate.feature import normalize as public_normalize
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            src_dir
                .join("main.incn")
                .to_str()
                .ok_or("main path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&output, "registry facade codegraph export");
    let records = parse_jsonl_stdout(&output)?;
    let registry = records
        .iter()
        .find(|record| {
            record["record"] == serde_json::json!("registry")
                && record["registry_identity"] == serde_json::json!("feature::functions")
        })
        .ok_or("missing checked feature registry record")?;
    assert_eq!(registry["subject_identity"], serde_json::json!("feature.normalize"));
    let reexport_paths = registry["reexport_paths"]
        .as_array()
        .ok_or("checked registry record must expose facade projections")?;
    assert_eq!(
        reexport_paths
            .iter()
            .map(|projection| projection["path"].clone())
            .collect::<Vec<_>>(),
        vec![
            serde_json::json!(["main", "public_functions"]),
            serde_json::json!(["main", "public_normalize"]),
        ]
    );
    assert!(
        reexport_paths.iter().all(|path| path["span"].is_object()),
        "facade projections must retain their public-import anchors: {registry}"
    );
    Ok(())
}

#[test]
fn inspect_codegraph_projects_the_selected_incan_package_features() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "feature_graph_demo",
        r#"

[project.features]
alpha = []
beta = []
"#,
    )?;
    fs::write(
        &main_path,
        r#"when feature("alpha"):
    pub def alpha_entrypoint() -> str:
        return "alpha"

when feature("beta"):
    pub def beta_entrypoint() -> str:
        return "beta"
"#,
    )?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;

    for (selected, expected, excluded) in [
        ("alpha", "alpha_entrypoint", "beta_entrypoint"),
        ("beta", "beta_entrypoint", "alpha_entrypoint"),
    ] {
        let output = run_incan(
            tmp.path(),
            &[
                "inspect",
                "codegraph",
                main_arg,
                "--format",
                "jsonl",
                "--no-default-features",
                "--features",
                selected,
            ],
        )?;
        assert_success(&output, &format!("codegraph projection for package feature {selected}"));
        let records = parse_jsonl_stdout(&output)?;
        let header = records.first().ok_or("codegraph did not emit a header")?;
        let semantic_context = header["semantic_contexts"]
            .as_array()
            .and_then(|contexts| contexts.first())
            .ok_or("codegraph header did not project semantic context")?;
        let package = semantic_context["packages"]
            .as_array()
            .and_then(|packages| packages.first())
            .ok_or("codegraph semantic context did not project package features")?;
        assert_eq!(package["active_features"], serde_json::json!([selected]));
        assert!(semantic_context["providers"].as_array().is_some_and(|providers| {
            providers.iter().any(|provider| {
                provider["provenance"]["kind"] == serde_json::json!("sdk")
                    && provider["enabled"] == serde_json::json!(true)
                    && provider["manifest_path"].as_str().is_some()
            })
        }));
        assert!(records.iter().any(|record| {
            record["record"] == serde_json::json!("declaration") && record["name"] == serde_json::json!(expected)
        }));
        assert!(
            records.iter().all(|record| {
                record["record"] != serde_json::json!("declaration") || record["name"] != serde_json::json!(excluded)
            }),
            "codegraph for `{selected}` retained inactive declaration `{excluded}`"
        );
    }

    Ok(())
}

#[test]
fn transient_sdk_profile_is_shared_by_check_and_provider_inspection() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "sdk_profile_override", "")?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;

    let minimal_core = run_incan(tmp.path(), &["check", main_arg, "--sdk-profile", "minimal"])?;
    assert_success(
        &minimal_core,
        "minimal SDK profile check using only core language surface",
    );
    let minimal_codegraph = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            main_arg,
            "--format",
            "jsonl",
            "--sdk-profile",
            "minimal",
        ],
    )?;
    assert_success(
        &minimal_codegraph,
        "minimal SDK profile codegraph using only core language surface",
    );

    fs::write(
        &main_path,
        r#"from std.fs.path import Path

def main() -> None:
    _ = Path("profile")
"#,
    )?;

    let minimal = run_incan(tmp.path(), &["check", main_arg, "--sdk-profile", "minimal"])?;
    assert_failure(&minimal, "minimal SDK profile check using std.fs");
    let minimal_stderr = String::from_utf8_lossy(&minimal.stderr);
    assert!(
        minimal_stderr.contains("stdlib-system") && minimal_stderr.contains("disabled"),
        "minimal profile should diagnose the disabled std.fs component:\n{minimal_stderr}"
    );
    let minimal_json = run_incan(
        tmp.path(),
        &["check", main_arg, "--format", "json", "--sdk-profile", "minimal"],
    )?;
    assert_failure(&minimal_json, "minimal SDK profile JSON diagnostic using std.fs");
    let minimal_json = parse_json_stdout(&minimal_json)?;
    assert_eq!(minimal_json["diagnostics"][0]["code"], serde_json::json!("INCAN-I0101"));

    let default = run_incan(tmp.path(), &["check", main_arg, "--sdk-profile", "default"])?;
    assert_success(&default, "default SDK profile check using std.fs");

    for command in ["build", "run"] {
        let output = run_incan(tmp.path(), &[command, main_arg, "--sdk-profile", "minimal"])?;
        assert_failure(&output, &format!("{command} using disabled std.fs component"));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("stdlib-system") && stderr.contains("disabled"),
            "{command} should use the transient provider projection:\n{stderr}"
        );
    }

    let inspection = run_incan(
        tmp.path(),
        &[
            "inspect",
            "providers",
            main_arg,
            "--format",
            "json",
            "--sdk-profile",
            "minimal",
        ],
    )?;
    assert_success(&inspection, "provider inspection with transient minimal SDK profile");
    let report = parse_json_stdout(&inspection)?;
    assert_eq!(report["sdk"]["profile"], serde_json::json!("minimal"));
    let components = report["sdk"]["components"]
        .as_array()
        .ok_or("provider report did not contain SDK components")?;
    let system = components
        .iter()
        .find(|component| component["id"] == serde_json::json!("stdlib-system"))
        .ok_or("provider report did not contain stdlib-system")?;
    assert_eq!(system["available"], serde_json::json!(true));
    assert_eq!(system["enabled"], serde_json::json!(false));
    let providers = report["providers"]
        .as_array()
        .ok_or("provider report did not contain providers")?;
    let system_provider = providers
        .iter()
        .find(|provider| provider["provenance"]["component_id"] == serde_json::json!("stdlib-system"))
        .ok_or("provider report did not contain the stdlib-system provider")?;
    assert_eq!(system_provider["available"], serde_json::json!(true));
    assert_eq!(system_provider["enabled"], serde_json::json!(false));
    assert_eq!(system_provider["used"], serde_json::json!(false));
    assert!(system_provider["provider_dependencies"].is_array());

    Ok(())
}

#[test]
fn lock_and_feature_inspection_record_transient_semantic_selections() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "semantic_selection_lock",
        r#"

[project.features]
default = ["alpha"]
alpha = []
beta = []
"#,
    )?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;
    let selection_args = [
        "--no-default-features",
        "--features",
        "beta",
        "--sdk-profile",
        "minimal",
    ];

    let inspection = run_incan(
        tmp.path(),
        &[
            "inspect",
            "features",
            main_arg,
            "--format",
            "json",
            selection_args[0],
            selection_args[1],
            selection_args[2],
            selection_args[3],
            selection_args[4],
        ],
    )?;
    assert_success(&inspection, "feature inspection with transient semantic selections");
    let report = parse_json_stdout(&inspection)?;
    let package = report["packages"]
        .as_array()
        .and_then(|packages| packages.first())
        .ok_or("feature report did not contain the root package")?;
    assert_eq!(package["active_features"], serde_json::json!(["beta"]));
    assert_eq!(package["reasons"]["beta"][0]["kind"], serde_json::json!("requested"));

    let lock = run_incan(
        tmp.path(),
        &[
            "lock",
            main_arg,
            selection_args[0],
            selection_args[1],
            selection_args[2],
            selection_args[3],
            selection_args[4],
        ],
    )?;
    assert_success(&lock, "semantic lock generation with transient selections");
    let lock: toml::Value = toml::from_str(&fs::read_to_string(tmp.path().join("incan.lock"))?)?;
    assert_eq!(lock["semantic"]["sdk"]["profile"].as_str(), Some("minimal"));
    let locked_package = lock["semantic"]["packages"]
        .as_array()
        .and_then(|packages| packages.first())
        .ok_or("semantic lock did not contain the root package")?;
    assert_eq!(
        locked_package["active_features"].as_array(),
        Some(&vec![toml::Value::String("beta".to_string())])
    );

    Ok(())
}

#[test]
fn locked_build_rejects_package_feature_or_sdk_projection_drift() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "semantic_lock_drift",
        r#"

[project.features]
alpha = []
beta = []
"#,
    )?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;

    let lock = run_incan(
        tmp.path(),
        &[
            "lock",
            main_arg,
            "--no-default-features",
            "--features",
            "alpha",
            "--sdk-profile",
            "minimal",
        ],
    )?;
    assert_success(&lock, "lock semantic alpha/minimal projection");

    for (feature, profile) in [("beta", "minimal"), ("alpha", "default")] {
        let build = run_incan(
            tmp.path(),
            &[
                "build",
                main_arg,
                "--locked",
                "--no-default-features",
                "--features",
                feature,
                "--sdk-profile",
                profile,
            ],
        )?;
        assert_failure(
            &build,
            &format!("locked build with drifted {feature}/{profile} projection"),
        );
        let stderr = String::from_utf8_lossy(&build.stderr);
        assert!(
            stderr.contains("incan.lock") && stderr.contains("out of date") && stderr.contains("Run `incan lock`"),
            "locked projection drift should fail as stale lock state:\n{stderr}"
        );
    }

    Ok(())
}

#[test]
fn codegraph_importer_example_consumes_compiler_jsonl_issue776() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let source_dir = tmp.path().join("source");
    fs::create_dir_all(&source_dir)?;
    fs::write(
        source_dir.join("incan.toml"),
        r#"[project]
name = "codegraph_importer_source"
version = "0.1.0"
"#,
    )?;
    let source_main = source_dir.join("main.incn");
    fs::write(
        &source_main,
        r#"from std.registry import Registry, SubjectKind, describe

@derive(Clone, Eq)
type FunctionId = newtype str

@derive(Descriptor)
model FunctionSpec:
    summary: str

pub static functions: Registry[FunctionId, FunctionSpec] = Registry.define(
    subjects=[SubjectKind.Function],
)

@describe(functions, FunctionId("greet"), FunctionSpec(summary="Greet a named user"))
pub def greet(name: str) -> str:
    return f"hello {name}"

def main() -> None:
    println(greet("Incan"))
"#,
    )?;

    let graph = run_incan(
        &source_dir,
        &[
            "inspect",
            "codegraph",
            source_main.to_str().ok_or("source path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&graph, "compiler codegraph export for importer example");
    let graph_records = parse_jsonl_stdout(&graph)?;
    assert_codegraph_record_contract(&graph_records);

    let importer_dir = tmp.path().join("importer");
    let importer_src = importer_dir.join("src");
    fs::create_dir_all(&importer_src)?;
    fs::write(
        importer_dir.join("incan.toml"),
        include_str!("../examples/pro/codegraph_importer/incan.toml"),
    )?;
    fs::write(
        importer_src.join("importer.incn"),
        include_str!("../examples/pro/codegraph_importer/src/importer.incn"),
    )?;
    fs::write(
        importer_src.join("main.incn"),
        include_str!("../examples/pro/codegraph_importer/src/main.incn"),
    )?;
    fs::write(importer_dir.join("codegraph.jsonl"), &graph.stdout)?;

    let first = run_incan(&importer_dir, &["run", "src/main.incn"])?;
    assert_success(&first, "Incan-authored codegraph importer example");
    let second = run_incan(&importer_dir, &["run", "src/main.incn"])?;
    assert_success(&second, "second Incan-authored codegraph importer example");
    assert_eq!(first.stdout, second.stdout, "importer summary must be deterministic");

    let summary = parse_json_stdout(&first)?;
    assert_eq!(summary["schema_version"], serde_json::json!(7));
    assert_eq!(summary["mode"], serde_json::json!("strict"));
    assert_eq!(summary["metadata_record_count"], serde_json::json!(1));
    assert!(
        summary["fact_count"].as_i64().is_some_and(|count| count > 0),
        "importer must observe compiler-owned graph facts: {summary}"
    );
    assert!(
        summary["declaration_count"].as_i64().is_some_and(|count| count > 0),
        "importer must preserve declaration records without parsing source itself: {summary}"
    );
    assert!(
        summary["registry_count"].as_i64().is_some_and(|count| count > 0),
        "importer must preserve compiler-checked typed registry facts: {summary}"
    );

    fs::write(
        importer_dir.join("codegraph.jsonl"),
        concat!(
            r#"{"record":"header","schema_version":1,"mode":"strict","degraded":false}"#,
            "\n",
            r#"{"record":"file","degraded":false}"#,
            "\n",
        ),
    )?;
    let legacy = run_incan(&importer_dir, &["run", "src/main.incn"])?;
    assert_success(&legacy, "schema-v1 codegraph importer compatibility");
    let legacy_summary = parse_json_stdout(&legacy)?;
    assert_eq!(legacy_summary["schema_version"], serde_json::json!(1));
    assert_eq!(legacy_summary["file_count"], serde_json::json!(1));

    Ok(())
}

#[test]
fn inspect_codegraph_tolerant_directory_keeps_parseable_facts_and_diagnostics() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    fs::write(
        tmp.path().join("ok.incn"),
        r#"pub def ok() -> int:
    return 1
"#,
    )?;
    let nested = tmp.path().join("nested");
    fs::create_dir_all(&nested)?;
    fs::write(
        nested.join("extra.incn"),
        r#"pub def extra() -> int:
    return 2
"#,
    )?;
    fs::write(tmp.path().join("broken.incn"), "def broken(:\n")?;

    let strict = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            tmp.path().to_str().ok_or("directory path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_failure(&strict, "strict incan inspect codegraph should reject broken source");

    let tolerant = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            tmp.path().to_str().ok_or("directory path was not valid UTF-8")?,
            "--format",
            "jsonl",
            "--allow-errors",
        ],
    )?;
    assert_success(&tolerant, "tolerant incan inspect codegraph");
    let records = parse_jsonl_stdout(&tolerant)?;
    assert_codegraph_record_contract(&records);
    assert_eq!(records[0]["degraded"], serde_json::json!(true));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("declaration")
            && record["name"] == serde_json::json!("ok")
            && record["provenance"] == serde_json::json!("syntax")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("module")
            && record["module_path"] == serde_json::json!(["nested", "extra"])
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("diagnostic")
            && record["code"] == serde_json::json!("INCAN-P0001")
            && record["phase"] == serde_json::json!("parse")
    }));

    Ok(())
}

#[test]
fn inspect_codegraph_strict_directory_rejects_semantic_diagnostics() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::write(
        tmp.path().join("bad.incn"),
        r#"pub def bad() -> int:
    return missing()
"#,
    )?;

    let strict = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            tmp.path().to_str().ok_or("directory path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_failure(
        &strict,
        "strict incan inspect codegraph should reject directory typecheck diagnostics",
    );
    let strict_stderr = String::from_utf8_lossy(&strict.stderr);
    assert!(
        strict_stderr.contains("Unknown symbol 'missing'"),
        "expected strict directory codegraph to report typecheck diagnostic, got:\n{strict_stderr}"
    );

    let tolerant = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            tmp.path().to_str().ok_or("directory path was not valid UTF-8")?,
            "--format",
            "jsonl",
            "--allow-errors",
        ],
    )?;
    assert_success(
        &tolerant,
        "tolerant incan inspect codegraph should keep syntax facts for directory typecheck diagnostics",
    );
    let records = parse_jsonl_stdout(&tolerant)?;
    assert_codegraph_record_contract(&records);
    assert_eq!(records[0]["degraded"], serde_json::json!(true));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("declaration")
            && record["name"] == serde_json::json!("bad")
            && record["provenance"] == serde_json::json!("syntax")
            && record["canonical_identity"] == serde_json::Value::Null
            && record["degraded"] == serde_json::json!(true)
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["callee"] == serde_json::json!("missing")
            && record["target_id"] == serde_json::Value::Null
            && record["canonical_identity"] == serde_json::Value::Null
            && record["provenance"] == serde_json::json!("syntax")
            && record["degraded"] == serde_json::json!(true)
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("diagnostic")
            && record["code"] == serde_json::json!("INCAN-T0001")
            && record["phase"] == serde_json::json!("typecheck")
            && record["message"] == serde_json::json!("Unknown symbol 'missing'")
    }));

    Ok(())
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
requires-incan = ">=0.6.0-0,<0.7.0"

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
    let parsed = incan::lockfile::IncanLock::load(&tmp.path().join("incan.lock"))?;
    assert_eq!(
        parsed.cargo_lock_payload, "version = 4\n",
        "normal `incan lock` records semantic Incan state, not a generated Cargo resolution"
    );

    let second_output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&second_output, "second incan lock");
    let second_lock = fs::read_to_string(tmp.path().join("incan.lock"))?;
    assert_eq!(lock, second_lock, "relocking unchanged inputs must be deterministic");
    Ok(())
}

#[cfg(unix)]
#[test]
fn lock_generates_semantic_state_without_starting_cargo() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "cli_guarded_lock_project", "")?;
    let marker = tmp.path().join("cargo-was-started");

    let output = run_incan_with_failing_cargo_guard(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
        &tmp.path().join("cargo-guard"),
        &marker,
    )?;

    assert_success(&output, "Cargo-guarded incan lock");
    assert!(
        !marker.exists(),
        "normal incan lock must not launch Cargo; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lock = incan::lockfile::IncanLock::load(&tmp.path().join("incan.lock"))?;
    assert_eq!(lock.cargo_lock_payload, "version = 4\n");
    Ok(())
}

#[test]
fn lock_records_oven_interop_requirements_and_detects_input_drift() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "oven_interop_lock",
        r#"

[oven.interop]
schema = 1

[[oven.interop.targets]]
target = "x86_64-unknown-linux-gnu"
toolchain = { capability = "clang", version = ">=18, <19" }
headers = ["interop/include/bridge.h"]
definitions = ["FIXTURE=1"]

[[oven.interop.targets.artifacts]]
name = "fixture"
kind = "static"
path = "interop/lib/libfixture.a"

[[oven.interop.targets.shims]]
name = "fixture_bridge"
language = "c"
sources = ["interop/src/bridge.c"]
headers = ["interop/include/bridge.h"]
output = "fixture_bridge"
"#,
    )?;
    fs::create_dir_all(tmp.path().join("interop/include"))?;
    fs::create_dir_all(tmp.path().join("interop/src"))?;
    fs::create_dir_all(tmp.path().join("interop/lib"))?;
    fs::write(tmp.path().join("interop/include/bridge.h"), "int bridge(void);\n")?;
    fs::write(
        tmp.path().join("interop/src/bridge.c"),
        "int bridge(void) { return 7; }\n",
    )?;
    fs::write(tmp.path().join("interop/lib/libfixture.a"), b"fixture archive")?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;
    let incan_home = tmp.path().join(".incan-home");
    let incan_home = incan_home.to_str().ok_or("Incan home was not valid UTF-8")?;

    let lock_output = run_incan_with_env(tmp.path(), &["lock", main_arg], &[("INCAN_HOME", incan_home)])?;
    assert_success(&lock_output, "incan lock with declared Oven interop requirements");
    let lock: toml::Value = toml::from_str(&fs::read_to_string(tmp.path().join("incan.lock"))?)?;
    let target = lock["semantic"]["oven"]["interop"]
        .as_array()
        .and_then(|targets| targets.first())
        .ok_or("lock did not contain Oven interop requirements")?;
    assert_eq!(target["target"].as_str(), Some("x86_64-unknown-linux-gnu"));
    assert_eq!(target["toolchain"]["capability"].as_str(), Some("clang"));
    assert_eq!(target["toolchain"]["version"].as_str(), Some(">=18, <19"));
    assert_eq!(
        target["headers"]
            .as_array()
            .and_then(|headers| headers.first())
            .and_then(|header| header["path"].as_str()),
        Some("interop/include/bridge.h")
    );
    assert_eq!(
        target["shims"]
            .as_array()
            .and_then(|shims| shims.first())
            .and_then(|shim| shim["sources"].as_array())
            .and_then(|sources| sources.first())
            .and_then(|source| source["path"].as_str()),
        Some("interop/src/bridge.c")
    );

    fs::write(
        tmp.path().join("interop/src/bridge.c"),
        "int bridge(void) { return 8; }\n",
    )?;
    let stale = run_incan_with_env(
        tmp.path(),
        &["build", main_arg, "--locked"],
        &[("INCAN_HOME", incan_home)],
    )?;
    assert_failure(&stale, "locked build after declared interop input drift");
    assert!(
        String::from_utf8_lossy(&stale.stderr).contains("incan.lock is out of date"),
        "declared interop input drift should invalidate the lock:\n{}",
        String::from_utf8_lossy(&stale.stderr)
    );
    Ok(())
}

#[test]
fn lock_records_android_platform_requirements_without_selecting_a_local_sdk() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "oven_android_platform_lock",
        r#"

[oven.interop]
schema = 1

[[oven.interop.targets]]
target = "aarch64-linux-android"
toolchain = { capability = "android-ndk", version = ">=29, <30" }
sdk = { capability = "android", version = ">=36, <37" }

[oven.interop.targets.platform]
kind = "android"
api-level = 34
"#,
    )?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;

    let lock_output = run_incan(tmp.path(), &["lock", main_arg])?;
    assert_success(&lock_output, "incan lock with Android platform requirements");
    let lock: toml::Value = toml::from_str(&fs::read_to_string(tmp.path().join("incan.lock"))?)?;
    let target = lock["semantic"]["oven"]["interop"]
        .as_array()
        .and_then(|targets| targets.first())
        .ok_or("lock did not contain an Android Oven interop target")?;
    assert_eq!(target["target"].as_str(), Some("aarch64-linux-android"));
    assert_eq!(target["sdk"]["capability"].as_str(), Some("android"));
    assert_eq!(target["platform"]["kind"].as_str(), Some("android"));
    assert_eq!(target["platform"]["api-level"].as_integer(), Some(34));
    Ok(())
}

#[test]
fn inspect_interop_plan_is_locked_complete_and_relocatable() -> Result<(), Box<dyn std::error::Error>> {
    // ---- Declare representative Android deployment requirements ----
    let tmp = tempfile::tempdir()?;
    let package = tmp.path().join("package");
    fs::create_dir_all(&package)?;
    let _main_path = write_minimal_project(
        &package,
        "interop_plan_handoff",
        r#"

[sdk]
profile = "minimal"

[oven.interop]
schema = 1

[[oven.interop.targets]]
target = "aarch64-linux-android"
toolchain = { capability = "android-ndk", version = ">=29, <30" }
sdk = { capability = "android", version = ">=36, <37" }
headers = ["interop/include/runtime.h"]
definitions = ["TFLITE_STATIC_MEMORY=1"]

[oven.interop.targets.platform]
kind = "android"
api-level = 34

[[oven.interop.targets.artifacts]]
name = "llama"
kind = "static"
path = "interop/lib/libllama.a"
dependencies = ["tflite"]

[[oven.interop.targets.artifacts]]
name = "tflite"
kind = "bundled"
path = "interop/lib/libtensorflowlite_c.so"
runtime-name = "libtensorflowlite_c.so"
placement = "jniLibs/arm64-v8a"
minimum-platform = "21"
dependencies = ["log"]

[[oven.interop.targets.artifacts]]
name = "log"
kind = "system"
capability = "android.library.log"

[[oven.interop.targets.bindings]]
module = ["runtime"]
name = "Runtime"
artifacts = ["llama", "tflite", "log"]

[[oven.interop.targets.shims]]
name = "llama_bridge"
language = "cxx"
sources = ["interop/src/llama_bridge.cc"]
headers = ["interop/include/runtime.h"]
output = "llama_bridge"
"#,
    )?;
    fs::create_dir_all(package.join("interop/include"))?;
    fs::create_dir_all(package.join("interop/src"))?;
    fs::create_dir_all(package.join("interop/lib"))?;
    fs::write(package.join("interop/include/runtime.h"), "int runtime(void);\n")?;
    fs::write(
        package.join("interop/src/llama_bridge.cc"),
        "extern \"C\" int runtime(void) { return 0; }\n",
    )?;
    fs::write(package.join("interop/lib/libllama.a"), b"llama archive")?;
    fs::write(
        package.join("interop/lib/libtensorflowlite_c.so"),
        b"tflite shared object",
    )?;
    // ---- Lock and inspect the complete structured requirement handoff ----
    write_locked_oven_interop_plan(&package)?;
    let output = run_incan(
        &package,
        &[
            "inspect",
            "interop-plan",
            "--target",
            "aarch64-linux-android",
            "--format",
            "json",
            ".",
        ],
    )?;
    assert_success(&output, "locked Android interop plan inspection");
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(plan["schema_version"].as_u64(), Some(3));
    assert!(
        plan["locked_target_identity"]
            .as_str()
            .is_some_and(|identity| identity.starts_with("sha256:")),
        "interop deployment plan must retain the portable locked-target join identity: {plan}"
    );
    assert_eq!(plan["target"].as_str(), Some("aarch64-linux-android"));
    assert_eq!(plan["toolchain"]["capability"].as_str(), Some("android-ndk"));
    assert_eq!(plan["sdk"]["capability"].as_str(), Some("android"));
    assert_eq!(plan["platform"]["kind"].as_str(), Some("android"));
    assert_eq!(plan["platform"]["api_level"].as_u64(), Some(34));
    assert_eq!(plan["include_roots"][0].as_str(), Some("interop/include"));
    assert_eq!(
        plan["artifacts"]
            .as_array()
            .ok_or("interop plan artifacts were not an array")?
            .iter()
            .filter_map(|artifact| artifact["name"].as_str())
            .collect::<Vec<_>>(),
        ["log", "tflite", "llama"]
    );
    assert_eq!(plan["artifacts"][0]["deployment"].as_str(), Some("system"));
    assert_eq!(plan["artifacts"][0]["capability"].as_str(), Some("android.library.log"));
    assert_eq!(plan["artifacts"][1]["deployment"].as_str(), Some("bundle"));
    assert_eq!(plan["artifacts"][1]["placement"].as_str(), Some("jniLibs/arm64-v8a"));
    assert_eq!(plan["artifacts"][2]["deployment"].as_str(), Some("static_link"));
    assert_eq!(plan["bindings"][0]["module"], serde_json::json!(["runtime"]));
    assert_eq!(plan["bindings"][0]["name"], serde_json::json!("Runtime"));
    assert_eq!(
        plan["bindings"][0]["artifacts"],
        serde_json::json!(["llama", "log", "tflite"])
    );
    assert_eq!(plan["shims"][0]["output"].as_str(), Some("llama_bridge"));
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains(&package.to_string_lossy().to_string()),
        "interop plan leaked its original package location"
    );

    let unknown = run_incan(
        &package,
        &["inspect", "interop-plan", "--target", "aarch64-apple-ios", "."],
    )?;
    assert_failure(&unknown, "undeclared interop plan target");
    assert!(
        String::from_utf8_lossy(&unknown.stderr).contains("is not declared and locked by this package"),
        "unexpected undeclared interop-plan diagnostic:\n{}",
        String::from_utf8_lossy(&unknown.stderr)
    );

    // ---- Preserve the plan across relocation and reject stale locked bytes ----
    let relocated = tmp.path().join("relocated");
    fs::rename(&package, &relocated)?;
    let relocated_output = run_incan(
        &relocated,
        &[
            "inspect",
            "interop-plan",
            "--target",
            "aarch64-linux-android",
            "--format",
            "json",
            ".",
        ],
    )?;
    assert_success(&relocated_output, "relocated interop plan inspection");
    assert_eq!(
        output.stdout, relocated_output.stdout,
        "relocating a locked interop package changed its deployment handoff"
    );

    fs::write(
        relocated.join("interop/lib/libtensorflowlite_c.so"),
        b"changed tflite shared object",
    )?;
    let stale = run_incan(
        &relocated,
        &["inspect", "interop-plan", "--target", "aarch64-linux-android", "."],
    )?;
    assert_failure(&stale, "stale interop plan inspection");
    assert!(
        String::from_utf8_lossy(&stale.stderr).contains("incan.lock Oven interop requirements are out of date"),
        "unexpected stale interop-plan diagnostic:\n{}",
        String::from_utf8_lossy(&stale.stderr)
    );
    Ok(())
}

#[test]
fn inspect_interop_plan_uses_the_selected_workspace_member_lock_projection() -> Result<(), Box<dyn std::error::Error>> {
    // ---- Declare one Oven interop workspace member ----
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("incan.toml"),
        "[workspace]\nmembers = [\"packages/mobile\"]\n",
    )?;
    let member = root.path().join("packages/mobile");
    let _main_path = write_minimal_project(
        &member,
        "mobile",
        r#"

[sdk]
profile = "minimal"

[oven.interop]
schema = 1

[[oven.interop.targets]]
target = "aarch64-apple-ios"
toolchain = { capability = "apple-clang", version = ">=17, <18" }
sdk = { capability = "iphoneos", version = ">=18, <19" }
headers = ["interop/include/accelerate_bridge.h"]

[oven.interop.targets.platform]
kind = "ios"
deployment-target = "13.0"

[[oven.interop.targets.artifacts]]
name = "accelerate"
kind = "system"
capability = "apple.framework.Accelerate"
"#,
    )?;
    fs::create_dir_all(member.join("interop/include"))?;
    fs::write(
        member.join("interop/include/accelerate_bridge.h"),
        "float incan_dot(const float *left, const float *right, unsigned long count);\n",
    )?;
    // ---- Publish the one canonical workspace lock ----
    write_locked_workspace_oven_interop_plan(root.path(), &member)?;
    assert!(
        root.path().join("incan.lock").is_file() && !member.join("incan.lock").exists(),
        "interop workspace fixture did not publish exactly one canonical root lock"
    );

    // ---- Inspect the selected member through its root-lock projection ----
    let output = run_incan(
        root.path(),
        &[
            "inspect",
            "interop-plan",
            "packages/mobile",
            "--target",
            "aarch64-apple-ios",
            "--format",
            "json",
        ],
    )?;
    assert_success(&output, "workspace member interop plan inspection");
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(plan["target"].as_str(), Some("aarch64-apple-ios"));
    assert_eq!(plan["platform"]["kind"].as_str(), Some("ios"));
    assert_eq!(plan["artifacts"][0]["deployment"].as_str(), Some("system"));
    assert_eq!(
        plan["artifacts"][0]["capability"].as_str(),
        Some("apple.framework.Accelerate")
    );
    Ok(())
}

#[test]
fn check_verifies_c_bindings_against_a_declared_android_interop_target() -> Result<(), Box<dyn std::error::Error>> {
    let Some(clang) = c_abi_test_clang() else {
        return Ok(());
    };
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "declared_android_c_abi_check",
        r#"

[sdk]
profile = "minimal"

[oven.interop]
schema = 1

[[oven.interop.targets]]
target = "aarch64-linux-android"
toolchain = { capability = "android-ndk", version = ">=29, <30" }
sdk = { capability = "android", version = ">=36, <37" }
definitions = ["INCAN_ANDROID_FIXTURE=1"]

[oven.interop.targets.platform]
kind = "android"
api-level = 34
"#,
    )?;
    let header = tmp.path().join("android_fixture.h");
    fs::write(
        &header,
        "#ifndef INCAN_ANDROID_FIXTURE\n#error expected Android target definition\n#endif\ntypedef struct fixture_pair { int left; int right; } fixture_pair;\n#define FIXTURE_OK 0\nint fixture_abs(int value);\n",
    )?;
    fs::write(
        &main_path,
        format!(
            "from std.interop import c\n\nbinding Fixture:\n    header = \"{}\"\n    link = c.system_library(\"c\")\n\n    symbol absolute(value: c.i32) -> c.i32:\n        native = \"fixture_abs\"\n\n    enum Status:\n        OK: c.i32 = FIXTURE_OK\n\n    struct Pair:\n        native = \"fixture_pair\"\n        left: c.i32 = left\n        right: c.i32 = right\n\ndef main() -> None:\n    assert Fixture.Status.OK == 0\n",
            header.display()
        ),
    )?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;

    let output = run_incan_with_env(
        tmp.path(),
        &["check", "--interop-target", "aarch64-linux-android", main_arg],
        &[("INCAN_C_ABI_CLANG", clang.as_str())],
    )?;
    assert_success(&output, "declared Android C ABI verification");
    Ok(())
}

#[test]
fn check_rejects_an_undeclared_interop_target() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "undeclared_interop_target", "")?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;

    let output = run_incan(
        tmp.path(),
        &["check", "--interop-target", "aarch64-linux-android", main_arg],
    )?;
    assert_failure(&output, "undeclared Oven interop target selection");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("requires an [oven.interop] declaration in incan.toml"),
        "unexpected undeclared-target diagnostic:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[cfg(target_os = "macos")]
#[test]
fn check_verifies_c_bindings_against_a_declared_ios_interop_target() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "declared_ios_c_abi_check",
        r#"

[sdk]
profile = "minimal"

[oven.interop]
schema = 1

[[oven.interop.targets]]
target = "aarch64-apple-ios"
toolchain = { capability = "apple-clang", version = ">=17, <18" }
sdk = { capability = "iphoneos", version = ">=18, <19" }
definitions = ["INCAN_IOS_FIXTURE=1"]

[oven.interop.targets.platform]
kind = "ios"
deployment-target = "13.0"
"#,
    )?;
    let header = tmp.path().join("ios_fixture.h");
    fs::write(
        &header,
        "#include <stdint.h>\n#ifndef INCAN_IOS_FIXTURE\n#error expected iOS target definition\n#endif\ntypedef struct fixture_pair { int32_t left; int32_t right; } fixture_pair;\n#define FIXTURE_OK 0\nint32_t fixture_abs(int32_t value);\n",
    )?;
    fs::write(
        &main_path,
        format!(
            "from std.interop import c\n\nbinding Fixture:\n    header = \"{}\"\n    link = c.system_library(\"c\")\n\n    symbol absolute(value: c.i32) -> c.i32:\n        native = \"fixture_abs\"\n\n    enum Status:\n        OK: c.i32 = FIXTURE_OK\n\n    struct Pair:\n        native = \"fixture_pair\"\n        left: c.i32 = left\n        right: c.i32 = right\n\ndef main() -> None:\n    assert Fixture.Status.OK == 0\n",
            header.display()
        ),
    )?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;

    let output = run_incan_with_env_and_removed(
        tmp.path(),
        &["check", "--interop-target", "aarch64-apple-ios", main_arg],
        &[],
        &["INCAN_C_ABI_CLANG"],
    )?;
    assert_success(&output, "declared iOS C ABI verification");
    Ok(())
}

#[test]
fn semantic_lock_records_registry_dependency_input_changes() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "registry_resolution_lock",
        r#"
[rust-dependencies]
bitflags = "=1.3.2"
"#,
    )?;
    fs::write(
        &main_path,
        "rust.module(\"bitflags\")\n\n\ndef main() -> None:\n  pass\n",
    )?;

    let first_output = run_incan_with_env(tmp.path(), &["lock"], &[("INCAN_LOCK_PREHEAT", "0")])?;
    assert_success(&first_output, "canonical lock with bitflags 1.3.2");
    let first_bytes = fs::read(tmp.path().join("incan.lock"))?;
    let first = incan::lockfile::IncanLock::load(&tmp.path().join("incan.lock"))?;
    assert_eq!(
        first.cargo_lock_payload, "version = 4\n",
        "normal lock generation must not resolve a Cargo package graph"
    );

    let manifest_path = tmp.path().join("incan.toml");
    let first_manifest = fs::read_to_string(&manifest_path)?;
    fs::write(&manifest_path, first_manifest.replace("=1.3.2", "=2.11.0"))?;
    let second_output = run_incan_with_env(tmp.path(), &["lock"], &[("INCAN_LOCK_PREHEAT", "0")])?;
    assert_success(&second_output, "canonical lock with bitflags 2.11.0");
    let second_bytes = fs::read(tmp.path().join("incan.lock"))?;
    let second = incan::lockfile::IncanLock::load(&tmp.path().join("incan.lock"))?;
    assert_eq!(second.cargo_lock_payload, "version = 4\n");
    assert_ne!(
        first.deps_fingerprint, second.deps_fingerprint,
        "the semantic dependency fingerprint must change with the declared registry input"
    );
    assert_ne!(
        first_bytes, second_bytes,
        "the published canonical lock must change byte-for-byte"
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
        tmp.path().join("incan.lock").is_file(),
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
fn build_lib_reuses_canonical_lock_when_manifest_dependency_is_unused() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let _main_path = write_minimal_project(
        tmp.path(),
        "cli_library_unused_manifest_dependency",
        r#"
[rust-dependencies]
serde_json = "1"
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("lib.incn"),
        "pub def exported_value() -> int:\n  return 7\n",
    )?;

    let build = run_incan(tmp.path(), &["build", "--lib"])?;
    assert_success(&build, "incan build --lib with an unused manifest Rust dependency");
    let generated_manifest = fs::read_to_string(tmp.path().join("target/lib/Cargo.toml"))?;
    assert!(
        !generated_manifest.contains("serde_json"),
        "unused manifest dependencies must not expand the generated library beyond the canonical reachable graph:\n\
         {generated_manifest}"
    );
    Ok(())
}

#[test]
fn cold_library_build_preserves_rust_string_compound_assignment_issue896() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let stdlib_crate = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/incan_stdlib");
    let stdlib_path = stdlib_crate.to_string_lossy().replace('\\', "\\\\");
    let _main_path = write_minimal_project(
        tmp.path(),
        "cold_rust_string_compound_assignment",
        &format!(
            r#"
[sdk]
profile = "minimal"

[rust-dependencies.incan_stdlib]
path = "{stdlib_path}"
"#,
        ),
    )?;
    fs::write(
        tmp.path().join("src/lib.incn"),
        r#"from rust::incan_stdlib::strings import str_slice_byte_range


pub def append_range(text: str, start: int, end: int) -> str:
  mut out = ""
  out += str_slice_byte_range(text, start, end)
  return out


pub def join_ranges(text: str, start: int, middle: int, end: int) -> str:
  return str_slice_byte_range(text, start, middle) + str_slice_byte_range(text, middle, end)
"#,
    )?;

    assert!(
        !tmp.path().join("target").exists(),
        "the regression must begin without a project-local Rust metadata cache"
    );
    let build = run_incan_with_env(tmp.path(), &["build", "--lib"], &[("INCAN_RUST_INSPECT_PREWARM", "0")])?;
    assert_success(
        &build,
        "cold library build with a direct Rust String compound assignment",
    );

    let generated = fs::read_to_string(tmp.path().join("target/lib/src/lib.rs"))?;
    let compact_generated = generated.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact_generated.contains("out=incan_stdlib::strings::str_concat(")
            && compact_generated.contains("&str_slice_byte_range(&text,start,end),"),
        "cold Rust metadata must select string-aware compound-assignment lowering:\n{generated}"
    );
    assert!(
        !compact_generated.contains("out=out+str_slice_byte_range"),
        "a direct Rust String result must not reach generated Rust's owned `String + String` path:\n{generated}"
    );
    assert!(
        compact_generated.contains("incan_stdlib::strings::str_concat(&str_slice_byte_range(&text,start,middle),&str_slice_byte_range(&text,middle,end),)"),
        "binary concatenation of direct Rust String results must use the string helper:\n{generated}"
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
fn default_build_and_test_leave_stale_lockfile_unchanged() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "cli_default_stale_lock_project", "")?;
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
    assert_success(&lock_output, "incan lock before default build");
    let stale_lock = stale_lockfile_without_changing_cargo_payload(tmp.path())?;

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;

    assert_success(&build_output, "incan build with stale lockfile by default");
    assert_eq!(
        fs::read_to_string(tmp.path().join("incan.lock"))?,
        stale_lock,
        "default build must not rewrite an existing stale incan.lock"
    );

    let test_output = run_incan(tmp.path(), &["test"])?;
    assert_success(&test_output, "incan test with stale lockfile by default");
    assert_eq!(
        fs::read_to_string(tmp.path().join("incan.lock"))?,
        stale_lock,
        "default test must not rewrite an existing stale incan.lock"
    );
    Ok(())
}

#[test]
fn build_assert_string_inequality_in_list_loop_issue739() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
        r#"[project]
name = "list_str_loop_assert_compare"
version = "0.1.0"
"#,
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"
def validate(values: list[str], target: str) -> None:
    for value in values:
        assert value != target, "duplicate"


def main() -> None:
    validate(["a"], "b")
"#,
    )?;

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&build_output, "incan build for assert string inequality in list loop");
    Ok(())
}

#[test]
fn build_union_widening_converts_generated_wrappers_issue741() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
        r#"[project]
name = "union_widening_conversion"
version = "0.1.0"
"#,
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"
pub model A:
    pub value: str


pub model B:
    pub value: str


pub model Holder:
    pub value: Extended


pub type Base = Union[A, B]
pub type Extra = Union[int, A]
pub type Extended = Union[Base, Extra, B]


pub def make_base() -> Base:
    return A(value="x")


pub def accept_extended(value: Extended) -> Extended:
    return value


pub def widen_argument(value: Base) -> Extended:
    return accept_extended(value)


pub def widen_assignment(value: Base) -> Extended:
    widened: Extended = value
    return widened


pub def widen_field(value: Base) -> Extended:
    holder = Holder(value=value)
    return holder.value


pub def widen_list_item(value: Base) -> None:
    values: list[Extended] = [value]
    return


pub def widen_return() -> Extended:
    return make_base()


pub def base_from_alias_pattern(value: Extended) -> Base:
    match value:
        Base(expr) => return expr
        int(number) => return A(value=f"{number}")


pub def keep_base(value: Base) -> bool:
    return true


pub def base_from_guarded_alias_pattern(value: Extended) -> Base:
    match value:
        case Base(expr) if keep_base(expr):
            return expr
        case Base(expr):
            return expr
        case int(number):
            return A(value=f"{number}")


pub def base_from_explicit_variants(value: Extended) -> Base:
    match value:
        A(expr) => return expr
        B(expr) => return expr
        int(number) => return A(value=f"{number}")


pub def base_from_fallback_binding(value: Extended) -> Base:
    match value:
        int(number) => return A(value=f"{number}")
        other => return other


pub def main() -> None:
    source = make_base()
    accept_extended(source)
    accept_extended(make_base())
    accept_extended(widen_argument(source))
    accept_extended(widen_assignment(source))
    accept_extended(widen_field(source))
    widen_list_item(source)
    accept_extended(widen_return())
    accept_extended(base_from_alias_pattern(source))
    accept_extended(base_from_guarded_alias_pattern(source))
    accept_extended(base_from_explicit_variants(source))
    accept_extended(base_from_fallback_binding(source))
    return
"#,
    )?;

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "incan build for union widening generated wrapper conversion",
    );

    let generated_main = fs::read_to_string(tmp.path().join("target/incan/union_widening_conversion/src/main.rs"))?;
    assert!(
        generated_main.contains("match make_base()"),
        "expected generated Rust to convert call-result union wrappers through a match, got:\n{generated_main}"
    );
    assert!(
        generated_main.contains("__incan_union_value"),
        "expected generated Rust to rebuild the wider union wrapper variant-by-variant, got:\n{generated_main}"
    );

    let imported_root = tmp.path().join("union_imported_alias");
    let imported_src = imported_root.join("src");
    fs::create_dir_all(&imported_src)?;
    fs::write(
        imported_root.join("incan.toml"),
        r#"[project]
name = "union_imported_alias"
version = "0.1.0"
"#,
    )?;
    fs::write(
        imported_src.join("types.incn"),
        r#"
pub model A:
    pub value: str


pub model B:
    pub value: str


pub type Base = Union[A, B]
"#,
    )?;
    fs::write(
        imported_src.join("normalizer.incn"),
        r#"
from types import A, Base


pub type Input = Union[Base, int]


pub def normalize(value: Input) -> Base:
    match value:
        int(number) => return A(value=f"{number}")
        expr => return expr
"#,
    )?;
    fs::write(
        imported_src.join("main.incn"),
        r#"
from normalizer import normalize
from types import A


pub def main() -> None:
    normalize(A(value="x"))
    normalize(1)
    return
"#,
    )?;
    let imported_main = imported_src.join("main.incn");
    let imported_build = run_incan(
        &imported_root,
        &[
            "build",
            imported_main
                .to_str()
                .ok_or("imported alias main path was not valid UTF-8")?,
        ],
    )?;
    assert_success(
        &imported_build,
        "incan build for imported alias fallback union narrowing issue741",
    );

    let producer_root = tmp.path().join("union_lib");
    let producer_src = producer_root.join("src");
    fs::create_dir_all(&producer_src)?;
    fs::write(
        producer_root.join("incan.toml"),
        r#"[project]
name = "union_lib"
version = "0.1.0"
"#,
    )?;
    fs::write(
        producer_src.join("defs.incn"),
        r#"
pub model A:
    pub value: str


pub model B:
    pub value: str


pub type Base = Union[A, B]
pub type Extra = Union[int, A]
pub type Extended = Union[Base, Extra, B]


pub def make_base() -> Base:
    return A(value="x")


pub def accept_extended(value: Extended) -> Extended:
    return value
"#,
    )?;
    fs::write(
        producer_src.join("lib.incn"),
        r#"pub from defs import accept_extended, make_base
"#,
    )?;
    let producer_build = run_explicit_oven_bake(&producer_root)?;
    assert_success(&producer_build, "explicit Oven bake for public union widening issue741");

    let consumer_root = tmp.path().join("union_consumer");
    let consumer_main = write_minimal_project(
        &consumer_root,
        "union_consumer",
        r#"
[dependencies]
union_lib = { path = "../union_lib" }
"#,
    )?;
    fs::write(
        &consumer_main,
        r#"from pub::union_lib import accept_extended, make_base


def main() -> None:
    accept_extended(make_base())
    return
"#,
    )?;
    let consumer_bake = run_explicit_oven_bake(&consumer_root)?;
    assert_success(
        &consumer_bake,
        "explicit Oven bake for public union widening consumer issue741",
    );
    let consumer_build = run_incan(
        &consumer_root,
        &[
            "build",
            consumer_main.to_str().ok_or("consumer main path was not valid UTF-8")?,
        ],
    )?;
    assert_success(&consumer_build, "pub consumer build for public union widening issue741");

    let generated_consumer = fs::read_to_string(consumer_root.join("target/incan/union_consumer/src/main.rs"))?;
    assert!(
        generated_consumer.contains("match union_lib::make_base()"),
        "expected public consumer to convert dependency-owned union call results through a match, got:\n{generated_consumer}"
    );
    assert!(
        generated_consumer.contains("union_lib::__IncanUnion"),
        "expected public consumer union conversion to use dependency-owned wrapper paths, got:\n{generated_consumer}"
    );
    Ok(())
}

#[test]
fn build_pub_helper_wraps_union_call_result_as_option_payload_issue745() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let producer_root = tmp.path().join("querykit");
    let producer_src = producer_root.join("src");
    fs::create_dir_all(&producer_src)?;
    fs::write(
        producer_root.join("incan.toml"),
        r#"[project]
name = "querykit"
version = "0.1.0"
"#,
    )?;
    fs::write(
        producer_src.join("defs.incn"),
        r#"
pub model IntExpr:
    pub value: int


pub model TextExpr:
    pub value: str


pub type Value = Union[IntExpr, TextExpr]


pub def lit(value: int) -> Value:
    return IntExpr(value=value)


pub def fallback() -> Value:
    return TextExpr(value="fallback")


pub def accept_optional(value: Option[Value] = None) -> Value:
    return fallback()


pub def combine(first: Value, second: Option[Value] = None) -> Value:
    return first
"#,
    )?;
    fs::write(
        producer_src.join("lib.incn"),
        r#"pub from defs import accept_optional, combine, fallback, lit
"#,
    )?;
    let producer_build = run_explicit_oven_bake(&producer_root)?;
    assert_success(&producer_build, "explicit Oven bake for optional union helper issue745");

    let consumer_root = tmp.path().join("consumer");
    let consumer_main = write_minimal_project(
        &consumer_root,
        "optional_union_consumer",
        r#"
[dependencies]
querykit = { path = "../querykit" }
"#,
    )?;
    fs::write(
        &consumer_main,
        r#"from pub::querykit import accept_optional, combine, lit


def main() -> None:
    accept_optional(lit(2))
    combine(lit(1), lit(2))
    combine(lit(1), second=lit(3))
    return
"#,
    )?;
    let consumer_bake = run_explicit_oven_bake(&consumer_root)?;
    assert_success(
        &consumer_bake,
        "explicit Oven bake for optional union helper consumer issue745",
    );
    let consumer_build = run_incan(
        &consumer_root,
        &[
            "build",
            consumer_main.to_str().ok_or("consumer main path was not valid UTF-8")?,
        ],
    )?;
    assert_success(&consumer_build, "pub consumer build for optional union helper issue745");

    let generated_consumer =
        fs::read_to_string(consumer_root.join("target/incan/optional_union_consumer/src/main.rs"))?;
    assert!(
        generated_consumer.contains("querykit::accept_optional(Some(querykit::lit(2)))"),
        "expected public optional helper call to wrap the dependency-owned union result in Some, got:\n{generated_consumer}"
    );
    assert!(
        generated_consumer.contains("querykit::combine(querykit::lit(1), Some(querykit::lit(2)))"),
        "expected positional optional union argument to be wrapped in Some, got:\n{generated_consumer}"
    );
    assert!(
        generated_consumer.contains("querykit::combine(querykit::lit(1), Some(querykit::lit(3)))"),
        "expected named optional union argument to be wrapped in Some, got:\n{generated_consumer}"
    );
    Ok(())
}

#[test]
fn build_pub_method_accepts_dependency_owned_union_alias_payload_issue755() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let producer_root = tmp.path().join("union_provider");
    let producer_src = producer_root.join("src");
    fs::create_dir_all(&producer_src)?;
    fs::write(
        producer_root.join("incan.toml"),
        r#"[project]
name = "union_provider"
version = "0.1.0"
"#,
    )?;
    fs::write(
        producer_src.join("surface.incn"),
        r#"
pub model ColumnRefExpr:
    pub name: str


pub model NumberColumnExpr:
    pub expr: ColumnRefExpr


pub model SortExpr:
    pub expr: ColumnRefExpr


pub type ColumnExpr = Union[ColumnRefExpr, NumberColumnExpr, SortExpr]
pub type NumberValueOrColumn = Union[ColumnRefExpr, NumberColumnExpr, int]


pub model Frame:
    pub source: str

    def filter(self, predicate: ColumnExpr) -> Self:
        return self

    def order_by(self, columns: list[ColumnExpr]) -> Self:
        return self


pub def frame() -> Frame:
    return Frame(source="orders")


pub def col(name: str) -> ColumnRefExpr:
    return ColumnRefExpr(name=name)


pub def add(left: NumberValueOrColumn, right: NumberValueOrColumn) -> NumberColumnExpr:
    return NumberColumnExpr(expr=col("sum"))


pub def desc(expr: ColumnExpr) -> ColumnExpr:
    return SortExpr(expr=col("sorted"))
"#,
    )?;
    fs::write(
        producer_src.join("lib.incn"),
        r#"pub from surface import ColumnExpr, ColumnRefExpr, Frame, NumberColumnExpr, NumberValueOrColumn, SortExpr, add, col, desc, frame
"#,
    )?;
    let producer_build = run_explicit_oven_bake(&producer_root)?;
    assert_success(
        &producer_build,
        "explicit Oven bake for dependency-owned union boundary issue755",
    );

    let consumer_root = tmp.path().join("union_consumer");
    let consumer_main = write_minimal_project(
        &consumer_root,
        "union_consumer",
        r#"
[dependencies]
union_provider = { path = "../union_provider" }
"#,
    )?;
    fs::write(
        &consumer_main,
        r#"from pub::union_provider import add as __incan_vocab_helper_union_provider_add
from pub::union_provider import col as __incan_vocab_helper_union_provider_col
from pub::union_provider import desc as __incan_vocab_helper_union_provider_desc
from pub::union_provider import frame as __incan_vocab_helper_union_provider_frame


def main() -> None:
    __incan_vocab_helper_union_provider_frame().filter(
        __incan_vocab_helper_union_provider_add(__incan_vocab_helper_union_provider_col("amount"), 5),
    )
    __incan_vocab_helper_union_provider_frame().order_by([
        __incan_vocab_helper_union_provider_desc(__incan_vocab_helper_union_provider_col("amount")),
    ])
    return
"#,
    )?;
    let consumer_bake = run_explicit_oven_bake(&consumer_root)?;
    assert_success(
        &consumer_bake,
        "explicit Oven bake for dependency-owned union consumer issue755",
    );
    let consumer_build = run_incan(
        &consumer_root,
        &[
            "build",
            consumer_main.to_str().ok_or("consumer main path was not valid UTF-8")?,
        ],
    )?;
    assert_success(
        &consumer_build,
        "pub consumer build for dependency-owned union boundary issue755",
    );

    let generated_consumer = fs::read_to_string(consumer_root.join("target/incan/union_consumer/src/main.rs"))?;
    assert!(
        generated_consumer.contains("union_provider::__IncanUnion"),
        "expected public method call to use dependency-owned wrapper paths, got:\n{generated_consumer}"
    );
    assert!(
        generated_consumer.contains("union_provider::desc(union_provider::__IncanUnion"),
        "expected public union-return helper call to use dependency-owned wrapper paths, got:\n{generated_consumer}"
    );
    assert!(
        !generated_consumer.contains("crate::__IncanUnion"),
        "expected public consumer not to re-own dependency union wrappers, got:\n{generated_consumer}"
    );
    assert!(
        !generated_consumer.contains("pub enum __IncanUnion"),
        "expected public consumer not to emit local duplicate dependency union wrappers, got:\n{generated_consumer}"
    );
    Ok(())
}

#[test]
fn build_narrowed_union_fallback_helper_calls_issue743() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "narrowed_fallback_call", "")?;
    fs::write(
        &main_path,
        r#"
pub model A:
    pub value: str


pub model B:
    pub value: str


pub model C:
    pub value: str


pub type Expr = Union[A, B, C]


pub def describe(expr: Expr) -> str:
    return "expr"


pub def combine(left: Expr, right: Expr) -> str:
    return "both"


pub def fallback_describe(expr: Expr) -> str:
    match expr:
        A(value) => return value.value
        _ => return describe(expr)


pub def fallback_binding_describe(expr: Expr) -> str:
    match expr:
        A(value) => return value.value
        other => return combine(expr, other)


pub def main() -> None:
    fallback_describe(B(value="b"))
    fallback_describe(C(value="c"))
    fallback_binding_describe(B(value="b"))
    fallback_binding_describe(C(value="c"))
    return
"#,
    )?;

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&build_output, "incan build for narrowed fallback helper calls issue743");
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
    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake_output, "explicit Oven bake for all declared project entrypoints");

    let main_after_default_lock = run_incan(
        tmp.path(),
        &[
            "run",
            "--locked",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
        ],
    )?;
    assert_success(&main_after_default_lock, "incan run --locked main after default lock");
    assert_no_stale_warning(&main_after_default_lock, "incan run --locked main after default lock");
    assert_eq!(
        String::from_utf8_lossy(&main_after_default_lock.stdout).trim(),
        "1",
        "the conventional main target must replay its own sealed executable"
    );

    let locked_test_after_default_lock = run_incan(tmp.path(), &["test", "--locked"])?;
    assert_success(
        &locked_test_after_default_lock,
        "incan test --locked after default lock",
    );
    assert_no_stale_warning(
        &locked_test_after_default_lock,
        "incan test --locked after default lock",
    );

    let extra_after_default_lock = run_incan(
        tmp.path(),
        &[
            "run",
            "--locked",
            extra_path.to_str().ok_or("extra path was not valid UTF-8")?,
        ],
    )?;
    assert_success(&extra_after_default_lock, "incan run --locked extra after default lock");
    assert_no_stale_warning(&extra_after_default_lock, "incan run --locked extra after default lock");
    assert_eq!(
        String::from_utf8_lossy(&extra_after_default_lock.stdout).trim(),
        "2",
        "the declared extra target must replay its own sealed executable"
    );

    let main_report_output = run_incan(
        tmp.path(),
        &[
            "build",
            "--locked",
            "--report",
            "json",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
        ],
    )?;
    assert_success(&main_report_output, "sealed main build report after one explicit bake");
    let main_report = parse_json_stdout(&main_report_output)?;
    let extra_report_output = run_incan(
        tmp.path(),
        &[
            "build",
            "--locked",
            "--report",
            "json",
            extra_path.to_str().ok_or("extra path was not valid UTF-8")?,
        ],
    )?;
    assert_success(
        &extra_report_output,
        "sealed extra build report after one explicit bake",
    );
    let extra_report = parse_json_stdout(&extra_report_output)?;
    assert_eq!(
        main_report["entrypoint"],
        serde_json::json!(main_path.to_string_lossy()),
        "main replay report selected the wrong entrypoint"
    );
    assert_eq!(
        extra_report["entrypoint"],
        serde_json::json!(extra_path.to_string_lossy()),
        "extra replay report selected the wrong entrypoint"
    );
    let binary_path = |report: &serde_json::Value| {
        report["artifacts"]
            .as_array()
            .and_then(|artifacts| artifacts.iter().find(|artifact| artifact["kind"] == "binary"))
            .and_then(|artifact| artifact["path"].as_str())
            .map(str::to_string)
    };
    let main_binary = binary_path(&main_report).ok_or("main report had no binary artifact")?;
    let extra_binary = binary_path(&extra_report).ok_or("extra report had no binary artifact")?;
    assert_ne!(
        main_binary, extra_binary,
        "distinct declared scripts must retain distinct caller-visible native outputs"
    );

    let extra_lock_output = run_incan(
        tmp.path(),
        &["lock", extra_path.to_str().ok_or("extra path was not valid UTF-8")?],
    )?;
    assert_success(&extra_lock_output, "incan lock extra");

    let test_after_extra_lock = run_incan(tmp.path(), &["test", "--locked"])?;
    assert_success(&test_after_extra_lock, "incan test --locked after extra lock");
    assert_no_stale_warning(&test_after_extra_lock, "incan test --locked after extra lock");

    Ok(())
}

#[test]
fn rust_generic_interop_scenarios_share_one_project() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "cli_generic_rust_param_scenarios",
        r#"

[rust-dependencies]
arc_callback = { path = "rust/arc_callback" }
generic_helpers = { path = "rust/generic_helpers" }
prost = { path = "rust/prost" }
prost-types = { path = "rust/prost-types" }
reexport_identity = { path = "rust/reexport_identity" }
stream_host = { path = "rust/stream_host" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from arc_callback import arc_callback_case, match_arm_callback_case
from borrowed_generic import borrowed_generic_case
from by_value_decode import by_value_decode_case
from cross_crate_decode import cross_crate_decode_case
from method_arity import method_arity_case
from reexport_identity import reexport_identity_case
from trait_by_value_decode import trait_by_value_decode_case

def main() -> None:
  println(arc_callback_case())
  println(match_arm_callback_case())
  println(borrowed_generic_case())
  println(by_value_decode_case())
  println(trait_by_value_decode_case())
  println(cross_crate_decode_case())
  println(reexport_identity_case())
  method_arity_case()
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("arc_callback.incn"),
        r#"from rust::arc_callback import CallbackError, ColumnarValue, DataType, ScalarFunctionImplementation, ScalarUDF, SliceCallback, Volatility, create_simple_udf, create_udf, create_udf_full
from rust::std::sync import Arc

def callback(args: list[ColumnarValue]) -> Result[ColumnarValue, CallbackError]:
  return Ok(args[0].clone())

def inline_arc_callback_value() -> int:
  match create_simple_udf(callback=Arc.from((args) => callback(args.to_vec())), name="inline"):
    Ok(value) => return value.value()
    Err(_) => return -1

def inline_datafusion_shaped_callback_value() -> int:
  match create_udf_full(
    name="sha1",
    input_types=[DataType.Utf8],
    return_type=DataType.Utf8,
    volatility=Volatility.Immutable,
    fun=Arc.from((args) => callback(args.to_vec())),
  ):
    Ok(value) => return value.value()
    Err(_) => return -1

pub def arc_callback_case() -> str:
  implementation: SliceCallback = Arc.from((args) => callback(args.to_vec()))
  match create_simple_udf(callback=implementation, name="assigned"):
    Ok(value) => return f"arc_callback:{value.value()}:{inline_arc_callback_value()}:{inline_datafusion_shaped_callback_value()}"
    Err(_) => return "arc_callback:err"

@derive(Clone)
enum ReproFunction(str):
  First = "first"
  Second = "second"

def make_udf(function: ReproFunction) -> ScalarUDF:
  match function:
    ReproFunction.First =>
      return create_udf(
        name=function.value(),
        input_types=[DataType.Utf8],
        return_type=DataType.Utf8,
        volatility=Volatility.Immutable,
        fun=Arc.from((args) => callback(args.to_vec())),
      )
    ReproFunction.Second =>
      return create_udf(
        name=function.value(),
        input_types=[DataType.Utf8],
        return_type=DataType.Utf8,
        volatility=Volatility.Immutable,
        fun=Arc.from((args) => callback(args.to_vec())),
      )

pub def match_arm_callback_case() -> str:
  first = make_udf(ReproFunction.First)
  second = make_udf(ReproFunction.Second)
  return f"match-callback:{first.value()}:{second.value()}"
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("borrowed_generic.incn"),
        r#"from rust::generic_helpers::borrow import takes_ref

model Payload:
  name: str

pub def borrowed_generic_case() -> str:
  payload = Payload(name="demo")
  return f"borrowed:{takes_ref(payload)}"
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("by_value_decode.incn"),
        r#"from rust::generic_helpers::inherent_decode import FileDescriptorSet
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
        r#"from rust::generic_helpers::trait_decode import FileDescriptorSet, Message

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
  match FileDescriptorSet.decode(encoded):
    Ok(_) => return "cross_crate:ok"
    Err(_) => return "cross_crate:err"
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("reexport_identity.incn"),
        r#"from rust::reexport_identity import Expr as RustExpr, ScalarFunction as RustScalarFunction, registry

pub def reexport_identity_case() -> str:
  state = registry()
  udf = state.udf()
  args: list[RustExpr] = []
  _ = RustExpr.ScalarFunction(RustScalarFunction.new_udf(udf, args))
  return "reexport_identity:ok"
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("method_arity.incn"),
        r#"from rust::stream_host import DeviceTrait, OutputCallbackInfo, device

def consume(_value: f32) -> None:
  pass

def write_silence(_data: &mut list[f32], _info: &OutputCallbackInfo) -> None:
  pass

def report_error(_error: str) -> None:
  pass

pub def method_arity_case() -> None:
  stream = device()
  stream.build_output_stream[f32, _, _](1.0, consume, consume)
  println("stream-built")
  stream.run[f32, _, _](write_silence, report_error)
  stream.run[f32, _, _]((_data, _info) => println(len(_data)), report_error)
  println("callbacks-built")
"#,
    )?;
    // Keep this fixture DataFusion-shaped but crate-light. The real DataFusion crate is far too expensive for a
    // compiler regression test; the behavior under test is the Rust metadata shape:
    // `ScalarFunctionImplementation -> SliceCallback -> Arc<dyn Fn(...)>`. The same fixture exercises both
    // assigned/inline callback coercion and #733's match-arm closure context.
    let helper_src = tmp.path().join("rust").join("arc_callback").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("arc_callback src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "arc_callback"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        r#"use std::sync::Arc;

#[derive(Clone)]
pub struct ColumnarValue {
    value: i64,
}

impl ColumnarValue {
    pub fn new(value: i64) -> Self {
        Self { value }
    }

    pub fn value(&self) -> i64 {
        self.value
    }
}

pub struct CallbackError;

pub type SliceCallback = Arc<dyn Fn(&[ColumnarValue]) -> Result<ColumnarValue, CallbackError> + Send + Sync>;
pub type ScalarFunctionImplementation = crate::SliceCallback;

#[derive(Clone)]
pub struct ScalarUDF {
    value: i64,
}

impl ScalarUDF {
    pub fn value(&self) -> i64 {
        self.value
    }
}

#[derive(Clone)]
pub enum DataType {
    Utf8,
}

#[derive(Clone)]
pub enum Volatility {
    Immutable,
}

pub fn invoke(callback: SliceCallback) -> Result<ColumnarValue, CallbackError> {
    let args = vec![ColumnarValue::new(7)];
    callback(&args)
}

pub fn create_simple_udf(name: &str, callback: crate::SliceCallback) -> Result<ColumnarValue, CallbackError> {
    let _ = name;
    let args = vec![ColumnarValue::new(11)];
    callback(&args)
}

pub fn create_udf_full(
    name: &str,
    input_types: Vec<DataType>,
    return_type: DataType,
    volatility: Volatility,
    fun: crate::ScalarFunctionImplementation,
) -> Result<ColumnarValue, CallbackError> {
    let _ = name;
    let _ = input_types;
    let _ = return_type;
    let _ = volatility;
    let args = vec![ColumnarValue::new(13)];
    fun(&args)
}

pub fn create_udf(
    name: &str,
    input_types: Vec<DataType>,
    return_type: DataType,
    volatility: Volatility,
    fun: crate::ScalarFunctionImplementation,
) -> ScalarUDF {
    let _ = name;
    let _ = input_types;
    let _ = return_type;
    let _ = volatility;
    let args = vec![ColumnarValue::new(13)];
    let value = match fun(&args) {
        Ok(value) => value.value(),
        Err(_) => -1,
    };
    ScalarUDF { value }
}
"#,
    )?;
    // These three isolated helper crates used to force separate package and
    // metadata walks in an already single-project regression. Their import
    // routes remain distinct Rust modules, while one fixture crate now owns
    // the shared package boundary.
    let helper_src = tmp.path().join("rust").join("generic_helpers").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("helper src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "generic_helpers"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        r#"pub mod borrow {
    pub fn takes_ref<TValue>(_value: &TValue) -> i64 {
        1
    }
}

pub mod inherent_decode {
    pub trait DecodeBuf {}

    impl DecodeBuf for std::io::Cursor<Vec<u8>> {}

    pub struct DecodeError;

    pub struct FileDescriptorSet;

    impl FileDescriptorSet {
        pub fn decode<T: DecodeBuf>(_buf: T) -> Result<Self, DecodeError> {
            Ok(Self)
        }
    }
}

pub mod trait_decode {
    pub trait DecodeBuf {}

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
    let reexport_identity_src = tmp.path().join("rust").join("reexport_identity").join("src");
    fs::create_dir_all(&reexport_identity_src)?;
    fs::write(
        reexport_identity_src
            .parent()
            .ok_or("reexport_identity src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "reexport_identity"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        reexport_identity_src.join("lib.rs"),
        r#"use std::sync::Arc;

pub mod udf {
    pub struct ScalarUDF;
}

pub use udf::ScalarUDF;

pub struct FunctionRegistry;

pub fn registry() -> FunctionRegistry {
    FunctionRegistry
}

impl FunctionRegistry {
    pub fn udf(&self) -> Arc<udf::ScalarUDF> {
        Arc::new(udf::ScalarUDF)
    }
}

pub struct Expr;
pub struct ScalarFunction;

impl ScalarFunction {
    pub fn new_udf(_udf: Arc<ScalarUDF>, _args: Vec<Expr>) -> Self {
        Self
    }
}

impl Expr {
    #[allow(non_snake_case)]
    pub fn ScalarFunction(_function: ScalarFunction) -> Self {
        Self
    }
}
"#,
    )?;

    let stream_host_src = tmp.path().join("rust").join("stream_host").join("src");
    fs::create_dir_all(&stream_host_src)?;
    fs::write(
        stream_host_src
            .parent()
            .ok_or("stream host source directory had no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "stream_host"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        stream_host_src.join("lib.rs"),
        r#"pub struct Device;
pub struct OutputCallbackInfo;

pub fn device() -> Device {
    Device
}

pub trait DeviceTrait {
    fn build_output_stream<T, D, E>(&self, value: T, data_callback: D, error_callback: E)
    where
        T: Copy,
        D: FnMut(T),
        E: FnMut(T);

    fn run<T, D, E>(&self, data_callback: D, error_callback: E)
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

    fn run<T, D, E>(&self, mut data_callback: D, mut error_callback: E)
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
"#,
    )?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(
        &bake_output,
        "explicit Oven bake for grouped generic Rust interop scenarios",
    );
    let output = run_incan(
        tmp.path(),
        &["run", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;

    assert_success(&output, "incan run with grouped generic Rust interop scenarios");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim(),
        "arc_callback:11:11:13\nmatch-callback:13:13\nborrowed:1\nby_value:ok\ntrait_by_value:ok\ncross_crate:ok\nreexport_identity:ok\nstream-built\n2\ncallbacks-built",
        "expected grouped generic Rust interop output, got:\n{stdout}"
    );
    Ok(())
}

#[test]
fn rust_method_into_bound_keeps_string_argument_inferable_issue804() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "cli_rust_method_into_bound",
        r#"

[rust-dependencies.into_method_helper]
path = "rust/into_method_helper"
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::into_method_helper import Tokenizer

def main() -> None:
  tokenizer = Tokenizer.new()
  println(tokenizer.encode("hello world", false))
"#,
    )?;

    let helper_src = tmp.path().join("rust").join("into_method_helper").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("into_method_helper src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "into_method_helper"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        r#"pub struct Tokenizer;

impl Tokenizer {
    pub fn new() -> Self {
        Self
    }

    pub fn encode<E: Into<String>>(&self, input: E, uppercase: bool) -> String {
        let text = input.into();
        if uppercase {
            text.to_uppercase()
        } else {
            text
        }
    }
}
"#,
    )?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake_output, "explicit Oven bake for a Rust Into-bound method argument");
    let output = run_incan(
        tmp.path(),
        &["run", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&output, "incan run with a Rust Into-bound method argument");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "hello world",
        "Rust Into-bound method should receive the original string type"
    );

    let generated = fs::read_to_string(tmp.path().join("target/incan/cli_rust_method_into_bound/src/main.rs"))?;
    assert!(
        generated.contains("tokenizer.encode(\"hello world\", false)"),
        "unresolved Rust method generic should preserve the string literal shape, got:\n{generated}"
    );
    assert!(
        !generated.contains("tokenizer.encode(\"hello world\".into(), false)"),
        "unresolved Rust method generic must not emit an ambiguous `.into()`, got:\n{generated}"
    );
    Ok(())
}

#[test]
fn test_runner_prefers_project_sibling_import_over_unimported_stdlib_stub_type()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_root = tmp.path();
    fs::write(
        project_root.join("incan.toml"),
        r#"[project]
name = "stdhash_sibling_collision"
version = "0.1.0"
"#,
    )?;

    let src_dir = project_root.join("src");
    let functions_dir = src_dir.join("functions");
    let hashing_dir = functions_dir.join("hashing");
    let session_dir = src_dir.join("session");
    let tests_dir = project_root.join("tests");
    fs::create_dir_all(&hashing_dir)?;
    fs::create_dir_all(&session_dir)?;
    fs::create_dir_all(&tests_dir)?;

    fs::write(
        hashing_dir.join("expr.incn"),
        r#"pub model Expr:
    pub value: int
"#,
    )?;
    fs::write(
        hashing_dir.join("sha224.incn"),
        r#"from functions.hashing.expr import Expr

pub def sha224(expr: Expr) -> Expr:
    return expr
"#,
    )?;
    fs::write(
        hashing_dir.join("sha2.incn"),
        r#"from functions.hashing.expr import Expr
from functions.hashing.sha224 import sha224

pub def sha2(expr: Expr) -> Expr:
    return sha224(expr)
"#,
    )?;
    fs::write(
        functions_dir.join("mod.incn"),
        r#"pub from functions.hashing.expr import Expr
pub from functions.hashing.sha224 import sha224
pub from functions.hashing.sha2 import sha2
"#,
    )?;
    fs::write(
        session_dir.join("bridge.incn"),
        r#"from std.hash import sha1 as std_sha1

pub def digest(data: bytes) -> bytes:
    return std_sha1.digest(data)
"#,
    )?;
    fs::write(
        session_dir.join("mod.incn"),
        r#"pub from session.bridge import digest
"#,
    )?;
    fs::write(
        src_dir.join("lib.incn"),
        r#"pub from functions import Expr, sha224, sha2
pub from session import digest
"#,
    )?;
    fs::write(
        tests_dir.join("test_collision.incn"),
        r#"from functions import Expr, sha2
from session import digest

def test_collision__sibling_import_wins() -> None:
    payload = Expr(value=1)
    assert len(digest(b"abc")) > 0
    assert sha2(payload).value == 1
"#,
    )?;

    let output = run_incan(project_root, &["test", "tests"])?;
    assert_success(
        &output,
        "incan test should keep project sibling imports ahead of unimported stdlib stub helper types",
    );
    Ok(())
}

#[test]
fn test_runner_resolves_imported_stdlib_enum_patterns_from_enum_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_root = tmp.path();
    fs::write(
        project_root.join("incan.toml"),
        r#"[project]
name = "stdlib_enum_pattern_metadata"
version = "0.1.0"
"#,
    )?;

    let src_dir = project_root.join("src");
    let substrait_dir = src_dir.join("substrait");
    let session_dir = src_dir.join("session");
    let tests_dir = project_root.join("tests");
    fs::create_dir_all(&substrait_dir)?;
    fs::create_dir_all(&session_dir)?;
    fs::create_dir_all(&tests_dir)?;

    fs::write(
        substrait_dir.join("schema.incn"),
        r#"pub enum PrimitiveKind(str):
    Bool = "bool"
    String = "string"
"#,
    )?;
    fs::write(
        session_dir.join("json_schema.incn"),
        r#"from std.json import JsonKind, JsonValue
from substrait.schema import PrimitiveKind

pub def primitive_kind() -> PrimitiveKind:
    return PrimitiveKind.Bool

pub def schema_name(value: JsonValue) -> str:
    match value.kind():
        JsonKind.Bool => return "BOOLEAN"
        JsonKind.String => return "STRING"
        _ => return "OTHER"
"#,
    )?;
    fs::write(
        session_dir.join("mod.incn"),
        r#"pub from session.json_schema import primitive_kind, schema_name
"#,
    )?;
    fs::write(
        src_dir.join("lib.incn"),
        r#"pub from session import primitive_kind, schema_name
"#,
    )?;
    fs::write(
        tests_dir.join("test_json_schema.incn"),
        r#"from session import primitive_kind, schema_name
from std.json import JsonValue

def test_stdlib_enum_patterns_survive_colliding_project_variants() -> None:
    assert primitive_kind().value() == "bool"
    assert schema_name(JsonValue.bool(True)) == "BOOLEAN"
    assert schema_name(JsonValue.string("x")) == "STRING"
"#,
    )?;

    let output = run_incan(project_root, &["test", "tests"])?;
    assert_success(
        &output,
        "incan test should resolve imported stdlib enum patterns from enum-owned metadata",
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
    pub name: str
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
fn run_generic_reflection_contracts_issues712_715_819() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "generic_reflection_contracts", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    fs::write(
        src_dir.join("generic_reflection_helpers.incn"),
        r#"pub def imported_field_count[T](value: T) -> int:
    return len(value.__fields__())


pub def imported_class_name[T](value: T) -> str:
    return str(value.__class_name__())
"#,
    )?;
    fs::write(
        src_dir.join("schema_helpers.incn"),
        r#"pub def class_name_for[T]() -> str:
    return T.__class_name__()


pub def field_count_for[T]() -> int:
    return len(T.__fields__())


pub def print_schema[T]() -> None:
    println(str(T.__class_name__()))
    for info in T.__fields__():
        println(f"{info.name}|{info.wire_name}|{info.type_name}|{info.has_default}")
"#,
    )?;
    fs::write(
        src_dir.join("reflection_helpers.incn"),
        r#"def requires_clone[T with Clone]() -> str:
    return "clone"


pub def reflected_schema_marker[T]() -> str:
    return f"{T.__class_name__()}:{len(T.__fields__())}:{requires_clone[T]()}"
"#,
    )?;
    fs::write(
        &main_path,
        r#"from generic_reflection_helpers import imported_class_name, imported_field_count
from schema_helpers import class_name_for as schema_class_name_for, field_count_for as schema_field_count_for, print_schema
from reflection_helpers import reflected_schema_marker


model NamedRow:
    name: str


class Bare:
    value: int


model MySchema:
    id [description="Stable id"]: int
    status [alias="state"]: str = "new"


class BareSchema:
    value: int


model Row:
    id: int
    status: str
    paid: bool


model ProbeRow:
    id: int
    score: float
    active: bool
    label: str
    optional_label: Option[str]


def summarize_lookup[T](rows: list[T]) -> str:
    mut parts: list[str] = []
    if len(rows) == 0:
        return ",".join(parts)
    for field in T.__fields__():
        match rows[0].__field_value__(str(field.wire_name)):
            Some(value) =>
                parts.append(f"{field.wire_name}={value}")
            None =>
                parts.append(f"{field.wire_name}=<missing>")
    return "|".join(parts)


def summarize_items[T](rows: list[T]) -> str:
    mut parts: list[str] = []
    if len(rows) == 0:
        return ",".join(parts)
    for name, value in rows[0].__field_items__():
        parts.append(f"{name}={value}")
    return "|".join(parts)


def show_items[T](row: T) -> None:
    for name, value in row.__field_items__():
        println(f"{name}={value}")


class InlineSession:
    def reflected_summary[T](self, rows: list[T]) -> str:
        mut names: list[str] = []
        for field in T.__fields__():
            names.append(str(field.wire_name))
        if len(rows) == 0:
            return f"{T.__class_name__()}:{','.join(names)}:<empty>"
        match rows[0].__field_value__("label"):
            Some(value) => return f"{T.__class_name__()}:{','.join(names)}:{value}"
            None => return f"{T.__class_name__()}:{','.join(names)}:<missing>"


static decorated_names: list[str] = []


def register[F]() -> ((F) -> F):
    return (func) => remember[F](func)


def remember[F](func: F) -> F:
    decorated_names.append(func.__name__)
    return func


@register()
def decorated_class_name_for[T]() -> str:
    return str(T.__class_name__())


@register()
def decorated_field_count_for[T]() -> int:
    return len(T.__fields__())


def requires_clone[T with Clone]() -> str:
    return "clone"


@register()
def clone_marker_for[T]() -> str:
    return requires_clone[T]()


@register()
def imported_reflection_for[T]() -> str:
    return reflected_schema_marker[T]()


def reflected_field_count[T](value: T) -> int:
    return len(value.__fields__())


def reflected_class_name[T](value: T) -> str:
    return str(value.__class_name__())


def local_field_count[T]() -> int:
    return len(T.__fields__())


def main() -> None:
    named = NamedRow(name="Ada")
    println(reflected_class_name(named))
    println(reflected_field_count(named))
    println(imported_class_name(named))
    println(imported_field_count(named))
    bare = Bare(value=1)
    println(bare.__class_name__())
    println(len(bare.__fields__()))
    println(reflected_class_name(bare))
    println(reflected_field_count(bare))
    println(imported_class_name(bare))
    println(imported_field_count(bare))
    println(schema_class_name_for[MySchema]())
    println(schema_field_count_for[MySchema]())
    println(local_field_count[MySchema]())
    print_schema[MySchema]()
    println(schema_class_name_for[BareSchema]())
    println(schema_field_count_for[BareSchema]())
    rows = [Row(id=1, status="paid", paid=true)]
    println(summarize_lookup[Row](rows))
    println(summarize_items[Row](rows))
    show_items[ProbeRow](ProbeRow(id=7, score=3.5, active=true, label="paid", optional_label=None))
    show_items[ProbeRow](ProbeRow(id=8, score=4.25, active=false, label="late", optional_label=Some("x")))
    session = InlineSession()
    println(session.reflected_summary[ProbeRow]([ProbeRow(id=7, score=3.5, active=true, label="paid", optional_label=None)]))
    println(decorated_class_name_for[MySchema]())
    println(decorated_field_count_for[MySchema]())
    println(clone_marker_for[MySchema]())
    println(imported_reflection_for[MySchema]())
    println(imported_reflection_for[MySchema]())
    println(decorated_names[0])
    println(decorated_names[1])
    println(decorated_names[2])
    println(decorated_names[3])
    println(len(decorated_names))
"#,
    )?;

    let run_output = run_incan(
        tmp.path(),
        &["run", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &run_output,
        "incan run for generic reflection contracts issues712/715/819",
    );
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec![
            "NamedRow",
            "1",
            "NamedRow",
            "1",
            "Bare",
            "0",
            "Bare",
            "0",
            "Bare",
            "0",
            "MySchema",
            "2",
            "2",
            "MySchema",
            "id|id|int|false",
            "status|state|str|true",
            "BareSchema",
            "0",
            "id=1|status=paid|paid=true",
            "id=1|status=paid|paid=true",
            "id=7",
            "score=3.5",
            "active=true",
            "label=paid",
            "optional_label=None",
            "id=8",
            "score=4.25",
            "active=false",
            "label=late",
            "optional_label=x",
            "ProbeRow:id,score,active,label,optional_label:paid",
            "MySchema",
            "2",
            "clone",
            "MySchema:2:clone",
            "MySchema:2:clone",
            "decorated_class_name_for",
            "decorated_field_count_for",
            "clone_marker_for",
            "imported_reflection_for",
            "4",
        ],
        "unexpected generic reflection contracts output:\n{stdout}"
    );
    Ok(())
}

#[test]
fn run_direct_type_token_contracts_issue750() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "direct_type_token_contracts_issue750", "")?;
    fs::write(
        &main_path,
        r#"pub def primitive_name[T]() -> str:
    return str(T.__class_name__())


pub def primitive_marker[T]() -> str:
    name = str(T.__class_name__())
    if name == "int":
        return "integer"
    if name == "float":
        return "floating"
    if name == "str":
        return "string"
    if name == "bool":
        return "boolean"
    return "other"


pub model ColumnExpr:
    pub name: str


pub model IntColumnExpr:
    pub source: str


pub model FloatColumnExpr:
    pub source: str


pub model StringColumnExpr:
    pub source: str


pub type NumberColumnExpr = Union[IntColumnExpr, FloatColumnExpr]


pub def col(name: str) -> ColumnExpr:
    return ColumnExpr(name=name)


pub def cast(expr: ColumnExpr, target: Type[int]) -> IntColumnExpr:
    return IntColumnExpr(source=expr.name)


pub def cast(expr: ColumnExpr, target: Type[float]) -> FloatColumnExpr:
    return FloatColumnExpr(source=expr.name)


pub def cast(expr: ColumnExpr, target: Type[str]) -> StringColumnExpr:
    return StringColumnExpr(source=expr.name)


pub def cast(expr: ColumnExpr, target: str) -> ColumnExpr:
    return ColumnExpr(name=f"{expr.name}:{target}")


pub safe_cast = alias cast


pub def mul(left: NumberColumnExpr, right: NumberColumnExpr) -> FloatColumnExpr:
    return FloatColumnExpr(source="mul")


model MySchema:
    id: int
    status: str


def accepts_schema_type(value: Type[MySchema]) -> str:
    return "schema-token"


def main() -> None:
    println(primitive_name[int]())
    println(primitive_name[float]())
    println(primitive_name[str]())
    println(primitive_name[bool]())
    println(primitive_marker[int]())
    println(primitive_marker[float]())
    println(primitive_marker[str]())
    println(primitive_marker[bool]())
    amount: IntColumnExpr = cast(col("amount"), int)
    unit_price: NumberColumnExpr = cast(col("unit_price"), float)
    total: FloatColumnExpr = mul(cast(col("unit_price"), float), cast(col("qty"), float))
    fallback: ColumnExpr = cast(col("amount"), "decimal(10,2)")
    safe: FloatColumnExpr = safe_cast(col("safe"), float)
    println(amount.source)
    println(safe.source)
    println(total.source)
    println(fallback.name)
    println(accepts_schema_type(MySchema))
"#,
    )?;

    let run_output = run_incan(
        tmp.path(),
        &["run", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&run_output, "incan run for direct type-token contracts issue750");
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec![
            "int",
            "float",
            "str",
            "bool",
            "integer",
            "floating",
            "string",
            "boolean",
            "amount",
            "safe",
            "mul",
            "amount:decimal(10,2)",
            "schema-token",
        ],
        "unexpected direct type-token contracts output:\n{stdout}"
    );
    Ok(())
}

#[test]
fn run_pub_type_token_contracts_issue750() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let producer_root = tmp.path().join("type_token_provider");
    let producer_src = producer_root.join("src");
    fs::create_dir_all(&producer_src)?;
    fs::write(
        producer_root.join("incan.toml"),
        r#"[project]
name = "type_token_provider"
version = "0.1.0"
"#,
    )?;
    fs::write(
        producer_src.join("type_names.incn"),
        r#"def register[F]() -> (F) -> F:
    return (func) => func


pub def primitive_name[T]() -> str:
    return str(T.__class_name__())


pub def primitive_marker[T]() -> str:
    name = str(T.__class_name__())
    if name == "int":
        return "integer"
    if name == "float":
        return "floating"
    if name == "str":
        return "string"
    if name == "bool":
        return "boolean"
    return "other"


@register()
pub def decorated_primitive_marker[T]() -> str:
    return primitive_marker[T]()
"#,
    )?;
    fs::write(
        producer_src.join("lib.incn"),
        r#"pub from type_names import decorated_primitive_marker, primitive_marker, primitive_name
pub from casts import ColumnExpr, FloatColumnExpr, IntColumnExpr, NumberColumnExpr, cast, col, mul, registered_cast_at, registered_cast_count
pub from safe_alias import safe_cast
"#,
    )?;
    fs::write(
        producer_src.join("casts.incn"),
        r#"pub model ColumnExpr:
    pub name: str


pub model IntColumnExpr:
    pub source: str


pub model FloatColumnExpr:
    pub source: str


pub model StringColumnExpr:
    pub source: str


pub type NumberColumnExpr = Union[IntColumnExpr, FloatColumnExpr]


pub static registered_casts: list[str] = []


def register_cast_float[F]() -> ((F) -> F):
    return (func) => remember_cast_float[F](func)


def register_cast_string[F]() -> ((F) -> F):
    return (func) => remember_cast_string[F](func)


def remember_cast_float[F](func: F) -> F:
    registered_casts.append(func.__name__)
    return func


def remember_cast_string[F](func: F) -> F:
    registered_casts.append(func.__name__)
    return func


pub def col(name: str) -> ColumnExpr:
    return ColumnExpr(name=name)


pub def cast(expr: ColumnExpr, target: Type[int]) -> IntColumnExpr:
    return IntColumnExpr(source=expr.name)


@register_cast_float()
pub def cast(expr: ColumnExpr, target: Type[float]) -> FloatColumnExpr:
    return FloatColumnExpr(source=expr.name)


pub def cast(expr: ColumnExpr, target: Type[str]) -> StringColumnExpr:
    return StringColumnExpr(source=expr.name)


@register_cast_string()
pub def cast(expr: ColumnExpr, target: str) -> ColumnExpr:
    return ColumnExpr(name=f"{expr.name}:{target}")


pub def mul(left: NumberColumnExpr, right: NumberColumnExpr) -> FloatColumnExpr:
    return FloatColumnExpr(source="mul")


pub def registered_cast_count() -> int:
    return len(registered_casts)


pub def registered_cast_at(index: int) -> str:
    return registered_casts[index]
"#,
    )?;
    fs::write(
        producer_src.join("safe_alias.incn"),
        r#"from casts import cast


pub safe_cast = alias cast
"#,
    )?;

    let producer_build = run_explicit_oven_bake(&producer_root)?;
    assert_success(
        &producer_build,
        "explicit Oven bake for public type-token contracts issue750",
    );

    let producer_tests = producer_root.join("tests");
    fs::create_dir_all(&producer_tests)?;
    fs::write(
        producer_tests.join("test_safe_cast.incn"),
        r#"from lib import ColumnExpr, FloatColumnExpr, col, registered_cast_at, registered_cast_count, safe_cast


def test_cross_module_alias_preserves_overload_set() -> None:
    typed: FloatColumnExpr = safe_cast(col("safe"), float)
    fallback: ColumnExpr = safe_cast(col("safe"), "float64")
    assert typed.source == "safe"
    assert fallback.name == "safe:float64"
    assert registered_cast_count() == 2
    assert registered_cast_at(0) == "cast"
    assert registered_cast_at(1) == "cast"
"#,
    )?;
    let producer_test = run_incan(&producer_root, &["test", "tests"])?;
    assert_success(
        &producer_test,
        "provider test batch for cross-module overloaded alias issue750",
    );

    let consumer_root = tmp.path().join("primitive_consumer");
    let consumer_main = write_minimal_project(
        &consumer_root,
        "type_token_consumer",
        r#"
[dependencies]
type_token_provider = { path = "../type_token_provider" }
"#,
    )?;
    fs::write(
        &consumer_main,
        r#"from pub::type_token_provider import ColumnExpr, FloatColumnExpr, IntColumnExpr, NumberColumnExpr, cast, col, decorated_primitive_marker, mul, primitive_marker, primitive_name, registered_cast_at, registered_cast_count, safe_cast


def main() -> None:
    println(primitive_name[str]())
    println(primitive_marker[int]())
    println(decorated_primitive_marker[bool]())
    amount: IntColumnExpr = cast(col("amount"), int)
    unit_price: NumberColumnExpr = cast(col("unit_price"), float)
    total: FloatColumnExpr = mul(cast(col("unit_price"), float), cast(col("qty"), float))
    fallback: ColumnExpr = cast(col("amount"), "decimal(10,2)")
    safe: FloatColumnExpr = safe_cast(col("safe"), float)
    println(amount.source)
    println(safe.source)
    println(total.source)
    println(fallback.name)
    println(str(registered_cast_count()))
    println(registered_cast_at(0))
    println(registered_cast_at(1))
"#,
    )?;

    let consumer_bake = run_explicit_oven_bake(&consumer_root)?;
    assert_success(
        &consumer_bake,
        "explicit Oven bake for type-token contracts consumer issue750",
    );

    let consumer_run = run_incan(
        &consumer_root,
        &[
            "run",
            consumer_main.to_str().ok_or("consumer main path was not valid UTF-8")?,
        ],
    )?;
    assert_success(&consumer_run, "public consumer run for type-token contracts issue750");
    let stdout = String::from_utf8_lossy(&consumer_run.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec![
            "str",
            "integer",
            "boolean",
            "amount",
            "safe",
            "mul",
            "amount:decimal(10,2)",
            "2",
            "cast",
            "cast",
        ],
        "unexpected public type-token contracts output:\n{stdout}"
    );
    Ok(())
}

#[test]
fn build_combined_rust_and_source_imports_preserves_never_return_issue381() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "combined_rust_and_source_imports_preserve_never_return",
        r#"

[rust-dependencies]
polyglot_probe = { path = "rust/polyglot_probe" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::polyglot_probe import DialectType
from prism import PrismCursor


def main() -> None:
    pass
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("prism.incn"),
        r#"from rust::incan_stdlib::errors import raise_value_error
from rust::std::primitive import i32 as RustI32


pub model PrismCursor:
    pub offset: int


def fail_to_lower() -> RustI32:
    return raise_value_error("cannot lower cursor")
"#,
    )?;

    let helper_src = tmp.path().join("rust").join("polyglot_probe").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("polyglot probe source directory had no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "polyglot_probe"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        r#"pub enum DialectType {
    PostgreSql,
}
"#,
    )?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake_output, "explicit Oven bake for combined Rust and source imports");
    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "generated Rust for combined imports with a diverging Rust helper",
    );
    Ok(())
}

#[test]
fn build_locked_map_err_string_literal_closure_issue880() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "map_err_string_literal_closure_issue880", "")?;
    fs::write(
        &main_path,
        r#"from std.json import JsonValue

def parse(source: str) -> Result[JsonValue, str]:
    return JsonValue.parse(source).map_err((_error) => "malformed_json")

def main() -> None:
    match parse("{"):
        Ok(_) => println("unexpected")
        Err(error) => println(error)
"#,
    )?;

    let lock_output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&lock_output, "incan lock for map_err closure issue880");

    let build_output = run_incan(
        tmp.path(),
        &[
            "build",
            "--locked",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
        ],
    )?;
    assert_success(&build_output, "incan build --locked for map_err closure issue880");

    let generated = fs::read_to_string(
        tmp.path()
            .join("target/incan/map_err_string_literal_closure_issue880/src/main.rs"),
    )?;
    assert!(
        generated.contains("map_err(|_error| \"malformed_json\".to_string())"),
        "expected generated Rust to own the map_err closure literal, got:\n{generated}"
    );
    Ok(())
}

/// Ensures f-string values compile through both borrowed and owned Rust interop boundaries.
#[test]
fn build_inline_fstring_rust_interop_variants_issue716() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let helper_dir = tmp.path().join("rust").join("tiny_error");
    fs::create_dir_all(helper_dir.join("src"))?;
    fs::write(
        helper_dir.join("Cargo.toml"),
        "[package]\nname = \"tiny_error\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(
        helper_dir.join("src").join("lib.rs"),
        r#"pub enum TinyError {
    Execution(String),
}

pub fn consume(err: TinyError) -> i64 {
    match err {
        TinyError::Execution(message) => message.len() as i64,
    }
}
"#,
    )?;
    let main_path = write_minimal_project(
        tmp.path(),
        "inline_fstring_rust_interop_variants_issue716",
        r#"
[rust-dependencies]
tiny_error = { path = "rust/tiny_error" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::incan_stdlib::errors import raise_value_error
from rust::tiny_error import TinyError, consume


def fail_inline(value: str) -> int:
    return raise_value_error(f"bad value `{value}`")


def fail_local(value: str) -> int:
    message = f"bad value `{value}`"
    return raise_value_error(message)


def make_error(value: str) -> int:
    return consume(TinyError.Execution(f"bad value `{value}`"))


def main() -> None:
    println(str(make_error("x")))
    fail_inline("x")
"#,
    )?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(
        &bake_output,
        "explicit Oven bake for inline f-string Rust interop variants",
    );
    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "incan build for inline f-string Rust &str and String enum variants issue716",
    );
    Ok(())
}

/// Verifies that a direct zero-argument call in an f-string survives the complete CLI pipeline.
#[test]
fn fstring_interpolation_zero_arg_function_call_issue979() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "fstring_zero_arg_call_issue979", "")?;
    fs::write(
        &main_path,
        r#"def enabled() -> bool:
  return True

def main() -> None:
  println(f"enabled:{enabled()}")
"#,
    )?;

    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;
    let output = run_incan(tmp.path(), &["run", main_arg, "--sdk-profile", "minimal"])?;
    assert_success(&output, "incan run with a direct f-string zero-argument call");
    assert_eq!(String::from_utf8(output.stdout)?, "enabled:true\n");
    Ok(())
}

#[test]
fn build_static_str_const_rust_string_struct_field() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let helper_dir = tmp.path().join("rust").join("tiny_option");
    fs::create_dir_all(helper_dir.join("src"))?;
    fs::write(
        helper_dir.join("Cargo.toml"),
        "[package]\nname = \"tiny_option\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(
        helper_dir.join("src").join("lib.rs"),
        r#"pub struct FunctionOption {
    pub name: String,
    pub enabled: bool,
}

pub fn option_name(option: FunctionOption) -> String {
    option.name
}
"#,
    )?;
    let main_path = write_minimal_project(
        tmp.path(),
        "static_str_const_rust_string_struct_field",
        r#"
[rust-dependencies]
tiny_option = { path = "rust/tiny_option" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::tiny_option import FunctionOption, option_name


pub const OPTION_NAME: str = "sketch_family"


def main() -> None:
    option = FunctionOption(name=OPTION_NAME, enabled=True)
    println(option_name(option))
"#,
    )?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(
        &bake_output,
        "explicit Oven bake for a static str const in a Rust String field",
    );
    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "incan build for static str const into Rust String struct field",
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

    let test_path = tests_dir.join("test_alias.incn");
    let test_output = run_incan(
        tmp.path(),
        &["test", test_path.to_str().ok_or("test path was not valid UTF-8")?],
    )?;
    assert_success(&test_output, "incan test for public alias issue631");
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
fn imported_registry_descriptions_keep_the_catalogue_as_canonical_authority_issue1004()
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
        provider_root.join("incan.toml"),
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
        consumer_root.join("incan.toml"),
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
fn oven_baked_public_direct_rust_provider_composes_into_consumer_issue1053() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let provider_root = tmp.path().join("uuid_provider");
    fs::create_dir_all(provider_root.join("src"))?;
    fs::write(
        provider_root.join("incan.toml"),
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
        consumer_root.join("incan.toml"),
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
        provider_root.join("incan.toml"),
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


def method_identity(func: (&Box, int) -> int) -> ((&Box, int) -> int):
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
fn test_incan_call_widens_list_elements_to_union_argument_issue57() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "incan_list_element_union_arg", "")?;
    fs::write(
        &main_path,
        r#"pub model ColumnRefExpr:
    pub name: str


pub model StringColumnExpr:
    pub name: str


pub type ColumnExpr = Union[ColumnRefExpr, StringColumnExpr]


pub def registered_application(arguments: list[ColumnExpr]) -> ColumnExpr:
    return arguments[0]


pub def str_col(name: str) -> StringColumnExpr:
    return StringColumnExpr(name=name)


pub def concat(first: str) -> ColumnExpr:
    mut arguments = [str_col(first)]
    arguments.append(str_col("tail"))
    return registered_application(arguments)


pub def concat_direct(first: str) -> ColumnExpr:
    return registered_application([str_col(first)])


def main() -> None:
    concat("name")
    concat_direct("name")
"#,
    )?;

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "incan build for list element union widening at an Incan call boundary",
    );
    Ok(())
}

#[test]
fn test_incan_call_widens_imported_list_elements_to_union_argument_issue57() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "incan_imported_list_element_union_arg", "")?;
    let src_dir = main_path.parent().ok_or("main path did not have a parent")?;
    fs::write(
        src_dir.join("types.incn"),
        r#"pub model A:
    pub value: str


pub model B:
    pub value: str


pub type U = Union[A, B]


pub type Outer = Union[U, int]
"#,
    )?;
    fs::write(
        src_dir.join("helpers.incn"),
        r#"from types import A


pub def a(value: str) -> A:
    return A(value=value)
"#,
    )?;
    fs::write(
        &main_path,
        r#"from helpers import a
from types import Outer, U


pub def repro(name: str) -> int:
    return takes([a(name)])


pub def repro_nested(name: str) -> int:
    return takes_nested([a(name)])


pub def takes(values: list[U]) -> int:
    return len(values)


pub def takes_nested(values: list[Outer]) -> int:
    return len(values)


def main() -> None:
    repro("name")
    repro_nested("name")
"#,
    )?;

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "incan build for imported list element union widening at an Incan call boundary",
    );
    Ok(())
}

#[test]
fn test_multi_file_test_batch_keeps_file_local_import_scopes_issue57() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "test_batch_file_local_import_scopes", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
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


pub def col(name: str) -> ColumnExpr:
    return ScalarFunctionExpr(name=name)
"#,
    )?;
    fs::write(
        tests_dir.join("test_projection_col.incn"),
        r#"from projection_builders import ColumnRefExpr, col


def test_projection_col_keeps_concrete_return_type() -> None:
    ref: ColumnRefExpr = col("customer_id")
    assert ref.name == "customer_id"
"#,
    )?;
    fs::write(
        tests_dir.join("test_aggregate_col.incn"),
        r#"from aggregate_builders import col
from projection_builders import ColumnExpr


def test_aggregate_col_keeps_union_return_type() -> None:
    expr: ColumnExpr = col("customer_id")
    assert true
"#,
    )?;

    let test_output = run_incan(tmp.path(), &["test", "tests"])?;
    assert_success(
        &test_output,
        "incan test multi-file batch with same local import name from different modules",
    );
    let test_batches_dir = tmp.path().join("target").join("incan_tests");
    let isolated_projection_module = fs::read_dir(&test_batches_dir)?.filter_map(Result::ok).any(|entry| {
        entry
            .path()
            .join("src")
            .join("tests")
            .join("test_projection_col.rs")
            .exists()
    });
    let isolated_aggregate_module = fs::read_dir(&test_batches_dir)?.filter_map(Result::ok).any(|entry| {
        entry
            .path()
            .join("src")
            .join("tests")
            .join("test_aggregate_col.rs")
            .exists()
    });
    assert!(
        isolated_projection_module && isolated_aggregate_module,
        "multi-file test batch should emit each test file as its own Rust module"
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

#[test]
fn build_metadata_free_into_bound_tokenizer_encode_issue804() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let helper_dir = tmp.path().join("rust").join("tokenizers");
    fs::create_dir_all(helper_dir.join("src"))?;
    fs::write(
        helper_dir.join("Cargo.toml"),
        "[package]\nname = \"tokenizers\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(
        helper_dir.join("src").join("lib.rs"),
        r#"pub struct Tokenizer;

pub struct EncodeInput<'a>(&'a str);

impl<'a> From<&'a str> for EncodeInput<'a> {
    fn from(value: &'a str) -> Self {
        Self(value)
    }
}

impl Tokenizer {
    pub fn new() -> Self {
        Self
    }

    pub fn encode<'a, E>(&self, value: E, _add_special_tokens: bool) -> Result<(), ()>
    where
        E: Into<EncodeInput<'a>>,
    {
        let _ = value.into();
        Ok(())
    }
}
"#,
    )?;
    let main_path = write_minimal_project(
        tmp.path(),
        "metadata_free_into_bound_tokenizer_encode_issue804",
        r#"
[rust-dependencies]
tokenizers = { path = "rust/tokenizers" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::tokenizers import Tokenizer

def main() -> None:
    tokenizer = Tokenizer.new()
    literal = tokenizer.encode("literal", False)
    text = "variable"
    variable = tokenizer.encode(text, False)
"#,
    )?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(
        &bake_output,
        "explicit Oven bake for metadata-free Into-bound tokenizer encode",
    );
    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "incan build for metadata-free Into-bound tokenizer encode issue804",
    );
    let generated = fs::read_to_string(
        tmp.path()
            .join("target/incan/metadata_free_into_bound_tokenizer_encode_issue804/src/main.rs"),
    )?;
    assert!(
        generated.contains("tokenizer.encode(\"literal\", false)"),
        "literal must preserve its direct &str shape, got:\n{generated}"
    );
    assert!(
        generated.contains("tokenizer.encode((text).as_str(), false)"),
        "owned Incan strings must become &str for the Into-bound method, got:\n{generated}"
    );
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
        stdout.contains("Oven build successful"),
        "frozen build should complete with the existing lockfile, got:\n{stdout}"
    );
    Ok(())
}

/// Non-fatal warnings must reach `incan check --format json`, not only stderr (#1117).
///
/// Covers both warning classes deliberately: the parser's RFC 005 dot-notation nudge and the typechecker's
/// unreachable-code warning. A fix that threaded only typechecker warnings would leave the README's "stable
/// diagnostics" surface half-true, so the parser case is a first-class assertion here rather than an afterthought.
#[test]
fn check_json_reports_parser_and_typechecker_warnings_without_failing() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;

    // ---- Typechecker warning: unreachable code after `return` ----
    let typecheck_path = tmp.path().join("typecheck_warning.incn");
    fs::write(
        &typecheck_path,
        r#"def f() -> int:
    return 1
    println("dead code")

def main() -> None:
    println(f"{f()}")
"#,
    )?;
    let typecheck_arg = typecheck_path.to_str().ok_or("path was not valid UTF-8")?;
    let typecheck = run_incan(tmp.path(), &["check", typecheck_arg, "--format", "json"])?;
    assert_success(&typecheck, "a warning must not fail `incan check`");
    let typecheck_json = parse_json_stdout(&typecheck)?;

    assert_eq!(typecheck_json["schema_version"], serde_json::json!(2));
    assert_eq!(
        typecheck_json["ok"],
        serde_json::json!(true),
        "`ok` reports the absence of errors, so warnings must not clear it"
    );
    assert_eq!(
        typecheck_json["diagnostics"][0]["code"],
        serde_json::json!("INCAN-T0101")
    );
    assert_eq!(
        typecheck_json["diagnostics"][0]["severity"],
        serde_json::json!("warning")
    );
    assert_eq!(
        typecheck_json["diagnostics"][0]["phase"],
        serde_json::json!("typecheck")
    );
    assert_eq!(
        typecheck_json["diagnostics"][0]["origin"],
        serde_json::json!("typechecker")
    );

    // ---- Parser warning: RFC 005 `import rust.crate` dot-notation ----
    let parse_path = tmp.path().join("parse_warning.incn");
    fs::write(
        &parse_path,
        r#"import rust.chrono

def main() -> None:
    println("parser warning")
"#,
    )?;
    let parse_arg = parse_path.to_str().ok_or("path was not valid UTF-8")?;
    let parse = run_incan(tmp.path(), &["check", parse_arg, "--format", "json"])?;
    assert_success(&parse, "a parser warning must not fail `incan check`");
    let parse_json = parse_json_stdout(&parse)?;

    assert_eq!(parse_json["schema_version"], serde_json::json!(2));
    assert_eq!(parse_json["ok"], serde_json::json!(true));
    assert_eq!(parse_json["diagnostics"][0]["severity"], serde_json::json!("warning"));
    assert_eq!(parse_json["diagnostics"][0]["phase"], serde_json::json!("parse"));
    assert_eq!(parse_json["diagnostics"][0]["origin"], serde_json::json!("parser"));

    Ok(())
}

/// A file with both a warning and an error must report both in JSON, not just the error (#1117).
///
/// Warnings ride a separate field on the failure envelope precisely so this case works: folding them into the
/// error list would print them to stderr a second time, and dropping them would mean the same warning is visible
/// when a file compiles and invisible the moment anything else in it fails.
#[test]
fn check_json_reports_warnings_alongside_errors_when_typechecking_fails() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("mixed.incn");
    fs::write(
        &path,
        r#"def f() -> int:
    return 1
    println("dead code")

def main() -> None:
    _ = undefined_symbol
"#,
    )?;

    let arg = path.to_str().ok_or("path was not valid UTF-8")?;
    let output = run_incan(tmp.path(), &["check", arg, "--format", "json"])?;
    assert_failure(&output, "an undefined symbol must still fail `incan check`");
    let report = parse_json_stdout(&output)?;

    assert_eq!(report["schema_version"], serde_json::json!(2));
    assert_eq!(report["ok"], serde_json::json!(false), "an error must clear `ok`");

    let diagnostics = report["diagnostics"]
        .as_array()
        .ok_or("check report had no diagnostics array")?;
    let severities: Vec<&str> = diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic["severity"].as_str())
        .collect();
    assert!(
        severities.contains(&"error"),
        "expected the undefined-symbol error to be reported, got: {severities:?}"
    );
    assert!(
        severities.contains(&"warning"),
        "expected the unreachable-code warning to survive the failure, got: {severities:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == serde_json::json!("INCAN-T0101")),
        "expected INCAN-T0101 in the failing report, got: {diagnostics:?}"
    );

    Ok(())
}

/// Statement tuple-unpack of a non-tuple must fail at the source language, not in generated Rust (#1132).
///
/// The regression is specifically about *where* the failure surfaces. Before this, `incan check` passed and the
/// program only failed while compiling emitted Rust, with `error[E0610]` pointing at a `__incan_tuple_unpack_*`
/// binding the user never wrote. Asserting the absence of both strings is the point: a diagnostic that merely
/// exists is not enough if the raw Rust error can still reach the user.
#[test]
fn check_rejects_statement_tuple_unpack_of_non_tuple_without_leaking_generated_rust()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("unpack.incn");
    fs::write(
        &path,
        r#"def main() -> None:
    a, b = 5
    println(f"{a} {b}")
"#,
    )?;

    let arg = path.to_str().ok_or("path was not valid UTF-8")?;
    let output = run_incan(tmp.path(), &["check", arg, "--format", "json"])?;
    assert_failure(&output, "destructuring an `int` must fail `incan check`");
    let report = parse_json_stdout(&output)?;

    assert_eq!(report["ok"], serde_json::json!(false));
    let diagnostics = report["diagnostics"]
        .as_array()
        .ok_or("check report had no diagnostics array")?;
    let first = diagnostics.first().ok_or("expected at least one diagnostic")?;
    assert_eq!(first["severity"], serde_json::json!("error"));
    assert_eq!(first["phase"], serde_json::json!("typecheck"));
    assert!(
        first["message"]
            .as_str()
            .is_some_and(|message| message.contains("Cannot destructure 2 values from value of type 'int'")),
        "the diagnostic must name the resolved value type: {first}"
    );
    assert_eq!(
        first["primary_span"]["start"]["line"],
        serde_json::json!(2),
        "the span must point at the offending statement, not the file: {first}"
    );

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !combined.contains("E0610"),
        "a raw rustc field-projection error must never reach the user:\n{combined}"
    );
    assert!(
        !combined.contains("__incan_tuple_unpack"),
        "a compiler-internal binding name must never reach the user:\n{combined}"
    );

    Ok(())
}

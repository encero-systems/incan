#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Barrier};

mod support;

/// Configure a project command with one caller-selected compiler binary.
///
/// A compiler-suite test normally launches its receipt-selected direct-Rustc CLI. These probes deliberately remove
/// that scheduler authority, so their ordinary-consumer phase must instead use the staged release CLI that owns the
/// release Loaf envelope. Explicit bakes continue to use the compiler-suite CLI and its narrowly injected capability.
fn configured_incan_command_with_binary(binary: PathBuf, project_root: &Path, incan_home: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .current_dir(project_root)
        // Normal Oven commands must not inherit a generated-Cargo cache control from the test harness.
        .env_remove("INCAN_GENERATED_CARGO_TARGET_DIR")
        .env_remove("INCAN_GENERATED_CACHE")
        .env_remove("INCAN_TEST_SHARED_TARGET_DIR")
        .env("INCAN_HOME", incan_home)
        .env("INCAN_SOURCE_ROOT", env!("CARGO_MANIFEST_DIR"))
        .env(
            "INCAN_STDLIB",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/incan_stdlib/stdlib"),
        )
        .env(
            "INCAN_STDLIB_DIR",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/incan_stdlib/stdlib"),
        )
        .env("CARGO_NET_OFFLINE", "true");
    if !support::oven_compiler_suite_is_active() {
        command.env("INCAN_INTERNAL_SDK_PROVIDER_STORE", support::sdk_provider_store());
    }
    command
}

/// Return the staged release CLI for the normal-consumer phase of a compiler-suite test.
///
/// Outside the stored suite, this remains the ordinary integration-test binary. The Make gate supplies the staged
/// release binary only after it has baked the matching release Loaf family.
fn normal_consumer_incan_binary() -> PathBuf {
    if support::oven_compiler_suite_is_active()
        && let Some(binary) =
            std::env::var_os("INCAN_INTERNAL_OVEN_NORMAL_CONSUMER_BIN").filter(|path| !path.is_empty())
    {
        return PathBuf::from(binary);
    }
    support::incan_binary()
}

/// Prepare a normal Incan command with any scheduler-granted baker capability removed.
///
/// The explicit bake below is allowed to use the package-qualified fixture Cargo proxy. Every replay, drift, and
/// lock assertion must instead reach the PATH guard if normal command handling ever regresses to Cargo.
fn incan_command(project_root: &Path, incan_home: &Path) -> Command {
    let mut command = configured_incan_command_with_binary(normal_consumer_incan_binary(), project_root, incan_home);
    command
        .env_remove("CARGO")
        .env_remove("INCAN_INTERNAL_OVEN_LOAF_EXECUTION")
        .env_remove("INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT")
        .env_remove("INCAN_INTERNAL_OVEN_RUNTIME_ROOT")
        .env_remove("INCAN_OVEN_LEGACY_CARGO_INSPECTION_AUTHORITY");
    command
}

/// Prepare the one explicit project-bake command that owns the Cargo fixture capability.
///
/// Its later normal commands use the staged release CLI, so the bake must select that exact release Loaf envelope
/// too. The compiler-suite contributes only the narrowly scoped Cargo publisher capability; it must not supply the
/// compiler-suite data-root authority that would make the persisted receipt incompatible with its normal consumer.
fn baker_incan_command(project_root: &Path, incan_home: &Path) -> Command {
    let mut command = configured_incan_command_with_binary(normal_consumer_incan_binary(), project_root, incan_home);
    command
        .env_remove("INCAN_INTERNAL_OVEN_LOAF_EXECUTION")
        .env_remove("INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT")
        .env_remove("INCAN_INTERNAL_OVEN_RUNTIME_ROOT");
    command
}

fn run_checked(mut command: Command, label: &str) -> Result<Output, Box<dyn std::error::Error>> {
    let timing = support::command_timing_started();
    let output = command.output()?;
    support::report_command_timing(label, timing);
    if output.status.success() {
        return Ok(output);
    }
    Err(format!(
        "{label} failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .into())
}

fn write_dependency_project(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join("tests"))?;
    fs::write(
        root.join("loaf.toml"),
        "[project]\nname = \"generated_cache_fixture\"\nversion = \"0.1.0\"\n\n[project.scripts]\nmain = \"src/main.incn\"\n\n[rust-dependencies]\nserde_json = \"1\"\n",
    )?;
    fs::write(
        root.join("src/main.incn"),
        "from rust::serde_json import Value\n\ndef cache_json(value: Value) -> Value:\n  return value\n\ndef main() -> None:\n  pass\n",
    )?;
    fs::write(
        root.join("src/lib.incn"),
        "from rust::serde_json import Value\n\npub def cache_json(value: Value) -> Value:\n  return value\n",
    )?;
    fs::write(
        root.join("tests/cache_test.incn"),
        "from rust::serde_json import Value\nfrom std.testing import test\n\ndef cache_json(value: Value) -> Value:\n  return value\n\n@test\ndef test_sealed_oven_dependency() -> None:\n  assert True\n",
    )?;
    Ok(())
}

/// Create the smallest project that requires a real registry source during Rust inspection.
///
/// The completed-output regression below uses one mixed conventional project so its single explicit bake can prove
/// executable replay, release-owned `std.json` source authority, and the normal test path without walking another bake.
fn write_release_json_authority_project(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join("tests"))?;
    fs::write(
        root.join("loaf.toml"),
        "[project]\nname = \"completed_output_registry_fixture\"\nversion = \"0.1.0\"\n\n[project.scripts]\nmain = \"src/main.incn\"\n",
    )?;
    fs::write(root.join("src/main.incn"), "def main() -> None:\n  pass\n")?;
    fs::write(
        root.join("src/lib.incn"),
        "from std.json import JsonValue\n\npub def payload() -> JsonValue:\n  return JsonValue.str(\"oven\")\n",
    )?;
    fs::write(
        root.join("tests/project_output_test.incn"),
        "from lib import payload\nfrom std.testing import test\n\n@test\ndef test_baked_project_source_authority() -> None:\n  assert payload().as_str() == Some(\"oven\")\n",
    )?;
    Ok(())
}

fn assert_no_generated_cargo_state(project_root: &Path, incan_home: &Path) {
    assert!(
        !incan_home.join("cache/generated-cargo").exists(),
        "normal Oven commands must not recreate the retired generated-Cargo cache"
    );
    assert!(
        !project_root.join("target/.cargo-target").exists(),
        "normal Oven commands must not create a project-local Cargo target"
    );
}

fn write_rejecting_rustc_wrapper(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::write(
        path,
        r#"#!/bin/sh
: > "$INCAN_OVEN_RUSTC_WRAPPER_MARKER"
exit 97
"#,
    )?;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

/// Prepend a Cargo executable which leaves durable evidence if a normal command attempts a fallback launch.
#[cfg(unix)]
fn guarded_incan_command(
    project_root: &Path,
    incan_home: &Path,
    guard_root: &Path,
    marker: &Path,
) -> Result<Command, Box<dyn std::error::Error>> {
    fs::create_dir_all(guard_root)?;
    let guard = guard_root.join("cargo");
    fs::write(
        &guard,
        "#!/bin/sh\nprintf cargo > \"$INCAN_OVEN_CARGO_GUARD_MARKER\"\nexit 97\n",
    )?;
    let mut permissions = fs::metadata(&guard)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&guard, permissions)?;

    let mut paths = vec![guard_root.to_path_buf()];
    if let Some(inherited) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&inherited));
    }
    let mut command = incan_command(project_root, incan_home);
    command
        .env("PATH", std::env::join_paths(paths)?)
        .env("INCAN_OVEN_CARGO_GUARD_MARKER", marker);
    Ok(command)
}

#[test]
fn normal_oven_rejects_generated_cargo_target_control_without_side_effects() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = tempfile::tempdir()?;
    let incan_home = fixture.path().join("incan-home");
    let project_root = fixture.path().join("project");
    let explicit_target = fixture.path().join("caller-owned-cargo-target");
    let explicit_output = fixture.path().join("caller-owned-output");
    write_dependency_project(&project_root)?;

    let mut build = incan_command(&project_root, &incan_home);
    let output = build
        .args(["build", "src/main.incn", "--offline", "--generated-cargo-target-dir"])
        .arg(&explicit_target)
        .arg(&explicit_output)
        .output()?;

    assert!(
        !output.status.success(),
        "normal Oven must reject Cargo target controls"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("normal build and run do not accept Cargo passthrough or target-directory controls"),
        "unexpected rejection: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!explicit_target.exists());
    assert!(!explicit_output.exists());
    assert_no_generated_cargo_state(&project_root, &incan_home);
    Ok(())
}

#[cfg(unix)]
#[test]
fn normal_oven_reuses_sealed_inputs_offline_across_projects() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = tempfile::tempdir()?;
    let incan_home = fixture.path().join("incan-home");
    let first_root = fixture.path().join("first");
    let second_root = fixture.path().join("second");
    let guard_root = fixture.path().join("cargo-guard");
    let marker = fixture.path().join("cargo-was-started");
    write_dependency_project(&first_root)?;
    write_dependency_project(&second_root)?;

    let mut first_bake = baker_incan_command(&first_root, &incan_home);
    first_bake.args(["oven", "bake", "--project", "."]);
    run_checked(first_bake, "explicit Oven bake before sealed cross-project reuse")?;
    fs::copy(first_root.join("oven.lock"), second_root.join("oven.lock"))?;

    let mut first_build = guarded_incan_command(&first_root, &incan_home, &guard_root, &marker)?;
    // This retired limit must not influence an Oven command or create its removed cache.
    first_build
        .args(["build", "src/main.incn", "--offline"])
        .env("INCAN_GENERATED_CACHE_MAX_ENTRY_BYTES", "1");
    run_checked(first_build, "sealed offline build without a generated Cargo cache")?;

    let mut first_run = guarded_incan_command(&first_root, &incan_home, &guard_root, &marker)?;
    first_run.args(["run", "src/main.incn", "--offline"]);
    run_checked(first_run, "sealed offline run")?;

    let mut first_test = guarded_incan_command(&first_root, &incan_home, &guard_root, &marker)?;
    first_test.args(["test", "tests/cache_test.incn", "--offline"]);
    run_checked(first_test, "sealed offline test")?;

    let mut first_library = guarded_incan_command(&first_root, &incan_home, &guard_root, &marker)?;
    first_library.args(["build", "--lib", "--offline"]);
    run_checked(first_library, "sealed offline library build")?;
    assert_no_generated_cargo_state(&first_root, &incan_home);

    // The first project covers every normal-command mode. One identical-project build is sufficient to prove that
    // its compatible sealed inputs are reusable across a project boundary; repeating the already-proven debug run
    // would only walk the same consumption path a second time.
    let mut second_build = guarded_incan_command(&second_root, &incan_home, &guard_root, &marker)?;
    second_build.args(["build", "src/main.incn", "--offline"]);
    let second_output = run_checked(second_build, "cross-project sealed offline build reuse")?;
    assert!(
        String::from_utf8_lossy(&second_output.stdout).contains("reused sealed project Loaf"),
        "cross-project build did not report completed Loaf reuse:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&second_output.stdout),
        String::from_utf8_lossy(&second_output.stderr)
    );
    assert!(!marker.exists(), "a normal cross-project command launched Cargo");
    assert_no_generated_cargo_state(&second_root, &incan_home);
    Ok(())
}

/// One explicit mixed-project bake must authorize both completed-output replay and stdlib-owned Rust inspection.
///
/// The library and its test deliberately use only `std.json`, reproducing #1056 without any direct registry
/// dependency that could mask release-Loaf source authority. Every normal command is Cargo-guarded and no second bake
/// walks the same publication path again.
#[cfg(unix)]
#[test]
fn explicitly_baked_project_reuses_release_json_authority_without_cargo() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = tempfile::tempdir()?;
    let project_root = fixture.path().join("project");
    let incan_home = fixture.path().join("incan-home");
    let guard_root = fixture.path().join("cargo-guard");
    let marker = fixture.path().join("cargo-was-started");
    write_release_json_authority_project(&project_root)?;

    let mut lock = guarded_incan_command(&project_root, &incan_home, &guard_root, &marker)?;
    lock.arg("lock");
    run_checked(lock, "Cargo-guarded initial lock for explicit project bake")?;
    assert!(
        !marker.exists(),
        "initial project lock launched the guarded Cargo executable"
    );
    let locked_before_bake = fs::read(project_root.join("oven.lock"))?;

    let mut bake = baker_incan_command(&project_root, &incan_home);
    bake.args(["oven", "bake", "--project", "."]);
    run_checked(bake, "explicit Oven bake for release-owned JSON authority")?;
    assert_eq!(
        fs::read(project_root.join("oven.lock"))?,
        locked_before_bake,
        "explicit project bake changed the canonical lock after the initial fixed point"
    );

    let mut build = guarded_incan_command(&project_root, &incan_home, &guard_root, &marker)?;
    build.args(["build", "src/main.incn", "--locked"]);
    let build_output = run_checked(build, "Cargo-guarded completed project-output replay")?;
    assert!(
        String::from_utf8_lossy(&build_output.stdout).contains("reused sealed project Loaf"),
        "normal build did not report completed project-output reuse:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build_output.stdout),
        String::from_utf8_lossy(&build_output.stderr)
    );
    assert!(
        !marker.exists(),
        "normal completed-output replay launched the guarded Cargo executable"
    );

    let mut build_lib = guarded_incan_command(&project_root, &incan_home, &guard_root, &marker)?;
    build_lib.args(["build", "--lib", "--locked"]);
    let build_lib_output = run_checked(build_lib, "Cargo-guarded completed library replay for #1056")?;
    assert!(
        String::from_utf8_lossy(&build_lib_output.stdout).contains("reused sealed project Loaf"),
        "normal library build did not report completed project-output reuse:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build_lib_output.stdout),
        String::from_utf8_lossy(&build_lib_output.stderr)
    );
    assert!(
        !marker.exists(),
        "normal completed-library replay launched the guarded Cargo executable"
    );

    let mut test = guarded_incan_command(&project_root, &incan_home, &guard_root, &marker)?;
    test.args(["test", "tests", "--fail-on-empty"]);
    run_checked(test, "Cargo-guarded test from explicit project source authority")?;
    assert!(!marker.exists(), "normal test launched the guarded Cargo executable");

    let source_path = project_root.join("src/main.incn");
    let original_source = fs::read_to_string(&source_path)?;
    fs::write(&source_path, format!("{original_source}\n# source drift\n"))?;
    let mut source_drift = guarded_incan_command(&project_root, &incan_home, &guard_root, &marker)?;
    source_drift.args(["build", "src/main.incn"]);
    let source_drift_output = source_drift.output()?;
    assert!(
        !source_drift_output.status.success(),
        "source drift must reject the completed project-output Loaf"
    );
    let source_drift_diagnostics = format!(
        "{}\n{}",
        String::from_utf8_lossy(&source_drift_output.stdout),
        String::from_utf8_lossy(&source_drift_output.stderr)
    );
    assert!(
        source_drift_diagnostics.contains("no receipt-compatible Loaf"),
        "source drift did not return to the explicit-bake boundary:\n{source_drift_diagnostics}"
    );
    assert!(
        !marker.exists(),
        "source-drift rejection launched the guarded Cargo executable"
    );
    fs::write(&source_path, &original_source)?;

    let manifest_path = project_root.join("loaf.toml");
    let original_manifest = fs::read_to_string(&manifest_path)?;
    let drifted_manifest = format!("{original_manifest}\n[rust-dependencies]\nsemver = \"1\"\n");
    assert_ne!(original_manifest, drifted_manifest);
    fs::write(&manifest_path, drifted_manifest)?;
    let reachable_drifted_source = format!("from rust::semver import Version\n{original_source}");
    fs::write(&source_path, reachable_drifted_source)?;
    let mut lock_drift = guarded_incan_command(&project_root, &incan_home, &guard_root, &marker)?;
    lock_drift.args(["build", "src/main.incn", "--locked"]);
    let lock_drift_output = lock_drift.output()?;
    assert!(
        !lock_drift_output.status.success(),
        "locked manifest drift must reject the completed project-output Loaf"
    );
    let diagnostics = format!(
        "{}\n{}",
        String::from_utf8_lossy(&lock_drift_output.stdout),
        String::from_utf8_lossy(&lock_drift_output.stderr)
    );
    assert!(
        diagnostics.contains("oven.lock is out of date"),
        "locked manifest drift did not fail at the canonical lock boundary:\n{diagnostics}"
    );
    assert!(
        !marker.exists(),
        "locked manifest-drift rejection launched the guarded Cargo executable"
    );
    Ok(())
}

#[test]
fn concurrent_normal_oven_builds_ignore_cargo_rustc_wrapper() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = tempfile::tempdir()?;
    let first_root = fixture.path().join("first");
    let second_root = fixture.path().join("second");
    let incan_home = fixture.path().join("incan-home");
    let wrapper = fixture.path().join("rejecting-rustc-wrapper.sh");
    let marker = fixture.path().join("rustc-wrapper-invoked");
    write_dependency_project(&first_root)?;
    write_dependency_project(&second_root)?;
    write_rejecting_rustc_wrapper(&wrapper)?;

    let mut first_bake = baker_incan_command(&first_root, &incan_home);
    first_bake.args(["oven", "bake", "--project", "."]);
    run_checked(first_bake, "one explicit Oven bake before concurrent replay")?;
    fs::copy(first_root.join("oven.lock"), second_root.join("oven.lock"))?;

    let barrier = Arc::new(Barrier::new(3));
    let first_barrier = Arc::clone(&barrier);
    let first_wrapper = wrapper.clone();
    let first_marker = marker.clone();
    let first_home = incan_home.clone();
    let first = std::thread::spawn(move || {
        first_barrier.wait();
        let mut build = incan_command(&first_root, &first_home);
        build
            .args(["build", "src/main.incn", "--offline"])
            .env("RUSTC_WRAPPER", first_wrapper)
            .env("INCAN_OVEN_RUSTC_WRAPPER_MARKER", first_marker);
        run_checked(build, "first concurrent Oven build")
            .map(|_| ())
            .map_err(|error| error.to_string())
    });

    let second_barrier = Arc::clone(&barrier);
    let second_home = incan_home.clone();
    let second = std::thread::spawn(move || {
        second_barrier.wait();
        let mut build = incan_command(&second_root, &second_home);
        build
            .args(["build", "src/main.incn", "--offline"])
            .env("RUSTC_WRAPPER", wrapper)
            .env("INCAN_OVEN_RUSTC_WRAPPER_MARKER", marker);
        run_checked(build, "second concurrent Oven build")
            .map(|_| ())
            .map_err(|error| error.to_string())
    });

    barrier.wait();
    match first.join() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(error.into()),
        Err(_) => return Err("first concurrent Oven build panicked".into()),
    }
    match second.join() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(error.into()),
        Err(_) => return Err("second concurrent Oven build panicked".into()),
    }
    assert!(
        !fixture.path().join("rustc-wrapper-invoked").exists(),
        "normal Oven direct-rustc execution must not consult Cargo's RUSTC_WRAPPER"
    );
    assert_no_generated_cargo_state(&fixture.path().join("first"), &incan_home);
    assert_no_generated_cargo_state(&fixture.path().join("second"), &incan_home);
    Ok(())
}

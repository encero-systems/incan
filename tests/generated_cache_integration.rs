#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::{Arc, Barrier};

mod support;

fn incan_command(project_root: &Path, incan_home: &Path) -> Command {
    let mut command = Command::new(support::incan_binary());
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

fn run_checked(mut command: Command, label: &str) -> Result<Output, Box<dyn std::error::Error>> {
    let output = command.output()?;
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
        root.join("incan.toml"),
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

#[test]
fn normal_oven_reuses_sealed_inputs_offline_across_projects() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = tempfile::tempdir()?;
    let incan_home = fixture.path().join("incan-home");
    let first_root = fixture.path().join("first");
    let second_root = fixture.path().join("second");
    write_dependency_project(&first_root)?;
    write_dependency_project(&second_root)?;

    for project_root in [&first_root, &second_root] {
        let mut build = incan_command(project_root, &incan_home);
        build.args(["build", "src/main.incn", "--offline"]);
        run_checked(build, "sealed offline build")?;

        let mut run = incan_command(project_root, &incan_home);
        run.args(["run", "src/main.incn", "--offline"]);
        run_checked(run, "sealed offline run")?;

        let mut test = incan_command(project_root, &incan_home);
        test.args(["test", "tests/cache_test.incn", "--offline"]);
        run_checked(test, "sealed offline test")?;

        let mut library = incan_command(project_root, &incan_home);
        library.args(["build", "--lib", "--offline"]);
        run_checked(library, "sealed offline library build")?;

        assert_no_generated_cargo_state(project_root, &incan_home);
    }
    Ok(())
}

#[test]
fn concurrent_normal_oven_builds_ignore_cargo_rustc_wrapper() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = tempfile::tempdir()?;
    let first_root = fixture.path().join("first");
    let second_root = fixture.path().join("second");
    let first_home = fixture.path().join("first-home");
    let second_home = fixture.path().join("second-home");
    let wrapper = fixture.path().join("rejecting-rustc-wrapper.sh");
    let marker = fixture.path().join("rustc-wrapper-invoked");
    write_dependency_project(&first_root)?;
    write_dependency_project(&second_root)?;
    write_rejecting_rustc_wrapper(&wrapper)?;

    let barrier = Arc::new(Barrier::new(3));
    let first_barrier = Arc::clone(&barrier);
    let first_wrapper = wrapper.clone();
    let first_marker = marker.clone();
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
    assert_no_generated_cargo_state(&fixture.path().join("first"), &fixture.path().join("first-home"));
    assert_no_generated_cargo_state(&fixture.path().join("second"), &fixture.path().join("second-home"));
    Ok(())
}

#[test]
fn normal_oven_has_no_generated_cargo_cache_to_bound() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = tempfile::tempdir()?;
    let incan_home = fixture.path().join("incan-home");
    let project_root = fixture.path().join("project");
    write_dependency_project(&project_root)?;

    for label in ["first Oven build", "second Oven build"] {
        let mut build = incan_command(&project_root, &incan_home);
        build
            .args(["build", "src/main.incn", "--offline"])
            .env("INCAN_GENERATED_CACHE_MAX_ENTRY_BYTES", "1");
        run_checked(build, label)?;
        assert_no_generated_cargo_state(&project_root, &incan_home);
    }
    Ok(())
}

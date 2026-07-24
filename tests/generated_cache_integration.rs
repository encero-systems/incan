#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant, SystemTime};

mod support;

fn incan_command(project_root: &Path, incan_home: &Path) -> Command {
    let mut command = Command::new(support::incan_binary());
    command
        .current_dir(project_root)
        // The repository test harness deliberately gives other nested Cargo tests one explicit shared target. This
        // integration test exercises the default managed policy, so inherited harness overrides must not bypass it.
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
        .env("CARGO_NET_OFFLINE", "true")
        .env("INCAN_INTERNAL_SDK_PROVIDER_STORE", support::sdk_provider_store());
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

fn write_project(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let source = root.join("src/main.incn");
    fs::create_dir_all(source.parent().ok_or("generated cache fixture has no source parent")?)?;
    fs::write(
        root.join("incan.toml"),
        "[project]\nname = \"generated_cache_fixture\"\nversion = \"0.1.0\"\n\n[project.scripts]\nmain = \"src/main.incn\"\n",
    )?;
    fs::write(&source, "def main() -> None:\n  pass\n")?;
    Ok(source)
}

fn write_dependency_project(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let source = write_project(root)?;
    fs::write(
        root.join("incan.toml"),
        "[project]\nname = \"generated_cache_fixture\"\nversion = \"0.1.0\"\n\n[project.scripts]\nmain = \"src/main.incn\"\n\n[rust-dependencies]\nserde_json = \"1\"\n",
    )?;
    fs::write(
        &source,
        "from rust::serde_json import Value\n\ndef cache_json(value: Value) -> Value:\n  return value\n\ndef main() -> None:\n  pass\n",
    )?;
    Ok(source)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArtifactSnapshot {
    path: PathBuf,
    len: u64,
    modified: SystemTime,
}

fn dependency_artifact_snapshots(
    incan_home: &Path,
    cache_profile: &str,
    cargo_profile_dir: &str,
    file_prefix: &str,
) -> Result<Vec<ArtifactSnapshot>, Box<dyn std::error::Error>> {
    let mut artifacts = Vec::new();
    for domain in cache_entries(incan_home)? {
        if domain.get("profile").and_then(serde_json::Value::as_str) != Some(cache_profile) {
            continue;
        }
        let domain_path = domain
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map(PathBuf::from)
            .ok_or("managed cache entry omitted its path")?;
        let deps = domain_path.join("target").join(cargo_profile_dir).join("deps");
        if let Ok(entries) = fs::read_dir(deps) {
            for entry in entries {
                let entry = entry?;
                let name = entry.file_name();
                if !name.to_string_lossy().starts_with(file_prefix) {
                    continue;
                }
                let metadata = entry.metadata()?;
                artifacts.push(ArtifactSnapshot {
                    path: entry.path(),
                    len: metadata.len(),
                    modified: metadata.modified()?,
                });
            }
        }
    }
    artifacts.sort_by(|left, right| left.path.cmp(&right.path));
    if artifacts.is_empty() {
        return Err(format!("managed cache omitted dependency artifacts with prefix `{file_prefix}`").into());
    }
    Ok(artifacts)
}

fn cache_logical_snapshot(incan_home: &Path) -> Result<Vec<(String, u64)>, Box<dyn std::error::Error>> {
    let mut snapshot = cache_entries(incan_home)?
        .into_iter()
        .map(|entry| {
            let identity = entry
                .get("identity")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or("cache entry omitted identity")?;
            let bytes = entry
                .get("bytes")
                .and_then(serde_json::Value::as_u64)
                .ok_or("cache entry omitted logical bytes")?;
            Ok::<_, Box<dyn std::error::Error>>((identity, bytes))
        })
        .collect::<Result<Vec<_>, _>>()?;
    snapshot.sort();
    Ok(snapshot)
}

fn write_test_and_library_sources(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(root.join("tests"))?;
    fs::write(
        root.join("tests/cache_test.incn"),
        "from rust::serde_json import Value\nfrom std.testing import test\n\ndef cache_json(value: Value) -> Value:\n  return value\n\n@test\ndef test_cache_domain() -> None:\n  assert True\n",
    )?;
    fs::write(
        root.join("src/lib.incn"),
        "from rust::serde_json import Value\n\npub def cache_json(value: Value) -> Value:\n  return value\n",
    )?;
    Ok(())
}

fn cache_entries(incan_home: &Path) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error>> {
    let mut inspect = incan_command(Path::new(env!("CARGO_MANIFEST_DIR")), incan_home);
    inspect.args(["cache", "inspect", "--format", "json"]);
    let output = run_checked(inspect, "cache inspect")?;
    let payload = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    Ok(payload
        .get("entries")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .ok_or("cache inspection omitted entries")?)
}

fn cache_entry_count_for_profile(incan_home: &Path, profile: &str) -> Result<usize, Box<dyn std::error::Error>> {
    Ok(cache_entries(incan_home)?
        .iter()
        .filter(|entry| entry.get("profile").and_then(serde_json::Value::as_str) == Some(profile))
        .count())
}

fn sole_cache_target_for_profile(incan_home: &Path, profile: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let matches = cache_entries(incan_home)?
        .into_iter()
        .filter(|entry| entry.get("profile").and_then(serde_json::Value::as_str) == Some(profile))
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(format!("expected exactly one `{profile}` cache domain, found {}", matches.len()).into());
    }
    let entry_root = matches[0]
        .get("path")
        .and_then(serde_json::Value::as_str)
        .ok_or("cache entry omitted its managed path")?;
    Ok(PathBuf::from(entry_root).join("target"))
}

fn assert_project_rust_inspect_target(
    stage: &str,
    project_root: &Path,
    expected_target: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let workspace_root = project_root.join("target/incan_lock/rust_inspect");
    let mut configs = Vec::new();
    for entry in fs::read_dir(&workspace_root)? {
        let entry = entry?;
        let config = entry.path().join(".cargo/config.toml");
        if config.is_file() {
            configs.push(fs::read_to_string(config)?);
        }
    }
    assert!(
        !configs.is_empty(),
        "command did not materialize a rust-inspect workspace"
    );
    let expected = expected_target.to_string_lossy();
    assert!(
        configs.iter().all(|config| config.contains(expected.as_ref())),
        "{stage} configured rust-inspect outside the canonical managed target {} for {}:\n{}",
        expected_target.display(),
        project_root.display(),
        configs.join("\n---\n")
    );
    Ok(())
}

fn reset_project_rust_inspect_workspaces(project_root: &Path) -> std::io::Result<()> {
    let workspace_root = project_root.join("target/incan_lock/rust_inspect");
    match fs::remove_dir_all(workspace_root) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn active_cache_identity(incan_home: &Path) -> Result<Option<String>, Box<dyn std::error::Error>> {
    Ok(cache_entries(incan_home)?.into_iter().find_map(|entry| {
        entry
            .get("active")
            .and_then(serde_json::Value::as_bool)
            .filter(|active| *active)
            .and_then(|_| entry.get("identity"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    }))
}

fn write_blocking_rustc_wrapper(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::write(
        path,
        r#"#!/bin/sh
set -eu
case "${CARGO_TARGET_DIR:-}" in
  "$INCAN_CACHE_TEST_MANAGED_ROOT"/*)
    : > "$INCAN_CACHE_TEST_RUSTC_READY"
    while [ ! -e "$INCAN_CACHE_TEST_RUSTC_RELEASE" ]; do
      sleep 0.05
    done
    ;;
esac
exec "$@"
"#,
    )?;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

struct RustcReleaseGuard {
    path: PathBuf,
}

impl Drop for RustcReleaseGuard {
    fn drop(&mut self) {
        let _ = fs::write(&self.path, b"release\n");
    }
}

#[test]
fn managed_cache_reuses_offline_across_projects_and_cleans_interrupted_entry() -> Result<(), Box<dyn std::error::Error>>
{
    let fixture = tempfile::tempdir()?;
    let incan_home = fixture.path().join("incan-home");
    let first_root = fixture.path().join("first");
    let second_root = fixture.path().join("second");
    write_dependency_project(&first_root)?;
    write_dependency_project(&second_root)?;
    write_test_and_library_sources(&first_root)?;
    write_test_and_library_sources(&second_root)?;

    let inherited_build_dir = fixture.path().join("inherited-cargo-build-dir");
    let check_root = fixture.path().join("check");
    write_dependency_project(&check_root)?;
    write_test_and_library_sources(&check_root)?;
    fs::create_dir_all(check_root.join(".cargo"))?;
    fs::write(
        check_root.join(".cargo/config.toml"),
        format!("[build]\nbuild-dir = {:?}\n", inherited_build_dir),
    )?;
    let mut check = incan_command(&check_root, &incan_home);
    check
        .args(["check", "src/main.incn"])
        .env("CARGO_BUILD_BUILD_DIR", &inherited_build_dir);
    run_checked(check, "cold managed rust-inspect check")?;
    let canonical_rust_inspect_target = sole_cache_target_for_profile(&incan_home, "rust-inspect")?;
    assert_project_rust_inspect_target("check", &check_root, &canonical_rust_inspect_target)?;
    assert!(!inherited_build_dir.exists());
    assert!(
        !check_root.join("target/.cargo-target").exists(),
        "incan check created an unleased project-local Cargo target"
    );

    for project_root in [&first_root, &second_root] {
        fs::create_dir_all(project_root.join(".cargo"))?;
        fs::write(
            project_root.join(".cargo/config.toml"),
            format!("[build]\nbuild-dir = {:?}\n", inherited_build_dir),
        )?;
    }
    let mut first = incan_command(&first_root, &incan_home);
    first
        .args(["build", "src/main.incn", "--offline"])
        .env("CARGO_BUILD_BUILD_DIR", &inherited_build_dir);
    run_checked(first, "first offline build")?;
    assert_project_rust_inspect_target("first build", &first_root, &canonical_rust_inspect_target)?;
    assert!(
        !inherited_build_dir.exists(),
        "managed build inherited Cargo output outside its lifecycle-owned target"
    );
    assert!(
        !first_root.join("target/.cargo-target").exists(),
        "rust-inspect created an unleased project-local Cargo target"
    );
    let dependency_before = dependency_artifact_snapshots(&incan_home, "release", "release", "libserde_json-")?;

    let mut second = incan_command(&second_root, &incan_home);
    second.args(["build", "src/main.incn", "--offline"]);
    run_checked(second, "compatible offline build")?;
    assert_project_rust_inspect_target("relocated build", &second_root, &canonical_rust_inspect_target)?;
    assert!(
        !second_root.join("target/.cargo-target").exists(),
        "relocated rust-inspect workspace created a project-local Cargo target"
    );
    let dependency_after = dependency_artifact_snapshots(&incan_home, "release", "release", "libserde_json-")?;
    assert_eq!(
        dependency_after, dependency_before,
        "compatible build recompiled or replaced the warmed serde_json dependency artifact"
    );

    assert_eq!(
        cache_entry_count_for_profile(&incan_home, "release")?,
        1,
        "compatible release builds should share one release domain"
    );
    let published_binary = Path::new("target/incan/generated_cache_fixture/target/release/generated_cache_fixture");
    assert!(first_root.join(published_binary).is_file());
    assert!(second_root.join(published_binary).is_file());

    reset_project_rust_inspect_workspaces(&first_root)?;
    reset_project_rust_inspect_workspaces(&second_root)?;
    let mut first_run = incan_command(&first_root, &incan_home);
    first_run.args(["run", "src/main.incn", "--offline"]);
    run_checked(first_run, "first offline run")?;
    assert_project_rust_inspect_target("first run", &first_root, &canonical_rust_inspect_target)?;
    let run_dependency_before = dependency_artifact_snapshots(&incan_home, "debug", "debug", "libserde_json-")?;
    let mut second_run = incan_command(&second_root, &incan_home);
    second_run.args(["run", "src/main.incn", "--offline"]);
    run_checked(second_run, "compatible offline run")?;
    assert_project_rust_inspect_target("relocated run", &second_root, &canonical_rust_inspect_target)?;
    let run_dependency_after = dependency_artifact_snapshots(&incan_home, "debug", "debug", "libserde_json-")?;
    assert_eq!(run_dependency_after, run_dependency_before);
    let run_warm_snapshot = cache_logical_snapshot(&incan_home)?;
    let mut repeated_run = incan_command(&second_root, &incan_home);
    repeated_run.args(["run", "src/main.incn", "--offline"]);
    run_checked(repeated_run, "repeated warm offline run")?;
    assert_eq!(cache_logical_snapshot(&incan_home)?, run_warm_snapshot);

    reset_project_rust_inspect_workspaces(&first_root)?;
    reset_project_rust_inspect_workspaces(&second_root)?;
    let mut first_test = incan_command(&first_root, &incan_home);
    first_test.args(["test", "tests/cache_test.incn", "--offline"]);
    run_checked(first_test, "first offline test")?;
    assert_project_rust_inspect_target("first test", &first_root, &canonical_rust_inspect_target)?;
    let test_dependency_before = dependency_artifact_snapshots(&incan_home, "test", "debug", "libserde_json-")?;
    let mut second_test = incan_command(&second_root, &incan_home);
    second_test.args(["test", "tests/cache_test.incn", "--offline"]);
    run_checked(second_test, "compatible offline test")?;
    assert_project_rust_inspect_target("relocated test", &second_root, &canonical_rust_inspect_target)?;
    let test_dependency_after = dependency_artifact_snapshots(&incan_home, "test", "debug", "libserde_json-")?;
    assert_eq!(test_dependency_after, test_dependency_before);
    let test_warm_snapshot = cache_logical_snapshot(&incan_home)?;
    let mut repeated_test = incan_command(&second_root, &incan_home);
    repeated_test.args(["test", "tests/cache_test.incn", "--offline"]);
    run_checked(repeated_test, "repeated warm offline test")?;
    assert_eq!(cache_logical_snapshot(&incan_home)?, test_warm_snapshot);

    reset_project_rust_inspect_workspaces(&first_root)?;
    reset_project_rust_inspect_workspaces(&second_root)?;
    let mut first_library = incan_command(&first_root, &incan_home);
    first_library.args(["build", "--lib", "--offline"]);
    run_checked(first_library, "first offline library build")?;
    assert_project_rust_inspect_target("first library", &first_root, &canonical_rust_inspect_target)?;
    let library_dependency_before = dependency_artifact_snapshots(&incan_home, "release", "release", "libserde_json-")?;
    let mut second_library = incan_command(&second_root, &incan_home);
    second_library.args(["build", "--lib", "--offline"]);
    run_checked(second_library, "compatible offline library build")?;
    assert_project_rust_inspect_target("relocated library", &second_root, &canonical_rust_inspect_target)?;
    let library_dependency_after = dependency_artifact_snapshots(&incan_home, "release", "release", "libserde_json-")?;
    assert_eq!(library_dependency_after, library_dependency_before);
    let library_warm_snapshot = cache_logical_snapshot(&incan_home)?;
    let mut repeated_library = incan_command(&second_root, &incan_home);
    repeated_library.args(["build", "--lib", "--offline"]);
    run_checked(repeated_library, "repeated warm offline library build")?;
    assert_eq!(cache_logical_snapshot(&incan_home)?, library_warm_snapshot);
    assert!(first_root.join("target/lib/generated_cache_fixture.incnlib").is_file());
    assert!(second_root.join("target/lib/generated_cache_fixture.incnlib").is_file());
    assert_eq!(
        cache_entry_count_for_profile(&incan_home, "rust-inspect")?,
        1,
        "check/build/run/test/library must share one canonical rust-inspect domain"
    );
    assert_eq!(
        sole_cache_target_for_profile(&incan_home, "rust-inspect")?,
        canonical_rust_inspect_target,
        "a command replaced the canonical rust-inspect compatibility target"
    );
    let lock_root = fixture.path().join("lock-only");
    write_dependency_project(&lock_root)?;
    write_test_and_library_sources(&lock_root)?;
    fs::create_dir_all(lock_root.join(".cargo"))?;
    fs::write(
        lock_root.join(".cargo/config.toml"),
        format!("[build]\nbuild-dir = {:?}\n", inherited_build_dir),
    )?;
    let mut lock = incan_command(&lock_root, &incan_home);
    lock.arg("lock");
    run_checked(lock, "standalone lock prewarm")?;
    assert_project_rust_inspect_target("standalone lock", &lock_root, &canonical_rust_inspect_target)?;
    assert_eq!(
        sole_cache_target_for_profile(&incan_home, "rust-inspect")?,
        canonical_rust_inspect_target,
        "standalone lock created a second rust-inspect compatibility target"
    );
    assert!(!first_root.join("target/.cargo-target").exists());
    assert!(!second_root.join("target/.cargo-target").exists());

    let invalidated_root = fixture.path().join("invalidated");
    write_dependency_project(&invalidated_root)?;
    let before_invalidation = cache_entry_count_for_profile(&incan_home, "release")?;
    let mut invalidated = incan_command(&invalidated_root, &incan_home);
    invalidated
        .args(["build", "src/main.incn", "--offline"])
        .env("RUSTC", "rustc");
    run_checked(invalidated, "backend-selector invalidation build")?;
    assert_eq!(
        cache_entry_count_for_profile(&incan_home, "release")?,
        before_invalidation + 1,
        "a relevant backend selector must isolate the generated target domain"
    );

    let before_explicit = cache_entries(&incan_home)?.len();
    let explicit_target = fixture.path().join("caller-owned-cargo-target");
    let explicit_output = fixture.path().join("caller-owned-output");
    let mut explicit = incan_command(&first_root, &incan_home);
    explicit
        .args(["build", "src/main.incn", "--offline", "--generated-cargo-target-dir"])
        .arg(&explicit_target)
        .arg(&explicit_output);
    run_checked(explicit, "explicit generated target and output build")?;
    assert!(explicit_output.join("Cargo.toml").is_file());
    assert!(explicit_output.join("target/release/generated_cache_fixture").is_file());
    assert!(explicit_target.exists());
    assert_eq!(cache_entries(&incan_home)?.len(), before_explicit);

    let opt_out_root = fixture.path().join("opt-out");
    write_project(&opt_out_root)?;
    let mut opt_out = incan_command(&opt_out_root, &incan_home);
    opt_out
        .args(["build", "src/main.incn", "--offline"])
        .env("INCAN_GENERATED_CACHE", "0");
    run_checked(opt_out, "managed cache opt-out build")?;
    assert!(
        opt_out_root
            .join("target/incan/generated_cache_fixture/target/release/generated_cache_fixture")
            .is_file()
    );
    assert_eq!(cache_entries(&incan_home)?.len(), before_explicit);

    let cache_root = incan_home.join("cache/generated-cargo/v1");
    let interrupted = cache_root.join("interrupted-publication");
    fs::create_dir_all(&interrupted)?;
    fs::write(interrupted.join("partial-artifact"), b"partial")?;
    let mut dry_run = incan_command(fixture.path(), &incan_home);
    dry_run.args([
        "cache",
        "prune",
        "--identity",
        "interrupted-publication",
        "--dry-run",
        "--format",
        "json",
    ]);
    run_checked(dry_run, "interrupted-entry dry-run")?;
    assert!(interrupted.exists(), "dry-run must not mutate the interrupted entry");

    let mut prune = incan_command(fixture.path(), &incan_home);
    prune.args([
        "cache",
        "prune",
        "--identity",
        "interrupted-publication",
        "--format",
        "json",
    ]);
    run_checked(prune, "interrupted-entry cleanup")?;
    assert!(!interrupted.exists());
    Ok(())
}

#[test]
fn active_cache_domain_cannot_be_pruned_during_concurrent_builds() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = tempfile::tempdir()?;
    let first_root = fixture.path().join("first");
    let second_root = fixture.path().join("second");
    write_dependency_project(&first_root)?;
    write_dependency_project(&second_root)?;
    fs::write(
        second_root.join("src/main.incn"),
        "from rust::serde_json import Value\n\ndef cache_json(value: Value) -> Value:\n  return value\n\ndef cache_variant() -> int:\n  return 2\n\ndef main() -> None:\n  pass\n",
    )?;

    let provider_prewarm_home = fixture.path().join("provider-prewarm-home");
    let mut provider_prewarm = incan_command(&first_root, &provider_prewarm_home);
    provider_prewarm.args(["check", "src/main.incn"]);
    run_checked(provider_prewarm, "provider prewarm before concurrent builds")?;

    let incan_home = fixture.path().join("incan-home");
    let managed_root = incan_home.join("cache/generated-cargo/v1");
    let rustc_wrapper = fixture.path().join("blocking-rustc-wrapper.sh");
    let rustc_ready = fixture.path().join("blocking-rustc-ready");
    let rustc_release = fixture.path().join("blocking-rustc-release");
    write_blocking_rustc_wrapper(&rustc_wrapper)?;
    let release_guard = RustcReleaseGuard {
        path: rustc_release.clone(),
    };

    let start_barrier = Arc::new(Barrier::new(3));
    let first_home = incan_home.clone();
    let first_start = Arc::clone(&start_barrier);
    let first_wrapper = rustc_wrapper.clone();
    let first_managed_root = managed_root.clone();
    let first_ready = rustc_ready.clone();
    let first_release = rustc_release.clone();
    let first = std::thread::spawn(move || {
        first_start.wait();
        let mut command = incan_command(&first_root, &first_home);
        command
            .args(["build", "src/main.incn", "--offline"])
            .env("RUSTC_WRAPPER", first_wrapper)
            .env("INCAN_CACHE_TEST_MANAGED_ROOT", first_managed_root)
            .env("INCAN_CACHE_TEST_RUSTC_READY", first_ready)
            .env("INCAN_CACHE_TEST_RUSTC_RELEASE", first_release);
        run_checked(command, "concurrent first build")
            .map(|_| ())
            .map_err(|error| error.to_string())
    });

    let second_home = incan_home.clone();
    let second_start = Arc::clone(&start_barrier);
    let second_wrapper = rustc_wrapper;
    let second_managed_root = managed_root;
    let second_ready = rustc_ready.clone();
    let second_release = rustc_release;
    let second = std::thread::spawn(move || {
        second_start.wait();
        let mut command = incan_command(&second_root, &second_home);
        command
            .args(["build", "src/main.incn", "--offline"])
            .env("RUSTC_WRAPPER", second_wrapper)
            .env("INCAN_CACHE_TEST_MANAGED_ROOT", second_managed_root)
            .env("INCAN_CACHE_TEST_RUSTC_READY", second_ready)
            .env("INCAN_CACHE_TEST_RUSTC_RELEASE", second_release);
        run_checked(command, "concurrent second build")
            .map(|_| ())
            .map_err(|error| error.to_string())
    });

    start_barrier.wait();
    let observation = (|| -> Result<(String, serde_json::Value), Box<dyn std::error::Error>> {
        let deadline = Instant::now() + Duration::from_secs(120);
        while !rustc_ready.is_file() {
            if first.is_finished() && second.is_finished() {
                return Err("concurrent builds completed before entering the blocked managed-Cargo phase".into());
            }
            if Instant::now() >= deadline {
                return Err(
                    "concurrent builds did not enter the blocked managed-Cargo phase within 120 seconds".into(),
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let active_identity = active_cache_identity(&incan_home)?
            .ok_or("blocked managed-Cargo build did not expose its active cache lease")?;
        let mut prune = incan_command(fixture.path(), &incan_home);
        prune.args(["cache", "prune", "--identity", &active_identity, "--format", "json"]);
        let prune_output = run_checked(prune, "active-domain prune")?;
        let prune_report = serde_json::from_slice::<serde_json::Value>(&prune_output.stdout)?;
        Ok((active_identity, prune_report))
    })();

    drop(release_guard);
    match first.join() {
        Ok(result) => result?,
        Err(_) => return Err("first cache build panicked".into()),
    }
    match second.join() {
        Ok(result) => result?,
        Err(_) => return Err("second cache build panicked".into()),
    }
    let (active_identity, prune_report) = observation?;
    assert_eq!(
        prune_report
            .get("skipped_active_entries")
            .and_then(serde_json::Value::as_array)
            .and_then(|entries| entries.first())
            .and_then(serde_json::Value::as_str),
        Some(active_identity.as_str())
    );
    assert_eq!(cache_entry_count_for_profile(&incan_home, "release")?, 1);
    Ok(())
}

#[test]
fn cold_explicit_target_owns_dependency_preheat_without_managed_side_effects() -> Result<(), Box<dyn std::error::Error>>
{
    let fixture = tempfile::tempdir()?;
    let incan_home = fixture.path().join("incan-home");
    let project_root = fixture.path().join("project");
    write_dependency_project(&project_root)?;
    let explicit_target = fixture.path().join("caller-owned-cargo-target");
    let explicit_output = fixture.path().join("caller-owned-output");

    let mut build = incan_command(&project_root, &incan_home);
    build
        .args(["build", "src/main.incn", "--offline", "--generated-cargo-target-dir"])
        .arg(&explicit_target)
        .arg(&explicit_output);
    run_checked(build, "cold explicit-target build")?;

    assert!(project_root.join("incan.lock").is_file());
    assert!(explicit_target.exists());
    assert!(explicit_output.join("Cargo.toml").is_file());
    assert!(explicit_output.join("target/release/generated_cache_fixture").is_file());
    assert!(
        !project_root.join("target/incan/generated_cache_fixture").exists(),
        "an explicit generated output also populated the default project-local generated directory"
    );
    assert!(
        cache_entries(&incan_home)?.is_empty(),
        "cold explicit-target lock preheat populated the managed cache"
    );
    Ok(())
}

#[test]
fn repeated_builds_discard_oversized_rebuildable_domain_output() -> Result<(), Box<dyn std::error::Error>> {
    const ENTRY_LIMIT: u64 = 65_536;
    let fixture = tempfile::tempdir()?;
    let incan_home = fixture.path().join("incan-home");
    let project_root = fixture.path().join("project");
    write_project(&project_root)?;
    let mut previous_identities = None;

    for label in ["first bounded build", "second bounded build"] {
        let mut build = incan_command(&project_root, &incan_home);
        build
            .args(["build", "src/main.incn", "--offline"])
            .env("INCAN_GENERATED_CACHE_MAX_ENTRY_BYTES", ENTRY_LIMIT.to_string());
        run_checked(build, label)?;

        let entries = cache_entries(&incan_home)?;
        assert!(!entries.is_empty(), "bounded build omitted its cache domains");
        let identities = entries
            .iter()
            .map(|entry| {
                entry
                    .get("identity")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .ok_or("bounded cache entry omitted its identity")
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        if let Some(previous_identities) = previous_identities.as_ref() {
            assert_eq!(
                &identities, previous_identities,
                "repeated bounded build created a new compatibility domain"
            );
        } else {
            previous_identities = Some(identities);
        }
        for entry in entries {
            let entry_path = entry
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map(PathBuf::from)
                .ok_or("bounded cache entry omitted its path")?;
            assert!(
                !entry_path.join("target").exists(),
                "oversized rebuildable Cargo output remained after the lease became idle"
            );
            assert!(
                entry
                    .get("bytes")
                    .and_then(serde_json::Value::as_u64)
                    .is_some_and(|bytes| bytes <= ENTRY_LIMIT),
                "retained metadata exceeded the configured per-domain bound"
            );
        }
        assert!(
            project_root
                .join("target/incan/generated_cache_fixture/target/release/generated_cache_fixture")
                .is_file()
        );
    }
    Ok(())
}

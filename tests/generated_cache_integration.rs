#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Barrier};
use std::time::{Duration, SystemTime};

fn incan_command(project_root: &Path, incan_home: &Path) -> Command {
    let provider_store = std::env::var_os("INCAN_INTERNAL_SDK_PROVIDER_STORE")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target/incan_test_sdk_provider_store"));
    let mut command = Command::new(env!("CARGO_BIN_EXE_incan"));
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
        .env("INCAN_INTERNAL_SDK_PROVIDER_STORE", provider_store);
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
        "command configured rust-inspect outside the canonical managed target {}",
        expected_target.display()
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
    assert_project_rust_inspect_target(&check_root, &canonical_rust_inspect_target)?;
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
    assert_project_rust_inspect_target(&first_root, &canonical_rust_inspect_target)?;
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
    assert_project_rust_inspect_target(&second_root, &canonical_rust_inspect_target)?;
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
    assert_project_rust_inspect_target(&first_root, &canonical_rust_inspect_target)?;
    let run_dependency_before = dependency_artifact_snapshots(&incan_home, "debug", "debug", "libserde_json-")?;
    let mut second_run = incan_command(&second_root, &incan_home);
    second_run.args(["run", "src/main.incn", "--offline"]);
    run_checked(second_run, "compatible offline run")?;
    assert_project_rust_inspect_target(&second_root, &canonical_rust_inspect_target)?;
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
    assert_project_rust_inspect_target(&first_root, &canonical_rust_inspect_target)?;
    let test_dependency_before = dependency_artifact_snapshots(&incan_home, "test", "debug", "libserde_json-")?;
    let mut second_test = incan_command(&second_root, &incan_home);
    second_test.args(["test", "tests/cache_test.incn", "--offline"]);
    run_checked(second_test, "compatible offline test")?;
    assert_project_rust_inspect_target(&second_root, &canonical_rust_inspect_target)?;
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
    assert_project_rust_inspect_target(&first_root, &canonical_rust_inspect_target)?;
    let library_dependency_before = dependency_artifact_snapshots(&incan_home, "release", "release", "libserde_json-")?;
    let mut second_library = incan_command(&second_root, &incan_home);
    second_library.args(["build", "--lib", "--offline"]);
    run_checked(second_library, "compatible offline library build")?;
    assert_project_rust_inspect_target(&second_root, &canonical_rust_inspect_target)?;
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
    let mut lock = incan_command(&lock_root, &incan_home);
    lock.arg("lock");
    run_checked(lock, "standalone lock prewarm")?;
    assert_project_rust_inspect_target(&lock_root, &canonical_rust_inspect_target)?;
    assert_eq!(
        sole_cache_target_for_profile(&incan_home, "rust-inspect")?,
        canonical_rust_inspect_target,
        "standalone lock created a second rust-inspect compatibility target"
    );
    assert!(!first_root.join("target/.cargo-target").exists());
    assert!(!second_root.join("target/.cargo-target").exists());

    let third_root = fixture.path().join("third");
    let fourth_root = fixture.path().join("fourth");
    write_dependency_project(&third_root)?;
    write_dependency_project(&fourth_root)?;
    fs::write(
        fourth_root.join("src/main.incn"),
        "from rust::serde_json import Value\n\ndef cache_json(value: Value) -> Value:\n  return value\n\ndef cache_variant() -> int:\n  return 4\n\ndef main() -> None:\n  pass\n",
    )?;
    let concurrent_home = fixture.path().join("concurrent-incan-home");
    let start_barrier = Arc::new(Barrier::new(3));
    let third_home = concurrent_home.clone();
    let third_start = Arc::clone(&start_barrier);
    let third = std::thread::spawn(move || {
        third_start.wait();
        let mut command = incan_command(&third_root, &third_home);
        command.args(["build", "src/main.incn", "--offline"]);
        run_checked(command, "concurrent third build")
            .map(|_| ())
            .map_err(|error| error.to_string())
    });
    let fourth_home = concurrent_home.clone();
    let fourth_start = Arc::clone(&start_barrier);
    let fourth = std::thread::spawn(move || {
        fourth_start.wait();
        let mut command = incan_command(&fourth_root, &fourth_home);
        command.args(["build", "src/main.incn", "--offline"]);
        run_checked(command, "concurrent fourth build")
            .map(|_| ())
            .map_err(|error| error.to_string())
    });
    start_barrier.wait();
    let mut active_identity = None;
    for _ in 0..100 {
        active_identity = active_cache_identity(&concurrent_home)?;
        if active_identity.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let active_identity = active_identity.ok_or("concurrent builds never exposed an active cache lease")?;
    let mut prune = incan_command(fixture.path(), &concurrent_home);
    prune.args(["cache", "prune", "--identity", &active_identity, "--format", "json"]);
    let prune_output = run_checked(prune, "active-domain prune")?;
    let prune_report = serde_json::from_slice::<serde_json::Value>(&prune_output.stdout)?;
    assert_eq!(
        prune_report
            .get("skipped_active_entries")
            .and_then(serde_json::Value::as_array)
            .and_then(|entries| entries.first())
            .and_then(serde_json::Value::as_str),
        Some(active_identity.as_str())
    );
    match third.join() {
        Ok(result) => result?,
        Err(_) => return Err("third cache build panicked".into()),
    }
    match fourth.join() {
        Ok(result) => result?,
        Err(_) => return Err("fourth cache build panicked".into()),
    }
    assert_eq!(cache_entry_count_for_profile(&concurrent_home, "release")?, 1);

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
    assert!(explicit_target.exists());
    assert_eq!(cache_entries(&incan_home)?.len(), before_explicit);

    let opt_out_root = fixture.path().join("opt-out");
    write_project(&opt_out_root)?;
    let mut opt_out = incan_command(&opt_out_root, &incan_home);
    opt_out
        .args(["build", "src/main.incn", "--offline"])
        .env("INCAN_GENERATED_CACHE", "0");
    run_checked(opt_out, "managed cache opt-out build")?;
    assert!(opt_out_root.join("target/incan/main/target").is_dir());
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

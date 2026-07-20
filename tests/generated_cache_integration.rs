#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn incan_command(project_root: &Path, incan_home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_incan"));
    command
        .current_dir(project_root)
        // The repository test harness deliberately gives other nested Cargo tests one explicit shared target. This
        // integration test exercises the installed default, so inherited harness overrides must not bypass it.
        .env_remove("INCAN_GENERATED_CARGO_TARGET_DIR")
        .env_remove("INCAN_GENERATED_CACHE")
        .env_remove("INCAN_TEST_SHARED_TARGET_DIR")
        .env("INCAN_HOME", incan_home)
        .env("CARGO_NET_OFFLINE", "true")
        .env(
            "INCAN_INTERNAL_SDK_PROVIDER_STORE",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("target/incan_test_sdk_provider_store"),
        );
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
    fs::write(&source, "def main() -> None:\n  pass\n")?;
    Ok(source)
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

#[test]
fn managed_cache_reuses_offline_across_projects_and_cleans_interrupted_entry() -> Result<(), Box<dyn std::error::Error>>
{
    let fixture = tempfile::tempdir()?;
    let incan_home = fixture.path().join("incan-home");
    let first_root = fixture.path().join("first");
    let second_root = fixture.path().join("second");
    write_project(&first_root)?;
    write_project(&second_root)?;

    let mut first = incan_command(&first_root, &incan_home);
    first.args(["build", "src/main.incn", "--offline"]);
    run_checked(first, "first offline build")?;

    let mut second = incan_command(&second_root, &incan_home);
    second.args(["build", "src/main.incn", "--offline"]);
    run_checked(second, "compatible offline build")?;

    let entries = cache_entries(&incan_home)?;
    assert_eq!(entries.len(), 1, "compatible release builds should share one domain");
    assert!(first_root.join("target/incan/main/target/release/main").is_file());
    assert!(second_root.join("target/incan/main/target/release/main").is_file());

    let third_root = fixture.path().join("third");
    let fourth_root = fixture.path().join("fourth");
    write_project(&third_root)?;
    write_project(&fourth_root)?;
    let third_home = incan_home.clone();
    let third = std::thread::spawn(move || {
        let mut command = incan_command(&third_root, &third_home);
        command.args(["build", "src/main.incn", "--offline"]);
        run_checked(command, "concurrent third build")
            .map(|_| ())
            .map_err(|error| error.to_string())
    });
    let fourth_home = incan_home.clone();
    let fourth = std::thread::spawn(move || {
        let mut command = incan_command(&fourth_root, &fourth_home);
        command.args(["build", "src/main.incn", "--offline"]);
        run_checked(command, "concurrent fourth build")
            .map(|_| ())
            .map_err(|error| error.to_string())
    });
    match third.join() {
        Ok(result) => result?,
        Err(_) => return Err("third cache build panicked".into()),
    }
    match fourth.join() {
        Ok(result) => result?,
        Err(_) => return Err("fourth cache build panicked".into()),
    }
    assert_eq!(cache_entries(&incan_home)?.len(), 1);

    let invalidated_root = fixture.path().join("invalidated");
    write_project(&invalidated_root)?;
    let mut invalidated = incan_command(&invalidated_root, &incan_home);
    invalidated
        .args(["build", "src/main.incn", "--offline"])
        .env("RUSTC", "rustc");
    run_checked(invalidated, "backend-selector invalidation build")?;
    assert_eq!(
        cache_entries(&incan_home)?.len(),
        2,
        "a relevant backend selector must isolate the generated target domain"
    );

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

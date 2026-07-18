use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn incan_binary() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_incan") {
        return PathBuf::from(path);
    }
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        let path = PathBuf::from(target_dir).join("debug").join("incan");
        if path.exists() {
            return path;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/incan")
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
        .env(
            "INCAN_INTERNAL_SDK_PROVIDER_STORE",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("target/incan_test_sdk_provider_store"),
        )
        .output()?)
}

fn run_cargo(current_dir: &Path, args: &[&str], target_dir: &Path) -> Result<Output, Box<dyn std::error::Error>> {
    Ok(Command::new("cargo")
        .args(args)
        .current_dir(current_dir)
        .env("CARGO_NET_OFFLINE", "true")
        .env("CARGO_TARGET_DIR", target_dir)
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

fn write_fixture_file(root: &Path, relative_path: &str, contents: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = root.join(relative_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}

fn write_producer(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let producer = root.join("native_items");
    write_fixture_file(
        &producer,
        "incan.toml",
        include_str!("fixtures/generated_rust_native_consumer/producer/incan.toml"),
    )?;
    write_fixture_file(
        &producer,
        "src/lib.incn",
        include_str!("fixtures/generated_rust_native_consumer/producer/src/lib.incn"),
    )?;
    write_fixture_file(
        &producer,
        "src/counters.incn",
        include_str!("fixtures/generated_rust_native_consumer/producer/src/counters.incn"),
    )?;
    Ok(producer)
}

fn write_consumer(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let consumer = root.join("native_consumer");
    write_fixture_file(
        &consumer,
        "Cargo.toml",
        include_str!("fixtures/generated_rust_native_consumer/consumer/Cargo.toml"),
    )?;
    write_fixture_file(
        &consumer,
        "src/lib.rs",
        include_str!("fixtures/generated_rust_native_consumer/consumer/src/lib.rs"),
    )?;
    Ok(consumer)
}

#[test]
fn native_rust_consumer_can_call_generated_public_items() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let producer = write_producer(tmp.path())?;

    let build_output = run_incan(&producer, &["build", "--lib"])?;
    assert_success(&build_output, "incan build --lib native consumer producer");

    let artifact_root = producer.join("target/lib");
    assert!(
        artifact_root.join("Cargo.toml").is_file(),
        "expected generated Rust library Cargo.toml at {}",
        artifact_root.display()
    );
    assert!(
        artifact_root.join("src/lib.rs").is_file(),
        "expected generated Rust library root at {}",
        artifact_root.join("src/lib.rs").display()
    );

    let consumer = write_consumer(tmp.path())?;
    let cargo_test = run_cargo(
        &consumer,
        &["test", "--offline"],
        &tmp.path().join("native-cargo-target"),
    )?;
    assert_success(&cargo_test, "native Rust cargo test against generated library");

    Ok(())
}

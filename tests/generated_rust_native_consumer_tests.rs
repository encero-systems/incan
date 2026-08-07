use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

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
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let stdlib_root = source_root.join("crates/incan_stdlib/stdlib");
    let stored_suite = compiler_suite_rustc_inputs().is_some();
    let mut command = Command::new(incan_binary());
    command
        .args(args)
        .current_dir(current_dir)
        .env("CARGO_NET_OFFLINE", "true")
        .env("INCAN_NO_BANNER", "1")
        .env(
            "INCAN_GENERATED_CARGO_TARGET_DIR",
            support::generated_cargo_target_dir(),
        )
        .env("INCAN_SOURCE_ROOT", source_root)
        .env("INCAN_STDLIB", &stdlib_root)
        .env("INCAN_STDLIB_DIR", &stdlib_root)
        .env("INCAN_TOOLCHAIN_CRATES_DIR", source_root.join("crates"));
    if stored_suite {
        // The stored compiler-suite runner supplies the direct-rustc closure below. Keep this producer step limited
        // to checked/generated library source so the test body never asks its normal-command child to run Cargo.
        command.env("INCAN_INTERNAL_LIBRARY_ARTIFACT_ONLY", "1");
    } else {
        command.env("INCAN_INTERNAL_SDK_PROVIDER_STORE", support::sdk_provider_store());
    }
    Ok(command.output()?)
}

/// The compiler-suite runner injects this exact direct-rustc closure only while executing its Cargo-free batch.
///
/// A normal `cargo test` keeps the pre-existing Cargo consumer branch below. That preserves the integration test's
/// broad compatibility coverage while ensuring the Oven-scheduled form neither launches Cargo nor reads a Cargo
/// target directory.
struct CompilerSuiteRustcInputs {
    rustc: PathBuf,
    stdlib: PathBuf,
    sdk_inventory: PathBuf,
    dependency_paths: Vec<PathBuf>,
    externs: BTreeMap<String, PathBuf>,
}

fn compiler_suite_rustc_inputs() -> Option<CompilerSuiteRustcInputs> {
    let rustc = std::env::var_os("INCAN_OVEN_COMPILER_SUITE_RUSTC")?;
    let stdlib = std::env::var_os("INCAN_OVEN_COMPILER_SUITE_STDLIB")?;
    let sdk_inventory = std::env::var_os("INCAN_SDK_INVENTORY")?;
    let dependency_path_count = std::env::var("INCAN_OVEN_COMPILER_SUITE_DEPENDENCY_PATH_COUNT")
        .ok()?
        .parse::<usize>()
        .ok()?;
    let dependency_paths = (0..dependency_path_count)
        .map(|index| std::env::var_os(format!("INCAN_OVEN_COMPILER_SUITE_DEPENDENCY_PATH_{index}")))
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .map(PathBuf::from)
        .collect();
    let extern_count = std::env::var("INCAN_OVEN_COMPILER_SUITE_EXTERN_COUNT")
        .ok()?
        .parse::<usize>()
        .ok()?;
    let externs = (0..extern_count)
        .map(|index| {
            Some((
                std::env::var(format!("INCAN_OVEN_COMPILER_SUITE_EXTERN_{index}_NAME")).ok()?,
                PathBuf::from(std::env::var_os(format!(
                    "INCAN_OVEN_COMPILER_SUITE_EXTERN_{index}_PATH"
                ))?),
            ))
        })
        .collect::<Option<BTreeMap<_, _>>>()?;
    Some(CompilerSuiteRustcInputs {
        rustc: PathBuf::from(rustc),
        stdlib: PathBuf::from(stdlib),
        sdk_inventory: PathBuf::from(sdk_inventory),
        dependency_paths,
        externs,
    })
}

fn suite_rustc_command(inputs: &CompilerSuiteRustcInputs, current_dir: &Path) -> Command {
    let mut command = Command::new(&inputs.rustc);
    command.current_dir(current_dir);
    for path in &inputs.dependency_paths {
        command.arg("-L").arg(format!("dependency={}", path.display()));
    }
    for (crate_name, path) in &inputs.externs {
        command.arg("--extern").arg(format!("{crate_name}={}", path.display()));
    }
    command
}

fn direct_oven_native_consumer_test(
    producer: &Path,
    consumer: &Path,
    forge: &Path,
    output_root: &Path,
    inputs: &CompilerSuiteRustcInputs,
) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(output_root)?;
    let sdk_components = inputs
        .sdk_inventory
        .parent()
        .ok_or("stored SDK inventory must have a provider root")?
        .join("components");
    let stdlib_core_source = sdk_components.join("stdlib-core/src/lib.rs");
    assert!(
        stdlib_core_source.is_file(),
        "stored compiler-suite SDK inventory lacks stdlib-core source at {}",
        stdlib_core_source.display()
    );
    let stdlib_core_library = output_root.join("libincan_stdlib_core.rlib");
    let stdlib_core_output = suite_rustc_command(inputs, &sdk_components)
        .args([
            "--edition=2021",
            "--crate-name",
            "incan_stdlib_core",
            "--crate-type",
            "lib",
            "--extern",
        ])
        .arg(format!("incan_stdlib={}", inputs.stdlib.display()))
        .arg(&stdlib_core_source)
        .arg("-o")
        .arg(&stdlib_core_library)
        .output()?;
    assert_success(&stdlib_core_output, "direct rustc sealed stdlib-core provider library");

    let producer_library = output_root.join("libnative_consumer_core.rlib");
    let mut producer_command = suite_rustc_command(inputs, producer);
    // This fixture deliberately compiles the generated Cargo-shaped source without starting Cargo. Supply the three
    // package facts Cargo would otherwise inject, from the fixture's declared project metadata, to preserve the
    // generated stdlib-version assertion under direct rustc.
    producer_command
        .env("CARGO_MANIFEST_DIR", producer.join("target/lib"))
        .env("CARGO_PKG_NAME", "native_consumer_core")
        .env("CARGO_PKG_VERSION", "0.1.0")
        .arg("-L")
        .arg(format!("dependency={}", output_root.display()));
    let producer_output = producer_command
        .args([
            "--edition=2024",
            "--crate-name",
            "native_consumer_core",
            "--crate-type",
            "lib",
            "--extern",
        ])
        .arg(format!("incan_stdlib={}", inputs.stdlib.display()))
        .arg("--extern")
        .arg(format!("incan_stdlib_core={}", stdlib_core_library.display()))
        .arg(producer.join("target/lib/src/lib.rs"))
        .arg("-o")
        .arg(&producer_library)
        .output()?;
    assert_success(&producer_output, "direct rustc generated producer library");

    let consumer_binary = output_root.join("native_consumer_host_tests");
    let consumer_output = suite_rustc_command(inputs, consumer)
        .arg("-L")
        .arg(format!("dependency={}", output_root.display()))
        .args([
            "--edition=2021",
            "--crate-name",
            "native_consumer_host",
            "--test",
            "--extern",
        ])
        .arg(format!("native_consumer_core={}", producer_library.display()))
        .arg("--extern")
        .arg(format!("incan_stdlib_core={}", stdlib_core_library.display()))
        .arg(consumer.join("src/lib.rs"))
        .arg("-o")
        .arg(&consumer_binary)
        .output()?;
    assert_success(&consumer_output, "direct rustc native consumer test");
    let consumer_run = Command::new(&consumer_binary).current_dir(consumer).output()?;
    assert_success(&consumer_run, "direct rustc native consumer test binary");

    let forge_output = suite_rustc_command(inputs, forge)
        .arg("-L")
        .arg(format!("dependency={}", output_root.display()))
        .args([
            "--edition=2021",
            "--crate-name",
            "native_constructor_forge",
            "--crate-type",
            "lib",
        ])
        .args(["--cfg", r#"feature="admission""#])
        .args(["--cfg", r#"feature="defaulted""#])
        .args(["--cfg", r#"feature="mixed""#])
        .arg("--extern")
        .arg(format!("native_consumer_core={}", producer_library.display()))
        .arg("--extern")
        .arg(format!("incan_stdlib_core={}", stdlib_core_library.display()))
        .arg(forge.join("src/lib.rs"))
        .arg("-o")
        .arg(output_root.join("libnative_constructor_forge.rlib"))
        .output()?;
    assert!(
        !forge_output.status.success(),
        "native Rust forge unexpectedly compiled private model constructor inputs.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&forge_output.stdout),
        String::from_utf8_lossy(&forge_output.stderr)
    );
    let forge_diagnostics = String::from_utf8_lossy(&forge_output.stderr);
    for nominal in ["Admission", "Defaulted", "Mixed"] {
        assert!(
            forge_diagnostics.contains(nominal),
            "native Rust forge failed for an unrelated reason; expected a constructor diagnostic for {nominal}:\n{forge_diagnostics}"
        );
    }
    assert!(
        forge_diagnostics.contains("expected function, tuple struct or tuple variant")
            || forge_diagnostics.contains("takes 1 argument but 2 arguments were supplied"),
        "native Rust forge did not fail at the sealed constructor boundary:\n{forge_diagnostics}"
    );
    Ok(())
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

/// Materialize the generated-library producer fixture, including sealed-model coverage.
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
    write_fixture_file(
        &producer,
        "src/admission.incn",
        include_str!("fixtures/generated_rust_native_consumer/producer/src/admission.incn"),
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

/// Materialize the native Rust crate that must not forge private model construction.
fn write_forge(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let forge = root.join("forge");
    write_fixture_file(
        &forge,
        "Cargo.toml",
        include_str!("fixtures/generated_rust_native_consumer/forge/Cargo.toml"),
    )?;
    write_fixture_file(
        &forge,
        "src/lib.rs",
        include_str!("fixtures/generated_rust_native_consumer/forge/src/lib.rs"),
    )?;
    Ok(forge)
}

#[test]
/// Verify generated-library Rust retains public capabilities without exposing private constructors.
fn native_rust_consumer_can_call_generated_public_items() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let producer = write_producer(tmp.path())?;

    let build_output = run_incan(&producer, &["build", "--lib"])?;
    assert_success(&build_output, "incan build --lib native consumer producer");
    let build_diagnostics = format!(
        "{}\n{}",
        String::from_utf8_lossy(&build_output.stdout),
        String::from_utf8_lossy(&build_output.stderr)
    );
    assert!(
        !build_diagnostics.contains("private_interfaces"),
        "generated producer leaked a private type through a public Rust interface:\n{build_diagnostics}"
    );

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
    let forge = write_forge(tmp.path())?;
    if let Some(inputs) = compiler_suite_rustc_inputs() {
        direct_oven_native_consumer_test(&producer, &consumer, &forge, &tmp.path().join("native-direct"), &inputs)?;
        return Ok(());
    }

    let cargo_test = run_cargo(
        &consumer,
        &["test", "--offline"],
        &tmp.path().join("native-cargo-target"),
    )?;
    assert_success(&cargo_test, "native Rust cargo test against generated library");
    let forge_check = run_cargo(
        &forge,
        &["check", "--offline", "--all-features"],
        &tmp.path().join("native-cargo-target"),
    )?;
    assert!(
        !forge_check.status.success(),
        "native Rust forge unexpectedly compiled private model constructor inputs.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&forge_check.stdout),
        String::from_utf8_lossy(&forge_check.stderr)
    );
    let forge_diagnostics = String::from_utf8_lossy(&forge_check.stderr);
    for nominal in ["Admission", "Defaulted", "Mixed"] {
        assert!(
            forge_diagnostics.contains(nominal),
            "native Rust forge failed for an unrelated reason; expected a constructor diagnostic for {nominal}:\n\
             {forge_diagnostics}"
        );
    }
    assert!(
        forge_diagnostics.contains("expected function, tuple struct or tuple variant")
            || forge_diagnostics.contains("takes 1 argument but 2 arguments were supplied"),
        "native Rust forge did not fail at the sealed constructor boundary:\n{forge_diagnostics}"
    );

    Ok(())
}

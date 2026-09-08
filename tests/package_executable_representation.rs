//! Real producer-to-source-unavailable-consumer acceptance for public package execution.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use incan::library_manifest::LibraryManifest;
use incan::library_manifest::published_layout::executable_surface_path;
use incan_semantics_core::executable_representation::SurfaceReader;

mod support;

/// Run the repository-built compiler with the test harness's coherent SDK/provider selection.
fn command(project: &Path) -> Command {
    let mut command = Command::new(support::incan_binary());
    command
        .current_dir(project)
        .env("INCAN_NO_BANNER", "1")
        .env_remove("INCAN_INTERNAL_PROJECT_ROOT")
        .env_remove("INCAN_INTERNAL_MANIFEST_OVERRIDE");
    command
}

/// Require a command to succeed while retaining complete diagnostics for a failed producer or consumer boundary.
fn success(output: Output, phase: &str) -> Result<Output, Box<dyn Error>> {
    if !output.status.success() {
        return Err(format!(
            "{phase} failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(output)
}

/// Publish a library or native consumer through the normal explicit Oven bake boundary.
fn bake(project: &Path) -> Result<(), Box<dyn Error>> {
    let mut build = command(project);
    build.args(["oven", "bake", "--project", "."]);
    support::configure_explicit_oven_bake_command(&mut build)?;
    success(build.output()?, "Oven bake")?;
    Ok(())
}

/// Create one minimal project without sharing mutable source or generated output with another test.
fn project(root: &Path, name: &str, source_name: &str, source: &str, dependencies: &str) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        root.join("loaf.toml"),
        format!("[project]\nname = \"{name}\"\nversion = \"1.2.3\"\n{dependencies}"),
    )?;
    fs::write(root.join("src").join(source_name), source)?;
    Ok(())
}

/// Native and non-linking consumers select the same alias target after the producer source has been removed.
#[test]
fn source_unavailable_package_executes_alias_defaults_and_public_closure() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let producer = temporary.path().join("producer");
    let consumer = temporary.path().join("consumer");
    project(
        &producer,
        "arithmetic",
        "lib.incn",
        r#"pub model Pair:
    pub value: int

pub enum Mode:
    Ready
    Idle

pub def base() -> int:
    return 20

pub def doubled(value: int = base()) -> int:
    return value * 2 + 2

def private_secret() -> int:
    return 999
"#,
        "",
    )?;
    bake(&producer)?;
    success(
        command(&producer)
            .args(["build", "--lib", "--locked", "src/lib.incn"])
            .output()?,
        "library publication",
    )?;
    let manifest_path = producer.join("target/lib/arithmetic.incnlib");
    let manifest = LibraryManifest::read_from_path(&manifest_path)?;
    let surface = executable_surface_path(&manifest_path, &manifest).ok_or("manifest has no representation")?;
    let bytes = fs::read(&surface)?;
    assert!(
        !bytes
            .windows(b"private_secret".len())
            .any(|part| part == b"private_secret")
    );
    let doubled = manifest
        .contract_metadata
        .identity_graph
        .canonical_for_public_name("doubled")
        .ok_or("doubled has no manifest identity")?;
    assert!(SurfaceReader::open(&bytes)?.covers(&doubled));
    let source = "from pub::renamed import doubled as answer, Pair as Item, Mode as State\n\ndef main() -> None:\n    item = Item(value=0)\n    if State.Ready == State.Ready:\n        println(answer() + item.value)\n";
    project(
        &consumer,
        "consumer",
        "main.incn",
        source,
        "\n[dependencies]\nrenamed = { path = \"../producer\" }\n",
    )?;
    bake(&consumer)?;
    let native = success(
        command(&consumer).args(["run", "--locked", "src/main.incn"]).output()?,
        "native consumer",
    )?;
    assert_eq!(String::from_utf8_lossy(&native.stdout).trim(), "42");
    fs::remove_dir_all(producer.join("src"))?;
    fs::remove_file(producer.join("loaf.toml"))?;
    let report_path = consumer.join("execution.json");
    let replacement = success(
        command(&consumer)
            .args([
                "build",
                "src/main.incn",
                "--backend",
                "replacement",
                "--report",
                "json",
                "--report-output",
            ])
            .arg(&report_path)
            .output()?,
        "source-unavailable replacement",
    )?;
    assert_eq!(replacement.stdout, native.stdout);
    let report: serde_json::Value = serde_json::from_slice(&fs::read(report_path)?)?;
    assert_eq!(report["replacement_execution"]["package_declarations_decoded"], 4);
    assert_eq!(
        report["replacement_execution"]["package_content_bytes_verified"],
        bytes.len()
    );
    assert!(
        report["replacement_execution"]["package_payload_bytes_read"]
            .as_u64()
            .is_some_and(|count| count > 0)
    );
    assert!(consumer.join(".incan/backend/receipt.json").is_file());
    assert_eq!(
        fs::read(surface)?,
        bytes,
        "consumer must never regenerate a dependency representation"
    );
    Ok(())
}

/// A required unavailable call behind a branch refuses before the earlier print or any successful receipt.
#[test]
fn missing_package_representation_refuses_before_output_and_receipt() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let producer = temporary.path().join("producer");
    let consumer = temporary.path().join("consumer");
    project(
        &producer,
        "conditional_provider",
        "lib.incn",
        "pub def answer() -> int:\n    return 42\n",
        "",
    )?;
    bake(&producer)?;
    let manifest_path = producer.join("target/lib/conditional_provider.incnlib");
    let mut manifest = LibraryManifest::read_from_path(&manifest_path)?;
    // Absence is an explicitly valid linking-only package, not corrupt semantic payload bytes.
    manifest.contract_metadata.executable_representation = None;
    manifest.write_to_path(&manifest_path)?;
    fs::remove_dir_all(producer.join("src"))?;
    fs::remove_file(producer.join("loaf.toml"))?;
    project(
        &consumer,
        "consumer",
        "main.incn",
        "from pub::renamed import answer\n\ndef main() -> None:\n    println(\"must not print\")\n    if False:\n        println(answer())\n",
        "\n[dependencies]\nrenamed = { path = \"../producer\" }\n",
    )?;
    let output = command(&consumer)
        .args(["build", "src/main.incn", "--backend", "replacement"])
        .output()?;
    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "program output must remain empty: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("conditional_provider")
            && error.contains("1.2.3")
            && error.contains("executable representation"),
        "{error}"
    );
    assert!(
        !error.contains("INCAN-R988-UNSUPPORTED"),
        "package refusal must not blame a language construct: {error}"
    );
    assert!(!consumer.join(".incan/backend/receipt.json").exists());
    Ok(())
}

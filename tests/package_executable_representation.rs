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

/// Capture complete artifact bytes and inventory without following symbolic links into external stores.
fn artifact_snapshot(root: &Path) -> Result<std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>, Box<dyn Error>> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = std::collections::BTreeMap::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                pending.push(path);
            } else {
                let bytes = if entry.file_type()?.is_symlink() {
                    fs::read_link(&path)?.to_string_lossy().as_bytes().to_vec()
                } else {
                    fs::read(&path)?
                };
                files.insert(path.strip_prefix(root)?.to_path_buf(), bytes);
            }
        }
    }
    Ok(files)
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

    // A native derive deliberately fails at rustc after preparation has generated a different public function.
    // Both the prior linked artifact and its executable publication must survive that ordinary rebuild failure.
    let artifact_before = artifact_snapshot(&producer.join("target/lib"))?;
    let authored = fs::read_to_string(producer.join("src/lib.incn"))?;
    fs::write(
        producer.join("src/lib.incn"),
        format!(
            "{}\n@rust.derive(Eq, Hash)\nmodel InvalidNativeDerive:\n    value: float\n",
            authored.replace("return 20", "return 99")
        ),
    )?;
    let mut rebuild = command(&producer);
    rebuild.args(["oven", "bake", "--project", "."]);
    support::configure_explicit_oven_bake_command(&mut rebuild)?;
    let failure = rebuild.output()?;
    assert!(!failure.status.success(), "invalid native derive unexpectedly compiled");
    let diagnostic = String::from_utf8_lossy(&failure.stderr);
    assert!(
        diagnostic.contains("f64") && (diagnostic.contains("Eq") || diagnostic.contains("Hash")),
        "expected native derive failure, got {diagnostic}"
    );
    assert_eq!(artifact_snapshot(&producer.join("target/lib"))?, artifact_before);
    fs::remove_dir_all(producer.join("src"))?;
    fs::remove_file(producer.join("loaf.toml"))?;
    let retained_native = success(
        command(&consumer).args(["run", "--locked", "src/main.incn"]).output()?,
        "native consumer after failed rebuild and source removal",
    )?;
    assert_eq!(retained_native.stdout, native.stdout);
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
    let local = temporary.path().join("local");
    let local_main = source
        .lines()
        .skip(1)
        .collect::<Vec<_>>()
        .join("\n")
        .replace("Item(value=", "Pair(value=")
        .replace("State.Ready", "Mode.Ready")
        .replace("answer()", "doubled()");
    project(&local, "local", "main.incn", &format!("{authored}\n{local_main}\n"), "")?;
    let local_execution = success(
        command(&local)
            .args(["build", "src/main.incn", "--backend", "replacement"])
            .output()?,
        "equivalent local source",
    )?;
    assert_eq!(local_execution.stdout, replacement.stdout);
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

/// A real debug bake followed by a release-only tool failure must restore both native and semantic generations.
#[cfg(unix)]
#[test]
fn release_failure_after_new_debug_output_restores_both_consumer_routes() -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir()?;
    let producer = temporary.path().join("producer");
    let consumer = temporary.path().join("consumer");
    project(
        &producer,
        "arithmetic",
        "lib.incn",
        "pub def answer() -> int:\n    return 41\n",
        "",
    )?;
    let mut initial = command(&producer);
    initial
        .args(["oven", "bake", "--project", "."])
        .env("INCAN_OVEN_BAKE_PROFILES", "all");
    support::configure_explicit_oven_bake_command(&mut initial)?;
    success(initial.output()?, "initial debug and release library publication")?;
    project(
        &consumer,
        "consumer",
        "main.incn",
        "from pub::renamed import answer\n\ndef main() -> None:\n    println(answer())\n",
        "\n[dependencies]\nrenamed = { path = \"../producer\" }\n",
    )?;
    bake(&consumer)?;
    let native_before = success(
        command(&consumer).args(["run", "--locked", "src/main.incn"]).output()?,
        "native consumer before partial rebuild",
    )?;
    assert_eq!(String::from_utf8_lossy(&native_before.stdout).trim(), "41");
    let artifact_before = artifact_snapshot(&producer.join("target/lib"))?;
    let receipt_root = incan::oven::default_receipt_path(&producer)
        .parent()
        .ok_or("receipt directory absent")?
        .to_path_buf();
    let receipts_before = artifact_snapshot(&receipt_root)?;
    let debug = producer.join("target/lib/oven/debug/libarithmetic.rlib");
    let release = producer.join("target/lib/oven/release/libarithmetic.rlib");
    let old_debug = fs::read(&debug)?;
    assert!(release.is_file());
    fs::write(
        producer.join("src/lib.incn"),
        "pub def answer() -> int:\n    return 42\n",
    )?;
    let shim = temporary.path().join("release-failing-rustc");
    let proof = temporary.path().join("new-debug.rlib");
    fs::write(
        &shim,
        r#"#!/bin/sh
is_debug=0
for arg in "$@"; do
    if [ "$arg" = "$RFC123_RELEASE_OUTPUT" ]; then
        echo "RFC123 injected release failure after successful debug output" >&2
        exit 87
    fi
    if [ "$arg" = "$RFC123_DEBUG_OUTPUT" ]; then is_debug=1; fi
done
"$RFC123_REAL_RUSTC" "$@"
result=$?
if [ "$result" -eq 0 ] && [ "$is_debug" -eq 1 ]; then
    cp "$RFC123_DEBUG_OUTPUT" "$RFC123_DEBUG_PROOF" || exit 88
fi
exit "$result"
"#,
    )?;
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755))?;
    let mut rebuild = command(&producer);
    rebuild
        .args(["oven", "bake", "--project", "."])
        .env("INCAN_OVEN_BAKE_PROFILES", "all")
        .env("RUSTC", &shim)
        .env("RFC123_REAL_RUSTC", incan::oven::rustc::resolve_active_rustc()?)
        .env("RFC123_DEBUG_OUTPUT", &debug)
        .env("RFC123_RELEASE_OUTPUT", &release)
        .env("RFC123_DEBUG_PROOF", &proof);
    support::configure_explicit_oven_bake_command(&mut rebuild)?;
    let failed = rebuild.output()?;
    assert!(!failed.status.success());
    assert!(
        String::from_utf8_lossy(&failed.stderr).contains("RFC123 injected release failure"),
        "{}",
        String::from_utf8_lossy(&failed.stderr)
    );
    assert_ne!(
        fs::read(proof)?,
        old_debug,
        "the debug native artifact must actually have changed before release failed"
    );
    assert_eq!(artifact_snapshot(&producer.join("target/lib"))?, artifact_before);
    // Immutable intermediate cache entries may remain, but all pre-existing successful receipt bytes must survive.
    let receipts_after = artifact_snapshot(&receipt_root)?;
    for (path, bytes) in receipts_before {
        assert_eq!(
            receipts_after.get(&path),
            Some(&bytes),
            "receipt changed: {}",
            path.display()
        );
    }
    fs::remove_dir_all(producer.join("src"))?;
    fs::remove_file(producer.join("loaf.toml"))?;
    let native = success(
        command(&consumer).args(["run", "--locked", "src/main.incn"]).output()?,
        "native consumer after partial rebuild failure",
    )?;
    let replacement = success(
        command(&consumer)
            .args(["build", "src/main.incn", "--backend", "replacement"])
            .output()?,
        "replacement consumer after partial rebuild failure",
    )?;
    assert_eq!(native.stdout, native_before.stdout);
    assert_eq!(replacement.stdout, native_before.stdout);
    Ok(())
}

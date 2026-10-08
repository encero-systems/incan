//! Prove a facade re-export survives a real package boundary.
//!
//! The RFC 120 test surface grew a large number of re-export fixtures, and every one of them re-exported inside a
//! single project. That blind spot is why the identity-graph validator shipped rejecting six of the eight
//! declaration kinds a `pub from` can republish, and why a stdlib facade could silently drop its functions: neither
//! defect is observable from a same-project fixture, and neither is observable from a hand-assembled manifest,
//! because a hand-built manifest never runs the producing path.
//!
//! This exercises the combination that was missing: a producer that declares nothing at its root and publishes
//! everything through a facade, consumed across a `[dependencies]` path package through `pub::`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use incan::library_manifest::{ExportIdentityKind, ExportIdentityProjection, LibraryManifest};

mod support;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Resolve the compiler binary the way the sibling artifact suites do.
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

/// Build one compiler invocation carrying the harness's generated-target and provider-store settings.
fn configured_incan_command(current_dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(incan_binary());
    command
        .args(args)
        .current_dir(current_dir)
        .env("CARGO_NET_OFFLINE", "true")
        .env("INCAN_NO_BANNER", "1");
    if !support::oven_compiler_suite_is_active() {
        command
            .env(
                "INCAN_GENERATED_CARGO_TARGET_DIR",
                support::generated_cargo_target_dir(),
            )
            .env("INCAN_INTERNAL_SDK_PROVIDER_STORE", support::sdk_provider_store());
    }
    command
}

/// Run a normal command with any scheduler-granted baker capability removed.
fn run_incan(current_dir: &Path, args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
    let mut command = configured_incan_command(current_dir, args);
    command.env_remove("CARGO");
    Ok(command.output()?)
}

/// Publish the producer closure through Oven's explicit project-bake boundary.
fn run_explicit_oven_bake(current_dir: &Path) -> Result<Output, Box<dyn std::error::Error>> {
    Ok(configured_incan_command(current_dir, &["oven", "bake", "--project", "."]).output()?)
}

/// Fail with the command's own output, which carries the diagnostic worth reading.
fn assert_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Write one fixture file, creating its parent directories.
fn write_fixture_file(root: &Path, relative_path: &str, contents: &str) -> TestResult {
    let path = root.join(relative_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}

/// A producer that declares nothing at its root and republishes an inner module through a facade.
fn write_producer(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let producer = root.join("facade_lib");
    write_fixture_file(
        &producer,
        "incan.toml",
        include_str!("fixtures/package_boundary_facade/producer/incan.toml"),
    )?;
    write_fixture_file(
        &producer,
        "src/inner.incn",
        include_str!("fixtures/package_boundary_facade/producer/src/inner.incn"),
    )?;
    write_fixture_file(
        &producer,
        "src/lib.incn",
        include_str!("fixtures/package_boundary_facade/producer/src/lib.incn"),
    )?;
    Ok(producer)
}

/// A consumer that reaches the producer's facade across a `[dependencies]` path package.
fn write_consumer(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let consumer = root.join("consumer");
    write_fixture_file(
        &consumer,
        "incan.toml",
        "[project]\nname = \"consumer\"\nversion = \"0.1.0\"\n\n[dependencies]\nfacade = { path = \"../facade_lib\" }\n",
    )?;
    write_fixture_file(
        &consumer,
        "src/main.incn",
        include_str!("fixtures/package_boundary_facade/consumer/src/main.incn"),
    )?;
    Ok(consumer)
}

/// A facade-only producer publishes every re-exported kind, and a consumer resolves them across the boundary.
///
/// The producer's root declares nothing; each export reaches the manifest as a `Reexport` projection carrying the
/// target's real kind rather than a kind of its own. Asserting the kinds here rather than counting entries is what
/// makes the test fail loudly if the projection ever starts flattening a re-export into an alias.
#[test]
fn a_facade_only_package_publishes_every_reexported_kind_across_a_dependency() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let producer = write_producer(tmp.path())?;
    assert_success(&run_explicit_oven_bake(&producer)?, "explicit producer Oven bake");

    let manifest = LibraryManifest::read_from_path(&producer.join("target/lib/facade_core.incnlib"))?;
    let reexported: Vec<(String, ExportIdentityKind)> = manifest
        .contract_metadata
        .identity_graph
        .exports
        .iter()
        .filter(|entry| matches!(entry.projection, ExportIdentityProjection::Reexport { .. }))
        .map(|entry| (entry.public_name.clone(), entry.kind))
        .collect();

    for expected in [
        ("build", ExportIdentityKind::Function),
        ("Item", ExportIdentityKind::Model),
        ("Holder", ExportIdentityKind::Class),
        ("Describable", ExportIdentityKind::Trait),
        ("Mode", ExportIdentityKind::Enum),
        ("Name", ExportIdentityKind::Newtype),
        ("Count", ExportIdentityKind::TypeAlias),
        ("LIMIT", ExportIdentityKind::Const),
    ] {
        assert!(
            reexported
                .iter()
                .any(|(name, kind)| name == expected.0 && *kind == expected.1),
            "facade must republish `{}` as {:?}, got: {reexported:?}",
            expected.0,
            expected.1
        );
    }

    Ok(())
}

/// A consumer resolves a facade-published declaration across a `[dependencies]` path package.
///
/// Split from the producer assertions deliberately. The producer half proves the manifest a facade-only package
/// publishes, and runs anywhere. This half needs the surrounding harness to have imported the provider closure, the
/// same requirement the sibling artifact suites carry, so it is the half that only proves out under the Oven suite.
#[test]
fn a_consumer_resolves_facade_published_declarations_across_a_dependency() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let producer = write_producer(tmp.path())?;
    assert_success(&run_explicit_oven_bake(&producer)?, "explicit producer Oven bake");

    let consumer = write_consumer(tmp.path())?;
    let main_path = consumer.join("src/main.incn");
    let main_arg = main_path.to_str().ok_or("consumer source path was not valid UTF-8")?;

    let run_output = run_incan(&consumer, &["run", main_arg])?;
    assert_success(&run_output, "consumer run across the package boundary");
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    assert!(
        stdout.contains("boundary") && stdout.contains('2'),
        "the consumer must execute the facade's model and function across the boundary, got:\n{stdout}"
    );
    Ok(())
}

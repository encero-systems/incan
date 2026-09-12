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
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use incan::library_manifest::{ExportIdentityKind, ExportIdentityProjection, LibraryManifest};
use sha2::{Digest, Sha256};

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
    let mut command = configured_incan_command(current_dir, &["oven", "bake", "--project", "."]);
    support::configure_explicit_oven_bake_command(&mut command)?;
    Ok(command.output()?)
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
        "loaf.toml",
        include_str!("fixtures/package_boundary_facade/producer/loaf.toml"),
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
        "loaf.toml",
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
        stdout.contains("boundary") && stdout.contains('2') && stdout.contains("fast"),
        "the consumer must execute the facade's model, function and enum across the boundary, got:\n{stdout}"
    );
    Ok(())
}

/// Hash every published file and retain empty directories, including store bookkeeping and lock files.
fn artifact_inventory(
    root: &Path,
) -> Result<std::collections::BTreeMap<PathBuf, Option<String>>, Box<dyn std::error::Error>> {
    let mut inventory = std::collections::BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    let mut buffer = [0_u8; 64 * 1024];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let relative = path.strip_prefix(root)?.to_path_buf();
            if entry.file_type()?.is_dir() {
                inventory.insert(relative, None);
                pending.push(path);
            } else {
                assert!(entry.file_type()?.is_file(), "unexpected package entry: {path:?}");
                let mut file = fs::File::open(path)?;
                let mut digest = Sha256::new();
                loop {
                    let count = file.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    digest.update(&buffer[..count]);
                }
                inventory.insert(relative, Some(format!("{:x}", digest.finalize())));
            }
        }
    }
    Ok(inventory)
}

#[test]
fn source_free_native_diamond_preserves_published_store_inventory_issue1458() -> TestResult {
    let fixture = tempfile::tempdir()?;
    let catalog = fixture.path().join("catalog");
    let pricing = fixture.path().join("pricing");
    let home = fixture.path().join("incan-home");
    write_fixture_file(
        &catalog,
        "loaf.toml",
        "[project]\nname = \"immutable_catalog\"\nversion = \"0.1.0\"\n\n[rust-dependencies]\nuuid = { version = \"=1.18.1\", features = [\"v4\"] }\n",
    )?;
    // The named Rust dependency forces a portable project entry, rather than an empty package store whose entire
    // native closure happens to be supplied by the compiler's release envelope — but only while the emitted Rust
    // actually reaches it. A dependency nothing in the generated crate calls is not in the closure, so the bake
    // resolves the plain stdlib Loaf and packages no entry at all, and this regression's precondition quietly
    // stops holding. `answer` therefore calls into the crate rather than merely naming it.
    write_fixture_file(
        &catalog,
        "src/lib.incn",
        "from rust::uuid import Uuid\n\npub def answer() -> int:\n    return 42\n\npub def fresh_id() -> str:\n    return Uuid.new_v4().to_string()\n",
    )?;
    write_fixture_file(
        &pricing,
        "loaf.toml",
        "[project]\nname = \"immutable_pricing\"\nversion = \"0.1.0\"\n\n[dependencies]\ncatalog = { path = \"../catalog\" }\n",
    )?;
    write_fixture_file(
        &pricing,
        "src/lib.incn",
        "from pub::catalog import answer\n\npub def quote() -> int:\n    return answer()\n",
    )?;
    let bake = |project: &Path| -> Result<Output, Box<dyn std::error::Error>> {
        let mut command = configured_incan_command(project, &["oven", "bake", "--project", "."]);
        support::configure_explicit_oven_bake_command(&mut command)?;
        command.env("INCAN_HOME", &home);
        Ok(command.output()?)
    };
    assert_success(&bake(&catalog)?, "catalog publication with a packaged Rust dependency");
    let catalog_artifact = catalog.join("target/lib");
    let catalog_before = artifact_inventory(&catalog_artifact)?;
    assert!(
        catalog_before
            .keys()
            .any(|path| path.starts_with("oven/loafs/entries")
                && path.file_name().is_some_and(|name| name == "loaf.json")),
        "the regression requires actual packaged store entries"
    );
    assert_success(&bake(&pricing)?, "pricing publication through catalog");
    assert_eq!(artifact_inventory(&catalog_artifact)?, catalog_before);
    let pricing_artifact = pricing.join("target/lib");
    let pricing_before = artifact_inventory(&pricing_artifact)?;
    for project in [&catalog, &pricing] {
        fs::remove_dir_all(project.join("src"))?;
        fs::remove_file(project.join("loaf.toml"))?;
    }
    for name in ["first", "second"] {
        let consumer = fixture.path().join(name);
        write_fixture_file(
            &consumer,
            "loaf.toml",
            &format!(
                "[project]\nname = \"{name}\"\nversion = \"0.1.0\"\n\n[dependencies]\nstock = {{ path = \"../catalog\" }}\npricing = {{ path = \"../pricing\" }}\n"
            ),
        )?;
        write_fixture_file(
            &consumer,
            "src/main.incn",
            "from pub::stock import answer\nfrom pub::pricing import quote\n\ndef main() -> None:\n    println(answer() + quote())\n",
        )?;
        assert_success(
            &bake(&consumer)?,
            "fresh native diamond publication without provider sources",
        );
        let mut command = configured_incan_command(&consumer, &["run", "--locked", "src/main.incn"]);
        command.env_remove("CARGO").env("INCAN_HOME", &home);
        let output = command.output()?;
        assert_success(&output, "source-free native diamond execution");
        assert_eq!(String::from_utf8(output.stdout)?.trim(), "84");
        assert_eq!(artifact_inventory(&catalog_artifact)?, catalog_before);
        assert_eq!(artifact_inventory(&pricing_artifact)?, pricing_before);
    }
    Ok(())
}

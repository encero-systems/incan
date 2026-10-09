//! Source-current Loaf reuse follows the SDK publisher's Rust input boundary and named path closure.

use incan_driver::build::ProjectSourceAuthorityDigester;
use incan_driver::build::output_paths::packaged_library_metadata_files;
use incan_driver::build::package_loafs::write_packaged_library_loaf_manifest;
use incan_driver::build::source_authority::digest_baked_project_source_authority;
use incan_driver::build::{
    OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION, OvenPackagedLibraryLoafManifest, OvenPackagedLibraryLoafProfile,
};
use incan_frontend::library_manifest::LibraryManifest;
use incan_frontend::library_manifest::{
    digest_provider_artifact, digest_provider_semantic_artifact_with_context_and_cache,
};
use incan_lang::version::INCAN_VERSION;
use oven_store::{OvenGeneratedProjectRequest, digest_bytes, receipt_generated_project};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Native compiler inputs reuse observed hashes but invalidate same-size, preserved-mtime edits and replacements.
#[cfg(unix)]
#[test]
fn native_compiler_input_observations_detect_edits_and_replacements() -> TestResult {
    const CHILD: &str = "INCAN_TEST_NATIVE_COMPILER_INPUT_CHILD";
    const CASE: &str = "native_compiler_input_observations_detect_edits_and_replacements";
    if let Some(path) = std::env::var_os(CHILD) {
        println!(
            "native compiler input digest: {}",
            incan_driver::build::native_runtime_inputs::digest_native_compiler_input(Path::new(&path))?
        );
        return Ok(());
    }
    let temporary = tempfile::tempdir()?;
    let input = fs::canonicalize(temporary.path())?.join("compiler-library");
    let original = b"native-input-v1";
    let changed = b"native-input-v2";
    fs::write(&input, original)?;
    let home = temporary.path().join("home");
    let child = std::env::current_exe()?;
    let probe = || -> Result<(String, Vec<u64>), Box<dyn std::error::Error>> {
        let output = std::process::Command::new(&child)
            .args(["--exact", CASE, "--nocapture"])
            .env(CHILD, &input)
            .env("INCAN_HOME", &home)
            .env("INCAN_OVEN_TRACE_FILE_DIGESTS", "1")
            .output()?;
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let stdout = String::from_utf8(output.stdout)?;
        let digest = stdout
            .lines()
            .find_map(|line| line.strip_prefix("native compiler input digest: "))
            .ok_or("child omitted its compiler input digest")?
            .to_string();
        let reads = String::from_utf8_lossy(&output.stderr)
            .lines()
            .filter_map(|line| line.strip_prefix("Oven file digest: "))
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|record| record["path"].as_str() == input.to_str())
            .map(|record| record["input_bytes_read"].as_u64().ok_or("missing byte observation"))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((digest, reads))
    };
    let length = u64::try_from(original.len())?;
    assert_eq!(probe()?, (digest_bytes(original), vec![length]));
    assert_eq!(probe()?, (digest_bytes(original), vec![0]));
    let modified = fs::metadata(&input)?.modified()?;
    fs::write(&input, changed)?;
    fs::File::options()
        .write(true)
        .open(&input)?
        .set_times(fs::FileTimes::new().set_modified(modified))?;
    assert_eq!(probe()?, (digest_bytes(changed), vec![length]));
    assert_eq!(probe()?, (digest_bytes(changed), vec![0]));
    fs::rename(&input, temporary.path().join("old-library"))?;
    fs::write(&input, original)?;
    fs::File::options()
        .write(true)
        .open(&input)?
        .set_times(fs::FileTimes::new().set_modified(modified))?;
    assert_eq!(probe()?, (digest_bytes(original), vec![length]));
    assert_eq!(probe()?, (digest_bytes(original), vec![0]));
    for entry in fs::read_dir(home.join("cache/file-digests-v1"))? {
        fs::write(entry?.path(), "invalid observation")?;
    }
    assert_eq!(probe()?, (digest_bytes(original), vec![length]));
    eprintln!(
        "native compiler input observations: cold={length}, warm=0, preserved-mtime edit={length}, replacement={length}, malformed cache={length}"
    );
    Ok(())
}

/// Retained compiler validation reads no unchanged executable bytes and rejects preserved-mtime corruption.
#[test]
fn native_runtime_compiler_binding_reuses_observed_digest_and_refuses_tampering() -> TestResult {
    const CHILD: &str = "INCAN_TEST_NATIVE_RUNTIME_BINDING_CHILD";
    const CASE: &str = "native_runtime_compiler_binding_reuses_observed_digest_and_refuses_tampering";
    if std::env::var_os(CHILD).is_some() {
        let compiler =
            std::path::PathBuf::from(std::env::var_os("CARGO_BIN_EXE_incan").ok_or("selected compiler is missing")?);
        let source_receipt: oven_store::OvenReceipt =
            serde_json::from_slice(&fs::read(format!("{}.receipt.json", compiler.display()))?)?;
        let store = oven_store::store::OvenStore::new(
            compiler.parent().ok_or("compiler has no parent")?.join("store"),
            oven_store::store::OvenStoreLimits::new(
                oven_store::DEFAULT_OVEN_MAX_PHYSICAL_BYTES,
                oven_store::DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES,
                oven_store::DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES,
            ),
        );
        incan_driver::build::native_sdk::select_prepared_native_sdk_plan(&store, &source_receipt, &[])?;
        return Ok(());
    }
    let selected =
        std::path::PathBuf::from(std::env::var_os("CARGO_BIN_EXE_incan").ok_or("selected compiler is missing")?);
    let temporary = tempfile::tempdir()?;
    let compiler = fs::canonicalize(temporary.path())?.join("incan");
    fs::copy(&selected, &compiler)?;
    for suffix in [".receipt.json", ".oven-output.json"] {
        fs::copy(
            format!("{}{suffix}", selected.display()),
            format!("{}{suffix}", compiler.display()),
        )?;
    }
    fs::copy(
        selected
            .parent()
            .ok_or("selected compiler has no parent")?
            .join("native-runtime-inputs"),
        compiler
            .parent()
            .ok_or("compiler has no parent")?
            .join("native-runtime-inputs"),
    )?;
    let cache = temporary.path().join("home");
    let child = std::env::current_exe()?;
    let probe = || {
        std::process::Command::new(&child)
            .args(["--exact", CASE, "--nocapture"])
            .env(CHILD, "1")
            .env("CARGO_BIN_EXE_incan", &compiler)
            .env("INCAN_HOME", &cache)
            .env("INCAN_OVEN_TRACE_FILE_DIGESTS", "1")
            .output()
    };
    let input_reads_for =
        |output: &std::process::Output, path: &Path| -> Result<Vec<u64>, Box<dyn std::error::Error>> {
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .filter_map(|line| line.strip_prefix("Oven file digest: "))
                .map(serde_json::from_str::<serde_json::Value>)
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .filter(|observation| observation["path"].as_str() == path.to_str())
                .map(|observation| {
                    Ok(observation["input_bytes_read"]
                        .as_u64()
                        .ok_or("missing input byte observation")?)
                })
                .collect()
        };
    let input_reads = |output: &std::process::Output| input_reads_for(output, &compiler);
    let engine = compiler
        .parent()
        .ok_or("compiler has no parent")?
        .join("native-runtime-inputs");
    let engine_length = fs::metadata(&engine)?.len();
    let length = fs::metadata(&compiler)?.len();
    let cold = probe()?;
    assert!(cold.status.success(), "{}", String::from_utf8_lossy(&cold.stderr));
    assert_eq!(input_reads(&cold)?, vec![length]);
    assert_eq!(input_reads_for(&cold, &engine)?, vec![engine_length]);
    let warm = probe()?;
    assert!(warm.status.success(), "{}", String::from_utf8_lossy(&warm.stderr));
    assert_eq!(input_reads(&warm)?, vec![0]);
    assert_eq!(input_reads_for(&warm, &engine)?, vec![0]);

    use std::io::{Read, Seek, SeekFrom, Write};
    let modified = fs::metadata(&compiler)?.modified()?;
    let mut file = fs::OpenOptions::new().read(true).write(true).open(&compiler)?;
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0];
    file.read_exact(&mut last)?;
    file.seek(SeekFrom::End(-1))?;
    file.write_all(&[last[0] ^ 1])?;
    file.set_times(fs::FileTimes::new().set_modified(modified))?;
    drop(file);
    let changed = probe()?;
    assert!(!changed.status.success());
    assert_eq!(input_reads(&changed)?, vec![length]);
    assert!(String::from_utf8_lossy(&changed.stderr).contains("compiler output is not bound to its source receipt"));

    fs::rename(&compiler, temporary.path().join("changed-compiler"))?;
    fs::copy(&selected, &compiler)?;
    fs::File::options()
        .write(true)
        .open(&compiler)?
        .set_times(fs::FileTimes::new().set_modified(modified))?;
    let restored = probe()?;
    assert!(
        restored.status.success(),
        "{}",
        String::from_utf8_lossy(&restored.stderr)
    );
    assert_eq!(input_reads(&restored)?, vec![length]);
    let restored_warm = probe()?;
    assert!(restored_warm.status.success());
    assert_eq!(input_reads(&restored_warm)?, vec![0]);
    for record in fs::read_dir(cache.join("cache/file-digests-v1"))? {
        fs::write(record?.path(), "malformed acceleration record")?;
    }
    let malformed_cache = probe()?;
    assert!(malformed_cache.status.success());
    assert_eq!(input_reads(&malformed_cache)?, vec![length]);
    assert_eq!(input_reads_for(&malformed_cache, &engine)?, vec![engine_length]);
    let engine_modified = fs::metadata(&engine)?.modified()?;
    let engine_bytes = fs::read(&engine)?;
    let mut changed_engine = engine_bytes.clone();
    let last = changed_engine.last_mut().ok_or("engine is empty")?;
    *last ^= 1;
    fs::write(&engine, changed_engine)?;
    fs::File::options()
        .write(true)
        .open(&engine)?
        .set_times(fs::FileTimes::new().set_modified(engine_modified))?;
    let changed_engine = probe()?;
    assert!(!changed_engine.status.success());
    assert_eq!(input_reads(&changed_engine)?, vec![0]);
    assert_eq!(input_reads_for(&changed_engine, &engine)?, vec![engine_length]);
    assert!(
        String::from_utf8_lossy(&changed_engine.stderr)
            .contains("identity engine differs from the compiler source receipt")
    );
    fs::write(&engine, engine_bytes)?;
    let restored_engine = probe()?;
    assert!(restored_engine.status.success());
    assert_eq!(input_reads(&restored_engine)?, vec![0]);
    assert_eq!(input_reads_for(&restored_engine, &engine)?, vec![engine_length]);
    let warm_engine = probe()?;
    assert!(warm_engine.status.success());
    assert_eq!(input_reads_for(&warm_engine, &engine)?, vec![0]);
    eprintln!(
        "engine identity input reads: cold={engine_length}, unchanged=0, preserved-mtime corruption refused, restored={engine_length}, restored unchanged=0"
    );
    eprintln!(
        "compiler identity input reads: cold={length}, unchanged=0, preserved-mtime corruption={length}, restored={length}, restored unchanged=0, malformed cache={length}"
    );
    Ok(())
}

#[test]
fn provider_semantics_ignore_root_lock_bookkeeping_but_bind_authored_and_nested_inputs() -> TestResult {
    let artifact = tempfile::tempdir()?;
    let root = artifact.path();
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join("nested_dependency"))?;
    let cargo_manifest = "[package]\nname = \"root_lib\"\nversion = \"0.1.0\"\n";
    fs::write(root.join("Cargo.toml"), cargo_manifest)?;
    fs::write(root.join("src/lib.rs"), "pub fn value() -> i32 { 1 }\n")?;
    let manifest = LibraryManifest::new("root_lib", "0.1.0");
    let manifest_path = root.join("root_lib.incnlib");
    manifest.write_to_path(&manifest_path)?;
    let semantic_digest = || {
        digest_provider_semantic_artifact_with_context_and_cache(
            root,
            &manifest_path,
            &root.join("Cargo.toml"),
            &manifest,
            &BTreeMap::new(),
            &[],
            &mut BTreeMap::new(),
        )
    };
    let initial_semantic = semantic_digest()?;
    let initial_physical = digest_provider_artifact(root)?;
    let marker = root.join(".incan-cargo-lock-manifest");
    fs::write(&marker, "sha256:previous-projection\n")?;
    let first_physical = digest_provider_artifact(root)?;
    assert_ne!(initial_physical, first_physical);
    assert_eq!(initial_semantic, semantic_digest()?);
    fs::write(&marker, "sha256:current-projection\n")?;
    assert_ne!(first_physical, digest_provider_artifact(root)?);
    assert_eq!(initial_semantic, semantic_digest()?);
    fs::remove_file(marker)?;
    assert_eq!(initial_physical, digest_provider_artifact(root)?);
    assert_eq!(initial_semantic, semantic_digest()?);

    for (relative, original, changed) in [
        (
            "src/lib.rs",
            "pub fn value() -> i32 { 1 }\n",
            "pub fn value() -> i32 { 2 }\n",
        ),
        (
            "Cargo.toml",
            cargo_manifest,
            "[package]\nname = \"root_lib\"\nversion = \"0.2.0\"\n",
        ),
    ] {
        fs::write(root.join(relative), changed)?;
        assert_ne!(
            initial_semantic,
            semantic_digest()?,
            "authored input {relative} must remain bound"
        );
        fs::write(root.join(relative), original)?;
        assert_eq!(initial_semantic, semantic_digest()?);
    }
    fs::write(
        root.join("nested_dependency/.incan-cargo-lock-manifest"),
        "nested-witness\n",
    )?;
    assert_ne!(
        initial_semantic,
        semantic_digest()?,
        "only provider-root bookkeeping is excluded"
    );
    Ok(())
}

/// Compute through a fresh command memo so persistent freshness observations also see edits.
fn authority(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    Ok(ProjectSourceAuthorityDigester::digest_rust_path_crate_authority(
        root,
        &mut BTreeMap::new(),
    )?)
}

/// Author a conventional Rust Loaf without relying on a neighboring Cargo declaration or an explicit Rust table.
fn write_loaf(root: &Path, name: &str, extra: &str) -> TestResult {
    fs::create_dir_all(root.join("src"))?;
    fs::write(root.join("loaf.toml"), format!("[project]\nname = {name:?}\n{extra}"))?;
    fs::write(root.join("src/lib.rs"), "pub fn value() -> u8 { 1 }\n")?;
    Ok(())
}

#[test]
fn rust_loaf_reuses_across_unrelated_edits_but_tracks_source_additions_and_replacements() -> TestResult {
    let project = tempfile::tempdir()?;
    write_loaf(project.path(), "authority_control", "")?;
    let initial = authority(project.path())?;
    assert_eq!(authority(project.path())?, initial);
    fs::create_dir_all(project.path().join("tests"))?;
    fs::write(
        project.path().join("tests/new_case.rs"),
        "deliberately invalid Rust test text",
    )?;
    fs::write(project.path().join("README.md"), "unrelated documentation")?;
    fs::write(project.path().join("Cargo.toml"), "deliberately invalid Cargo metadata")?;
    fs::write(project.path().join("Cargo.lock"), "deliberately invalid Cargo lock")?;
    assert_eq!(authority(project.path())?, initial);
    fs::write(project.path().join("src/data.txt"), "new embedded bytes")?;
    assert_ne!(authority(project.path())?, initial);
    fs::remove_file(project.path().join("src/data.txt"))?;
    assert_eq!(authority(project.path())?, initial);
    let source = project.path().join("src/lib.rs");
    let modified = fs::metadata(&source)?.modified()?;
    let replacement = project.path().join("replacement.rs");
    fs::write(&replacement, "pub fn value() -> u8 { 2 }\n")?;
    fs::File::options()
        .write(true)
        .open(&replacement)?
        .set_modified(modified)?;
    fs::rename(replacement, &source)?;
    assert_ne!(authority(project.path())?, initial);
    Ok(())
}

#[test]
fn rust_loaf_binds_transitive_and_target_specific_path_alternatives() -> TestResult {
    let workspace = tempfile::tempdir()?;
    let root = workspace.path().join("root");
    let left = workspace.path().join("left");
    let right = workspace.path().join("right");
    let leaf = workspace.path().join("leaf");
    write_loaf(&leaf, "leaf", "")?;
    write_loaf(
        &left,
        "left",
        "\n[dependencies]\nleaf = { loaf = \"leaf\", path = \"../leaf\" }\n",
    )?;
    write_loaf(&right, "right", "")?;
    write_loaf(
        &root,
        "root",
        "\n[dependencies]\nplatform = [\n{ loaf = \"left\", path = \"../left\", target = \"cfg(unix)\" },\n{ loaf = \"right\", path = \"../right\", target = \"cfg(windows)\" },\n]\n",
    )?;
    let initial = authority(&root)?;
    fs::write(leaf.join("src/lib.rs"), "pub fn value() -> u8 { 2 }\n")?;
    assert_ne!(authority(&root)?, initial);
    fs::write(leaf.join("src/lib.rs"), "pub fn value() -> u8 { 1 }\n")?;
    assert_eq!(authority(&root)?, initial);
    fs::write(right.join("src/lib.rs"), "pub fn value() -> u8 { 3 }\n")?;
    assert_ne!(authority(&root)?, initial);
    fs::write(right.join("src/lib.rs"), "pub fn value() -> u8 { 1 }\n")?;
    fs::write(
        left.join("loaf.toml"),
        "[project]\nname = \"left\"\n\n[dependencies]\nleaf = { loaf = \"leaf\", path = \"../leaf\", default-features = false }\n",
    )?;
    assert_ne!(authority(&root)?, initial);
    fs::write(
        leaf.join("loaf.toml"),
        "[project]\nname = \"leaf\"\n\n[dependencies]\nroot = { loaf = \"root\", path = \"../root\" }\n",
    )?;
    assert!(
        authority(&root).is_err(),
        "a source cycle must refuse rather than reuse its previous observation"
    );
    Ok(())
}

#[test]
fn mixed_loaf_and_embedded_compiler_sources_keep_the_publishers_geometry() -> TestResult {
    let workspace = tempfile::tempdir()?;
    let mixed = workspace.path().join("mixed");
    write_loaf(&mixed, "mixed", "\n[rust.source]\nroot = \"rust\"\n")?;
    fs::create_dir_all(mixed.join("rust/src"))?;
    fs::write(mixed.join("rust/src/lib.rs"), "pub fn value() -> u8 { 1 }\n")?;
    let initial = authority(&mixed)?;
    fs::write(mixed.join("src/lib.rs"), "outside the declared Rust root")?;
    assert_eq!(authority(&mixed)?, initial);
    fs::write(mixed.join("rust/src/lib.rs"), "pub fn value() -> u8 { 2 }\n")?;
    assert_ne!(authority(&mixed)?, initial);

    let language = workspace.path().join("loaves/kernel/incan_lang");
    write_loaf(&language, "incan_lang", "")?;
    for component in [
        "async",
        "codecs",
        "compression",
        "core",
        "data",
        "interop",
        "observability",
        "system",
        "testing",
        "web",
    ] {
        let component = workspace.path().join("loaves/stdlib").join(component);
        fs::create_dir_all(&component)?;
        fs::write(component.join("loaf.toml"), "embedded declaration\n")?;
    }
    let initial = authority(&language)?;
    fs::write(
        workspace.path().join("loaves/stdlib/core/loaf.toml"),
        "changed embedded declaration\n",
    )?;
    assert_ne!(authority(&language)?, initial);
    let emitter = workspace.path().join("loaves/compiler/incan_emit");
    write_loaf(&emitter, "incan_emit", "")?;
    fs::write(
        workspace.path().join("loaves/stdlib/zen.txt"),
        "original embedded text\n",
    )?;
    let initial = authority(&emitter)?;
    fs::write(
        workspace.path().join("loaves/stdlib/zen.txt"),
        "changed embedded text\n",
    )?;
    assert_ne!(authority(&emitter)?, initial);
    Ok(())
}

#[cfg(unix)]
#[test]
fn rust_loaf_refuses_source_links_and_forbidden_build_inputs_without_cargo_fallback() -> TestResult {
    let project = tempfile::tempdir()?;
    write_loaf(project.path(), "authority_control", "")?;
    let initial = authority(project.path())?;
    let linked = project.path().join("src/linked.rs");
    std::os::unix::fs::symlink(project.path().join("src/lib.rs"), &linked)?;
    assert!(authority(project.path()).is_err());
    fs::remove_file(&linked)?;
    assert_eq!(authority(project.path())?, initial);
    fs::rename(project.path().join("src"), project.path().join("plain-src"))?;
    std::os::unix::fs::symlink(project.path().join("plain-src"), project.path().join("src"))?;
    assert!(authority(project.path()).is_err());
    fs::remove_file(project.path().join("src"))?;
    fs::rename(project.path().join("plain-src"), project.path().join("src"))?;
    for forbidden in ["build.rs", "Cargo.toml", "Cargo.lock"] {
        let path = project.path().join("src").join(forbidden);
        fs::write(&path, "forbidden source input")?;
        assert!(authority(project.path()).is_err());
        fs::remove_file(path)?;
    }
    fs::write(
        project.path().join("loaf.toml"),
        "[project]\nname = \"authority_control\"\n[rust]\nbuild-script = true\n",
    )?;
    assert!(authority(project.path()).is_err());
    Ok(())
}

#[test]
fn cargo_only_path_sources_retain_conservative_external_input_coverage() -> TestResult {
    let project = tempfile::tempdir()?;
    fs::create_dir_all(project.path().join("src"))?;
    fs::write(
        project.path().join("Cargo.toml"),
        "[package]\nname = \"legacy_control\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(
        project.path().join("src/lib.rs"),
        "pub const TEXT: &str = include_str!(\"../outside.txt\");\n",
    )?;
    fs::write(project.path().join("outside.txt"), "original external input")?;
    let initial = authority(project.path())?;
    fs::write(project.path().join("outside.txt"), "changed external input")?;
    assert_ne!(authority(project.path())?, initial);
    Ok(())
}

/// Author a checked native handoff with complete source evidence and a separate Incan consumer.
///
/// Native bytes here are an integrity fixture, not executable output; the direct compilation tests own execution proof.
/// This exercises the public source-authority path without any generated Cargo declaration.
fn write_sealed_provider_fixture(
    workspace: &Path,
) -> Result<OvenPackagedLibraryLoafManifest, Box<dyn std::error::Error>> {
    let provider = workspace.join("provider");
    let consumer = workspace.join("consumer");
    let artifact = provider.join("target/lib");
    fs::create_dir_all(provider.join("src"))?;
    fs::create_dir_all(consumer.join("src"))?;
    fs::create_dir_all(artifact.join("src"))?;
    fs::create_dir_all(artifact.join("oven/debug"))?;
    fs::write(
        provider.join("loaf.toml"),
        "[project]\nname = \"provider\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(provider.join("src/lib.incn"), "pub def value() -> int:\n    return 1\n")?;
    fs::write(
        consumer.join("loaf.toml"),
        "[project]\nname = \"consumer\"\nversion = \"0.1.0\"\n[dependencies]\nprovider = { path = \"../provider\" }\n",
    )?;
    fs::write(consumer.join("src/main.incn"), "def main() -> None:\n    pass\n")?;
    let root = artifact.join("src/lib.rs");
    fs::write(&root, "pub fn value() -> i64 { 1 }\n")?;
    fs::write(artifact.join("src/data.txt"), "sealed embedded data")?;
    let output = artifact.join("oven/debug/libprovider.rlib");
    fs::write(&output, "sealed native bytes")?;
    let library = LibraryManifest::new("provider", "0.1.0");
    let metadata = artifact.join("provider.incnlib");
    library.write_to_path(&metadata)?;
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            &artifact,
            "provider",
            "0.1.0",
            "aarch64-apple-darwin",
            "rustc integrity fixture",
            "debug",
            Vec::new(),
        )
        .with_generated_source("generated-root", &root)
        .with_generated_source_tree("generated-source-tree", artifact.join("src")),
    )?;
    let package = OvenPackagedLibraryLoafManifest {
        schema_version: OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION,
        source_authority_digest: digest_baked_project_source_authority(&provider)?,
        compiler_version: INCAN_VERSION.to_string(),
        metadata_files: packaged_library_metadata_files(&metadata, &library, &artifact)?,
        profiles: BTreeMap::from([(
            "debug".to_string(),
            OvenPackagedLibraryLoafProfile {
                receipt,
                entries: Vec::new(),
                library_relative_path: "oven/debug/libprovider.rlib".to_string(),
                library_digest: digest_bytes(&fs::read(output)?),
            },
        )]),
    };
    write_packaged_library_loaf_manifest(&artifact, &package)?;
    Ok(package)
}

#[test]
fn sealed_provider_reuse_ignores_cargo_projections_and_refuses_changed_execution_inputs() -> TestResult {
    let workspace = tempfile::tempdir()?;
    let package = write_sealed_provider_fixture(workspace.path())?;
    let provider = workspace.path().join("provider");
    let consumer = workspace.path().join("consumer");
    let artifact = provider.join("target/lib");
    let initial = digest_baked_project_source_authority(&consumer)?;

    for name in ["Cargo.toml", "Cargo.lock", ".incan-cargo-lock-manifest"] {
        fs::write(artifact.join(name), "deliberately invalid generated Cargo bookkeeping")?;
        assert_eq!(digest_baked_project_source_authority(&consumer)?, initial);
        fs::remove_file(artifact.join(name))?;
        assert_eq!(digest_baked_project_source_authority(&consumer)?, initial);
    }
    let authored = provider.join("src/lib.incn");
    let original = fs::read(&authored)?;
    fs::write(&authored, "pub def value() -> int:\n    return 2\n")?;
    assert_ne!(digest_baked_project_source_authority(&consumer)?, initial);
    fs::write(&authored, original)?;
    assert_eq!(digest_baked_project_source_authority(&consumer)?, initial);

    for relative in [
        "provider.incnlib",
        "src/lib.rs",
        "src/data.txt",
        "oven/debug/libprovider.rlib",
    ] {
        let path = artifact.join(relative);
        let original = fs::read(&path)?;
        fs::write(&path, "changed sealed execution input")?;
        assert!(
            digest_baked_project_source_authority(&consumer).is_err(),
            "accepted changed {relative}"
        );
        fs::write(&path, original)?;
        assert_eq!(digest_baked_project_source_authority(&consumer)?, initial);
    }
    let mut broken = package.clone();
    broken
        .profiles
        .get_mut("debug")
        .ok_or("missing fixture profile")?
        .receipt
        .intent
        .features
        .push("changed".to_string());
    write_packaged_library_loaf_manifest(&artifact, &broken)?;
    assert!(digest_baked_project_source_authority(&consumer).is_err());
    write_packaged_library_loaf_manifest(&artifact, &package)?;
    assert_eq!(digest_baked_project_source_authority(&consumer)?, initial);
    fs::write(artifact.join("oven/package-loafs.json"), "malformed package seal")?;
    assert!(digest_baked_project_source_authority(&consumer).is_err());
    Ok(())
}

#[test]
fn unsealed_provider_reuse_retains_conservative_artifact_authority() -> TestResult {
    let workspace = tempfile::tempdir()?;
    write_sealed_provider_fixture(workspace.path())?;
    let consumer = workspace.path().join("consumer");
    let artifact = workspace.path().join("provider/target/lib");
    fs::remove_file(artifact.join("oven/package-loafs.json"))?;
    fs::write(artifact.join("Cargo.toml"), "legacy generated metadata")?;
    fs::write(artifact.join("outside.txt"), "original external execution input")?;
    let initial = digest_baked_project_source_authority(&consumer)?;
    fs::write(artifact.join("outside.txt"), "changed external execution input")?;
    assert_ne!(digest_baked_project_source_authority(&consumer)?, initial);
    Ok(())
}

/// One edited leaf propagates identity while unchanged Rust source members contribute retained hashes.
#[cfg(unix)]
#[test]
fn rust_loaf_incremental_source_hashing_reads_only_changed_leaf_bytes() -> TestResult {
    const CHILD: &str = "INCAN_TEST_INCREMENTAL_SOURCE_CHILD";
    const CASE: &str = "rust_loaf_incremental_source_hashing_reads_only_changed_leaf_bytes";
    if let Some(project) = std::env::var_os(CHILD) {
        println!("source-authority={}", authority(Path::new(&project))?);
        return Ok(());
    }
    let temporary = tempfile::tempdir()?;
    let project = temporary.path().join("project");
    let left = project.join("left");
    let right = project.join("right");
    write_loaf(&left, "incremental_left", "")?;
    write_loaf(&right, "incremental_right", "")?;
    write_loaf(
        &project,
        "incremental_root",
        "[dependencies]\nleft = { loaf='incremental_left', path='left' }\nright = { loaf='incremental_right', path='right' }\n",
    )?;
    let probe = || {
        std::process::Command::new(std::env::current_exe()?)
            .args(["--exact", CASE, "--nocapture"])
            .env(CHILD, &project)
            .env("INCAN_HOME", temporary.path().join("home"))
            .env("INCAN_OVEN_TRACE_FILE_DIGESTS", "1")
            .output()
    };
    let observation =
        |output: &std::process::Output| -> Result<(String, BTreeMap<String, u64>), Box<dyn std::error::Error>> {
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            let stdout = String::from_utf8_lossy(&output.stdout);
            let digest = stdout
                .lines()
                .find_map(|line| line.strip_prefix("source-authority="))
                .ok_or("missing source identity")?
                .to_string();
            let mut reads = BTreeMap::new();
            for line in String::from_utf8_lossy(&output.stderr)
                .lines()
                .filter_map(|line| line.strip_prefix("Oven file digest: "))
            {
                let value: serde_json::Value = serde_json::from_str(line)?;
                let path = value["path"].as_str().ok_or("missing source path")?.to_string();
                let bytes = value["input_bytes_read"].as_u64().ok_or("missing input-byte count")?;
                *reads.entry(path).or_insert(0) += bytes;
            }
            Ok((digest, reads))
        };
    let source = fs::canonicalize(left.join("src/lib.rs"))?;
    let source_key = source.to_str().ok_or("source path is not UTF-8")?.to_string();
    let initial = observation(&probe()?)?;
    assert!(
        initial.1.values().sum::<u64>() > 0,
        "cold identity must read actual source bytes"
    );
    let warm = observation(&probe()?)?;
    assert_eq!(warm.0, initial.0);
    assert_eq!(warm.1.values().sum::<u64>(), 0);
    let original = fs::read(&source)?;
    let modified = fs::metadata(&source)?.modified()?;
    let changed = b"pub fn value() -> u8 { 2 }\n";
    fs::write(&source, changed)?;
    fs::File::options()
        .write(true)
        .open(&source)?
        .set_times(fs::FileTimes::new().set_modified(modified))?;
    let edited = observation(&probe()?)?;
    assert_ne!(edited.0, initial.0);
    assert_eq!(edited.1.get(&source_key), Some(&(changed.len() as u64)));
    assert_eq!(
        edited.1.values().sum::<u64>(),
        changed.len() as u64,
        "unchanged branch and root source bytes must not be reread"
    );
    let repeated = observation(&probe()?)?;
    assert_eq!(repeated.0, edited.0);
    assert_eq!(repeated.1.values().sum::<u64>(), 0);
    fs::write(project.join("README.md"), "unrelated documentation")?;
    let unrelated = observation(&probe()?)?;
    assert_eq!(unrelated.0, edited.0);
    assert_eq!(unrelated.1.values().sum::<u64>(), 0);
    fs::write(&source, &original)?;
    let restored = observation(&probe()?)?;
    assert_eq!(restored.0, initial.0);
    assert_eq!(restored.1.values().sum::<u64>(), original.len() as u64);
    let added = left.join("src/extra.txt");
    fs::write(&added, b"added")?;
    let addition = observation(&probe()?)?;
    assert_ne!(addition.0, initial.0);
    assert_eq!(addition.1.values().sum::<u64>(), 5);
    fs::remove_file(&added)?;
    let removal = observation(&probe()?)?;
    assert_eq!(removal.0, initial.0);
    assert_eq!(removal.1.values().sum::<u64>(), 0);
    println!(
        "incremental source bytes: cold={}, warm=0, edited={}, repeat=0, unrelated=0, restored={}, addition=5, removal=0",
        initial.1.values().sum::<u64>(),
        changed.len(),
        original.len()
    );
    Ok(())
}

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use incan::library_manifest::{LibraryManifest, TypeRef};

mod support;

#[path = "support/canonical_projection.rs"]
mod canonical_projection;

/// Generated Rust read back with RFC 120 projections decoded to the spellings the source used.
///
/// Every linker-visible Incan-origin declaration reaches generated Rust as an encoded projection, so an assertion
/// written against a source spelling can only be evaluated after decoding. Both views are retained because decoding
/// preserves comments while re-formatting restores the one-line signatures that the longer encoded names had forced
/// `prettyplease` to wrap; an assertion holds when either view contains it.
struct GeneratedSource {
    decoded: String,
    reformatted: Option<String>,
}

impl GeneratedSource {
    /// Read one generated file and decode the projections inside it.
    fn read(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let decoded = canonical_projection::decoded_source_spellings(&fs::read_to_string(path)?);
        let reformatted = canonical_projection::reformatted_after_decode(&decoded);
        Ok(Self { decoded, reformatted })
    }

    /// Report whether either decoded view contains `needle`.
    fn contains(&self, needle: &str) -> bool {
        self.decoded.contains(needle) || self.reformatted.as_deref().is_some_and(|code| code.contains(needle))
    }
}

impl std::fmt::Display for GeneratedSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.decoded)
    }
}

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

fn configured_incan_command(current_dir: &Path, args: &[&str]) -> Result<Command, Box<dyn std::error::Error>> {
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
    if let Some(path) = std::env::var_os("INCAN_TEST_CARGO_GUARD_PATH") {
        let mut guarded_paths = vec![PathBuf::from(path)];
        if let Some(inherited) = std::env::var_os("PATH") {
            guarded_paths.extend(std::env::split_paths(&inherited));
        }
        command.env("PATH", std::env::join_paths(guarded_paths)?);
        if let Some(log) = std::env::var_os("INCAN_TEST_CARGO_GUARD_LOG") {
            command.env("OVEN_CARGO_GUARD_LOG", log);
        }
    }
    Ok(command)
}

/// Run a normal command with any scheduler-granted baker capability removed.
///
/// The consumer phase remains Cargo-guarded even when this root first performs its one explicit producer bake.
fn run_incan(current_dir: &Path, args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
    let mut command = configured_incan_command(current_dir, args)?;
    command.env_remove("CARGO");
    Ok(command.output()?)
}

/// Run the one explicit producer bake that owns the package-qualified Cargo fixture capability.
fn run_explicit_oven_bake(current_dir: &Path) -> Result<Output, Box<dyn std::error::Error>> {
    Ok(configured_incan_command(current_dir, &["oven", "bake", "--project", "."])?.output()?)
}

fn assert_cargo_guard_was_not_called() -> Result<(), Box<dyn std::error::Error>> {
    let Some(log) = std::env::var_os("INCAN_TEST_CARGO_GUARD_LOG") else {
        return Ok(());
    };
    let contents = match fs::read_to_string(&log) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    assert!(
        contents.is_empty(),
        "normal Incan command launched Cargo despite the Oven guard:\n{contents}"
    );
    Ok(())
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
    let producer = root.join("callability_lib");
    write_fixture_file(
        &producer,
        "incan.toml",
        include_str!("fixtures/generated_rust_callability/producer/incan.toml"),
    )?;
    write_fixture_file(
        &producer,
        "src/transforms.incn",
        include_str!("fixtures/generated_rust_callability/producer/src/transforms.incn"),
    )?;
    write_fixture_file(
        &producer,
        "src/lib.incn",
        include_str!("fixtures/generated_rust_callability/producer/src/lib.incn"),
    )?;
    Ok(producer)
}

fn build_producer(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let producer = write_producer(root)?;
    let output = run_explicit_oven_bake(&producer)?;
    assert_success(&output, "explicit producer Oven bake");
    Ok(producer)
}

fn write_consumer(
    root: &Path,
    dir_name: &str,
    main_source: &str,
) -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error>> {
    let consumer = root.join(dir_name);
    write_fixture_file(
        &consumer,
        "incan.toml",
        "[project]\nname = \"consumer\"\nversion = \"0.1.0\"\n\n[dependencies]\ncallability = { path = \"../callability_lib\" }\n",
    )?;
    write_fixture_file(&consumer, "src/main.incn", main_source)?;
    Ok((consumer.clone(), consumer.join("src/main.incn")))
}

fn function_param_ty<'a>(
    manifest: &'a LibraryManifest,
    function_name: &str,
    param_name: &str,
) -> Result<&'a TypeRef, Box<dyn std::error::Error>> {
    let function = manifest
        .exports
        .functions
        .iter()
        .find(|function| function.name == function_name)
        .ok_or_else(|| format!("expected function export `{function_name}`"))?;
    Ok(&function
        .params
        .iter()
        .find(|param| param.name == param_name)
        .ok_or_else(|| format!("expected parameter `{param_name}` on `{function_name}`"))?
        .ty)
}

#[test]
fn generated_callable_artifact_and_consumers_share_producer_build() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let producer = build_producer(tmp.path())?;
    let artifact = producer.join("target/lib");

    let cargo_toml = fs::read_to_string(artifact.join("Cargo.toml"))?;
    assert!(
        cargo_toml.contains("name = \"callability_core\""),
        "expected generated Cargo package name, got:\n{cargo_toml}"
    );

    let lib_rs = GeneratedSource::read(&artifact.join("src/lib.rs"))?;
    assert!(
        lib_rs.contains("pub use crate::transforms::map_owned;")
            && lib_rs.contains("pub use crate::transforms::inspect_payload;"),
        "expected callable exports re-exported from generated package root, got:\n{lib_rs}"
    );

    let transforms_rs = GeneratedSource::read(&artifact.join("src/transforms.rs"))?;
    assert!(
        transforms_rs.contains("pub fn map_owned(items: Vec<i64>, f: fn(i64) -> i64) -> Vec<i64>"),
        "expected owned scalar callable to lower to an exported fn pointer parameter, got:\n{transforms_rs}"
    );
    assert!(
        transforms_rs.contains("out.push(f(item));"),
        "expected owned scalar callable invocation in generated package artifact, got:\n{transforms_rs}"
    );
    assert!(
        transforms_rs.contains("f: fn(&Payload) -> ()") && transforms_rs.contains("f(&value);"),
        "expected non-Copy payload observer to lower as borrowed callable in generated package artifact, got:\n{transforms_rs}"
    );

    let manifest = LibraryManifest::read_from_path(&artifact.join("callability_core.incnlib"))?;
    assert!(matches!(
        function_param_ty(&manifest, "map_owned", "f")?,
        TypeRef::Function { params, return_type }
            if matches!(params.as_slice(), [TypeRef::Named { name }] if name == "int")
                && matches!(&**return_type, TypeRef::Named { name } if name == "int")
    ));
    assert!(matches!(
        function_param_ty(&manifest, "inspect_payload", "f")?,
        TypeRef::Function { params, return_type }
            if matches!(params.as_slice(), [TypeRef::Named { name }] if name == "Payload")
                && matches!(&**return_type, TypeRef::Named { name } if name == "Unit")
    ));

    let (owned_consumer, owned_main_path) = write_consumer(
        tmp.path(),
        "owned_consumer",
        include_str!("fixtures/generated_rust_callability/consumer_owned/src/main.incn"),
    )?;

    let out_dir = owned_consumer.join("target/incan/consumer");
    let run_output = run_incan(
        &owned_consumer,
        &["run", owned_main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&run_output, "consumer incan run for owned callable import");
    assert_eq!(String::from_utf8_lossy(&run_output.stdout).trim(), "2\n3\n4");

    let generated_toml = fs::read_to_string(out_dir.join("Cargo.toml"))?;
    assert!(
        generated_toml.contains("[dependencies.callability]")
            && generated_toml.contains("package = \"callability_core\"")
            && generated_toml.contains("callability_lib/target/lib"),
        "expected consumer generated Cargo.toml to depend on producer target/lib, got:\n{generated_toml}"
    );
    let generated_main = GeneratedSource::read(&out_dir.join("src/main.rs"))?;
    assert!(
        !generated_main.contains("use callability::map_owned;")
            && generated_main.contains("use callability::plus_one;")
            && generated_main.contains("callability::map_owned(vec![1, 2, 3], plus_one)"),
        "expected final generated Rust project to qualify the provider-owned call while importing the callable value, got:\n{generated_main}"
    );
    assert_cargo_guard_was_not_called()?;

    Ok(())
}

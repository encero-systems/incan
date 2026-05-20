use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use incan::library_manifest::{LibraryManifest, TypeRef};

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

fn assert_failure(output: &Output, context: &str) {
    assert!(
        !output.status.success(),
        "{context} unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn strip_ansi_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            let _ = chars.next();
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
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
    let output = run_incan(&producer, &["build", "--lib"])?;
    assert_success(&output, "producer incan build --lib");
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
fn build_lib_emits_package_facing_callable_artifact_layout() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let producer = build_producer(tmp.path())?;
    let artifact = producer.join("target/lib");

    let cargo_toml = fs::read_to_string(artifact.join("Cargo.toml"))?;
    assert!(
        cargo_toml.contains("name = \"callability_core\""),
        "expected generated Cargo package name, got:\n{cargo_toml}"
    );

    let lib_rs = fs::read_to_string(artifact.join("src/lib.rs"))?;
    assert!(
        lib_rs.contains("pub use crate::transforms::map_owned;")
            && lib_rs.contains("pub use crate::transforms::inspect_payload;"),
        "expected callable exports re-exported from generated package root, got:\n{lib_rs}"
    );

    let transforms_rs = fs::read_to_string(artifact.join("src/transforms.rs"))?;
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
    Ok(())
}

#[test]
fn consumer_can_call_owned_callable_export_across_generated_package_boundary() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    build_producer(tmp.path())?;
    let (consumer, main_path) = write_consumer(
        tmp.path(),
        "owned_consumer",
        include_str!("fixtures/generated_rust_callability/consumer_owned/src/main.incn"),
    )?;

    let out_dir = consumer.join("out");
    let build_output = run_incan(
        &consumer,
        &[
            "build",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
            out_dir.to_str().ok_or("out path was not valid UTF-8")?,
        ],
    )?;
    assert_success(&build_output, "consumer incan build for owned callable import");

    let generated_toml = fs::read_to_string(out_dir.join("Cargo.toml"))?;
    assert!(
        generated_toml.contains("[dependencies.callability]")
            && generated_toml.contains("package = \"callability_core\"")
            && generated_toml.contains("callability_lib/target/lib"),
        "expected consumer generated Cargo.toml to depend on producer target/lib, got:\n{generated_toml}"
    );
    let generated_main = fs::read_to_string(out_dir.join("src/main.rs"))?;
    assert!(
        generated_main.contains("use callability::map_owned;")
            && generated_main.contains("use callability::plus_one;")
            && generated_main.contains("map_owned(vec![1, 2, 3], plus_one)"),
        "expected final generated Rust project to call imported callable export, got:\n{generated_main}"
    );

    let run_output = run_incan(
        &consumer,
        &["run", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&run_output, "consumer incan run for owned callable import");
    assert_eq!(String::from_utf8_lossy(&run_output.stdout).trim(), "2\n3\n4");
    Ok(())
}

#[test]
fn borrowed_callable_export_is_characterized_as_current_pub_consumer_blocker() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    build_producer(tmp.path())?;
    let (consumer, main_path) = write_consumer(
        tmp.path(),
        "borrowed_consumer",
        include_str!("fixtures/generated_rust_callability/consumer_borrowed_blocker/src/main.incn"),
    )?;

    let out_dir = consumer.join("out");
    let build_output = run_incan(
        &consumer,
        &[
            "build",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
            out_dir.to_str().ok_or("out path was not valid UTF-8")?,
        ],
    )?;
    assert_failure(&build_output, "consumer incan build for borrowed callable import");

    let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&build_output.stderr));
    assert!(
        stderr.contains("expected fn pointer") && stderr.contains("found fn item") && stderr.contains("observe"),
        "expected borrowed callable mismatch to document current pub consumer blocker, got:\n{stderr}"
    );
    let generated_main = fs::read_to_string(out_dir.join("src/main.rs"))?;
    assert!(
        generated_main.contains("fn observe(_: Payload)")
            && generated_main.contains("inspect_payload(")
            && generated_main.contains(", observe)"),
        "expected final generated Rust project to show consumer observer shape before Cargo type failure, got:\n{generated_main}"
    );
    Ok(())
}

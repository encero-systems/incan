use crate::support::repo_root;

use super::{
    compiled_sdk_provider_artifact_root, incan_command, strip_ansi_escapes, support, unique_test_project_name,
};
use incan_driver::backend::IrCodegen;
use incan_frontend::{lexer, parser, typechecker};
use incan_semantics_core::{SemanticSourceTargetKind, decode_incan_symbol_identity, encode_incan_symbol_identity};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn run_incan_source(source: &str) -> std::process::Output {
    incan_command()
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()
        .unwrap_or_else(|e| panic!("failed to run incan source: {e}"))
}

/// Build an Incan source fixture and return its compiler-reported executable artifact.
fn build_incan_source_binary(source: &Path, output_dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let source_arg = source.to_str().ok_or("source path was not valid UTF-8")?;
    let output_arg = output_dir.to_str().ok_or("output path was not valid UTF-8")?;
    let output = incan_command()
        .args(["build", source_arg, output_arg, "--report", "json"])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "Incan build failed for {}. stdout:\n{}\nstderr:\n{}",
            source.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        )
        .into());
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let binary = report["artifacts"]
        .as_array()
        .and_then(|artifacts| artifacts.iter().find(|artifact| artifact["kind"] == "binary"))
        .and_then(|artifact| artifact["path"].as_str())
        .ok_or("build report did not contain a binary artifact")?;
    let binary = PathBuf::from(binary);
    if !binary.is_file() {
        return Err(format!("reported binary does not exist: {}", binary.display()).into());
    }
    Ok(binary)
}

fn rustc_compile_ok(source: &str) -> Result<(), String> {
    let mut dir = std::env::temp_dir();
    let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        panic!("system time before UNIX epoch");
    };
    let uniq = duration.as_nanos();
    dir.push(format!("incan_bench_smoke_{}", uniq));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let rs_path = dir.join("main.rs");
    let bin_path = dir.join("bin");
    std::fs::write(&rs_path, source).map_err(|e| e.to_string())?;

    let out = Command::new("rustc")
        .arg("--edition=2021")
        .arg(&rs_path)
        .arg("-o")
        .arg(&bin_path)
        .output()
        .map_err(|e| e.to_string())?;

    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).to_string())
    }
}

fn make_temp_dir(prefix: &str) -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        panic!("system time before UNIX epoch");
    };
    let uniq = duration.as_nanos();
    dir.push(format!("{}_{}", prefix, uniq));
    let Ok(()) = std::fs::create_dir_all(&dir) else {
        panic!("failed to create temp dir");
    };
    dir
}

/// Compile and run one standalone local-partial program through the normal Oven-backed generated-Rust path.
fn compile_and_run_local_partial_project(
    source: &str,
    project_stem: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_name = unique_test_project_name(project_stem);
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        format!("[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n"),
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(&main_path, source)?;

    let oven_home = tmp.path().join("incan-home");
    let mut bake_command = incan_command();
    bake_command
        .args(["oven", "bake", "--project", "."])
        .current_dir(tmp.path())
        .env("CARGO_NET_OFFLINE", "true")
        .env("INCAN_HOME", &oven_home);
    support::configure_explicit_oven_bake_command(&mut bake_command)?;
    let bake_output = bake_command.output()?;
    assert!(
        bake_output.status.success(),
        "expected explicit Oven bake to prepare local-partial regression.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&bake_output.stdout),
        String::from_utf8_lossy(&bake_output.stderr)
    );

    let out_dir = tmp.path().join("out");
    let build_output = incan_command()
        .args([
            "build",
            main_path.to_string_lossy().as_ref(),
            out_dir.to_string_lossy().as_ref(),
        ])
        .current_dir(tmp.path())
        .env("CARGO_NET_OFFLINE", "true")
        .env("INCAN_HOME", &oven_home)
        .output()?;
    assert!(
        build_output.status.success(),
        "local partial generated-Rust build regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
        build_output.status,
        String::from_utf8_lossy(&build_output.stdout),
        String::from_utf8_lossy(&build_output.stderr)
    );

    let binary = out_dir.join("oven/release").join(&project_name);
    assert!(
        binary.is_file(),
        "expected generated Rust executable at {}",
        binary.display()
    );
    let run_output = Command::new(&binary).output()?;
    assert!(
        run_output.status.success(),
        "local partial generated Rust executable failed.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );
    Ok(strip_ansi_escapes(&String::from_utf8_lossy(&run_output.stdout)))
}

/// Exercise `std.regex` through the immutable SDK inventory injected for the Oven suite.
///
/// The provider inventory is the normal-command authority here. A former “fresh Cargo home” lane re-published
/// the provider while testing the consumer, which both weakened this boundary and contradicted Oven Alpha's
/// no-Cargo normal-command contract.
fn assert_std_regex_surface_from_sealed_sdk_inventory() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let source = tmp.path().join("std_regex_surface.incn");
    fs::copy(incan_test_support::fixture("valid/std_regex_surface.incn"), &source)?;
    let generated_project = tmp.path().join("target/incan/std_regex_surface");
    let oven_home = tmp.path().join("oven-home");
    let inventory = std::env::var_os("INCAN_SDK_INVENTORY")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .ok_or("std.regex Oven regression requires the sealed SDK inventory from test prewarm")?;

    let mut command = incan_command();
    command
        .current_dir(tmp.path())
        .env("INCAN_SOURCE_ROOT", repo_root())
        .env("INCAN_STDLIB", repo_root().join("loaves/stdlib"))
        .env_remove("INCAN_STDLIB_DIR")
        .env("INCAN_HOME", &oven_home)
        .env("INCAN_SDK_INVENTORY", &inventory)
        .env_remove("INCAN_INTERNAL_SDK_PROVIDER_STORE")
        .arg("run")
        .arg(&source);
    let output = command.output()?;

    assert!(
        output.status.success(),
        "incan run std_regex_surface failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
    let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    assert_eq!(
        lines,
        vec![
            "true",
            "xx@0:2",
            "ALPHA-12",
            "beta",
            "beta",
            "0:4",
            "<none>",
            "<none>",
            "beta|<none>",
            "one,two",
            "a:1,b:2",
            "a|b|c",
            "a|b,c",
            "a|b,c",
            "a|b|c",
            "Lovelace, Ada",
            "Lovelace/Ada",
            "Lovelace, Ada",
            "$2, $1",
            "x x three",
            "$1 two",
            "é@6:8|6:8",
            "unicode-full",
            ";31m",
            ";31m",
            ";31m",
            ";31m",
            ";31m",
            ";31m",
            ";31m",
            ";31m",
            ";31m",
            "; !",
            "; !",
            "é|a",
            "before ;31mafter",
            "before|;31m",
            "before|;31m",
            "|;|!",
        ],
        "unexpected std.regex output:\n{stdout}"
    );
    let consumer_core = generated_project.join("src/__incan_std/regex/_core.rs");
    assert!(
        !consumer_core.exists(),
        "a compiled std.regex module must not be materialized in the consumer: {}",
        consumer_core.display()
    );
    let artifact_root = fs::canonicalize(compiled_sdk_provider_artifact_root(
        &generated_project,
        "incan_stdlib_data",
    )?)?;
    let provider_root = inventory.parent().ok_or("SDK inventory has no provider root")?;
    assert!(
        artifact_root.starts_with(provider_root),
        "Oven std.regex child must use the scheduler-injected immutable SDK inventory rather than an ambient provider store: {}",
        artifact_root.display()
    );
    let generated_core = fs::read_to_string(artifact_root.join("src/regex/_core.rs"))?;
    for unexpected in [
        "RegexBuilder::new(&(pattern).to_string())",
        "raw.find(&(text).to_string())",
        "raw.find_iter(&(text).to_string())",
        "raw.captures(&(text).to_string())",
        "raw.captures_iter(&(text).to_string())",
        "found.as_str().to_string()",
        "raw_name.to_string()",
    ] {
        assert!(
            !generated_core.contains(unexpected),
            "std.regex should let the compiler borrow Incan strings for Rust regex APIs instead of cloning them:\n{generated_core}"
        );
    }
    for expected in [
        "RegexBuilder::new(&pattern)",
        "raw.find(&text)",
        "raw.find_iter(&text)",
        "raw.captures(&text)",
        "raw.captures_iter(&text)",
    ] {
        assert!(
            generated_core.contains(expected),
            "std.regex should preserve compiler-managed Rust borrow boundaries; missing `{expected}`:\n{generated_core}"
        );
    }
    assert_eq!(
        generated_core.matches("RustString::from(found.as_str())").count(),
        6,
        "every borrowed regex match view should be copied into owned public match text:\n{generated_core}"
    );
    assert_eq!(
        generated_core.matches("RustString::from(raw_name)").count(),
        2,
        "every borrowed regex capture name should be copied into an owned dictionary key:\n{generated_core}"
    );
    let compact_generated_core = generated_core
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();
    assert_eq!(
        compact_generated_core
            .matches("incan_std_core::strings::str_concat(&out,&str_slice_byte_range(&text,last_end,start),)")
            .count(),
        3,
        "every replacement loop should pass the owned Rust byte-range result directly to str_concat:\n{generated_core}"
    );
    assert_eq!(
        compact_generated_core
            .matches("incan_std_core::strings::str_concat(&out,&str_slice_from_byte_offset(&text,last_end),)")
            .count(),
        3,
        "every replacement loop should pass the owned Rust suffix directly to str_concat:\n{generated_core}"
    );
    assert!(
        !generated_core.contains("out = out + str_slice"),
        "compiled std.regex providers must not lower owned Rust String results through infix String + String:\n{generated_core}"
    );
    Ok(())
}

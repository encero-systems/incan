//! Native compile-and-run coverage for RFC 009 integer helpers and explicit resize targets.

use incan_emit::IrCodegen;
use incan_frontend::{lexer, parser};
use oven_model::compiler_suite_env;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn rfc009_integer_helpers_and_explicit_resizes_compile_and_run() -> TestResult {
    let source = r#"
pub def numeric_helper_score() -> int:
    mut score: int = 0
    maximum: i8 = 127
    match maximum.checked_add(1i8):
        Some(_) => score += 100_000
        None => score += 1
    clamped: i8 = maximum.saturating_add(1i8)
    if clamped == 127i8:
        score += 10
    wrapped: u8 = 255u8.wrapping_add(1u8)
    if wrapped == 0u8:
        score += 100
    power: i8 = 3i8.saturating_pow(8u32)
    if power == 127i8:
        score += 1_000
    wide: i16 = 300
    match wide.try_resize[u8]():
        Some(_) => score += 100_000
        None => score += 10_000
    resized: u8 = wide.saturating_resize[u8]()
    if resized == 255u8:
        score += 100_000
    return score
"#;
    let generated = generate_rust(source)?;
    compile_and_run(&generated, "assert_eq!(numeric_helper_score(), 111_111)")
}

/// Compile Incan source through the checked AST-to-IR pipeline and return generated Rust.
fn generate_rust(source: &str) -> Result<String, std::io::Error> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("lex: {errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("parse: {errors:?}")))?;
    IrCodegen::new()
        .try_generate(&program)
        .map_err(|error| std::io::Error::other(format!("codegen: {error:?}")))
}

/// Compile generated Rust with the active compiler-suite linkage and run the resulting binary.
fn compile_and_run(generated: &str, rust_assertion: &str) -> TestResult {
    let directory = tempfile::tempdir()?;
    let consumer = directory.path().join("consumer.rs");
    std::fs::write(&consumer, format!("{generated}\nfn main() {{ {rust_assertion}; }}\n"))?;
    let binary = directory
        .path()
        .join(format!("consumer{}", std::env::consts::EXE_SUFFIX));
    let capability = compiler_suite_env::OvenCompilerSuiteCapability::from_environment(
        compiler_suite_env::OVEN_COMPILER_SUITE_CAPABILITY_ENV,
    )?;
    let rustc = capability
        .as_ref()
        .map(|capability| capability.rustc.clone())
        .unwrap_or_else(|| {
            std::env::var_os("RUSTC")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| "rustc".into())
        });
    let mut command = std::process::Command::new(&rustc);
    command.args(["--edition=2024", "--crate-name=rfc009_numeric_helpers"]);
    if let Some(capability) = capability {
        for path in capability.dependency_search_paths {
            command.arg("-L").arg(format!("dependency={}", path.display()));
        }
        for (name, path) in capability.externs {
            command.arg("--extern").arg(format!("{name}={}", path.display()));
        }
    } else {
        add_local_runtime_linkage(&mut command)?;
    }
    let output = command.arg(&consumer).arg("-o").arg(&binary).output()?;
    assert!(
        output.status.success(),
        "{}\n{generated}",
        String::from_utf8_lossy(&output.stderr)
    );
    let execution = std::process::Command::new(binary).output()?;
    assert!(
        execution.status.success(),
        "{}\n{generated}",
        String::from_utf8_lossy(&execution.stderr)
    );
    Ok(())
}

/// Link the generated program to the runtime artifacts used by the current test executable.
fn add_local_runtime_linkage(command: &mut std::process::Command) -> TestResult {
    let executable = std::env::current_exe()?;
    let target_profile = executable
        .ancestors()
        .find(|ancestor| ancestor.join("build").is_dir())
        .ok_or("test executable has no target profile directory")?;
    let mut directories = build_output_directories(&target_profile.join("build"))?;
    let dependencies = target_profile.join("deps");
    if dependencies.is_dir() {
        directories.push(dependencies);
    }
    for directory in &directories {
        command.arg("-L").arg(format!("dependency={}", directory.display()));
    }
    let stdlib = newest_artifact(&directories, |name| {
        name.starts_with("libincan_std_core-") && name.ends_with(".rlib")
    })?
    .ok_or("numeric helper proof requires a compiled stdlib artifact")?;
    command
        .arg("--extern")
        .arg(format!("incan_std_core={}", stdlib.display()));
    let derive = newest_artifact(&directories, |name| {
        name.trim_start_matches("lib").starts_with("incan_derive-")
            && std::path::Path::new(name)
                .extension()
                .and_then(|extension| extension.to_str())
                == Some(std::env::consts::DLL_EXTENSION)
    })?
    .ok_or("numeric helper proof requires a compiled derive artifact")?;
    command
        .arg("--extern")
        .arg(format!("incan_derive={}", derive.display()));
    Ok(())
}

/// Return Cargo's build-script and unit output directories under one target profile.
fn build_output_directories(build_directory: &std::path::Path) -> Result<Vec<std::path::PathBuf>, std::io::Error> {
    let mut outputs = Vec::new();
    for package in std::fs::read_dir(build_directory)? {
        let package = package?;
        for fingerprint in std::fs::read_dir(package.path())? {
            let output = fingerprint?.path().join("out");
            if output.is_dir() {
                outputs.push(output);
            }
        }
    }
    Ok(outputs)
}

/// Return the most recently modified artifact whose file name satisfies `matches` in any candidate directory.
fn newest_artifact(
    directories: &[std::path::PathBuf],
    matches: impl Fn(&str) -> bool,
) -> Result<Option<std::path::PathBuf>, std::io::Error> {
    let mut newest: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    for directory in directories {
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !matches(name) {
                continue;
            }
            let modified = std::fs::metadata(&path)?.modified()?;
            if newest.as_ref().is_none_or(|(when, _)| modified > *when) {
                newest = Some((modified, path));
            }
        }
    }
    Ok(newest.map(|(_, path)| path))
}

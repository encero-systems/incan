//! Build generated programs with rustc and run them: a program alone, with the standard-library source modules its
//! generated Rust reaches through `crate::__incan_std`, and a consumer with the `pub::` dependency it imports.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use incan_frontend::{lexer, parser};
use oven_model::compiler_suite_env;

use super::packages::{parse, provider_plan_of, publish_package};
use crate::IrCodegen;

/// Result of a helper that reports failures as errors.
type BuildResult<T> = Result<T, Box<dyn std::error::Error>>;

/// The stdlib source modules a generated program may reach, each with the `__incan_std` module path it is mounted at.
const MOUNTED_MODULES: [(&str, &str, &str); 3] = [
    (
        "derives",
        "collection",
        "loaves/stdlib/core/src/derives/collection.incn",
    ),
    (
        "derives",
        "comparison",
        "loaves/stdlib/core/src/derives/comparison.incn",
    ),
    ("traits", "callable", "loaves/stdlib/core/src/traits/callable.incn"),
];

/// Lex, parse, check and generate Rust for one program.
fn generate(source: &str) -> BuildResult<String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?;
    Ok(IrCodegen::new().try_generate(&program)?)
}

/// Generate one stdlib source module as the Rust a project build mounts under `crate::__incan_std`, checked as the
/// standard library's own source. Its stdlib version check is dropped: the program that mounts it carries one.
fn generate_stdlib_module(relative_path: &str) -> BuildResult<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let source = std::fs::read_to_string(root.join(relative_path))?;
    let tokens = lexer::lex(&source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?;
    let mut codegen = IrCodegen::new();
    codegen.set_standard_library_source(true);
    Ok(codegen
        .try_generate(&program)?
        .replace("incan_std_core::__incan_stdlib_version_check!", "// "))
}

/// Mount the stdlib modules a generated program reaches through `crate::__incan_std`, when it reaches any.
fn with_stdlib_modules(program: String) -> BuildResult<String> {
    if !program.contains("__incan_std") {
        return Ok(program);
    }
    let mut groups: Vec<(&str, Vec<String>)> = Vec::new();
    for (group, module, path) in MOUNTED_MODULES {
        let mounted = format!("pub mod {module} {{\n{}\n}}", generate_stdlib_module(path)?);
        match groups.iter_mut().find(|(name, _)| *name == group) {
            Some((_, modules)) => modules.push(mounted),
            None => groups.push((group, vec![mounted])),
        }
    }
    let groups = groups
        .into_iter()
        .map(|(group, modules)| format!("pub mod {group} {{\n{}\n}}", modules.join("\n")))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(format!("{program}\npub mod __incan_std {{\n{groups}\n}}\n"))
}

/// Generate `source`, mount the stdlib modules it reaches, build it, run it and return its standard output.
pub(super) fn run_with_stdlib(source: &str) -> BuildResult<String> {
    let directory = tempfile::tempdir()?;
    let program = build_program(directory.path(), &with_stdlib_modules(generate(source)?)?, &[])?;
    run(&program)
}

/// Generate a consumer against the dependency `name` published from `provider`, or return the check's refusal.
pub(super) fn generate_consumer(name: &str, provider: &str, consumer: &str) -> BuildResult<(String, String)> {
    let (manifest, provider_code) = publish_package(name, provider, &[])?;
    let program = parse(consumer)?;
    let mut codegen = IrCodegen::new();
    codegen.set_provider_plan(Arc::new(provider_plan_of(&[&manifest])?));
    Ok((provider_code, codegen.try_generate(&program)?))
}

/// Publish `provider` as the dependency `name`, build its crate and the consumer against it, run the consumer and
/// return its standard output.
pub(super) fn run_with_dependency(name: &str, provider: &str, consumer: &str) -> BuildResult<String> {
    let (provider_code, consumer_code) = generate_consumer(name, provider, consumer)?;
    let directory = tempfile::tempdir()?;
    let input = directory.path().join("dependency.rs");
    let library = directory.path().join(format!("lib{name}.rlib"));
    std::fs::write(&input, &provider_code)?;
    let built = rustc("lib", name)?.arg(&input).arg("-o").arg(&library).output()?;
    if !built.status.success() {
        return Err(format!("{}\n{provider_code}", String::from_utf8_lossy(&built.stderr)).into());
    }
    let program = build_program(directory.path(), &consumer_code, &[(name, library)])?;
    run(&program)
}

/// Build `source` as a program in `directory`, with each of `externs` as a crate it may name.
fn build_program(directory: &Path, source: &str, externs: &[(&str, PathBuf)]) -> BuildResult<PathBuf> {
    let input = directory.join("fixture.rs");
    let program = directory.join("fixture");
    std::fs::write(&input, source)?;
    let mut command = rustc("bin", "fixture")?;
    for (name, path) in externs {
        command.arg("--extern").arg(format!("{name}={}", path.display()));
    }
    let built = command.arg(&input).arg("-o").arg(&program).output()?;
    if !built.status.success() {
        return Err(format!("{}\n{source}", String::from_utf8_lossy(&built.stderr)).into());
    }
    Ok(program)
}

/// Run a built program and return its standard output.
fn run(program: &Path) -> BuildResult<String> {
    let ran = Command::new(program).output()?;
    if !ran.status.success() {
        return Err(format!("program failed: {}", String::from_utf8_lossy(&ran.stderr)).into());
    }
    Ok(String::from_utf8(ran.stdout)?)
}

/// Return a `rustc` invocation for one generated crate named `crate_name` of `crate_type`, with the runtime crates the
/// generated code names in scope.
fn rustc(crate_type: &str, crate_name: &str) -> BuildResult<Command> {
    let capability = compiler_suite_env::OvenCompilerSuiteCapability::from_environment(
        compiler_suite_env::OVEN_COMPILER_SUITE_CAPABILITY_ENV,
    )?;
    let rustc = capability
        .as_ref()
        .map(|capability| capability.rustc.clone())
        .unwrap_or_else(|| {
            std::env::var_os("RUSTC")
                .map(PathBuf::from)
                .unwrap_or_else(|| "rustc".into())
        });
    let mut command = Command::new(rustc);
    command.args(["--edition=2024", "-A", "warnings"]);
    command.arg(format!("--crate-name={crate_name}"));
    command.arg(format!("--crate-type={crate_type}"));
    if let Some(capability) = capability {
        for path in capability.dependency_search_paths {
            command.arg("-L").arg(format!("dependency={}", path.display()));
        }
        for (name, path) in capability.externs {
            command.arg("--extern").arg(format!("{name}={}", path.display()));
        }
        return Ok(command);
    }
    let executable = std::env::current_exe()?;
    let target_profile = executable
        .ancestors()
        .find(|ancestor| ancestor.join("build").is_dir())
        .ok_or("test executable has no target profile directory")?;
    let mut directories = build_output_directories(&target_profile.join("build"))?;
    let deps = target_profile.join("deps");
    if deps.is_dir() {
        directories.push(deps);
    }
    for directory in &directories {
        command.arg("-L").arg(format!("dependency={}", directory.display()));
    }
    let stdlib = newest_artifact(&directories, |name| {
        name.starts_with("libincan_std_core-") && name.ends_with(".rlib")
    })?
    .ok_or("generated-Rust proof requires a compiled incan_std_core artifact")?;
    command
        .arg("--extern")
        .arg(format!("incan_std_core={}", stdlib.display()));
    let derive = newest_artifact(&directories, |name| {
        name.trim_start_matches("lib").starts_with("incan_derive-")
            && Path::new(name).extension().and_then(|extension| extension.to_str())
                == Some(std::env::consts::DLL_EXTENSION)
    })?
    .ok_or("generated-Rust proof requires a compiled incan_derive artifact")?;
    command
        .arg("--extern")
        .arg(format!("incan_derive={}", derive.display()));
    Ok(command)
}

/// Return Cargo's build-script output directories under one target profile.
fn build_output_directories(build_directory: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
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

/// Find the newest artifact matching `predicate` in any of `directories`.
fn newest_artifact(
    directories: &[PathBuf],
    predicate: impl Fn(&str) -> bool,
) -> Result<Option<PathBuf>, std::io::Error> {
    let mut matches = Vec::new();
    for directory in directories {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if predicate(name) {
                let modified = entry.metadata().and_then(|metadata| metadata.modified()).ok();
                matches.push((modified, entry.path()));
            }
        }
    }
    matches.sort_by_key(|(modified, _)| *modified);
    Ok(matches.pop().map(|(_, path)| path))
}

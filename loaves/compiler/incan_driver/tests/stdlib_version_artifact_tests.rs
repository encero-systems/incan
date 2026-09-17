//! Native artifact proof for the stdlib version contract embedded by the current emitter.
//!
//! The fixture builds the real stdlib `version.rs` as an isolated contract library. Its crate wrapper only exposes
//! that module; no replacement macro or compatibility implementation is supplied. An empty Incan program needs no
//! other runtime surface, so its complete generated Rust can link this library without rebuilding unrelated facets.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use incan_emit::{GENERATED_FOR_STDLIB_VERSION, IrCodegen};
use incan_frontend::{lexer, parser};
use incan_test_support as support;

/// Use the runner-selected consumer compiler when Oven provides one, or Cargo's configured Rust compiler otherwise.
fn rustc_command() -> Command {
    Command::new(
        std::env::var_os("INCAN_OVEN_COMPILER_SUITE_RUSTC")
            .or_else(|| std::env::var_os("RUSTC"))
            .unwrap_or_else(|| "rustc".into()),
    )
}

/// Return a subprocess failure with the full compiler or execution diagnostic.
fn require_success(output: &Output, context: &str) -> Result<(), Box<dyn Error>> {
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "{context} failed ({})\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        ))
        .into());
    }
    Ok(())
}

/// Compile the unchanged real version module with the package version supplied by the simulated stdlib publication.
fn compile_stdlib_contract(directory: &Path, version: &str) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(directory)?;
    fs::copy(
        support::repo_root().join("loaves/stdlib/core/rust/src/version.rs"),
        directory.join("version.rs"),
    )?;
    fs::write(directory.join("lib.rs"), "pub mod version;\n")?;
    let output = rustc_command()
        .current_dir(directory)
        .env("CARGO_PKG_VERSION", version)
        .args([
            "--edition=2024",
            "--crate-name",
            "incan_std_core",
            "--crate-type=rlib",
            "lib.rs",
            "-o",
            "libincan_std_core.rlib",
        ])
        .output()?;
    require_success(&output, &format!("stdlib contract {version}"))
}

/// Link the same complete generated source against one versioned stdlib artifact, with no consumer Cargo metadata.
fn compile_generated_program(directory: &Path, generated: &str) -> Result<Output, Box<dyn Error>> {
    fs::write(directory.join("main.rs"), generated)?;
    Ok(rustc_command()
        .current_dir(directory)
        .env_remove("CARGO_PKG_VERSION")
        .env_remove("CARGO_PKG_NAME")
        .env_remove("CARGO_MANIFEST_DIR")
        .args([
            "--edition=2024",
            "--crate-name",
            "version_contract_consumer",
            "--extern",
            "incan_std_core=libincan_std_core.rlib",
            "main.rs",
            "-o",
        ])
        .arg(format!("consumer{}", std::env::consts::EXE_SUFFIX))
        .output()?)
}

/// Exercise the current emitted requirement against exact, later compatible, and incompatible native artifacts.
#[test]
fn current_generated_program_accepts_patch_ahead_and_rejects_major_ahead() -> Result<(), Box<dyn Error>> {
    let tokens =
        lexer::lex("def main() -> None:\n    pass\n").map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let generated = IrCodegen::new().try_generate(&program)?;
    assert!(generated.contains("incan_std_core::__incan_stdlib_version_check!"));
    assert!(generated.contains(GENERATED_FOR_STDLIB_VERSION));

    let required = semver::Version::parse(GENERATED_FOR_STDLIB_VERSION)?;
    let next_patch = required.patch.checked_add(1).ok_or("stdlib patch overflow")?;
    let next_major = required.major.checked_add(1).ok_or("stdlib major overflow")?;
    let patch_ahead = format!("{}.{}.{next_patch}", required.major, required.minor);
    let major_ahead = format!("{next_major}.0.0");
    let temporary = tempfile::tempdir()?;

    for (case, version, accepted) in [
        ("exact", GENERATED_FOR_STDLIB_VERSION, true),
        ("released-patch-ahead", patch_ahead.as_str(), true),
        ("released-major-ahead", major_ahead.as_str(), false),
    ] {
        let directory = temporary.path().join(case);
        compile_stdlib_contract(&directory, version)?;
        let output = compile_generated_program(&directory, &generated)?;
        if accepted {
            require_success(&output, &format!("generated consumer with stdlib {version}"))?;
            let execution = Command::new(directory.join(format!("consumer{}", std::env::consts::EXE_SUFFIX)))
                .current_dir(&directory)
                .output()?;
            require_success(
                &execution,
                &format!("generated consumer execution with stdlib {version}"),
            )?;
        } else {
            assert!(
                !output.status.success(),
                "incompatible stdlib {version} unexpectedly compiled"
            );
            let diagnostics = String::from_utf8_lossy(&output.stderr);
            assert!(
                diagnostics.contains("Incan stdlib version mismatch"),
                "expected the real version guard to reject stdlib {version}:\n{diagnostics}"
            );
        }
    }
    Ok(())
}

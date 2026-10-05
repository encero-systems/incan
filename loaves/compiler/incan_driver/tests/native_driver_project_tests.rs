//! Oven-built pinned driver conformance at the Incan-plan-to-MIR boundary.

use incan_test_support as support;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Copy source fixtures without admitting a checkout's generated targets.
fn copy_tree(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Select the installed pinned compiler including rustc-dev, which ordinary suite compiler closures omit.
fn pinned_driver_rustc() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let output = Command::new("rustup")
        .args(["which", "--toolchain", "1.98.0", "rustc"])
        .output()?;
    success(&output, "pinned compiler selection");
    Ok(PathBuf::from(String::from_utf8(output.stdout)?.trim()))
}

/// Run the explicit publisher in the fixture-owned home and preserve full failure diagnostics.
fn bake(project: &Path, home: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut command = support::repo_command();
    support::configure_explicit_oven_bake_command(&mut command)?;
    let rustc = pinned_driver_rustc()?;
    let output = command
        .env("RUSTC", rustc)
        .args(["oven", "bake", "--project"])
        .arg(project)
        .env("INCAN_HOME", home)
        .env("RUSTC_BOOTSTRAP", "ambient-unit")
        .output()?;
    success(&output, "Oven bake");
    Ok(())
}

/// Assert native success with both output streams available on failure.
fn success(output: &Output, operation: &str) {
    assert!(
        output.status.success(),
        "{operation}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Startup refusals precede output creation, including a substituted loaded driver library.
fn check_startup_refusals(
    binary: &Path,
    source: &Path,
    root: &Path,
    sysroot: &Path,
    runtime_rlib: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    // ---- Ambient permission and detached sysroot ----
    let ambient = Command::new(binary)
        .env("RUSTC_BOOTSTRAP", "1")
        .arg(source)
        .arg("scalar")
        .arg(root.join("ambient-output"))
        .arg(sysroot)
        .arg(runtime_rlib)
        .output()?;
    assert!(!ambient.status.success());
    assert!(String::from_utf8_lossy(&ambient.stderr).contains("Bootstrap"));
    assert!(!root.join("ambient-output").exists());
    let mismatch = Command::new(binary)
        .env_remove("RUSTC_BOOTSTRAP")
        .arg(source)
        .arg("scalar")
        .arg(root.join("refused-output"))
        .arg(root)
        .arg(runtime_rlib)
        .output()?;
    assert!(!mismatch.status.success());
    assert!(String::from_utf8_lossy(&mismatch.stderr).contains("Mismatch"));
    assert!(!root.join("refused-output").exists());

    // ---- Loaded-library substitution ----
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let substitute = root.join("substituted-driver");
        fs::create_dir_all(&substitute)?;
        let directory = sysroot
            .join("lib/rustlib")
            .join(oven_rustc::rustc::rustc_host_target(&pinned_driver_rustc()?)?)
            .join("lib");
        let mut library = None;
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("librustc_driver-") && (name.ends_with(".dylib") || name.ends_with(".so")) {
                if library.is_some() {
                    return Err("pinned driver library is ambiguous".into());
                }
                library = Some(entry.path());
            }
        }
        let library = library.ok_or("pinned driver library is missing")?;
        fs::copy(
            &library,
            substitute.join(library.file_name().ok_or("driver library has no name")?),
        )?;
        let loader_variable = if cfg!(target_os = "macos") {
            "DYLD_LIBRARY_PATH"
        } else {
            "LD_LIBRARY_PATH"
        };
        let substituted = Command::new(binary)
            .env_remove("RUSTC_BOOTSTRAP")
            .env(loader_variable, &substitute)
            .arg(source)
            .arg("scalar")
            .arg(root.join("substituted-output"))
            .arg(sysroot)
            .arg(runtime_rlib)
            .output()?;
        assert!(!substituted.status.success());
        assert!(
            String::from_utf8_lossy(&substituted.stderr).contains("loaded a different rustc_driver"),
            "{}",
            String::from_utf8_lossy(&substituted.stderr)
        );
        assert!(!root.join("substituted-output").exists());
    }
    Ok(())
}

/// One Oven invocation builds the Incan plan and driver, then native output, spans and typed refusals are checked.
#[test]
fn oven_driver_compiles_scalar_plan_and_refuses_invalid_inputs() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let repo = support::repo_root();
    let root = temporary.path();
    let driver_source = repo.join("loaves/toolchain/incan-rustc-driver");
    let driver = root.join("driver");
    copy_tree(&driver_source.join("src"), &driver.join("src"))?;
    fs::write(
        driver.join("loaf.toml"),
        fs::read_to_string(driver_source.join("loaf.toml"))?.replace("../../compiler/incan_mir_plan", "../library"),
    )?;
    let library = root.join("library");
    fs::create_dir_all(&library)?;
    fs::copy(
        repo.join("loaves/compiler/incan_mir_plan/loaf.toml"),
        library.join("loaf.toml"),
    )?;
    copy_tree(&repo.join("loaves/compiler/incan_mir_plan/src"), &library.join("src"))?;
    let runtime = root.join("runtime");
    copy_tree(&driver_source.join("tests/fixtures/native_output"), &runtime)?;
    let source = root.join("scalar.incn");
    fs::copy(driver_source.join("tests/fixtures/scalar.incn"), &source)?;
    let home = root.join("home");
    bake(&driver, &home)?;
    let runtime_caller = root.join("runtime-caller");
    fs::create_dir_all(runtime_caller.join("src"))?;
    fs::write(
        runtime_caller.join("loaf.toml"),
        "[project]\nname = \"runtime-caller\"\n[dependencies]\nnative_output = { loaf = \"native_output\", path = \"../runtime\" }\n[[rust.bin]]\nname = \"runtime-caller\"\npath = \"src/main.rs\"\n",
    )?;
    fs::write(
        runtime_caller.join("src/main.rs"),
        "use native_output::caller::incan::print_int;\nfn main() { print_int(42); }\n",
    )?;
    bake(&runtime_caller, &home)?;
    let rustc = pinned_driver_rustc()?;
    let sysroot = oven_rustc::rustc::rustc_sysroot(&rustc)?;
    for profile in ["debug", "release"] {
        let receipt_path = if profile == "release" {
            oven_store::default_receipt_path(&runtime)
        } else {
            oven_store::default_receipt_path(&runtime).with_file_name("library-debug-receipt.json")
        };
        let receipt: oven_store::OvenReceipt = serde_json::from_slice(&fs::read(receipt_path)?)?;
        let closure = oven_rustc::loaf::resolve_compiler_owned_loaf_for_registry_dependencies(&receipt, &[])?
            .ok_or("runtime fixture must select its compiler-owned native closure")?;
        let directories = &closure.artifact_plan.dependency_search_paths;
        let binary = driver.join("target/rust").join(profile).join("incan-rustc-driver");
        let runtime_rlib = runtime
            .join("target/lib/oven")
            .join(profile)
            .join("libnative_output.rlib");
        for mode in ["normal", "overflow", "dangling"] {
            let output_binary = root.join(format!("scalar-{profile}-{mode}"));
            let compile = Command::new(&binary)
                .env_remove("RUSTC_BOOTSTRAP")
                .arg(&source)
                .arg("scalar")
                .arg(&output_binary)
                .arg(&sysroot)
                .arg(&runtime_rlib)
                .arg(mode)
                .args(directories)
                .output()?;
            if mode == "dangling" {
                assert!(!compile.status.success());
                assert!(
                    String::from_utf8_lossy(&compile.stderr).contains("UnknownBlock"),
                    "{}",
                    String::from_utf8_lossy(&compile.stderr)
                );
                assert!(!output_binary.exists());
                continue;
            }
            success(&compile, "native plan compilation");
            let run = Command::new(&output_binary).output()?;
            if mode == "normal" {
                success(&run, "scalar binary");
                assert_eq!(String::from_utf8(run.stdout)?, "42\n");
            } else {
                assert!(!run.status.success());
                assert!(
                    String::from_utf8_lossy(&run.stderr).contains("scalar.incn:3:12"),
                    "{}",
                    String::from_utf8_lossy(&run.stderr)
                );
            }
        }
        check_startup_refusals(&binary, &source, root, &sysroot, &runtime_rlib)?;
    }
    Ok(())
}

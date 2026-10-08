//! Unchanged Fibonacci benchmark proof using the Oven-built driver and authored formatting runtime.

use super::*;

/// Explicit runtime artifacts and metadata search paths retained by its publisher receipt.
pub(super) struct NativeClosure {
    externs: Vec<(String, PathBuf)>,
    directories: Vec<PathBuf>,
}

/// Select exactly one published profile, without discovering similarly named ambient rlibs.
pub(super) fn runtime_closure(runtime: &Path, profile: &str) -> Result<NativeClosure, Box<dyn std::error::Error>> {
    let receipt = oven_store::default_receipt_path(runtime);
    let receipt = if profile == "debug" {
        receipt.with_file_name("library-debug-receipt.json")
    } else {
        receipt
    };
    let receipt: oven_store::OvenReceipt = serde_json::from_slice(&fs::read(receipt)?)?;
    let closure = oven_rustc::loaf::resolve_compiler_owned_loaf_for_registry_dependencies(&receipt, &[])?
        .ok_or("formatting runtime has no retained native closure")?;
    let mut externs = closure.artifact_plan.externs;
    externs.push((
        "incan_native_runtime".into(),
        runtime
            .join("target/lib/oven")
            .join(profile)
            .join("libincan_native_runtime.rlib"),
    ));
    Ok(NativeClosure {
        externs,
        directories: closure.artifact_plan.dependency_search_paths,
    })
}

/// Run the actual frontend and lowering with exact selected native dependency bindings.
fn compile_source(
    driver: &Path,
    source: &Path,
    output: &Path,
    sysroot: &Path,
    closure: &NativeClosure,
) -> Result<Output, Box<dyn std::error::Error>> {
    Ok(source_command(driver, source, output, sysroot, closure).output()?)
}

/// Construct the exact direct-route invocation so bounded census execution shares dependency selection.
pub(super) fn source_command(
    driver: &Path,
    source: &Path,
    output: &Path,
    sysroot: &Path,
    closure: &NativeClosure,
) -> Command {
    let mut command = Command::new(driver);
    command
        .env_remove("RUSTC_BOOTSTRAP")
        .arg("--source")
        .arg(source)
        .arg("native_corpus")
        .arg(output)
        .arg(sysroot);
    for (name, artifact) in &closure.externs {
        command.arg("--extern").arg(format!("{name}={}", artifact.display()));
    }
    for directory in &closure.directories {
        command.arg("--search").arg(directory);
    }
    command
}

/// Preserve the Fibonacci source bytes and compare direct-native output with its normal backend output.
pub(super) fn check_fib(
    driver: &Path,
    root: &Path,
    sysroot: &Path,
    runtime: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let closure = runtime_closure(runtime, "release")?;
    let name = "fib";
    let original = support::repo_root()
        .join("workspaces/benchmarks/compute")
        .join(name)
        .join(format!("{name}.incn"));
    let project = root.join(format!("benchmark-{name}"));
    fs::create_dir_all(&project)?;
    let source = project.join(format!("{name}.incn"));
    fs::copy(&original, &source)?;
    assert_eq!(fs::read(&source)?, fs::read(&original)?);
    let native = project.join("native");
    success(
        &compile_source(driver, &source, &native, sysroot, &closure)?,
        "native benchmark compilation",
    );
    let legacy_output = project.join("legacy");
    let build = support::repo_command()
        .current_dir(&project)
        .arg("build")
        .arg(&source)
        .arg(&legacy_output)
        .output()?;
    success(&build, "legacy benchmark compilation");
    let legacy = legacy_output.join("oven/release").join(name);
    let expected = Command::new(legacy).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy benchmark execution");
    success(&actual, "native benchmark execution");
    assert_eq!(actual.stdout, expected.stdout, "{name} output must be byte-identical");
    println!(
        "benchmark {name}: pass, unchanged source, byte-identical stdout {:?}",
        String::from_utf8_lossy(&actual.stdout)
    );
    Ok(())
}

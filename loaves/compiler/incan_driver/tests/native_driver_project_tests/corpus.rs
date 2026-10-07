//! Unchanged compute benchmark proof using the Oven-built driver and authored formatting runtime.

use super::*;

/// Explicit runtime artifacts and metadata search paths retained by its publisher receipt.
pub(super) struct NativeClosure {
    externs: Vec<(String, PathBuf)>,
    directories: Vec<PathBuf>,
    _plan: oven_rustc::plan::OvenDirectRustcPlanSelection,
}

/// Select exactly one published profile, without discovering similarly named ambient rlibs.
pub(super) fn runtime_closure(runtime: &Path) -> Result<NativeClosure, Box<dyn std::error::Error>> {
    let closure = runtime_plan(runtime)?;
    let mut externs = closure.artifact_plan().externs.clone();
    externs.push((
        "incan_native_runtime".into(),
        runtime
            .join("target/lib/oven")
            .join("debug")
            .join("libincan_native_runtime.rlib"),
    ));
    Ok(NativeClosure {
        externs,
        directories: closure.artifact_plan().dependency_search_paths.clone(),
        _plan: closure,
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

/// Preserve benchmark source bytes and compare direct-native output with the normal backend's debug execution.
pub(super) fn check_benchmark(
    driver: &Path,
    root: &Path,
    sysroot: &Path,
    runtime: &Path,
    name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let closure = runtime_closure(runtime)?;
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
    let expected = support::repo_command()
        .current_dir(&project)
        .env("INCAN_HOME", support::oven_fixture_home()?)
        .env("INCAN_OVEN_BAKE_PROFILES", "debug")
        .env("INCAN_NO_BANNER", "1")
        .arg("run")
        .arg(&source)
        .output()?;
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

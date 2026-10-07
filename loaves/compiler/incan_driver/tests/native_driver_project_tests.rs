//! Oven-built pinned driver conformance at the Incan-plan-to-MIR boundary.

#[path = "native_driver_project_tests/corpus.rs"]
mod corpus;

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

/// Bake one debug fixture in the persistent Oven home and preserve full failure diagnostics.
fn bake(project: &Path, home: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut command = support::repo_command();
    support::configure_explicit_oven_bake_command(&mut command)?;
    let rustc = pinned_driver_rustc()?;
    let started = support::command_timing_started();
    let output = command
        .env("RUSTC", rustc)
        .args(["oven", "bake", "--project"])
        .arg(project)
        .env("INCAN_HOME", home)
        .env("RUSTC_BOOTSTRAP", "ambient-unit")
        .output()?;
    support::report_command_timing("oven bake --project (debug)", started);
    if started.is_some() {
        eprintln!("{}", String::from_utf8_lossy(&output.stdout));
    }
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

/// Retain a debug runtime's receipt-selected native plan, including its immutable output leases.
///
/// Native SDK publication supplies a stored direct-Rustc plan. Older installed toolchains supply a compiler-owned
/// Loaf instead; both paths validate the same receipt and keep their selected closure alive through execution.
fn runtime_plan(runtime: &Path) -> Result<oven_rustc::plan::OvenDirectRustcPlanSelection, Box<dyn std::error::Error>> {
    let path = oven_store::default_receipt_path(runtime).with_file_name("library-debug-receipt.json");
    let receipt: oven_store::OvenReceipt = serde_json::from_slice(&fs::read(path)?)?;
    let store = oven_store::store::OvenStore::new(
        oven_store::store::store_root_for_home(&support::oven_fixture_home()?),
        oven_store::store::OvenStoreLimits::new(
            oven_store::DEFAULT_OVEN_MAX_PHYSICAL_BYTES,
            oven_store::DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES,
            oven_store::DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES,
        ),
    );
    if let Some(plan) = oven_rustc::plan::selection::select_receipt_direct_rustc_execution_plan(&store, &receipt)? {
        return Ok(oven_rustc::plan::OvenDirectRustcPlanSelection::Stored(Box::new(plan)));
    }
    let loaf = oven_rustc::loaf::resolve_compiler_owned_loaf_for_registry_dependencies(&receipt, &[])?
        .ok_or("runtime fixture has no receipt-selected native closure")?;
    Ok(oven_rustc::plan::OvenDirectRustcPlanSelection::ToolchainLoaf(Box::new(
        loaf,
    )))
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

/// Prepare one source-only driver graph whose plan and lowering share the same semantics-core declaration.
fn prepare_source_driver(root: &Path, repo: &Path) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let driver_source = repo.join("loaves/toolchain/incan-rustc-driver");
    let driver = root.join("driver");
    let library = root.join("library");
    let lowering = root.join("lowering");
    copy_tree(&driver_source.join("src"), &driver.join("src"))?;
    fs::create_dir_all(&library)?;
    fs::create_dir_all(&lowering)?;
    fs::copy(
        repo.join("loaves/compiler/incan_mir_plan/loaf.toml"),
        library.join("loaf.toml"),
    )?;
    copy_tree(&repo.join("loaves/compiler/incan_mir_plan/src"), &library.join("src"))?;
    let core = repo.join("loaves/kernel/incan_semantics_core");
    let frontend = repo.join("loaves/compiler/incan_frontend");
    let core = core.to_str().ok_or("semantics-core path is not UTF-8")?;
    let frontend = frontend.to_str().ok_or("frontend path is not UTF-8")?;
    let manifest = fs::read_to_string(repo.join("loaves/compiler/incan_mir_lowering/loaf.toml"))?
        .replace("../incan_mir_plan", "../library")
        .replace("../../kernel/incan_semantics_core", core);
    fs::write(lowering.join("loaf.toml"), manifest)?;
    copy_tree(
        &repo.join("loaves/compiler/incan_mir_lowering/src"),
        &lowering.join("src"),
    )?;
    let manifest = fs::read_to_string(driver_source.join("loaf.toml"))?
        .replace("../../compiler/incan_mir_plan", "../library")
        .replace("../../compiler/incan_mir_lowering", "../lowering")
        .replace("../../kernel/incan_semantics_core", core)
        .replace("../../compiler/incan_frontend", frontend);
    fs::write(driver.join("loaf.toml"), manifest)?;
    Ok(driver)
}

/// Fresh fixture roots share the plan library's immutable debug output and retain its native SDK closure.
#[test]
fn oven_plan_fixture_shares_debug_native_output() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let source = support::repo_root().join("loaves/compiler/incan_mir_plan");
    let home = support::oven_fixture_home()?;
    let mut outputs = Vec::new();
    for name in ["first", "second"] {
        let project = root.path().join(name);
        copy_tree(&source.join("src"), &project.join("src"))?;
        fs::copy(source.join("loaf.toml"), project.join("loaf.toml"))?;
        bake(&project, &home)?;
        let plan = runtime_plan(&project)?;
        assert!(matches!(
            plan,
            oven_rustc::plan::OvenDirectRustcPlanSelection::Stored(_)
        ));
        assert!(!project.join("target/lib/oven/release").exists());
        assert!(!oven_store::default_receipt_path(&project).is_file());
        outputs.push(oven_store::digest_bytes(&fs::read(
            project.join("target/lib/oven/debug/libincan_mir_plan.rlib"),
        )?));
    }
    assert_eq!(
        outputs[0], outputs[1],
        "identical authored sources must select the same debug output bytes"
    );
    Ok(())
}

/// Publish the compiler Loafs, then reuse one driver bake for native output, spans, and typed refusals.
#[test]
fn oven_driver_compiles_scalar_plan_and_refuses_invalid_inputs() -> Result<(), Box<dyn std::error::Error>> {
    let (root, _temporary) = fixture_root()?;
    let repo = support::repo_root();
    let root = root.as_path();
    let driver_source = repo.join("loaves/toolchain/incan-rustc-driver");
    let driver = prepare_source_driver(root, &repo)?;
    let runtime = root.join("runtime");
    copy_tree(&driver_source.join("tests/fixtures/native_output"), &runtime)?;
    let source = root.join("scalar.incn");
    fs::copy(driver_source.join("tests/fixtures/scalar.incn"), &source)?;
    let home = support::oven_fixture_home()?;
    bake(&root.join("library"), &home)?;
    bake(&root.join("lowering"), &home)?;
    bake(&driver, &home)?;
    let formatting = prepare_formatting_runtime(root, &repo, &home)?;
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
    {
        let profile = "debug";
        let closure = runtime_plan(&runtime)?;
        let directories = &closure.artifact_plan().dependency_search_paths;
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
        check_source_pipeline(&binary, root, &sysroot, &formatting, profile)?;
    }
    let debug = driver.join("target/rust/debug/incan-rustc-driver");
    for name in ["fib", "collatz", "mandelbrot"] {
        corpus::check_benchmark(&debug, root, &sysroot, &formatting, name)?;
    }
    Ok(())
}

/// Keep explicitly requested diagnostic evidence outside wrapper scratch; ordinary test runs clean their fixtures.
fn fixture_root() -> Result<(PathBuf, Option<tempfile::TempDir>), Box<dyn std::error::Error>> {
    if let Some(parent) = std::env::var_os("INCAN_NATIVE_DRIVER_EVIDENCE") {
        fs::create_dir_all(&parent)?;
        let path = tempfile::tempdir_in(parent)?.keep();
        eprintln!("retained native driver fixture: {}", path.display());
        Ok((path, None))
    } else {
        let directory = tempfile::tempdir()?;
        Ok((directory.path().to_path_buf(), Some(directory)))
    }
}

/// Materialize the authored Incan runtime through a real caller dependency, retaining its native closure.
fn prepare_formatting_runtime(root: &Path, repo: &Path, home: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let runtime = root.join("formatting-runtime");
    let original = repo.join("loaves/compiler/incan_native_runtime");
    copy_tree(&original.join("src"), &runtime.join("src"))?;
    fs::copy(original.join("loaf.toml"), runtime.join("loaf.toml"))?;
    let caller = root.join("formatting-caller");
    fs::create_dir_all(caller.join("src"))?;
    fs::write(
        caller.join("loaf.toml"),
        "[project]\nname = \"formatting-caller\"\n[dependencies]\nincan_native_runtime = { loaf = \"incan_native_runtime\", path = \"../formatting-runtime\" }\n[[rust.bin]]\nname = \"formatting-caller\"\npath = \"src/main.rs\"\n",
    )?;
    fs::write(
        caller.join("src/main.rs"),
        "/// Link the selected runtime without printing during fixture setup.\nfn main() { let _function: fn(String) = incan_native_runtime::caller::incan::println_text; }\n",
    )?;
    bake(&caller, home)?;
    Ok(runtime)
}

/// Exercise real checking and Incan lowering, including branches, calls, ranges, conversions, and formatting.
fn check_source_pipeline(
    driver: &Path,
    root: &Path,
    sysroot: &Path,
    runtime: &Path,
    profile: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let source = root.join("checked-source.incn");
    fs::write(
        &source,
        "def twice(mut n: int) -> int:\n  n += n\n  return n\n\ndef main() -> None:\n  mut total = 0\n  for i in range(5, -1, -2):\n    if i == 3:\n      continue\n    total += twice(i)\n  println(f\"total={total} float={float(total)} div={-7 // 3} mod={-7 % 3}\")\n",
    )?;
    let closure = runtime_plan(runtime)?;
    let binary = root.join(format!("checked-source-{profile}"));
    let library = runtime
        .join("target/lib/oven")
        .join(profile)
        .join("libincan_native_runtime.rlib");
    let mut command = Command::new(driver);
    command
        .env_remove("RUSTC_BOOTSTRAP")
        .args(["--source"])
        .arg(&source)
        .arg("checked_source")
        .arg(&binary)
        .arg(sysroot)
        .arg("--extern")
        .arg(format!("incan_native_runtime={}", library.display()));
    for (name, artifact) in &closure.artifact_plan().externs {
        command.arg("--extern").arg(format!("{name}={}", artifact.display()));
    }
    for directory in &closure.artifact_plan().dependency_search_paths {
        command.arg("--search").arg(directory);
    }
    success(&command.output()?, "checked source native compilation");
    let run = Command::new(binary).output()?;
    success(&run, "checked source execution");
    assert_eq!(String::from_utf8(run.stdout)?, "total=12 float=12.0 div=-3 mod=2\n");
    Ok(())
}

#[path = "native_driver_project_tests/census.rs"]
mod census;

/// Measure every behavior fixture only when explicitly requested.
#[test]
#[ignore = "explicit full direct-route census"]
fn direct_route_fixture_census() -> Result<(), Box<dyn std::error::Error>> {
    census::run()
}

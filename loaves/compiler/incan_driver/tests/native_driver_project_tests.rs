//! Oven-built pinned driver conformance at the Incan-plan-to-MIR boundary.

#[path = "native_driver_project_tests/corpus.rs"]
mod corpus;

use incan_test_support as support;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

/// Replace `destination` with a copy of the source tree, without admitting a checkout's generated targets.
///
/// The destination is removed first: in a kept workspace, a source file deleted from the checkout must not survive
/// from an earlier run. Callers name source directories, never a project root whose `target/` they want to keep.
fn copy_tree(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if destination.exists() {
        fs::remove_dir_all(destination)?;
    }
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
    success(&output, &format!("Oven bake {}", project.display()));
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
        .replace("../../kernel/incan_semantics_core", core)
        .replace(
            "../../kernel/incan_lang",
            oven_model::toolchain_layout::resolve_toolchain_crate_path("incan_lang")
                .to_str()
                .ok_or("language registry path is not UTF-8")?,
        );
    fs::write(lowering.join("loaf.toml"), manifest)?;
    copy_tree(
        &repo.join("loaves/compiler/incan_mir_lowering/src"),
        &lowering.join("src"),
    )?;
    let manifest = fs::read_to_string(driver_source.join("loaf.toml"))?
        .replace("../../compiler/incan_mir_plan", "../library")
        .replace("../../compiler/incan_mir_lowering", "../lowering")
        .replace("../../kernel/incan_semantics_core", core)
        .replace("../../compiler/incan_frontend", frontend)
        .replace(
            "../../compiler/incan_driver",
            repo.join("loaves/compiler/incan_driver")
                .to_str()
                .ok_or("driver path is not UTF-8")?,
        );
    fs::write(driver.join("loaf.toml"), manifest)?;
    Ok(driver)
}

/// Publish the compiler Loafs, then reuse one driver bake for native output, spans, and typed refusals.
#[test]
fn oven_driver_compiles_scalar_plan_and_refuses_invalid_inputs() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("scalar")?;
    let root = root.as_path();
    let driver_source = support::repo_root().join("loaves/toolchain/incan-rustc-driver");
    let runtime = root.join("runtime");
    copy_tree(&driver_source.join("tests/fixtures/native_output"), &runtime)?;
    let source = root.join("scalar.incn");
    fs::copy(driver_source.join("tests/fixtures/scalar.incn"), &source)?;
    let home = &fixture.home;
    let formatting = &fixture.formatting;
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
    bake(&runtime_caller, home)?;
    let sysroot = &fixture.sysroot;
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
        let binary = fixture.driver_binary(profile);
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
                .arg(sysroot)
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
        check_startup_refusals(&binary, &source, root, sysroot, &runtime_rlib)?;
        check_source_pipeline(&binary, root, &fixture.sysroot, formatting, profile)?;
    }
    let release = fixture.driver_binary("release");
    corpus::check_strings(&release, root, &fixture.sysroot, formatting)?;
    for name in ["fib", "collatz", "mandelbrot"] {
        corpus::check_benchmark(&release, root, &fixture.sysroot, formatting, name)?;
    }
    Ok(())
}

/// Choose where the shared fixture graph lives; each bake still validates the current copied sources.
///
/// In order: a replayed graph, a retained evidence directory, the suite's kept workspace (so Oven reuses the previous
/// run's bakes), and otherwise a fresh temporary directory.
fn fixture_root() -> Result<(PathBuf, Option<tempfile::TempDir>), Box<dyn std::error::Error>> {
    if let Some(path) = std::env::var_os("INCAN_NATIVE_DRIVER_REPLAY") {
        let path = PathBuf::from(path);
        for project in ["library", "lowering", "driver"] {
            if !path.join(project).join("loaf.toml").is_file() {
                return Err(format!("native driver replay requires a retained {project} fixture").into());
            }
        }
        eprintln!("replaying native driver fixture: {}", path.display());
        return Ok((path, None));
    }
    if let Some(parent) = std::env::var_os("INCAN_NATIVE_DRIVER_EVIDENCE") {
        fs::create_dir_all(&parent)?;
        let path = tempfile::tempdir_in(parent)?.keep();
        eprintln!("retained native driver fixture: {}", path.display());
        Ok((path, None))
    } else if let Some(workspace) = support::explicit_bake_workspace() {
        fs::create_dir_all(&workspace)?;
        Ok((workspace, None))
    } else {
        let directory = tempfile::tempdir()?;
        Ok((directory.path().to_path_buf(), Some(directory)))
    }
}

/// The baked fixture graph every test in this root shares: the plan, lowering and driver Loaves and the formatting
/// runtime, baked once per test process into one Oven home.
struct DriverFixture {
    /// Root of the graph: the copied Loaves, their home, and each test's scratch directory under `cases/`.
    root: PathBuf,
    /// The driver Loaf project; its binaries live under `target/rust/<profile>/`.
    driver: PathBuf,
    /// The Oven home every fixture bake shares.
    home: PathBuf,
    /// The authored Incan runtime Loaf, baked through a caller.
    formatting: PathBuf,
    /// The pinned driver compiler's sysroot.
    sysroot: PathBuf,
    /// Keeps a fresh temporary root alive for the whole process when no workspace was granted.
    _temporary: Option<tempfile::TempDir>,
}

impl DriverFixture {
    /// The driver binary for one profile.
    fn driver_binary(&self, profile: &str) -> PathBuf {
        self.driver.join("target/rust").join(profile).join("incan-rustc-driver")
    }

    /// Return an empty scratch directory owned by one test, so concurrent tests and earlier runs never collide.
    fn scratch(&self, name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let path = self.root.join("cases").join(name);
        if path.exists() {
            fs::remove_dir_all(&path)?;
        }
        fs::create_dir_all(&path)?;
        Ok(path)
    }
}

/// The process-wide fixture; a bake failure is kept as text so every test reports it.
static DRIVER_FIXTURE: OnceLock<Result<DriverFixture, String>> = OnceLock::new();

/// Return the shared fixture, baking it on first use. Tests running in parallel wait for that single bake.
fn driver_fixture() -> Result<&'static DriverFixture, Box<dyn std::error::Error>> {
    DRIVER_FIXTURE
        .get_or_init(|| bake_driver_fixture().map_err(|error| error.to_string()))
        .as_ref()
        .map_err(|error| error.clone().into())
}

/// Copy the current Loaf sources into the fixture root and bake the graph the driver tests compile through.
fn bake_driver_fixture() -> Result<DriverFixture, Box<dyn std::error::Error>> {
    let (root, temporary) = fixture_root()?;
    let repo = support::repo_root();
    let driver = prepare_source_driver(&root, &repo)?;
    let home = root.join("home");
    bake(&root.join("library"), &home)?;
    bake(&root.join("lowering"), &home)?;
    bake(&driver, &home)?;
    let formatting = prepare_formatting_runtime(&root, &repo, &home)?;
    let sysroot = oven_rustc::rustc::rustc_sysroot(&pinned_driver_rustc()?)?;
    Ok(DriverFixture {
        root,
        driver,
        home,
        formatting,
        sysroot,
        _temporary: temporary,
    })
}

/// Materialize the authored Incan runtime through a real caller dependency, retaining its native closure.
fn prepare_formatting_runtime(root: &Path, repo: &Path, home: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let runtime = root.join("formatting-runtime");
    let original = repo.join("loaves/compiler/incan_native_runtime");
    copy_tree(&original.join("src"), &runtime.join("src"))?;
    // Retain the math dependency in the caller's explicit native closure instead of discovering ambient artifacts.
    fs::write(
        runtime.join("loaf.toml"),
        format!(
            "{}\n[rust-dependencies]\nlibm = \"0.2\"\n",
            fs::read_to_string(original.join("loaf.toml"))?
        ),
    )?;
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
    let receipt_path = oven_store::default_receipt_path(runtime);
    let receipt_path = if profile == "debug" {
        receipt_path.with_file_name("library-debug-receipt.json")
    } else {
        receipt_path
    };
    let receipt: oven_store::OvenReceipt = serde_json::from_slice(&fs::read(receipt_path)?)?;
    let closure = oven_rustc::loaf::resolve_compiler_owned_loaf_for_registry_dependencies(&receipt, &[])?
        .ok_or("formatting runtime has no retained native closure")?;
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
    for (name, artifact) in &closure.artifact_plan.externs {
        command.arg("--extern").arg(format!("{name}={}", artifact.display()));
    }
    for directory in &closure.artifact_plan.dependency_search_paths {
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

/// Imported aliases and module-qualified scalar calls preserve canonical binding and legacy output; async vocabulary
/// reaches lowering.
#[test]
fn direct_route_stdlib_imports_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("imports")?;
    let runtime = &fixture.formatting;
    let sysroot = &fixture.sysroot;
    let source = root.join("imports.incn");
    fs::write(
        &source,
        "from std.math import gcd as common, sqrt as root\nimport std.math\nfrom std.derives.comparison import Eq\n\ndef gcd(a: int, b: int) -> int:\n  return a + b\n\ndef main() -> None:\n  println(common(b=18, a=48))\n  println(math.lcm(4, 6))\n  println(gcd(4, 6))\n  println(root(16.0))\n  println(math.sqrt(25.0))\n",
    )?;
    let closure = corpus::runtime_closure(runtime, "release")?;
    let native = root.join("imports-native");
    let binary = fixture.driver_binary("release");
    check_async_frontend_refusal(&binary, &root, sysroot, &closure)?;
    success(
        &corpus::source_command(&binary, &source, &native, sysroot, &closure).output()?,
        "native imported scalar compilation",
    );
    let legacy_root = root.join("imports-legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy imported scalar compilation",
    );
    let legacy = Command::new(legacy_root.join("oven/release/imports")).output()?;
    let actual = Command::new(native).output()?;
    success(&legacy, "legacy imported scalar execution");
    success(&actual, "native imported scalar execution");
    assert_eq!(actual.stdout, b"6\n12\n10\n4.0\n5.0\n");
    assert_eq!(actual.stdout, legacy.stdout);
    Ok(())
}

/// Compare concrete carrier construction, matching, propagation, defaults, and printing against legacy.
#[test]
fn option_result_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("option-result")?;
    let source = root.join("option_result.incn");
    fs::write(
        &source,
        r#"enum Failure:
    Bad

def optional_text() -> Option[str]:
    return Some("text")

def optional_list() -> Option[List[int]]:
    return Some([2, 3])

def show_result(value: Result[str, str]) -> None:
    match value:
        Ok(text) => println(text)
        Err(error) => println(error)

def unit_result() -> Result[None, str]:
    return Ok(None)

def failure_result() -> Result[int, Failure]:
    return Err(Failure.Bad)

def value(good: bool) -> Result[int, str]:
    if good:
        return Ok(7)
    return Err("failure")

def doubled(good: bool) -> Result[int, str]:
    number = value(good)?
    return Ok(number * 2)

def selected(option: Option[int]) -> int:
    match option:
        Some(number) => return number
        None => return -1

def main() -> None:
    show_result(Ok("parameter"))
    println(selected(Some(5)))
    println(selected(None))
    match optional_text():
        Some(text) => println(text)
        None => println("missing")
    match optional_list():
        Some(values) => println(values[1])
        None => println(0)
    match unit_result():
        Ok(_) => println("unit")
        Err(error) => println(error)
    match failure_result():
        Ok(number) => println(number)
        Err(_) => println("bad")
    option: Option[int] = Some(9)
    missing: Option[int] = None
    println(option.unwrap_or(0))
    println(missing.unwrap_or(4))
    println(option)
    for_good = doubled(true)
    for_bad = doubled(false)
    println(for_good.unwrap_or(0))
    println(for_bad.unwrap_or(3))
    match for_bad:
        Ok(number) => println(number)
        Err(error) => println(error)
"#,
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting, "release")?;
    let native = root.join("native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("release"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "Option/Result native compilation",
    );
    let legacy = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy)
            .output()?,
        "Option/Result legacy compilation",
    );
    let expected = Command::new(legacy.join("oven/release/option_result")).output()?;
    let actual = Command::new(native).output()?;
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stderr, expected.stderr);
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(
        actual.stdout,
        b"parameter\n5\n-1\ntext\n3\nunit\nbad\n9\n4\nSome(9)\n14\n3\nfailure\n"
    );
    Ok(())
}

/// Measure every behavior fixture only when explicitly requested.
#[test]
#[ignore = "explicit full direct-route census"]
fn direct_route_fixture_census() -> Result<(), Box<dyn std::error::Error>> {
    census::run()
}

/// Import-activated async vocabulary must check through the CLI session before the lowering refuses async bodies.
fn check_async_frontend_refusal(
    driver: &Path,
    root: &Path,
    sysroot: &Path,
    closure: &corpus::NativeClosure,
) -> Result<(), Box<dyn std::error::Error>> {
    let source = root.join("async_frontend.incn");
    fs::write(
        &source,
        "import std.async\n\nasync def value() -> int:\n    return 1\n\nasync def main() -> None:\n    result = await value()\n    println(result)\n",
    )?;
    success(
        &support::repo_command().arg("check").arg(&source).output()?,
        "legacy async checking",
    );
    let output = corpus::source_command(driver, &source, &root.join("async-native"), sysroot, closure).output()?;
    assert!(!output.status.success());
    let diagnostic = String::from_utf8(output.stderr)?;
    assert!(diagnostic.contains("unsupported Body IR async Body"), "{diagnostic}");
    Ok(())
}

/// Prove named construction, field mutation, argument passing, returned models, and final drops against legacy.
#[test]
fn plain_model_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_plain_model(
        &fixture.driver_binary("release"),
        &fixture.scratch("plain-model")?,
        &fixture.sysroot,
        &fixture.formatting,
    )
}

/// Prove list indexing, mutation, shared parameters, owned returns and iteration against legacy.
#[test]
fn direct_route_lists_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_lists(
        &fixture.driver_binary("release"),
        &fixture.scratch("lists")?,
        &fixture.sysroot,
        &fixture.formatting,
    )
}

/// Compare scalar tuple list construction, indexed reads, and copies byte-for-byte with legacy.
#[test]
fn direct_route_tuple_lists_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("tuple-lists")?;
    let source = root.join("tuple_lists.incn");
    fs::write(
        &source,
        r#"
def main() -> None:
    """Exercise copied tuple lists, iteration, and owned string fields."""
    mut pairs: list[tuple[int, int]] = [(1, 2), (3, 4)]
    pairs.append((5, 6))
    copied = pairs
    pair = copied[1]
    println(pair[0])
    println(pair[1])
    println(len(pairs))
    empty: list[tuple[int, int]] = []
    println(len(empty))
    for item in pairs:
        println(item[0] + item[1])
    texts: list[tuple[str, int]] = [("one", 1), ("two", 2)]
    text = texts[0]
    println(text[0])
    for item in texts:
        println(item[0])
"#,
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting, "release")?;
    let native = root.join("tuple-lists-native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("release"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native tuple lists compilation",
    );
    let legacy_root = root.join("tuple-lists-legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy tuple lists compilation",
    );
    let expected = Command::new(legacy_root.join("oven/release/tuple_lists")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy tuple lists execution");
    success(&actual, "native tuple lists execution");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stdout, b"3\n4\n3\n0\n3\n7\n11\none\none\ntwo\n");
    Ok(())
}

/// Compare model list construction, copied reads, and iteration byte-for-byte with legacy.
#[test]
fn direct_route_model_lists_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("model-lists")?;
    let source = root.join("model_lists.incn");
    fs::write(
        &source,
        r#"
model Entry:
    value: int
    text: str

def main() -> None:
    """Exercise source-local model leaves and owned text through list copies."""
    mut entries = [Entry(value=1, text="one"), Entry(value=2, text="two")]
    entries.append(Entry(value=3, text="three"))
    copied = entries
    entry = copied[1]
    println(entry.value)
    println(entry.text)
    println(len(entries))
    for item in entries:
        println(item.value)
        println(item.text)
"#,
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting, "release")?;
    let native = root.join("model-lists-native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("release"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native model lists compilation",
    );
    let legacy_root = root.join("model-lists-legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy model lists compilation",
    );
    let expected = Command::new(legacy_root.join("oven/release/model_lists")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy model lists execution");
    success(&actual, "native model lists execution");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stdout, b"2\ntwo\n3\n1\none\n2\ntwo\n3\nthree\n");
    Ok(())
}

/// Compare exact byte literals, copied byte vectors, and borrowed parameters with legacy.
#[test]
fn direct_route_bytes_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("bytes")?;
    let source = root.join("bytes.incn");
    fs::write(
        &source,
        r#"
def count(value: bytes) -> int:
    """Observe borrowed byte storage without consuming the caller's vector."""
    return len(value)

def main() -> None:
    """Exercise empty and unsigned byte literals and independent copied storage."""
    values = b"\x00\x7f\x80\xff"
    copied = values
    println(count(values))
    println(count(b""))
    println(values[0])
    println(copied[1])
    println(values[2])
    println(copied[3])
"#,
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting, "release")?;
    let native = root.join("bytes-native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("release"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native bytes compilation",
    );
    let legacy_root = root.join("bytes-legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy bytes compilation",
    );
    let expected = Command::new(legacy_root.join("oven/release/bytes")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy bytes execution");
    success(&actual, "native bytes execution");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stdout, b"4\n0\n0\n127\n128\n255\n");
    Ok(())
}

/// Compare unit and scalar tuple hash-key membership and insertion against legacy, including string ownership.
#[test]
fn direct_route_hash_leaves_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("hash-leaves")?;
    let source = root.join("hash_leaves.incn");
    fs::write(
        &source,
        r#"
def unit_value() -> None:
    """Return a concrete unit hash key."""
    pass

def main() -> None:
    """Exercise unit keys, tuple keys, duplicate elimination, and owned text."""
    mut units = {unit_value()}
    units.add(unit_value())
    println(len(units))
    println(unit_value() in units)
    mut rows: dict[tuple[str, int], int] = {("key", 1): 7}
    println(("key", 1) in rows)
    rows.insert(("other", 2), 9)
    println(len(rows))
    keys = {("left", 1), ("right", 2)}
    println(("left", 1) in keys)
"#,
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting, "release")?;
    let native = root.join("hash-leaves-native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("release"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native hash leaves compilation",
    );
    let legacy_root = root.join("hash-leaves-legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy hash leaves compilation",
    );
    let expected = Command::new(legacy_root.join("oven/release/hash_leaves")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy hash leaves execution");
    success(&actual, "native hash leaves execution");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stdout, b"1\ntrue\ntrue\n2\ntrue\n");
    Ok(())
}

/// Compare exact decimal scale, function boundaries, numeric comparisons, and hashed duplicate elimination.
#[test]
fn direct_route_decimals_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("decimals")?;
    let source = root.join("decimals.incn");
    fs::write(
        &source,
        r#"
def identity(value: decimal[38, 2]) -> decimal[38, 2]:
    """Retain the written scale across a function boundary."""
    println(value)
    return value

def main() -> None:
    """Observe exact digits, scale, numeric ordering, and collection hashing."""
    a: decimal[4, 2] = 1.50d
    b: decimal[3, 1] = 1.5d
    c: decimal[4, 2] = 1.49d
    println(a == b)
    println(a != b)
    println(c < b)
    println(c <= b)
    println(b > c)
    println(b >= c)
    println(len({a, b}))
    println(identity(19.90d))
    large: decimal[38, 2] = 123456789012345678901234567890123456.78d
    println(identity(large))
    values = [a, b]
    println(values[0])
    println(values[1])
"#,
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting, "release")?;
    let native = root.join("decimals-native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("release"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native decimal compilation",
    );
    let legacy_root = root.join("decimals-legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy decimal compilation",
    );
    let expected = Command::new(legacy_root.join("oven/release/decimals")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy decimal execution");
    success(&actual, "native decimal execution");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stdout, b"true\nfalse\ntrue\ntrue\ntrue\ntrue\n1\n19.90\n19.90\n123456789012345678901234567890123456.78\n123456789012345678901234567890123456.78\n1.50\n1.5\n");
    Ok(())
}

/// Prove class construction, shared and mutable receivers, and passing classes against legacy.
#[test]
fn source_class_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_source_class(
        &fixture.driver_binary("release"),
        &fixture.scratch("source-class")?,
        &fixture.sysroot,
        &fixture.formatting,
    )
}

/// Prove canonical scalar casts and default values byte-identical to legacy.
#[test]
fn numeric_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_numerics(
        &fixture.driver_binary("release"),
        &fixture.scratch("numerics")?,
        &fixture.sysroot,
        &fixture.formatting,
    )
}

/// Prove concrete class and model trait methods and a static trait default against legacy.
#[test]
fn source_trait_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_source_trait(
        &fixture.driver_binary("release"),
        &fixture.scratch("source-trait")?,
        &fixture.sysroot,
        &fixture.formatting,
    )
}

/// Compare hashed collection literals, mutation, membership, and indexed reads with legacy.
#[test]
fn direct_route_collections_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_collections(
        &fixture.driver_binary("release"),
        &fixture.scratch("collections")?,
        &fixture.sysroot,
        &fixture.formatting,
    )?;
    Ok(())
}

/// Prove unit and payload construction, enum passing/returning, and variant-bound match output against legacy.
#[test]
fn source_enum_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("source-enum")?;
    let source = root.join("source_enum.incn");
    fs::write(
        &source,
        r#"enum Signal:
    Ready
    Waiting

enum Shape:
    Empty
    Circle(int)
    Rectangle(int, int)
    Label(str)

def make_shape(size: int) -> Shape:
    return Shape.Circle(size)

def area(shape: Shape) -> int:
    match shape:
        Shape.Empty => return 0
        Shape.Circle(radius) => return radius * radius
        Shape.Rectangle(width, height) => return width * height
        Shape.Label(_) => return -1

def label(shape: Shape) -> str:
    match shape:
        Shape.Label(text) => return text
        _ => return "unlabeled"

def signal_value(signal: Signal) -> int:
    match signal:
        Signal.Ready => return 1
        Signal.Waiting => return 2

def main() -> None:
    shape = make_shape(7)
    println(area(shape))
    println(area(shape))
    println(area(Shape.Empty))
    println(area(Shape.Rectangle(3, 5)))
    println(signal_value(Signal.Ready))
    println(signal_value(Signal.Waiting))
    text = Shape.Label("payload")
    println(label(text))
    println(label(text))
"#,
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting, "release")?;
    let native = root.join("native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("release"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "source enum native compilation",
    );
    let legacy = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy)
            .output()?,
        "source enum legacy compilation",
    );
    let expected = Command::new(legacy.join("oven/release/source_enum")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "source enum legacy execution");
    success(&actual, "source enum native execution");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stdout, b"49\n49\n0\n15\n1\n2\npayload\npayload\n");
    Ok(())
}

/// Newtypes, erased aliases, scalar constants, and persistent scalar statics retain exactly the legacy output.
#[test]
fn declarations_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_declarations(
        &fixture.driver_binary("release"),
        &fixture.scratch("declarations")?,
        &fixture.sysroot,
        &fixture.formatting,
    )
}
/// Scalar and string defaults execute at omitted calls, while supplied arguments bypass them.
#[test]
fn direct_route_defaults_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "defaults",
        "def compute() -> int:\n    println(100)\n    return 7\n\ndef choose(value: int = compute()) -> int:\n    return value\n\ndef flag(value: bool = true) -> bool:\n    return value\n\ndef fraction(value: float = 2.5) -> float:\n    return value\n\ndef number(value: int = 2 + 3) -> int:\n    return value\n\ndef greeting() -> str:\n    prefix = \"hello\"\n    return prefix + \"!\"\n\ndef text(value: str = greeting()) -> str:\n    return value\n\ndef literal_text(value: str = \"literal\") -> str:\n    return value\n\ndef main() -> None:\n    println(choose(9))\n    println(choose())\n    println(flag())\n    println(fraction())\n    println(number())\n    println(number(9))\n    println(text())\n    println(text(\"supplied\"))\n    println(literal_text())\n",
        None,
    )?;
    check_declaration_case(
        "defaults_with_caller_collections",
        "class DefaultBox:\n    value: int\n    def get(self) -> int:\n        return self.value\n\ndef compute() -> int:\n    return 7\n\ndef method_number(value: int = DefaultBox(value=5).get()) -> int:\n    return value\n\ndef number(value: int = compute() + 2) -> int:\n    return value\n\ndef main() -> None:\n    first = {1}\n    second = {2}\n    println(number())\n    println(method_number())\n    println(len(first) + len(second))\n",
        None,
    )
}

/// Condition assertions preserve successful output, failure payloads, and exit codes.
#[test]
fn direct_route_assertions_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    for (name, source, message) in [
        (
            "assert_pass",
            "def message() -> str:\n    println(99)\n    return \"message\"\n\ndef main() -> None:\n    assert true, message()\n    assert 3 > 2, \"unused\"\n    println(42)\n",
            None,
        ),
        (
            "assert_fail",
            "def main() -> None:\n    assert false\n",
            Some("AssertionError"),
        ),
        (
            "assert_message",
            "def main() -> None:\n    assert false, \"failed check\"\n",
            Some("AssertionError: failed check"),
        ),
        (
            "assert_empty",
            "def main() -> None:\n    assert false, \"\"\n",
            Some("AssertionError"),
        ),
    ] {
        check_declaration_case(name, source, message)?;
    }
    Ok(())
}

/// Compare complete output streams and exit codes, including the canonical panic payload without Rust's wrapper.
fn check_declaration_case(
    name: &str,
    text: &str,
    panic_message: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch(name)?;
    let source = root.join(format!("{name}.incn"));
    fs::write(&source, text)?;
    let native = root.join("native");
    let closure = corpus::runtime_closure(&fixture.formatting, "release")?;
    success(
        &corpus::source_command(
            &fixture.driver_binary("release"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native declaration compilation",
    );
    let legacy_root = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy declaration compilation",
    );
    let expected = Command::new(legacy_root.join("oven/release").join(name)).output()?;
    let actual = Command::new(native).output()?;
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
    assert_eq!(actual.status.code(), expected.status.code());
    if let Some(message) = panic_message {
        let expected_error = String::from_utf8_lossy(&expected.stderr);
        let actual_error = String::from_utf8_lossy(&actual.stderr);
        assert_eq!(
            expected_error.lines().find(|line| line.starts_with("AssertionError")),
            Some(message),
            "{expected_error}"
        );
        assert_eq!(
            actual_error.lines().find(|line| line.starts_with("AssertionError")),
            Some(message),
            "{actual_error}"
        );
    } else {
        success(&actual, "declaration execution");
    }
    Ok(())
}

/// Prove union injection at assignment, argument and return boundaries against legacy output.
#[test]
fn source_union_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("source-union")?;
    let source = root.join("source_union.incn");
    fs::write(
        &source,
        r#"def choose(flag: bool) -> int | str:
    if flag:
        return 42
    return "payload"

def classify(value: int | str) -> int:
    if isinstance(value, int):
        return 1
    return 2

def narrowed(value: int | str) -> int:
    if isinstance(value, int):
        return value + 1
    return -1

def captured(value: int | str) -> str:
    match value:
        int(_) => return "number"
        str(text) => return text

def defaulted(value: int | str, step: int = 4) -> int:
    return classify(value) + step

def main() -> None:
    value: int | str = 7
    println(classify(value))
    println(classify(8))
    println(classify("text"))
    println(classify(choose(true)))
    println(classify(choose(false)))
    println(narrowed(41))
    println(narrowed("text"))
    println(captured(7))
    println(captured("payload"))
    println(defaulted(7))
    println(defaulted("text", 8))
"#,
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting, "release")?;
    let native = root.join("native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("release"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "source union native compilation",
    );
    let legacy = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy)
            .output()?,
        "source union legacy compilation",
    );
    let expected = Command::new(legacy.join("oven/release/source_union")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "source union legacy execution");
    success(&actual, "source union native execution");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stdout, b"1\n1\n2\n1\n2\n42\n-1\nnumber\npayload\n5\n10\n");

    // ---- Unadmitted payload types stay a named refusal ----
    let refused_source = root.join("unadmitted_union.incn");
    fs::write(
        &refused_source,
        "def accept(value: int | list[int]) -> None:\n    pass\n\ndef main() -> None:\n    accept(7)\n",
    )?;
    let refused_binary = root.join("refused-union");
    let refusal = corpus::source_command(
        &fixture.driver_binary("release"),
        &refused_source,
        &refused_binary,
        &fixture.sysroot,
        &closure,
    )
    .output()?;
    assert!(!refusal.status.success());
    assert!(
        String::from_utf8_lossy(&refusal.stderr).contains("List[int]"),
        "{}",
        String::from_utf8_lossy(&refusal.stderr)
    );
    assert!(!refused_binary.exists());
    Ok(())
}

/// Prove admitted string methods and Unicode lengths against legacy output.
#[test]
fn direct_route_string_methods_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_string_methods(
        &fixture.driver_binary("release"),
        &fixture.scratch("string-methods")?,
        &fixture.sysroot,
        &fixture.formatting,
    )
}

/// Compare booleans with legacy, including evaluation order and retained owners.
#[test]
fn direct_route_booleans_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("release"),
        &fixture.scratch("booleans")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def probe(label: str, value: bool) -> bool:
    println(label)
    return value

def main() -> None:
    println(false and probe("skipped-and", true))
    println(true or probe("skipped-or", false))
    println(true and probe("selected-and", true))
    println(false or probe("selected-or", true))
    println(not false)
"#,
    )
}

/// Compare builtins with legacy, including evaluation order and retained owners.
#[test]
fn direct_route_builtins_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("release"),
        &fixture.scratch("builtins")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def main() -> None:
    values = [3, -2, 1]
    ordered = sorted(values)
    println(ordered[0])
    println(ordered[2])
    println(sum(values))
    println(abs(-12))
    println(2 ** 10)
    println(2.0 ** 3.0)
    small: f32 = 1.1
    exponent: f32 = 2.0
    println(small ** exponent)
    println(bool(0))
    println(bool(-2))
    println(bool(0.0))
    println(bool(""))
    println(bool("x"))
    println(bool(values))
    merged = values + [8, 9]
    println(len(merged))
    println(len(values))
"#,
    )
}

/// A helper re-export and a qualified call retain their canonical bodies and byte-identical legacy output.
#[test]
fn direct_route_modules_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("modules")?;
    let source = root.join("src/main.incn");
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        &source,
        "from facade import exported as compute\nimport helper\n\ndef calculate(a: int, b: int) -> int:\n  return a - b\n\ndef main() -> None:\n  println(compute(b=2, a=40))\n  println(helper.calculate(3, 4))\n  println(calculate(9, 2))\n",
    )?;
    fs::write(
        root.join("src/helper.incn"),
        "def hidden(a: int) -> int:\n  return a\n\npub def calculate(a: int, b: int) -> int:\n  return hidden(a) + b\n",
    )?;
    fs::create_dir_all(root.join("src/facade"))?;
    fs::write(
        root.join("src/facade/__init__.incn"),
        "pub from helper import calculate as exported\n",
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting, "release")?;
    let native = root.join("native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("release"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native three-module compilation",
    );
    let legacy_root = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy three-module compilation",
    );
    let expected = Command::new(legacy_root.join("oven/release/main")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy three-module execution");
    success(&actual, "native three-module execution");
    assert_eq!(actual.stdout, b"42\n7\n7\n");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
    assert_eq!(actual.status.code(), expected.status.code());
    Ok(())
}

/// Prove tuple construction, typed signatures, constant projections, and simultaneous unpacking against legacy.
#[test]
fn tuple_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("tuples")?;
    let source = root.join("tuples.incn");
    fs::write(
        &source,
        r#"model Boxed:
    value: int

def swap(value: tuple[int, int]) -> tuple[int, int]:
    return (value[1], value[0])

def identity(value: tuple[int, str]) -> tuple[int, str]:
    return value

def main() -> None:
    mut a = 3
    mut b = 7
    a, b = b, a
    pair: tuple[int, int] = swap((a, b))
    x, y = pair
    println(x)
    println(y)
    println(pair)
    text_pair = identity((9, "hello\n\"tuple\""))
    number, text = text_pair
    println(text_pair)
    println(number)
    println(text)
    println(text_pair[-1])
    println((true, 1.5))
    mut boxed = Boxed(value=0)
    boxed.value, a = pair
    println(boxed.value)
    println(a)
"#,
    )?;
    let native = root.join("native");
    let closure = corpus::runtime_closure(&fixture.formatting, "release")?;
    success(
        &corpus::source_command(
            &fixture.driver_binary("release"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native tuple compilation",
    );
    let legacy_root = root.join("legacy");
    success(
        &support::repo_command()
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy tuple compilation",
    );
    let expected = Command::new(legacy_root.join("oven/release/tuples")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy tuple execution");
    success(&actual, "native tuple execution");
    assert_eq!(actual.stdout, expected.stdout);
    assert!(actual.stdout.starts_with(b"3\n7\n(3, 7)\n"));
    for (name, source_text, kind) in [
        (
            "function_value",
            "def apply(value: (int) -> int) -> int:\n    return value(1)\n\ndef main() -> None:\n    println(1)\n",
            "Function",
        ),
        (
            "singleton",
            "def main() -> None:\n    println((42,))\n",
            "singleton Tuple",
        ),
    ] {
        let refused_source = root.join(format!("{name}.incn"));
        fs::write(&refused_source, source_text)?;
        let refused_output = root.join(format!("{name}-native"));
        let refused = corpus::source_command(
            &fixture.driver_binary("release"),
            &refused_source,
            &refused_output,
            &fixture.sysroot,
            &closure,
        )
        .output()?;
        assert!(!refused.status.success());
        assert!(
            String::from_utf8_lossy(&refused.stderr).contains(kind),
            "{}",
            String::from_utf8_lossy(&refused.stderr)
        );
        assert!(!refused_output.exists());
    }
    Ok(())
}

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

/// Start the pinned driver without the caller's dynamic-loader search paths.
///
/// The driver must load the `rustc_driver` its runpath names, and its startup check refuses any other copy. On Linux an
/// inherited `LD_LIBRARY_PATH` is searched before that runpath, and the compiler-suite runner exports one naming the
/// toolchain's `lib` directory to every libtest child, so an inheriting launch loads rustup's second, byte-identical
/// copy of the library from the rustc component and the check refuses it (#1755, #1785).
fn driver_command(driver: &Path) -> Command {
    let mut command = Command::new(driver);
    for name in ["LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "DYLD_FALLBACK_LIBRARY_PATH"] {
        command.env_remove(name);
    }
    command
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
    let ambient = driver_command(binary)
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
    let mismatch = driver_command(binary)
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
            let compile = driver_command(&binary)
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
    let mut command = driver_command(driver);
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

#[path = "native_driver_project_tests/tail.rs"]
mod tail;

/// Compile one source through both routes and compare successful execution bytes against a fixed oracle.
fn assert_native_legacy_bytes(name: &str, program: &str, stdout: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch(name)?;
    let source = root.join(format!("{name}.incn"));
    fs::write(&source, program)?;
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
        "native parity compilation",
    );
    let legacy = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy)
            .output()?,
        "legacy parity compilation",
    );
    let expected = Command::new(legacy.join("oven/release").join(name)).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy parity execution");
    success(&actual, "native parity execution");
    assert_eq!(actual.stdout, stdout);
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
    Ok(())
}

/// Nested empty list literals retain checker-proven element types and match legacy bytes.
#[test]
fn direct_route_nested_empty_lists_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    assert_native_legacy_bytes(
        "nested_empty_lists",
        "def main() -> None:\n    rows = [[], [1]]\n    println(len(rows))\n    println(rows[1][0])\n    deep = [[[]], [[2]]]\n    println(len(deep))\n    println(deep[1][0][0])\n",
        b"2\n1\n2\n2\n",
    )
}

/// Contextual None payloads construct nested intrinsic carriers with their proven types.
#[test]
fn direct_route_contextual_none_payloads_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    assert_native_legacy_bytes(
        "contextual_none_payloads",
        "def number(value: Result[Option[int], str]) -> int:\n    match value:\n        Ok(optional) => return optional.unwrap_or(0)\n        Err(_) => return -1\n\ndef main() -> None:\n    println(number(Ok(None)))\n    println(number(Ok(Some(7))))\n    println(number(Err(\"failure\")))\n",
        b"0\n7\n-1\n",
    )
}

/// Inferred bindings and in-place matches construct Results with the checker-settled open sides.
#[test]
fn direct_route_settled_result_sides_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    assert_native_legacy_bytes(
        "settled_result_sides",
        "def value() -> Result[int, str]:\n    inferred = Ok(9)\n    return inferred\n\ndef main() -> None:\n    inferred = Ok(7)\n    match inferred:\n        Ok(number) => println(number)\n        Err(_) => pass\n    match Ok(8):\n        Ok(number) => println(number)\n        Err(_) => pass\n    println(value().unwrap_or(0))\n",
        b"7\n8\n9\n",
    )
}

/// Compiler-generated collection writes in filtered comprehensions preserve canonical dispatch and legacy bytes.
#[test]
fn direct_route_comprehension_writes_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    assert_native_legacy_bytes(
        "comprehension_writes",
        "def main() -> None:\n    xs = [1, 2, 3, 4]\n    values = [x * x for x in xs if x > 2]\n    indexed = {x: x * x for x in xs if x % 2 == 0}\n    println(values[0])\n    println(values[1])\n    println(indexed[2])\n    println(indexed[4])\n    println(len(xs))\n",
        b"9\n16\n4\n16\n4\n",
    )
}

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

/// Compare stored function values, callable parameters, and capturing closure expressions with legacy execution.
#[test]
fn direct_route_function_values_and_closures_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("closures")?;
    check_function_item_values(fixture, &root)?;
    let source = root.join("closures.incn");
    fs::write(
        &source,
        concat!(
            "def double(value: int) -> int:\n    \"\"\"Double the argument.\"\"\"\n    return value * 2\n\n",
            "def apply(f: (int) -> int, value: int) -> int:\n    \"\"\"Invoke a function-typed argument.\"\"\"\n    return f(value)\n\n",
            "def apply_callable(f: Callable[int, int], value: int) -> int:\n    \"\"\"Invoke a Callable-typed argument.\"\"\"\n    return f(value)\n\n",
            "def make_adder(offset: int) -> (int) -> int:\n    \"\"\"Return a closure owning its offset.\"\"\"\n    return (value) => value + offset\n\n",
            "def scaled(value: int) -> int:\n    \"\"\"Box the same item as main does, from a second function.\"\"\"\n    return apply_callable(double, value)\n\n",
            "def main() -> None:\n    \"\"\"Exercise stored, borrowed, returned, and snapshot closures.\"\"\"\n",
            "    stored = double\n",
            "    println(stored(4))\n",
            "    println(apply(double, 5))\n",
            "    println(apply((value) => value + 1, 4))\n",
            "    println(apply_callable(double, 6))\n",
            "    println(scaled(7))\n",
            "    offset = 2\n",
            "    add: (int) -> int = (value) => value + offset\n",
            "    println(add(40))\n",
            "    println(apply(add, 39))\n",
            "    println(add(41))\n",
            "    returned = make_adder(3)\n",
            "    println(returned(40))\n",
            "    mut bias = 2\n",
            "    snapshot: (int) -> int = (value) => value + bias\n",
            "    bias = 5\n",
            "    println(snapshot(40))\n",
            "    println(bias)\n",
        ),
    )?;
    let legacy_root = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy function values and closure expressions compilation",
    );
    let legacy = Command::new(legacy_root.join("oven/release/closures")).output()?;
    success(&legacy, "legacy function values and closure expressions execution");
    assert_eq!(legacy.stdout, b"8\n10\n5\n12\n14\n42\n41\n43\n43\n42\n5\n");
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
        "native function values and closure expressions compilation",
    );
    let actual = Command::new(native).output()?;
    success(&actual, "native function values and closure expressions execution");
    assert_eq!(actual.stdout, legacy.stdout);
    Ok(())
}

/// A closure expression over a sized carrier keeps that carrier's native arithmetic through its lifted function,
/// pointer reification, and indirect calls: `100000 * 100000` wraps in `i32` exactly as legacy's release build does,
/// where an `i64` slip would print `10000000000`.
#[test]
fn direct_route_sized_numeric_closures_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("sized-closures")?;
    let source = root.join("sized_closures.incn");
    fs::write(
        &source,
        concat!(
            "def main() -> None:\n",
            "    square: (i32) -> i32 = (value) => value * value\n",
            "    println(square(100000))\n",
            "    cube: (i32) -> i32 = (value) => value * value * value\n",
            "    println(cube(2000))\n",
            "    println(cube(-7))\n",
        ),
    )?;
    let legacy_root = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy sized-numeric closure compilation",
    );
    let legacy = Command::new(legacy_root.join("oven/release/sized_closures")).output()?;
    success(&legacy, "legacy sized-numeric closure execution");
    assert_eq!(legacy.stdout, b"1410065408\n-589934592\n-343\n");
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
        "native sized-numeric closure compilation",
    );
    let actual = Command::new(native).output()?;
    success(&actual, "native sized-numeric closure execution");
    assert_eq!(actual.stdout, legacy.stdout);
    Ok(())
}

/// Prove stored items, aliases, explicit pointer coercions, returned pointers, and indirect invocation separately
/// before the same focused test exercises closure-holding contracts.
fn check_function_item_values(fixture: &DriverFixture, root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let source = root.join("function_items.incn");
    fs::write(
        &source,
        concat!(
            "def double(value: int) -> int:\n    return value * 2\n\n",
            "def pointer() -> (int) -> int:\n    return double\n\n",
            "def echo(value: str) -> str:\n    return value\n\n",
            "def main() -> None:\n",
            "    stored = double\n    alias = stored\n    typed: (int) -> int = double\n",
            "    println(alias(4))\n    println(typed(5))\n",
            "    returned = pointer()\n    println(returned(6))\n",
            "    stored_echo = echo\n    println(stored_echo(\"hello\"))\n",
        ),
    )?;
    let legacy_root = root.join("function_items_legacy");
    success(
        &support::repo_command()
            .current_dir(root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy function-item compilation",
    );
    let legacy = Command::new(legacy_root.join("oven/release/function_items")).output()?;
    success(&legacy, "legacy function-item execution");
    assert_eq!(legacy.stdout, b"8\n10\n12\nhello\n");
    let closure = corpus::runtime_closure(&fixture.formatting, "release")?;
    let native = root.join("function_items_native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("release"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native function-item compilation",
    );
    let actual = Command::new(native).output()?;
    success(&actual, "native function-item execution");
    assert_eq!(actual.stdout, legacy.stdout);
    eprintln!("stored function items and pointers matched legacy byte for byte");
    Ok(())
}

/// Measure every behavior fixture only when explicitly requested.
#[test]
#[ignore = "explicit full direct-route census"]
fn direct_route_fixture_census() -> Result<(), Box<dyn std::error::Error>> {
    census::run()
}

/// Import-activated async vocabulary checks before pending runtime operations receive a named direct-route refusal.
fn check_async_frontend_refusal(
    driver: &Path,
    root: &Path,
    sysroot: &Path,
    closure: &corpus::NativeClosure,
) -> Result<(), Box<dyn std::error::Error>> {
    let source = root.join("async_frontend.incn");
    fs::write(
        &source,
        "import std.async\nfrom std.async.time import sleep_ms\n\nasync def main() -> None:\n    await sleep_ms(1)\n",
    )?;
    success(
        &support::repo_command().arg("check").arg(&source).output()?,
        "legacy async checking",
    );
    let output = corpus::source_command(driver, &source, &root.join("async-native"), sysroot, closure).output()?;
    assert!(!output.status.success());
    let diagnostic = String::from_utf8(output.stderr)?;
    assert!(diagnostic.contains("unsupported Body IR"), "{diagnostic}");
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

/// Prove two checked instantiations of a function with type parameters against legacy output.
#[test]
fn type_parameter_function_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("release"),
        &fixture.scratch("type-parameter-function")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def identity[T](value: T) -> T:
    return value

def forward[T](value: T) -> T:
    copied = value
    return identity[T](value)

def main() -> None:
    println(identity[int](42))
    println(identity[bool](true))
    println(identity(7))
    println(forward[int](9))
    println(identity[str]("text"))
"#,
    )
}

/// Prove closed class layouts and methods with type parameters against legacy output.
#[test]
fn type_parameter_class_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("release"),
        &fixture.scratch("type-parameter-class")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def identity[T](value: T) -> T:
    return value

class Box[T]:
    pub value: T

    def get(self) -> T:
        return self.value

    def forward(self) -> T:
        return identity[T](self.value)

    def pick[U](self, value: U) -> U:
        return value

class Picker:
    def pick[T](self, value: T) -> T:
        return value

def read_box(value: Box[int]) -> int:
    return value.get()

def main() -> None:
    whole = Box[int](value=42)
    flag = Box[bool](value=true)
    text = Box[str](value="stored")
    println(whole.get())
    println(read_box(whole))
    println(text.forward())
    println(flag.get())
    println(text.get())
    println(whole.pick[int](7))
    println(flag.pick[int](8))
    picker = Picker()
    println(picker.pick[int](9))
    println(picker.pick(true))
    println(picker.pick[str]("picked"))
"#,
    )
}

/// Prove carriers retained only inside closed function instances, including forwarded calls, against legacy.
#[test]
fn type_parameter_carrier_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("release"),
        &fixture.scratch("type-parameter-carrier")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def show[T with Display](value: T) -> None:
    stored: Option[T] = Some(value)
    println(stored.unwrap_or(value))

def forward[T with Display](value: T) -> None:
    show[T](value)

def identity[T](value: Option[T]) -> Option[T]:
    return value

def main() -> None:
    forward[int](7)
    forward[str]("text")
    println(identity[int](Some(9)).unwrap_or(0))
"#,
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

/// Compare frozen static text, UTF-8 length, copied calls, string conversion, fields, and collection leaves.
#[test]
fn direct_route_frozen_strings_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("frozen-strings")?;
    let source = root.join("frozen_strings.incn");
    fs::write(
        &source,
        r#"
model Holder:
    label: FrozenStr

def keep(value: FrozenStr) -> FrozenStr:
    """Keep the static carrier across copied calls."""
    return value

def policy() -> FrozenStr:
    """Return a frozen Unicode literal."""
    return "é😀"

def label(text: str) -> str:
    """Accept owned text at an explicit string boundary."""
    return f"[{text}]"

def main() -> None:
    """Observe frozen carriers without replacing their storage with owned strings."""
    println(policy())
    println(len(policy()))
    println(keep("strict"))
    held = Holder(label="held")
    println(held.label)
    println(label(held.label))
    values: list[FrozenStr] = ["left", "right"]
    copied = values
    println(copied[0])
    println(copied[1])
    words: dict[str, FrozenStr] = {"k": "value"}
    println(words["k"])
"#,
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting, "release")?;
    let native = root.join("frozen-strings-native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("release"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native frozen strings compilation",
    );
    let legacy_root = root.join("frozen-strings-legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy frozen strings compilation",
    );
    let expected = Command::new(legacy_root.join("oven/release/frozen_strings")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy frozen strings execution");
    success(&actual, "native frozen strings execution");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(
        actual.stdout,
        "é😀\n2\nstrict\nheld\n[held]\nleft\nright\nvalue\n".as_bytes()
    );
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

/// Compare deferred creation, ordered yields, collection, mutable polling, and exhaustion with legacy.
#[test]
fn direct_route_generators_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("generators")?;
    let source = root.join("generators.incn");
    fs::write(
        &source,
        r#"def mark(value: int) -> int:
    println(value)
    return value

def parameterized(start: int, end: int) -> Generator[int]:
    println(start)
    for value in range(start, end):
        yield value

def labels(prefix: str) -> Generator[str]:
    println(prefix)
    yield prefix

def counter() -> Generator[int]:
    println(10)
    for value in range(1, 3):
        yield value
    yield 3

def counter__producer() -> int:
    return 40

def first(mut values: Generator[int]) -> int:
    for value in values:
        return value
    return -1

def outer(mut values: Generator[int]) -> int:
    return first(values)

def main() -> None:
    unused = counter()
    values = counter()
    println(20)
    collected = values.collect()
    println(collected[0])
    println(collected[1])
    println(collected[2])
    mut remaining = counter()
    println(first(remaining))
    println(outer(remaining))
    println(first(remaining))
    println(first(remaining))
    println(counter__producer())
    unused_parameters = parameterized(90, 92)
    pending = parameterized(mark(4), mark(6))
    println(50)
    for value in pending:
        println(value)
    mut prefix = "captured"
    texts = labels(prefix)
    prefix = "changed"
    println(prefix)
    println(60)
    for text in texts:
        println(text)
"#,
    )?;
    let native = root.join("native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("release"),
            &source,
            &native,
            &fixture.sysroot,
            &corpus::runtime_closure(&fixture.formatting, "release")?,
        )
        .output()?,
        "generator native compilation",
    );
    let legacy = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy)
            .output()?,
        "generator legacy compilation",
    );
    let expected = Command::new(legacy.join("oven/release/generators")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "generator legacy execution");
    success(&actual, "generator native execution");
    assert_eq!(
        actual.stdout, expected.stdout,
        "generator output must be byte-identical"
    );
    assert_eq!(
        actual.stderr, expected.stderr,
        "generator stderr must be byte-identical"
    );
    assert_eq!(
        actual.stdout,
        b"20\n10\n1\n2\n3\n10\n1\n2\n3\n-1\n40\n4\n6\n50\n4\n4\n5\nchanged\n60\ncaptured\ncaptured\n"
    );
    let named = root.join("named-generator.incn");
    fs::write(
        &named,
        "def mark(value: int) -> int:\n    println(value)\n    return value\n\ndef values(first: int, second: int) -> Generator[int]:\n    yield first\n    yield second\n\ndef main() -> None:\n    pending = values(second=mark(2), first=mark(1))\n    for value in pending:\n        println(value)\n",
    )?;
    let rejected = corpus::source_command(
        &fixture.driver_binary("release"),
        &named,
        &root.join("named-native"),
        &fixture.sysroot,
        &corpus::runtime_closure(&fixture.formatting, "release")?,
    )
    .output()?;
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("unsupported Body IR reordered generator ArgumentBinding")
    );
    Ok(())
}

/// Compare checked integer and float widening at the generator yield boundary with legacy.
#[test]
fn direct_route_generator_numeric_yields_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("release"),
        &fixture.scratch("generator-numeric-yields")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def integers(value: i8) -> Generator[int]:
    """Yield a narrow integer through the checked int destination."""
    yield value

def floats(value: f32) -> Generator[float]:
    """Yield a narrow float through the checked float destination."""
    yield value

def main() -> None:
    """Observe both numeric generator yield conversions."""
    small: i8 = 7
    single: f32 = 1.5
    for value in integers(small):
        println(value)
    for value in floats(single):
        println(value)
"#,
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

/// Ordinary newtype builders and receiver methods preserve their wrapped values and legacy output.
#[test]
fn newtype_methods_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("newtype-methods")?;
    let source = root.join("newtype_methods.incn");
    fs::write(
        &source,
        r#"trait Doubled:
    """Expose a shared read of a wrapped count."""

    def doubled(self) -> int

type Count = newtype int with Doubled:
    """Wrap a count without introducing checked construction."""

    def create(value: int) -> Count:
        """Construct through an ordinary associated builder."""
        return Count(value)

    def doubled(self) -> int:
        """Read tuple slot zero through a nominal receiver."""
        return self.0 * 2

type Message = newtype str:
    """Keep owned text behind a nominal wrapper."""

    def create(value: str) -> Message:
        """Wrap the supplied text through an associated method."""
        return Message(value)

    def text(self) -> str:
        """Read the wrapped text without consuming the receiver."""
        return self.0

def main() -> None:
    """Exercise builders, scalar projection, and repeated text reads."""
    count = Count.create(21)
    println(count.doubled())
    println(count.0)
    message = Message.create("wrapped")
    println(message.text())
    println(message.text())
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
        "newtype methods native compilation",
    );
    let legacy = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy)
            .output()?,
        "newtype methods legacy compilation",
    );
    let expected = Command::new(legacy.join("oven/release/newtype_methods")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "newtype methods legacy execution");
    success(&actual, "newtype methods native execution");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stdout, b"42\n21\nwrapped\nwrapped\n");
    let checked_source = root.join("checked_newtype.incn");
    fs::write(
        &checked_source,
        "type Positive = newtype int:\n    def from_underlying(value: int) -> Result[Self, ValidationError]:\n        if value <= 0:\n            return Err(ValidationError(\"must be positive\"))\n        return Ok(Positive(value))\n\ndef main() -> None:\n    value = Positive(1)\n",
    )?;
    let checked_output = root.join("checked-native");
    let refused = corpus::source_command(
        &fixture.driver_binary("release"),
        &checked_source,
        &checked_output,
        &fixture.sysroot,
        &closure,
    )
    .output()?;
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("unsupported source checked Newtype construction"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!checked_output.exists());
    Ok(())
}

/// Fieldless enum equality and inequality borrow their values and compare exactly like the legacy route.
#[test]
fn fieldless_enum_comparisons_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "fieldless_enum_comparisons",
        r#"enum Signal:
    Ready
    Waiting

enum HttpStatus(int):
    Ok = 200
    NotFound = 404

def same(left: Signal, right: Signal) -> bool:
    return left == right

def main() -> None:
    signal = Signal.Ready
    println(same(signal, Signal.Ready))
    println(signal == Signal.Waiting)
    println(signal != Signal.Waiting)
    println(signal != Signal.Ready)
    println(same(signal, signal))
    println(HttpStatus.Ok == HttpStatus.NotFound)
    println(HttpStatus.Ok != HttpStatus.NotFound)
    println(HttpStatus.Ok == HttpStatus.Ok)
"#,
        None,
    )?;
    let fixture = driver_fixture()?;
    let root = fixture.scratch("payload-enum-comparison-refusal")?;
    let source = root.join("payload.incn");
    fs::write(
        &source,
        "enum Shape:\n    Square(int)\n    Empty\n\ndef main() -> None:\n    println(Shape.Square(2) == Shape.Square(3))\n",
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting, "release")?;
    let binary = root.join("native");
    let refused = corpus::source_command(
        &fixture.driver_binary("release"),
        &source,
        &binary,
        &fixture.sysroot,
        &closure,
    )
    .output()?;
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("unsupported Body IR payload enum comparison"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!binary.exists());
    Ok(())
}

/// Integer and string value enums retain canonical construction, parameter passing, and repeated scalar extraction.
#[test]
fn value_enum_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "value_enum_getters",
        r#"enum HttpStatus(int):
    Ok = 200
    NotFound = 404

enum Env(str):
    Dev = "development"
    Prod = "production"

def status_code(status: HttpStatus) -> int:
    return status.value()

def environment_name(environment: Env) -> str:
    return environment.value()

def main() -> None:
    status = HttpStatus.NotFound
    println(status_code(status))
    println(status.value())
    println(HttpStatus.Ok.value())
    environment = Env.Prod
    println(environment_name(environment))
    println(environment.value())
    println(Env.Dev.value())
"#,
        None,
    )
}

/// String static reads preserve live aliases, detached bindings, assignments, and returned snapshots.
#[test]
fn string_statics_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "string_statics",
        r#"static TEXT: str = "initial"

def read() -> str:
    return TEXT

def replace(value: str) -> None:
    TEXT = value

def main() -> None:
    first = read()
    live = TEXT
    mut changing = TEXT
    println(TEXT)
    println(read())
    replace("changed")
    println(live)
    println(changing)
    changing += "!"
    replace("final")
    println(live)
    println(changing)
    println(first)
    println(TEXT)
    println(read())
"#,
        None,
    )
}

/// Primitive list storage preserves live aliases, detached snapshots, and argument effects before mutations.
#[test]
fn list_statics_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case("list_statics", corpus::LIST_STATICS_SOURCE, None)
}

/// Primitive and tuple patterns preserve literal tests, source arm order, and owned binding snapshots.
#[test]
fn structural_matches_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case("structural_matches", corpus::STRUCTURAL_MATCHES_SOURCE, None)
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
/// Builtin model derives retain the legacy declaration and shared-argument cloning behavior.
#[test]
fn model_derives_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "model_derives",
        r#"@derive(Debug, Clone, Eq, Hash, Ord, Default)
model Point:
    x: int
    label: str

def copy(point: Point) -> Point:
    return point

def main() -> None:
    point = Point(x=3, label="derived\"\n")
    other = copy(point)
    later = Point(x=3, label="z")
    higher = Point(x=4, label="a")
    println(point.x)
    println(other.label)
    println(f"{point:?}")
    println(f"debug={point:?}")
    println(point == other)
    println(point != later)
    println(point < later)
    println(point <= other)
    println(higher > later)
    println(higher >= later)
    println(point.x)
"#,
        None,
    )
}

/// Field aliases share canonical storage across construction, reads, receiver methods, and writes.
#[test]
fn model_field_aliases_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "model_field_aliases",
        r#"model AliasRecord:
    value [alias="wire_value"]: int
    label [alias="wire_label"]: str

    def read(self) -> int:
        return self.wire_value

def main() -> None:
    mut value = AliasRecord(wire_label="aliased", wire_value=7)
    println(value.value)
    println(value.wire_value)
    println(value.read())
    value.value = 9
    println(value.value)
    println(value.label)
    println(value.wire_label)
"#,
        None,
    )
}

/// Model field defaults execute for each omitted slot and preserve the legacy default effects and owned text.
#[test]
fn model_field_defaults_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "model_field_defaults",
        r#"def default_number() -> int:
    println(100)
    return 7

def default_text() -> str:
    println(200)
    prefix = "default"
    return prefix + "!"

model Defaults:
    required: int
    number: int = default_number()
    label: str = default_text()
    flag: bool = true
    fraction: float = 2.5

def main() -> None:
    first = Defaults(required=1)
    second = Defaults(label="supplied", required=2, number=9)
    third = Defaults(required=3)
    println(first.required)
    println(first.number)
    println(first.label)
    println(first.flag)
    println(first.fraction)
    println(second.number)
    println(second.label)
    println(third.number)
    println(third.label)
"#,
        None,
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

/// Source async calls defer discarded body effects and evaluate arguments before their immediately ready awaits.
#[test]
fn direct_route_async_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "async_ready",
        r#"
import std.async

def argument() -> int:
    """Observe argument construction before polling."""
    println("argument")
    return 40

async def answer(value: int) -> int:
    """Observe deferred execution and return a ready value."""
    println(f"body={value}")
    return value + 1

async def nested(value: int) -> int:
    """Await one source future inside another."""
    return await answer(value)

async def main() -> None:
    """Discard a future and then evaluate nested ready awaits."""
    answer(999)
    println("constructed")
    println(await nested(argument()))
    println(await answer(7))
"#,
        None,
    )
}

/// Ready races construct every arm, poll in source order, run only the winner, and preserve first-poll failures.
#[test]
fn direct_route_ready_races_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "ready_race",
        r#"
import std.async

def argument(value: int) -> int:
    """Observe construction of each race arm."""
    println(f"construct={value}")
    return value

async def answer(value: int) -> int:
    """Observe polling of the selected source future."""
    println(f"body={value}")
    return value + 1

async def select() -> int:
    """Run one block callback and leave the losing body and callback unexecuted."""
    winner = race for value:
        await answer(argument(2)) =>
            println(f"winner={value}")
            value * 10
        await answer(argument(1)) =>
            println("losing callback")
            value * 100
    return winner

async def main() -> None:
    """Await a ready source race."""
    println(await select())
"#,
        None,
    )?;
    check_declaration_case(
        "ready_race_failure",
        r#"
import std.async

def argument() -> int:
    """Show that the losing future is constructed before the first arm fails."""
    println("loser constructed")
    return 2

async def fail() -> int:
    """Fail on the first poll of the winning source future."""
    assert false, "first"
    return 1

async def loser(value: int) -> int:
    """Observe an incorrect poll of the losing future."""
    println("loser polled")
    return value

async def main() -> None:
    """Keep the first failure ahead of both callbacks and all later effects."""
    winner = race for value:
        await fail() => value
        await loser(argument()) => value
    println(winner)
"#,
        Some("AssertionError: first"),
    )
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

/// Published two-module library bodies retain function, nominal, enum, and adopted-method output.
#[test]
fn direct_route_packages_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("packages")?;
    let library = root.join("deps/library");
    fs::create_dir_all(library.join("src"))?;
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        root.join("loaf.toml"),
        "[project]\nname = \"package_consumer\"\nversion = \"0.1.0\"\n[project.scripts]\nmain = \"src/main.incn\"\n[dependencies]\nlibrary = { path = \"deps/library\" }\n",
    )?;
    fs::write(
        library.join("loaf.toml"),
        "[project]\nname = \"library\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        library.join("src/lib.incn"),
        "pub from values import Reading, Counter, Signal, add, BASE, Answer, classify\n",
    )?;
    fs::write(
        library.join("src/values.incn"),
        "pub const BASE: int = 2\n\npub type Answer = int | str\n\npub def classify(value: Answer) -> int:\n    if isinstance(value, int):\n        return 1\n    return 2\n\npub trait Reading:\n    def get(self) -> int: ...\n\n    def doubled(self) -> int:\n        return self.get() + self.get()\n\npub model Counter with Reading:\n    pub value: int\n\n    def get(self) -> int:\n        return self.value\n\npub enum Signal:\n    Ready\n    Waiting\n\npub def add(value: int) -> int:\n    return value + BASE\n",
    )?;
    let source = root.join("src/main.incn");
    fs::write(
        &source,
        "from pub::library import Counter, Signal, add, BASE, Answer, classify\n\ndef main() -> None:\n    println(add(40))\n    counter = Counter(value=7)\n    println(counter.get())\n    println(counter.doubled())\n    println(BASE)\n    answer: Answer = 7\n    println(classify(answer))\n    signal = Signal.Ready\n    match signal:\n        Signal.Ready => println(1)\n        Signal.Waiting => println(2)\n",
    )?;
    let mut publish = support::cli_project::configured_incan_command(&library, &["oven", "bake", "--project", "."]);
    support::configure_explicit_oven_bake_command(&mut publish)?;
    success(&publish.output()?, "package publication");
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
        .current_dir(&root)
        .output()?,
        "package native compilation",
    );
    let legacy = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy)
            .output()?,
        "package legacy compilation",
    );
    let expected = Command::new(legacy.join("oven/release/package_consumer")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "package legacy execution");
    success(&actual, "package native execution");
    assert_eq!(actual.stdout, b"42\n7\n14\n2\n1\n1\n");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
    assert_eq!(actual.status.code(), expected.status.code());
    Ok(())
}

/// A checked package module binding selects callable bodies without requesting a namespace fragment.
#[test]
fn direct_route_package_module_binding_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("package-module-binding")?;
    let library = root.join("deps/widgets");
    fs::create_dir_all(library.join("src"))?;
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        root.join("loaf.toml"),
        "[project]\nname = 'package_module_binding'\nversion = '0.1.0'\n[dependencies]\nwidgets = { path = 'deps/widgets' }\n",
    )?;
    fs::write(
        library.join("loaf.toml"),
        "[project]\nname = 'widgets'\nversion = '0.1.0'\n",
    )?;
    fs::write(
        library.join("src/lib.incn"),
        "pub def label(value: str) -> str:\n    return value\n",
    )?;
    fs::write(
        root.join("src/main.incn"),
        "import pub::widgets as widgets_alias\n\ndef main() -> None:\n    println(widgets_alias.label('aliased'))\n",
    )?;
    let mut publish = support::cli_project::configured_incan_command(&library, &["oven", "bake", "--project", "."]);
    support::configure_explicit_oven_bake_command(&mut publish)?;
    success(&publish.output()?, "module binding package publication");
    assert_package_routes_match(fixture, &root, "package_module_binding", b"aliased\n")
}

/// A sibling-module model's canonical identity survives public package method calls without source recovery.
#[test]
fn direct_route_package_sibling_model_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("package-sibling-model")?;
    let area = support::fixtures_dir().join("behavior/lowering_dependencies");
    let behavior = support::behavior_fixtures::discover(&area)?
        .into_iter()
        .find(|case| case.name == "package_inherent_methods_issue1174")
        .ok_or("package sibling model fixture is missing")?;
    support::behavior_fixtures::materialize(&behavior, &root)?;
    for provider in &behavior.providers {
        let mut publish = support::cli_project::configured_incan_command(
            &root.join(&provider.path),
            &["oven", "bake", "--project", "."],
        );
        support::configure_explicit_oven_bake_command(&mut publish)?;
        success(&publish.output()?, "sibling model package publication");
    }
    assert_package_routes_match(
        fixture,
        &root,
        "package_inherent_methods_issue1174",
        b"base=10 tax=2\ntotal=12\n",
    )
}

/// Compare a published package consumer's complete native output and exit status against the legacy route.
fn assert_package_routes_match(
    fixture: &DriverFixture,
    root: &Path,
    binary_name: &str,
    stdout: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let source = root.join("src/main.incn");
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
        .current_dir(root)
        .output()?,
        "package native compilation",
    );
    let legacy = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(root)
            .arg("build")
            .arg(&source)
            .arg(&legacy)
            .output()?,
        "package legacy compilation",
    );
    let expected = Command::new(legacy.join("oven/release").join(binary_name)).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "package legacy execution");
    success(&actual, "package native execution");
    assert_eq!(actual.stdout, stdout);
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

/// List display preserves legacy Debug spelling, escaping, nesting, and repeated owner reads.
#[test]
fn list_display_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "list_display",
        r#"def main() -> None:
    values = [1, 2]
    println(values)
    println(f"values={values}")
    println(values)
    println([true, false])
    println([1.0, -2.5])
    words = ["quoted\"", "line\n", "é"]
    println(words)
    println(words)
    println([[1, 2], [3]])
    println([1] + [2])
"#,
        None,
    )
}

/// Derived Display uses the same Debug structure in println, string conversion, and interpolation as legacy.
#[test]
fn model_display_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "model_display",
        r#"@derive(Display)
model Record:
    number: int
    label: str

def main() -> None:
    record = Record(number=7, label="quoted\"\n")
    println(record)
    println(f"record={record}")
    println(str(record))
    println(f"{record:?}")
    println(record.label)
"#,
        None,
    )
}

/// JSON module bundles, imported aliases, and qualified derives serialize through the legacy serde boundary.
#[test]
fn model_json_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "model_json",
        r#"from std.serde import json
from std.serde.json import Serialize as JsonSerialize

@derive(json)
model Record:
    number: int
    label: str

@derive(JsonSerialize)
model Alias:
    enabled: bool

@derive(json.Serialize)
model Qualified:
    value: float

@derive(json.Serialize)
model Custom:
    number: int

    def to_json(self) -> str:
        return "custom"

def next_value() -> str:
    println("evaluated")
    return "line\né"

def main() -> None:
    record = Record(number=7, label="quoted\"\n")
    alias = Alias(enabled=true)
    qualified = Qualified(value=2.5)
    custom = Custom(number=9)
    println(record.to_json())
    println(json_stringify(record))
    println(json_stringify(123))
    println(json_stringify(None))
    println(json_stringify(9223372036854775807))
    println(json_stringify(-9223372036854775807))
    println(json_stringify(next_value()))
    println(std.builtins.json_stringify(next_value()))
    println(json_stringify(-7))
    println(json_stringify(2.5))
    println(json_stringify(true))
    println(json_stringify("escaped\"\n"))
    println(alias.to_json())
    println(qualified.to_json())
    println(custom.to_json())
    println(json_stringify(custom))
    println(record.to_json())
    println(record.label)
"#,
        None,
    )
}

/// Prove lazy primitive-list zip snapshots, tuple items, and independent aliases against legacy.
#[test]
fn direct_route_tuple_zip_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("release"),
        &fixture.scratch("tuple-zip")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def main() -> None:
    """Observe equal, unequal, empty, and independently aliased zip inputs."""
    left = [1, 2, 3]
    right = [10, 20]
    pairs = zip(left, right)
    alias = pairs
    again = pairs
    for a, b in alias:
        println(a + b)
    for a, b in pairs:
        println(a + b)
    for a, b in again:
        println(a + b)
    for a, b in zip([4], [5]):
        println(a + b)
    empty: list[int] = []
    for a, b in zip(empty, right):
        println(a + b)
    for a, b in zip([7, 8], ["x"]):
        println(a)
        println(b)
"#,
    )?;
    Ok(())
}

/// Prove stored and immediate builtin enumeration, deferred yields, and tuple-string ownership against legacy.
#[test]
fn direct_route_enumerate_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("release"),
        &fixture.scratch("enumerate")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def words() -> list[str]:
    """Show that the enumeration source is evaluated once."""
    println("source")
    return ["é", "cat"]

def generated() -> Generator[int]:
    """Enumerate inside a lazy producer."""
    for index, value in enumerate([5, 6]):
        yield index + value

def main() -> None:
    """Observe stored pairs, source preservation, Unicode, empty inputs, and deferred enumeration."""
    source = words()
    stored = enumerate(source)
    alias = stored
    for index, value in stored:
        println(index)
        println(value)
    for pair in alias:
        println(pair.0)
        println(pair.1)
    println(source[0])
    for index, value in enumerate("é猫"):
        println(index)
        println(value)
    for index, value in enumerate([1.5, 2.5]):
        println(index)
        println(value)
    for index, value in enumerate([true, false]):
        println(index)
        println(value)
    empty: list[int] = []
    println(len(enumerate(empty)))
    collected = generated().collect()
    println(collected[0])
    println(collected[1])
    for number, label in zip([7], ["zip"]):
        println(number)
        println(label)
"#,
    )?;
    Ok(())
}

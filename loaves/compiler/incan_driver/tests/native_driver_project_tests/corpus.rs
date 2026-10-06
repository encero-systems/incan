//! Unchanged compute benchmark proof using the Oven-built driver and authored formatting runtime.

use super::*;

/// Exercise newtype construction and projection, aliases, scalar constants, and static mutation against legacy.
pub(super) fn check_declarations(
    driver: &Path,
    root: &Path,
    sysroot: &Path,
    runtime: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let project = root.join("declarations");
    fs::create_dir_all(&project)?;
    let source = project.join("declarations.incn");
    fs::write(
        &source,
        "type Count = int\ntype Ratio = float\nconst BASE: int = 42\nconst SCALE: float = 1.5\nconst WHOLE: float = 2\nconst ENABLED: bool = true\ntype Id = newtype int\ntype Label = newtype str\nstatic COUNT: int = 4\nstatic RATIO: float = 2.5\nstatic FLAG: bool = false\n\ndef doubled(mut value: Count) -> Count:\n    value += value\n    return value\n\nagain = alias doubled\n\ndef increment() -> None:\n    COUNT += 1\n\ndef main() -> None:\n    println(again(BASE))\n    ratio: Ratio = SCALE\n    println(ratio)\n    println(WHOLE)\n    println(ENABLED)\n    id = Id(9)\n    label = Label(\"wrapped\")\n    println(id.0)\n    println(label.0)\n    increment()\n    increment()\n    println(COUNT)\n    RATIO = RATIO + 1.0\n    println(RATIO)\n    FLAG = true\n    println(FLAG)\n",
    )?;
    let native = project.join("native");
    success(
        &compile_source(driver, &source, &native, sysroot, &runtime_closure(runtime, "release")?)?,
        "declarations native compilation",
    );
    let legacy = project.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&project)
            .arg("build")
            .arg(&source)
            .arg(&legacy)
            .output()?,
        "declarations legacy compilation",
    );
    let expected = Command::new(legacy.join("oven/release/declarations")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "declarations legacy execution");
    success(&actual, "declarations native execution");
    assert_eq!(
        actual.stdout, expected.stdout,
        "declaration output must be byte-identical"
    );
    assert_eq!(actual.stdout, b"84\n1.5\n2.0\ntrue\n9\nwrapped\n6\n3.5\ntrue\n");
    let unsupported = project.join("effectful_static.incn");
    fs::write(
        &unsupported,
        "def initial() -> int:\n    println(99)\n    return 4\n\nstatic COUNT: int = initial()\n\ndef main() -> None:\n    println(COUNT)\n",
    )?;
    let refused = compile_source(
        driver,
        &unsupported,
        &project.join("effectful"),
        sysroot,
        &runtime_closure(runtime, "release")?,
    )?;
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("unsupported source Static initializer or carrier"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    Ok(())
}

/// Prove constants, shared parameters, owned returns, concatenation, comparison, and display against legacy.
pub(super) fn check_strings(
    driver: &Path,
    root: &Path,
    sysroot: &Path,
    runtime: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let project = root.join("strings");
    fs::create_dir_all(&project)?;
    let source = project.join("strings.incn");
    fs::write(
        &source,
        "const PREFIX: str = \"hello\"\n\ndef greet(value: str) -> str:\n    \"\"\"Return the greeting.\"\"\"\n    return value + \" world\"\n\ndef identity(value: str) -> str:\n    return value\n\ndef main() -> None:\n    \"\"\"Exercise owned string boundaries.\"\"\"\n    \"discarded text\"\n    text = greet(PREFIX)\n    same = identity(text)\n    same\n    println(text == same)\n    println(text < \"z\")\n    println(text)\n    println(f\"result={same}\")\n    mut changing = same\n    changing += \"!\"\n    changing = identity(changing)\n    println(changing)\n",
    )?;
    let native = project.join("native");
    success(
        &compile_source(driver, &source, &native, sysroot, &runtime_closure(runtime, "release")?)?,
        "native strings compilation",
    );
    let legacy_output = project.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&project)
            .arg("build")
            .arg(&source)
            .arg(&legacy_output)
            .output()?,
        "legacy strings compilation",
    );
    let expected = Command::new(legacy_output.join("oven/release/strings")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy strings execution");
    success(&actual, "native strings execution");
    assert_eq!(actual.stdout, expected.stdout, "string output must be byte-identical");
    assert_eq!(
        actual.stdout,
        b"true\ntrue\nhello world\nresult=hello world\nhello world!\n"
    );
    Ok(())
}

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

/// Preserve the benchmark source bytes and compare direct-native output with its normal backend output.
pub(super) fn check_benchmark(
    driver: &Path,
    root: &Path,
    sysroot: &Path,
    runtime: &Path,
    name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let closure = runtime_closure(runtime, "release")?;
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

/// Exercise the plain nominal contract with reversed written field order and a returned model.
pub(super) fn check_plain_model(
    driver: &Path,
    root: &Path,
    sysroot: &Path,
    runtime: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let closure = runtime_closure(runtime, "release")?;
    let project = root.join("plain-model");
    fs::create_dir_all(&project)?;
    let source = project.join("plain_model.incn");
    fs::write(
        &source,
        "model Point:\n  x: int\n  y: int\n  label: str\n\ndef shifted(point: Point) -> Point:\n  return Point(label=point.label, y=point.y, x=point.x + 2)\n\ndef main() -> None:\n  mut point = Point(label=\"model\", y=7, x=3)\n  point.x += 4\n  point.label = \"updated\"\n  next = shifted(point)\n  println(point.x)\n  println(next.x)\n  println(next.y)\n  println(next.label)\n",
    )?;
    let native = project.join("native");
    success(
        &compile_source(driver, &source, &native, sysroot, &closure)?,
        "plain model native compilation",
    );
    let legacy_output = project.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&project)
            .arg("build")
            .arg(&source)
            .arg(&legacy_output)
            .output()?,
        "plain model legacy compilation",
    );
    let expected = Command::new(legacy_output.join("oven/release/plain_model")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "plain model legacy execution");
    success(&actual, "plain model native execution");
    assert_eq!(
        actual.stdout, expected.stdout,
        "plain model output must be byte-identical"
    );
    assert_eq!(actual.stdout, b"7\n9\n7\nupdated\n");

    // A newly admitted model must not let an unadmitted numeric coercion escape as an invalid plan.
    let division = project.join("integer_division.incn");
    fs::write(
        &division,
        "model Reading:\n  value: int\n\ndef main() -> None:\n  reading = Reading(value=10)\n  println(str(reading.value / 2))\n",
    )?;
    let refused = compile_source(driver, &division, &project.join("division"), sysroot, &closure)?;
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("unsupported Body IR non-float true division operands"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    Ok(())
}

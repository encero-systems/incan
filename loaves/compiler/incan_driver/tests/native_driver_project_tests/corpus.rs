//! Unchanged compute benchmark proof using the Oven-built driver and authored formatting runtime.

use super::*;

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

/// Lists preserve legacy indexing, mutation, shared parameters, owned returns, and loop output.
pub(super) fn check_lists(
    driver: &Path,
    root: &Path,
    sysroot: &Path,
    runtime: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let project = root.join("lists");
    fs::create_dir_all(&project)?;
    let source = project.join("lists.incn");
    fs::write(
        &source,
        r#"
def bump(mut values: list[int]) -> None:
    values.append(8)
    values[-1] = 9

def count(values: list[int]) -> int:
    return len(values)

def copy_values(values: list[int]) -> list[int]:
    return values

def main() -> None:
    mut numbers = [1, 2, 3]
    mut words = ["one", "two"]
    numbers.append(4)
    words.append("three")
    numbers[-1] = 7
    println(numbers[-1])
    println(words[-1])
    println(2 in numbers)
    println("three" in words)
    bump(numbers)
    println(count(numbers))
    copied = copy_values(numbers)
    println(copied[-1])
    for value in numbers:
        println(value)
    for word in words:
        println(word)
    mut fractions = [1.5, 2.5]
    fractions.append(-1.0)
    for fraction in fractions:
        println(fraction)
    mut flags = [true, false]
    flags.append(true)
    println(flags[-1])
    mut nested = [[1, 2], [3]]
    nested[0][-1] = 8
    println(nested[0][1])
    nested.append([4])
    for row in nested:
        println(len(row))
"#,
    )?;
    let native = project.join("native");
    success(
        &compile_source(driver, &source, &native, sysroot, &runtime_closure(runtime, "release")?)?,
        "native lists compilation",
    );
    let legacy_output = project.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&project)
            .arg("build")
            .arg(&source)
            .arg(&legacy_output)
            .output()?,
        "legacy lists compilation",
    );
    let expected = Command::new(legacy_output.join("oven/release/lists")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy lists execution");
    success(&actual, "native lists execution");
    assert_eq!(actual.stdout, expected.stdout, "list output must be byte-identical");
    assert_eq!(
        actual.stdout,
        b"7\nthree\ntrue\ntrue\n5\n9\n1\n2\n3\n7\n9\none\ntwo\nthree\n1.5\n2.5\n-1.0\ntrue\n8\n2\n1\n1\n"
    );
    Ok(())
}

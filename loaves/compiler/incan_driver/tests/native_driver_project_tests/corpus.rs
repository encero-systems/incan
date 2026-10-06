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

/// Hash collections must keep legacy key membership, overwrites, and caller-owned mutation.
///
/// Dictionary traversal uses the admitted `keys()` method: legacy bare dictionary iteration emits entries despite
/// the frontend's key item type, so indexing with that loop binding cannot compile on the comparison route.
pub(super) fn check_collections(
    driver: &Path,
    root: &Path,
    sysroot: &Path,
    runtime: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let project = root.join("collections");
    fs::create_dir_all(&project)?;
    let source = project.join("collections.incn");
    fs::write(
        &source,
        r#"
def add(mut values: Set[int]) -> None:
    values.add(7)

def copied(values: Dict[str, int]) -> Dict[str, int]:
    return values

def main() -> None:
    mut numbers = {1, 2, 2}
    println(len(numbers))
    println(2 in numbers)
    println(9 not in numbers)
    add(numbers)
    println(numbers.contains(7))
    mut mapping = {"one": 1, "two": 2, "one": 3}
    mapping.insert("three", 4)
    mapping["two"] = 8
    println(mapping["two"])
    println(mapping["one"])
    println("three" in mapping)
    println("absent" not in mapping)
    println(len(mapping))
    copy = copied(mapping)
    println(copy["one"])
    words = {"first", "second"}
    println("first" in words)
    flags = {true, false}
    println(len(flags))
    fractions = {1: 1.5, 2: 2.5}
    println(fractions[2])
    empty_numbers: Set[int] = Set()
    empty_mapping: Dict[str, int] = Dict()
    println(len(empty_numbers))
    println(len(empty_mapping))
    println(mapping.get("absent", 11))
    println(mapping.get("one", 12))
    println(len(mapping.keys()))
    println(len(mapping.values()))
    mut total = 0
    for number in numbers:
        total += number
    println(total)
    mut mapped_total = 0
    for key in mapping.keys():
        mapped_total += mapping[key]
    println(mapped_total)
    inputs = ["a", "b", "a"]
    unique = Set(inputs)
    println(len(inputs))
    println(len(unique))
    copied_set = set(unique)
    println(len(copied_set))
    spread = {**mapping, "two": 19}
    println(spread["two"])
    println(mapping["two"])
    for word in {"single"}:
        println(word)

"#,
    )?;
    let native = project.join("native");
    success(
        &compile_source(driver, &source, &native, sysroot, &runtime_closure(runtime, "release")?)?,
        "native collections compilation",
    );
    let legacy_output = project.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&project)
            .arg("build")
            .arg(&source)
            .arg(&legacy_output)
            .output()?,
        "legacy collections compilation",
    );
    let expected = Command::new(legacy_output.join("oven/release/collections")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy collections execution");
    success(&actual, "native collections execution");
    assert_eq!(
        actual.stdout, expected.stdout,
        "collections output must be byte-identical"
    );
    assert_eq!(
        actual.stdout,
        b"2\ntrue\ntrue\ntrue\n8\n3\ntrue\ntrue\n3\n3\ntrue\n2\n2.5\n0\n0\n11\n3\n3\n3\n10\n15\n3\n2\n2\n19\n8\nsingle\n"
    );
    Ok(())
}

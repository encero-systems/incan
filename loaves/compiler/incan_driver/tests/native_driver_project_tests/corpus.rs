//! Native/legacy output comparisons using the Oven-built driver and authored formatting runtime.

use super::*;

/// Persistent primitive hashed storage exercises read snapshots, live aliases, replacement, and argument effects.
pub(super) const HASHED_STATICS_SOURCE: &str = r#"
static counts: dict[str, int] = {"initial": 4}
static seen: set[int] = Set()
static words: set[str] = {"before"}
static flags: dict[bool, str] = {true: "yes"}
static ratios: dict[int, float] = {1: 2.5}

def record(name: str) -> None:
    counts[name] = counts.get(name, 0) + 1

def argument() -> int:
    counts.insert("side", 9)
    return 3

def main() -> None:
    record("a")
    record("a")
    println(counts.get("a").unwrap_or(0))
    println(counts.get("missing", 7))
    println(counts["initial"])
    counts.insert("a", argument())
    println(counts["a"])
    println(counts["side"])
    live = counts
    counts["a"] = 5
    println(live["a"])
    println(len(counts))
    println("side" in counts)
    seen.add(4)
    seen.add(4)
    seen.add(6)
    println(len(seen))
    println(seen.contains(4))
    println(seen.contains(5))
    words.add("after")
    println("before" in words)
    println(len(words))
    flags.insert(false, "no")
    println(flags[true])
    println(flags.get(false, "missing"))
    previous = flags.get(true)
    flags.insert(true, "changed")
    println(previous.unwrap_or("missing"))
    println(flags.get(true).unwrap_or("missing"))
    ratios.insert(2, 3.5)
    println(ratios[1] + ratios[2])
    counts = {"reset": 8}
    println(live["reset"])
    println(len(counts))
"#;

/// Standard newtype derives preserve tuple carriers, ordinary methods, and owned argument/return snapshots.
pub(super) const DERIVED_NEWTYPES_SOURCE: &str = r#"
@derive(Clone, Eq, Ord, Hash, Default)
type Label = newtype str:
    def upper(self) -> str:
        return self.0.upper()

@derive(Clone, Copy, Eq, Ord, Hash, Default)
type Count = newtype int:
    def next(self) -> int:
        return self.0 + 1

def keep(value: Label) -> Label:
    return value

def make() -> Label:
    return Label("returned")

def main() -> None:
    label = Label("original")
    first = keep(label)
    mut text = first.0
    text += "!"
    println(text)
    println(label.0)
    println(first.upper())
    println(make().0)
    count = Count(4)
    println(count.0)
    println(count.next())
"#;

/// Primitive and flat tuple match parity, including rejected string literals and branch-owned bindings.
pub(super) const STRUCTURAL_MATCHES_SOURCE: &str = r#"
def pick(number: int) -> str:
    match number:
        1 => "one"
        _ => "other"

def describe(pair: tuple[int, str]) -> str:
    match pair:
        (0, _) => return "zero"
        (_, "answer") => return "answer"
        (number, word) => return f"{number + 1} {word.upper()}"

def classify(code: str) -> str:
    mut kind = "other"
    match code:
        "a" => kind = "alpha"
        "b" => kind = "beta"
        _ => pass
    return kind

def main() -> None:
    println(pick(1))
    println(pick(2))
    println(classify("b"))
    println(classify("z"))
    pair: tuple[int, str] = (42, "hello")
    println(describe(pair))
    println(describe((0, "unused")))
    println(describe((7, "answer")))
    match pair:
        (0, "absent") => println("wrong")
        (number, word) => println(f"{number} {word}")
    println(pair)
    flag = true
    match flag:
        false => println("wrong")
        true => println("true")
        _ => println("wrong")
"#;

/// Primitive static-list parity, including an argument that changes the same cell before its outer append.
pub(super) const LIST_STATICS_SOURCE: &str = r#"
static ITEMS: list[int] = [1]
static FLAGS: list[bool] = [true, false]
static TEXTS: list[str] = ["alpha"]
static REALS: list[float] = [1.5]

def push(value: int) -> None:
    ITEMS.append(value)

def argument_effect() -> int:
    ITEMS.append(8)
    return 9

def snapshot() -> list[int]:
    return ITEMS

def total() -> int:
    mut result = 0
    for item in ITEMS:
        result += item
    return result

def main() -> None:
    live = ITEMS
    first = snapshot()
    push(2)
    println(len(live))
    ITEMS.append(argument_effect())
    live.extend([5, 6])
    live.swap(0, 5)
    println(live[0])
    println(live.pop())
    live.remove(0)
    println(len(ITEMS))
    println(total())
    println(len(first))
    mut detached = ITEMS
    detached = [7]
    detached.append(10)
    println(len(detached))
    println(len(ITEMS))
    ITEMS = [3]
    println(live[0])
    println(FLAGS[0])
    FLAGS.append(true)
    println(len(FLAGS))
    TEXTS.append("beta")
    println(TEXTS[1])
    REALS.append(2.5)
    println(REALS[1])
"#;

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

/// Compare Unicode text helpers and global length byte for byte with legacy.
pub(super) fn check_string_methods(
    driver: &Path,
    root: &Path,
    sysroot: &Path,
    runtime: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    check_builtin_source(
        driver,
        root,
        sysroot,
        runtime,
        r#"def main() -> None:
    text = "  café 世界  "
    println(len(text))
    println(text.len())
    println(text.upper())
    println(text.lower())
    println(text.strip())
    println(text.replace("café", "tea"))
    println("世界" in text)
    println(not ("absent" in text))
    println("absent" not in text)
    println("|".join("é,,猫".split(",")))
    println("|".join(" a b ".split()))
"#,
    )
}

/// Compare a focused builtin program with legacy without normalizing its output.
pub(super) fn check_builtin_source(
    driver: &Path,
    root: &Path,
    sysroot: &Path,
    runtime: &Path,
    program: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let project = root.join("builtin-parity");
    fs::create_dir_all(&project)?;
    let source = project.join("probe.incn");
    fs::write(&source, program)?;
    let native = project.join("native");
    success(
        &compile_source(driver, &source, &native, sysroot, &runtime_closure(runtime, "release")?)?,
        "native builtin compilation",
    );
    let legacy_output = project.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&project)
            .arg("build")
            .arg(&source)
            .arg(&legacy_output)
            .output()?,
        "legacy builtin compilation",
    );
    let expected = Command::new(legacy_output.join("oven/release/probe")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy builtin execution");
    success(&actual, "native builtin execution");
    assert_eq!(actual.stdout, expected.stdout, "builtin output must be byte-identical");
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

/// Prove class receiver borrowing, field writes, canonical method dispatch, and class passing against legacy.
pub(super) fn check_source_class(
    driver: &Path,
    root: &Path,
    sysroot: &Path,
    runtime: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let closure = runtime_closure(runtime, "release")?;
    let project = root.join("source-class");
    fs::create_dir_all(&project)?;
    let source = project.join("source_class.incn");
    fs::write(
        &source,
        r#"class Counter:
  value: int
  label: str

  def get(self) -> int:
    return self.value

  def bump(mut self, amount: int) -> None:
    self.value += amount

  def twice(mut self) -> None:
    self.bump(amount=self.get())

  def rename(mut self, label: str) -> None:
    self.label = label

  def text(self) -> str:
    return self.label

class Other:
  value: int

  def get(self) -> int:
    return self.value + 1

def inspect(counter: Counter) -> int:
  return counter.get()

def main() -> None:
  mut counter = Counter(label="counter", value=3)
  counter.bump(amount=4)
  println(counter.get())
  counter.twice()
  println(inspect(counter))
  other = Other(value=8)
  println(other.get())
  counter.rename("renamed")
  println(counter.text())
"#,
    )?;
    let native = project.join("native");
    success(
        &compile_source(driver, &source, &native, sysroot, &closure)?,
        "source class native compilation",
    );
    let legacy_output = project.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&project)
            .arg("build")
            .arg(&source)
            .arg(&legacy_output)
            .output()?,
        "source class legacy compilation",
    );
    let expected = Command::new(legacy_output.join("oven/release/source_class")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "source class legacy execution");
    success(&actual, "source class native execution");
    assert_eq!(
        actual.stdout, expected.stdout,
        "source class output must be byte-identical"
    );
    assert_eq!(actual.stdout, b"7\n14\n9\nrenamed\n");

    Ok(())
}

/// Prove numeric conversions against the legacy native route.
pub(super) fn check_numerics(
    driver: &Path,
    root: &Path,
    sysroot: &Path,
    runtime: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let closure = runtime_closure(runtime, "release")?;
    let project = root.join("numerics");
    fs::create_dir_all(&project)?;
    let source = project.join("numerics.incn");
    fs::write(
        &source,
        "def main() -> None:\n  println(int())\n  println(float())\n  println(int(true))\n  println(int(false))\n  println(int(12.75))\n  println(int(-12.75))\n  println(float(7))\n  println(int(\"1_024\"))\n  println(float(\"1.25\"))\n  small: u8 = 250\n  delta: u8 = 4\n  println(small + delta)\n  wide: i128 = 170141183460469231731687303715884105727\n  println(wide)\n  rounded: f32 = 1.23456789\n  println(rounded)\n  println(int(small))\n  println(float(small))\n  println(int(float(\"inf\")))\n  println(int(float(\"-inf\")))\n  println(int(float(\"NaN\")))\n",
    )?;
    let native = project.join("native");
    success(
        &compile_source(driver, &source, &native, sysroot, &closure)?,
        "numerics native compilation",
    );
    let legacy_output = project.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&project)
            .arg("build")
            .arg(&source)
            .arg(&legacy_output)
            .output()?,
        "numerics legacy compilation",
    );
    let expected = Command::new(legacy_output.join("oven/release/numerics")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "numerics legacy execution");
    success(&actual, "numerics native execution");
    assert_eq!(actual.stdout, expected.stdout, "numerics output must be byte-identical");
    assert_eq!(actual.stdout, b"0\n0.0\n1\n0\n12\n-12\n7.0\n1024\n1.25\n254\n170141183460469231731687303715884105727\n1.2345679\n250\n250.0\n9223372036854775807\n-9223372036854775808\n0\n");

    check_numeric_wrapping(driver, &project, sysroot, &closure)?;
    for (name, source, message) in [
        (
            "nonfinite",
            "def grow(value: f32) -> f32:\n  return value * value\n\ndef main() -> None:\n  value: f32 = 3.0e38\n  println(grow(value))\n",
            "non-finite",
        ),
        (
            "invalid_parse",
            "def main() -> None:\n  println(int(\"1__2\"))\n",
            "ValueError",
        ),
    ] {
        check_numeric_failure(driver, &project, sysroot, &closure, name, source, message)?;
    }
    Ok(())
}

/// Compare sized integer overflow in the legacy release profile, which uses wrapping arithmetic.
fn check_numeric_wrapping(
    driver: &Path,
    project: &Path,
    sysroot: &Path,
    closure: &NativeClosure,
) -> Result<(), Box<dyn std::error::Error>> {
    let source = project.join("wrapping.incn");
    fs::write(
        &source,
        "def grow(value: u8) -> u8:\n  return value + 10\n\ndef main() -> None:\n  value: u8 = 250\n  println(grow(value))\n",
    )?;
    let native = project.join("wrapping-native");
    success(
        &compile_source(driver, &source, &native, sysroot, closure)?,
        "wrapping native compilation",
    );
    let legacy = project.join("wrapping-legacy");
    success(
        &support::repo_command()
            .current_dir(project)
            .arg("build")
            .arg(&source)
            .arg(&legacy)
            .output()?,
        "wrapping legacy compilation",
    );
    let expected = Command::new(legacy.join("oven/release/wrapping")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "wrapping legacy execution");
    success(&actual, "wrapping native execution");
    assert_eq!(actual.stdout, expected.stdout, "wrapping output must be byte-identical");
    assert_eq!(actual.stdout, b"4\n");
    Ok(())
}

/// Compare runtime rejection after both routes compile, without comparing panic location paths.
fn check_numeric_failure(
    driver: &Path,
    project: &Path,
    sysroot: &Path,
    closure: &NativeClosure,
    name: &str,
    source: &str,
    message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = project.join(format!("{name}.incn"));
    fs::write(&path, source)?;
    let native = project.join(format!("{name}-native"));
    success(
        &compile_source(driver, &path, &native, sysroot, closure)?,
        "numeric failure native compilation",
    );
    let legacy = project.join(format!("{name}-legacy"));
    success(
        &support::repo_command()
            .current_dir(project)
            .arg("build")
            .arg(&path)
            .arg(&legacy)
            .output()?,
        "numeric failure legacy compilation",
    );
    let expected = Command::new(legacy.join("oven/release").join(name)).output()?;
    let actual = Command::new(native).output()?;
    assert!(!expected.status.success(), "legacy {name} unexpectedly succeeded");
    assert!(!actual.status.success(), "native {name} unexpectedly succeeded");
    assert_eq!(
        actual.stdout, expected.stdout,
        "numeric failure output must be byte-identical"
    );
    for output in [&expected, &actual] {
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(message), "{name}: {error}");
    }
    Ok(())
}

/// Prove required/default trait methods, receiver borrowing, field writes, and static dispatch against legacy.
pub(super) fn check_source_trait(
    driver: &Path,
    root: &Path,
    sysroot: &Path,
    runtime: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let closure = runtime_closure(runtime, "release")?;
    let project = root.join("source-trait");
    fs::create_dir_all(&project)?;
    let source = project.join("source_trait.incn");
    fs::write(
        &source,
        r#"trait Reading:
  def get(self) -> int: ...
  def bump(mut self, amount: int) -> None: ...
  def seed() -> int: ...

  def advance(mut self) -> None:
    self.bump(1)

  def doubled(self) -> int:
    return self.get() + self.get()

  def offset() -> int:
    return 3

class Counter with Reading:
  value: int

  def get(self) -> int:
    return self.value

  def bump(mut self, amount: int) -> None:
    self.value += amount

  def seed() -> int:
    return 5

model Other with Reading:
  value: int

  def get(self) -> int:
    return self.value + 1

  def bump(mut self, amount: int) -> None:
    self.value += amount

  def seed() -> int:
    return 6

def main() -> None:
  mut counter = Counter(value=7)
  println(counter.get())
  println(counter.doubled())
  other = Other(value=8)
  println(other.get())
  println(other.doubled())
  println(Other.offset())
  println(Counter.seed())
  println(Other.seed())
  counter.advance()
  println(counter.get())
"#,
    )?;
    let native = project.join("native");
    success(
        &compile_source(driver, &source, &native, sysroot, &closure)?,
        "source trait native compilation",
    );
    let legacy_output = project.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&project)
            .arg("build")
            .arg(&source)
            .arg(&legacy_output)
            .output()?,
        "source trait legacy compilation",
    );
    let expected = Command::new(legacy_output.join("oven/release/source_trait")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "source trait legacy execution");
    success(&actual, "source trait native execution");
    assert_eq!(
        actual.stdout, expected.stdout,
        "source trait output must be byte-identical"
    );
    assert_eq!(actual.stdout, b"7\n14\n9\n18\n3\n5\n6\n8\n");

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

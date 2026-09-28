//! Checked programs whose reads and iterator calls the Rust-source backend used to hand rustc in a shape it refuses:
//! the reads of a `const` frozen collection (#1757), the RFC 088 surface on a generator, `dict.get(key, default)` and a
//! `flat_map` callback that returns an iterable other than a list. Each test compiles the generated Rust.

use incan_frontend::{lexer, parser};
use oven_model::compiler_suite_env;

use crate::IrCodegen;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Lex, parse, check and generate Rust for one program.
fn generate(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?;
    Ok(IrCodegen::new().try_generate(&program)?)
}

/// Generate one stdlib source module as the Rust a project build mounts under `crate::__incan_std`.
///
/// The checker checks it as the standard library's own source, as a project build does. The module's own stdlib
/// version check is dropped: the program that mounts it carries one already.
fn generate_stdlib_module(relative_path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let source = std::fs::read_to_string(root.join(relative_path))?;
    let tokens = lexer::lex(&source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?;
    let mut codegen = IrCodegen::new();
    codegen.set_standard_library_source(true);
    Ok(codegen
        .try_generate(&program)?
        .replace("incan_std_core::__incan_stdlib_version_check!", "// "))
}

/// Mount the stdlib modules generated iterator code reaches through `crate::__incan_std`, when it reaches any: the
/// collection protocols and adapter models, and the callable traits they are bounded by.
fn with_iterator_stdlib(program: String) -> Result<String, Box<dyn std::error::Error>> {
    if !program.contains("__incan_std") {
        return Ok(program);
    }
    let collection = generate_stdlib_module("loaves/stdlib/core/src/derives/collection.incn")?;
    let callable = generate_stdlib_module("loaves/stdlib/core/src/traits/callable.incn")?;
    Ok(format!(
        "{program}\npub mod __incan_std {{\n pub mod derives {{\n pub mod collection {{\n{collection}\n}}\n}}\n pub mod traits {{\n pub mod callable {{\n{callable}\n}}\n}}\n}}\n"
    ))
}

/// Return Cargo's build-script output directories under one target profile.
fn build_output_directories(build_directory: &std::path::Path) -> Result<Vec<std::path::PathBuf>, std::io::Error> {
    let mut outputs = Vec::new();
    for package in std::fs::read_dir(build_directory)? {
        let package = package?;
        for fingerprint in std::fs::read_dir(package.path())? {
            let output = fingerprint?.path().join("out");
            if output.is_dir() {
                outputs.push(output);
            }
        }
    }
    Ok(outputs)
}

/// Find the newest artifact matching `predicate` in any of `directories`.
fn newest_artifact(
    directories: &[std::path::PathBuf],
    predicate: impl Fn(&str) -> bool,
) -> Result<Option<std::path::PathBuf>, std::io::Error> {
    let mut matches = Vec::new();
    for directory in directories {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if predicate(name) {
                let modified = entry.metadata().and_then(|metadata| metadata.modified()).ok();
                matches.push((modified, entry.path()));
            }
        }
    }
    matches.sort_by_key(|(modified, _)| *modified);
    Ok(matches.pop().map(|(_, path)| path))
}

/// Compile generated Rust as a library against the runtime this test binary links, so the assertion is rustc's.
fn compile_generated_rust(program: &str) -> TestResult {
    let rust = with_iterator_stdlib(generate(program)?)?;
    let directory = tempfile::tempdir()?;
    let input = directory.path().join("fixture.rs");
    let output = directory.path().join("libfixture.rlib");
    std::fs::write(&input, &rust)?;
    let capability = compiler_suite_env::OvenCompilerSuiteCapability::from_environment(
        compiler_suite_env::OVEN_COMPILER_SUITE_CAPABILITY_ENV,
    )?;
    let rustc = capability
        .as_ref()
        .map(|capability| capability.rustc.clone())
        .unwrap_or_else(|| {
            std::env::var_os("RUSTC")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| "rustc".into())
        });
    let mut command = std::process::Command::new(rustc);
    command.args([
        "--edition=2024",
        "--crate-type=lib",
        "--crate-name=reads_fixture",
        "-A",
        "warnings",
    ]);
    if let Some(capability) = capability {
        for path in capability.dependency_search_paths {
            command.arg("-L").arg(format!("dependency={}", path.display()));
        }
        for (name, path) in capability.externs {
            command.arg("--extern").arg(format!("{name}={}", path.display()));
        }
    } else {
        let executable = std::env::current_exe()?;
        let target_profile = executable
            .ancestors()
            .find(|ancestor| ancestor.join("build").is_dir())
            .ok_or("test executable has no target profile directory")?;
        let mut directories = build_output_directories(&target_profile.join("build"))?;
        let deps = target_profile.join("deps");
        if deps.is_dir() {
            directories.push(deps);
        }
        for directory in &directories {
            command.arg("-L").arg(format!("dependency={}", directory.display()));
        }
        let stdlib = newest_artifact(&directories, |name| {
            name.starts_with("libincan_std_core-") && name.ends_with(".rlib")
        })?
        .ok_or("generated-Rust proof requires a compiled incan_std_core artifact")?;
        command
            .arg("--extern")
            .arg(format!("incan_std_core={}", stdlib.display()));
        let derive = newest_artifact(&directories, |name| {
            name.trim_start_matches("lib").starts_with("incan_derive-")
                && std::path::Path::new(name)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    == Some(std::env::consts::DLL_EXTENSION)
        })?
        .ok_or("generated-Rust proof requires a compiled incan_derive artifact")?;
        command
            .arg("--extern")
            .arg(format!("incan_derive={}", derive.display()));
    }
    let result = command.arg(&input).arg("-o").arg(output).output()?;
    assert!(
        result.status.success(),
        "{}\n{rust}",
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(())
}

/// #1757: every read a `const` frozen collection answers compiles: `len()` as an `int`, `FrozenSet.contains` and `in`
/// over each family, indexing a `FrozenList` of text, `for` over each family, `set()` over frozen text,
/// comprehensions over a `FrozenDict`, generator clauses over a `FrozenList`, and `bytes` items and values.
#[test]
fn const_frozen_collection_reads_compile_issue1757() -> TestResult {
    compile_generated_rust(
        r#"
const NAMES: FrozenList[str] = ["alpha", "beta"]
const NUMS: FrozenList[int] = [3, 1, 2]
const TAGS: FrozenSet[str] = {"a", "b"}
const IDS: FrozenSet[int] = {3}
const TABLE: FrozenDict[str, int] = {"a": 1, "b": 2}
const CODES: FrozenDict[int, str] = {1: "one"}
const BLOBS: FrozenList[bytes] = [b"ab"]
const BY_NAME: FrozenDict[str, bytes] = {"a": b"x"}


def main() -> None:
    probe = "a"
    sizes: int = NAMES.len() + TAGS.len() + TABLE.len()
    println(sizes)
    println(TAGS.contains(probe))
    println(IDS.contains(3))
    println("alpha" in NAMES)
    println(probe not in TAGS)
    println(2 in NUMS)
    println(probe in TABLE)
    println(1 in CODES)
    first: str = NAMES[0]
    last: str = NAMES[-1]
    println(first + last.upper())
    println(NUMS[0] + 1)
    blob: bytes = BLOBS[0]
    one: bytes = BY_NAME["a"]
    println(len(blob) + len(one))
    mut total = 0
    for n in NUMS:
        total += n
    for i in IDS:
        total += i
    println(total)
    for name in NAMES:
        println(name)
    for tag in TAGS:
        println(tag)
    for key in TABLE:
        println(key)
    for item in BLOBS:
        println(len(item))
    unique: set[str] = set(NAMES)
    println(len(unique))
    upper = [key.upper() for key in TABLE]
    codes = [code + 1 for code in CODES]
    println(len(upper) + len(codes))
    doubled = (n * 2 for n in NUMS)
    println(len(list(doubled)))
"#,
    )
}

/// A `Generator[T]` takes the RFC 088 adapters and terminal consumers beyond its RFC 006 helpers, is accepted where an
/// `Iterator[T]` parameter is declared, and keeps its own `map`, `filter`, `take` and `collect`. The adapters consume
/// the generator, which is not `Clone`. (`sum()` resolves through the primitive `Sum` bridge a project build mounts
/// beside the collection module, which this standalone compile does not; the `generator_iterator_surface` fixture runs
/// it.)
#[test]
fn generator_adapters_and_consumers_compile() -> TestResult {
    compile_generated_rust(
        r#"
def numbers(limit: int) -> Generator[int]:
    for value in range(limit):
        yield value


def is_even(n: int) -> bool:
    return n % 2 == 0


def is_small(n: int) -> bool:
    return n < 2


def add(acc: int, n: int) -> int:
    return acc + n


def show(n: int) -> None:
    println(n)


def inc(n: int) -> int:
    return n + 1


def consume(items: Iterator[int]) -> int:
    mut total = 0
    for item in items:
        total += item
    return total


def main() -> None:
    counted: int = numbers(4).count()
    any_even: bool = numbers(4).any(is_even)
    all_even: bool = numbers(4).all(is_even)
    found: Option[int] = numbers(4).find(is_even)
    folded: int = numbers(4).fold(0, add)
    reduced: int = numbers(4).reduce(0, add)
    numbers(2).for_each(show)
    indexed: list[tuple[int, int]] = numbers(3).enumerate().collect()
    zipped: list[tuple[int, int]] = numbers(3).zip(numbers(3)).collect()
    skipped: list[int] = numbers(4).skip(1).collect()
    chained: list[int] = numbers(2).chain(numbers(2)).collect()
    taken: list[int] = numbers(4).take_while(is_small).collect()
    dropped: list[int] = numbers(4).skip_while(is_small).collect()
    batched: list[list[int]] = numbers(5).batch(2).collect()
    mapped: list[int] = numbers(3).map(inc).filter(is_even).take(1).collect()
    expressed: int = (n * 2 for n in [1, 2, 3]).count()
    println(counted + folded + reduced + consume(numbers(3)) + expressed)
    println(any_even and not all_even)
    match found:
        Some(value) => println(value)
        None => println("none")
    println(len(indexed) + len(zipped) + len(skipped) + len(chained) + len(taken) + len(dropped))
    println(len(batched) + len(mapped))
"#,
    )
}

/// `dict.get(key, default)` is the value, or the default when no entry holds the key, on a local dict and on a module
/// static; the one-argument form stays the optional entry.
#[test]
fn dict_get_with_default_reads_the_value() -> TestResult {
    compile_generated_rust(
        r#"
static counts: dict[str, int] = {}


def record(name: str) -> None:
    counts[name] = counts.get(name, 0) + 1


def main() -> None:
    table = {"a": 1}
    labels = {"x": "ex"}
    hit: int = table.get("a", 0)
    missed: int = table.get("b", 5) + 1
    label: str = labels.get("y", "why")
    record("z")
    println(hit + missed)
    println(label)
    maybe: Option[int] = table.get("a")
    match maybe:
        Some(value) => println(value)
        None => println("none")
"#,
    )
}

/// `flat_map` takes a callback returning any iterable: a set, a generator or a list expands in place.
#[test]
fn flat_map_expands_any_iterable_compiles() -> TestResult {
    compile_generated_rust(
        r#"
def numbers(limit: int) -> Generator[int]:
    for value in range(limit):
        yield value


def pair_set(n: int) -> set[int]:
    return {n, n + 10}


def pair_gen(n: int) -> Generator[int]:
    yield n
    yield n + 1


def pair_list(n: int) -> list[int]:
    return [n, n]


def main() -> None:
    items = [1, 2]
    from_sets: list[int] = items.iter().flat_map(pair_set).collect()
    from_generators: list[int] = items.iter().flat_map(pair_gen).collect()
    from_lists: list[int] = items.iter().flat_map(pair_list).collect()
    over_generator: list[int] = numbers(2).flat_map(pair_gen).collect()
    println(len(from_sets) + len(from_generators) + len(from_lists) + len(over_generator))
"#,
    )
}

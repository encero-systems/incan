//! Mutation and ownership regressions discovered while closing the `mut` parameter contract.

use incan_frontend::{ast, lexer, parser};
use oven_model::compiler_suite_env;

use crate::IrCodegen;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Parse, check, lower and emit one module, returning the generated Rust.
fn generated_rust(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    let program: ast::Program = parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?;
    Ok(IrCodegen::new().try_generate(&program)?)
}

/// Parse, check, lower and emit one module, returning whitespace-free Rust for stable shape assertions.
fn compact_rust(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(compact(&generated_rust(source)?))
}

/// Drop every whitespace character from generated Rust.
fn compact(rust: &str) -> String {
    rust.chars().filter(|character| !character.is_whitespace()).collect()
}

/// Compile generated Rust as a library so ownership and numeric-conversion assertions are backed by rustc.
pub(super) fn compile_generated_rust(source: &str) -> TestResult {
    let directory = tempfile::tempdir()?;
    let input = directory.path().join("fixture.rs");
    let output = directory.path().join("libfixture.rlib");
    std::fs::write(&input, source)?;
    let mut command = generated_rust_rustc("lib")?;
    let result = command.arg(&input).arg("-o").arg(output).output()?;
    assert!(
        result.status.success(),
        "{}\n{source}",
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(())
}

/// Build generated Rust as a program against the runtime this test binary links, run it, and return its standard
/// output, so a behavior assertion is the program's own.
pub(super) fn run_generated_program(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let input = directory.path().join("fixture.rs");
    let program = directory.path().join("fixture");
    std::fs::write(&input, source)?;
    let mut command = generated_rust_rustc("bin")?;
    let built = command.arg(&input).arg("-o").arg(&program).output()?;
    if !built.status.success() {
        return Err(format!("{}\n{source}", String::from_utf8_lossy(&built.stderr)).into());
    }
    let ran = std::process::Command::new(&program).output()?;
    if !ran.status.success() {
        return Err(format!("program failed: {}\n{source}", String::from_utf8_lossy(&ran.stderr)).into());
    }
    Ok(String::from_utf8(ran.stdout)?)
}

/// Build a generated dependency crate as a library named `dependency`, then build generated Rust against it as a
/// program, run it, and return its standard output, so a consumer of a `pub::` dependency is proved by rustc and by
/// what it prints.
pub(super) fn run_generated_program_with_dependency(
    dependency: &str,
    dependency_source: &str,
    source: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let dependency_input = directory.path().join("dependency.rs");
    let library = directory.path().join(format!("lib{dependency}.rlib"));
    std::fs::write(&dependency_input, dependency_source)?;
    let mut command = generated_rust_rustc_named("lib", dependency)?;
    let built = command.arg(&dependency_input).arg("-o").arg(&library).output()?;
    if !built.status.success() {
        return Err(format!("{}\n{dependency_source}", String::from_utf8_lossy(&built.stderr)).into());
    }
    let input = directory.path().join("fixture.rs");
    let program = directory.path().join("fixture");
    std::fs::write(&input, source)?;
    let mut command = generated_rust_rustc("bin")?;
    let built = command
        .arg("--extern")
        .arg(format!("{dependency}={}", library.display()))
        .arg(&input)
        .arg("-o")
        .arg(&program)
        .output()?;
    if !built.status.success() {
        return Err(format!("{}\n{source}", String::from_utf8_lossy(&built.stderr)).into());
    }
    let ran = std::process::Command::new(&program).output()?;
    if !ran.status.success() {
        return Err(format!("program failed: {}\n{source}", String::from_utf8_lossy(&ran.stderr)).into());
    }
    Ok(String::from_utf8(ran.stdout)?)
}

/// Build a generated program of several source modules, its root and each module at its module path, run it, and
/// return its standard output.
///
/// Each module is written where Rust looks for it, and every module directory declares the modules below it, as a
/// generated project does.
pub(super) fn run_generated_modules(
    root: &str,
    modules: &std::collections::HashMap<Vec<String>, String>,
) -> Result<String, Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let mut children: std::collections::BTreeMap<Vec<String>, std::collections::BTreeSet<String>> =
        std::collections::BTreeMap::new();
    for (path, source) in modules {
        let Some((leaf, parent)) = path.split_last() else {
            continue;
        };
        let mut file = directory.path().to_path_buf();
        file.extend(parent);
        std::fs::create_dir_all(&file)?;
        std::fs::write(file.join(format!("{leaf}.rs")), source)?;
        for depth in 0..path.len() {
            children
                .entry(path[..depth].to_vec())
                .or_default()
                .insert(path[depth].clone());
        }
    }
    let declarations = |parent: &[String]| {
        children
            .get(parent)
            .map(|names| {
                names
                    .iter()
                    .map(|name| format!("pub mod {name};\n"))
                    .collect::<String>()
            })
            .unwrap_or_default()
    };
    for parent in children.keys().filter(|parent| !parent.is_empty()) {
        if modules.contains_key(parent) {
            continue;
        }
        let mut file = directory.path().to_path_buf();
        file.extend(parent);
        std::fs::write(file.join("mod.rs"), declarations(parent))?;
    }
    let input = directory.path().join("main.rs");
    let program = directory.path().join("program");
    let root = root.replacen("// __INCAN_INSERT_MODS__", &declarations(&[]), 1);
    std::fs::write(&input, &root)?;
    let mut command = generated_rust_rustc("bin")?;
    let built = command.arg(&input).arg("-o").arg(&program).output()?;
    if !built.status.success() {
        return Err(format!("{}\n{root}\n{modules:?}", String::from_utf8_lossy(&built.stderr)).into());
    }
    let ran = std::process::Command::new(&program).output()?;
    if !ran.status.success() {
        return Err(format!("program failed: {}\n{root}", String::from_utf8_lossy(&ran.stderr)).into());
    }
    Ok(String::from_utf8(ran.stdout)?)
}

/// Return a `rustc` invocation for one generated crate of `crate_type`, with the runtime crates the generated code
/// names in scope.
fn generated_rust_rustc(crate_type: &str) -> Result<std::process::Command, Box<dyn std::error::Error>> {
    generated_rust_rustc_named(crate_type, "mut_ownership_fixture")
}

/// Return a `rustc` invocation for one generated crate of `crate_type` named `crate_name`, with the runtime crates
/// the generated code names in scope.
fn generated_rust_rustc_named(
    crate_type: &str,
    crate_name: &str,
) -> Result<std::process::Command, Box<dyn std::error::Error>> {
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
    command.arg("--edition=2024").arg(format!("--crate-name={crate_name}"));
    command.arg(format!("--crate-type={crate_type}"));
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
        let build = target_profile.join("build");
        for output_directory in build_output_directories(&build)? {
            command
                .arg("-L")
                .arg(format!("dependency={}", output_directory.display()));
        }
        let stdlib = newest_build_artifact(&build, |name| {
            name.starts_with("libincan_std_core-") && name.ends_with(".rlib")
        })?
        .ok_or("generated-Rust proof requires a compiled incan_std_core artifact")?;
        command
            .arg("--extern")
            .arg(format!("incan_std_core={}", stdlib.display()));
        let derive = newest_build_artifact(&build, |name| {
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
    Ok(command)
}

/// Find the newest matching dependency artifact beside the current test binary.
fn newest_build_artifact(
    build_directory: &std::path::Path,
    predicate: impl Fn(&str) -> bool,
) -> Result<Option<std::path::PathBuf>, std::io::Error> {
    let mut matches = Vec::new();
    for output_directory in build_output_directories(build_directory)? {
        for entry in std::fs::read_dir(output_directory)? {
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

/// #1852: `dict(source)`, `set(source)`, `list(source)` and `[*source]` copy a collection, owned or reached through a
/// `mut` parameter, and a change to the copy leaves the source as it was. The program is built and run, so the
/// copies are what rustc accepts and what the program prints.
#[test]
fn collection_copies_from_mut_parameters_are_owned_issue1852() -> TestResult {
    let rust = generated_rust(
        r#"
def copies(mut table: dict[str, int], mut tags: set[str], mut items: list[int]) -> int:
    mut b = dict(table)
    b["z"] = 9
    c = set(tags)
    d = [*items]
    e = set(items)
    f = list(items)
    return len(b) + len(c) + len(d) + len(e) + len(f) + len(table)


def owned(table: dict[str, int], tags: set[str]) -> int:
    b = dict(table)
    c = set(tags)
    return len(b) + len(c) + len(table) + len(tags)


def main() -> None:
    mut table = {"a": 1, "b": 2}
    mut tags = {"x", "y"}
    mut items = [1, 2, 2]
    println(copies(table, tags, items))
    println(owned({"a": 1}, {"x"}))
    println(len(table))
"#,
    )?;
    assert_eq!(run_generated_program(&rust)?, "15\n4\n2\n");
    Ok(())
}

/// #1852: a `*` or `**` spread in a literal copies a list or dict the program reads again, whether it is a binding, a
/// field or a `mut` parameter, and consumes it only at its last use.
#[test]
fn spread_literals_copy_a_collection_read_again_issue1852() -> TestResult {
    let rust = generated_rust(
        r#"
class Bag:
    items: list[int]

    def doubled(self) -> int:
        more = [*self.items, *self.items]
        return len(more) + len(self.items)


def literal_spreads(table: dict[str, int], items: list[int], mut extra: list[int]) -> int:
    mut both = {**table, "z": 9}
    both["y"] = 8
    mut joined = [*items, *extra, 4]
    joined.append(5)
    last = [*items]
    return len(both) + len(joined) + len(table) + len(extra) + len(last)


def main() -> None:
    mut extra = [7]
    println(literal_spreads({"a": 1}, [1, 2], extra))
    println(Bag(items=[1, 2]).doubled())
    println(len(extra))
"#,
    )?;
    assert_eq!(run_generated_program(&rust)?, "12\n6\n1\n");
    Ok(())
}

#[test]
fn task_handle_field_is_refused_before_loop_lowering_issue1853() -> TestResult {
    let result = compact_rust(
        r#"
from std.async import spawn
from std.async.task import JoinHandle

class Pool:
    handles: list[JoinHandle[int]]

    async def wait_all(mut self) -> None:
        for handle in self.handles:
            match await handle:
                Ok(value) => println(value)
                Err(_) => println("join failed")
"#,
    );
    let error = match result {
        Ok(rust) => return Err(format!("expected JoinHandle field refusal, generated: {rust}").into()),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("does not support Clone and Debug"),
        "{error}"
    );
    Ok(())
}

#[test]
fn mut_method_on_list_element_borrows_the_element_issue1857() -> TestResult {
    let rust = generated_rust(
        r#"
def main() -> None:
    mut rows: list[list[int]] = [[1]]
    rows[0].append(9)
    println(len(rows[0]))
"#,
    )?;
    compile_generated_rust(&rust)?;
    let rust = compact(&rust);
    assert!(rust.contains("list_get_mut(&mutrows,(0)asi64)).push(9)"), "{rust}");
    assert!(!rust.contains("list_get(&rows,0).clone().push(9)"), "{rust}");
    Ok(())
}

#[test]
fn tuple_index_after_list_index_has_no_bad_deref_issue1861() -> TestResult {
    let rust = generated_rust(
        r#"
def main() -> None:
    pts: list[tuple[float, float]] = [(1.5, 2.5)]
    println(pts[0][0])
"#,
    )?;
    compile_generated_rust(&rust)?;
    let rust = compact(&rust);
    assert!(
        rust.contains("(*incan_std_core::collections::list_get(&pts,(0)asi64)).0"),
        "{rust}"
    );
    Ok(())
}

#[test]
fn derived_iterables_keep_mutable_elements_live_issue1863() -> TestResult {
    let rust = generated_rust(
        r#"
def main() -> None:
    mut rows: list[list[int]] = [[0]]
    for i, row in enumerate(rows):
        row.append(i)
    mut other: list[list[int]] = [[1]]
    for left, right in zip(rows, other):
        left.append(len(right))
    mut table: dict[str, list[int]] = {"x": [1]}
    for value in table.values():
        value.append(2)
    mut groups: list[list[list[int]]] = [[[3]]]
    for row in groups[0]:
        row.append(4)
"#,
    )?;
    compile_generated_rust(&rust)?;
    let rust = compact(&rust);
    assert!(!rust.contains("value.clone()"), "{rust}");
    assert!(!rust.contains(".values().cloned()"), "{rust}");
    assert!(!rust.contains("list_get(&groups,0).clone()"), "{rust}");
    Ok(())
}

#[test]
fn changed_match_comprehension_and_closure_bindings_build_issue1864() -> TestResult {
    let rust = generated_rust(
        r#"
def main() -> None:
    mut rows: list[list[int]] = [[1, 2]]
    popped = [row.pop() for row in rows]
    match rows[0]:
        xs => xs.append(3)
    mut items: list[int] = []
    count = () => len(items)
    items.append(1)
    println(count() + len(popped))
"#,
    )?;
    compile_generated_rust(&rust)?;
    let rust = compact(&rust);
    assert!(rust.contains("iter_mut()).map(|mutrow|"), "{rust}");
    // The arm changes the element through `xs`, so the element is matched in place and the change reaches `rows`
    // (#1561).
    assert!(rust.contains("list_get_mut(&mutrows,"), "{rust}");
    assert!(rust.contains("__incan_in_place_xs=>"), "{rust}");
    assert!(rust.contains("letitems=items.clone();move||"), "{rust}");
    Ok(())
}

/// A loop that changes each list element through a helper taking the element mutably (`pop`) binds the `&mut` item
/// `mut`, so the helper can borrow it; a loop destructuring tuple items keeps its elements unmarked (#1869).
#[test]
fn loop_popping_each_element_builds_issue1869() -> TestResult {
    let rust = generated_rust(
        r#"
def main() -> None:
    mut rows: list[list[int]] = [[1, 2], [3]]
    for row in rows:
        row.pop()
    println(len(rows[0]))
"#,
    )?;
    compile_generated_rust(&rust)?;
    let rust = compact(&rust);
    assert!(rust.contains("formutrowinrows.iter_mut()"), "{rust}");
    Ok(())
}

/// A comprehension whose filter alone mutates the binding iterates the source in place, so the change lands in the
/// source rather than in a copy of each row (#1864).
#[test]
fn comprehension_filter_mutation_reaches_the_source_issue1864() -> TestResult {
    let rust = generated_rust(
        r#"
def main() -> None:
    mut rows: list[list[int]] = [[1, 2]]
    sizes = [len(row) for row in rows if row.pop() > 0]
    println(len(sizes) + len(rows[0]))
"#,
    )?;
    compile_generated_rust(&rust)?;
    let rust = compact(&rust);
    assert!(rust.contains("iter_mut()).filter_map(|mutrow|"), "{rust}");
    assert!(!rust.contains("(*row).clone()"), "{rust}");
    Ok(())
}

/// A closure reaches a module function by path rather than capturing it as a local, a capturing closure passed to a
/// `Result` combinator still takes its parameter type from the combinator, and a `str` match keeps the binding its
/// literal arms' guards compare (#1864).
#[test]
fn closure_module_function_reads_and_str_literal_arms_build_issue1864() -> TestResult {
    let rust = generated_rust(
        r#"
model Failure:
    detail: str

    def message(self) -> str:
        return self.detail

def describe(key: str, code: int) -> str:
    return f"{key}:{code}"

def lookup(key: str) -> Result[str, Failure]:
    return Err(Failure(detail=key))

def fetch(key: str) -> Result[str, str]:
    result = lookup(key).map_err((failure) => describe(key, len(failure.message())))
    println(key)
    return result

def classify(code: int) -> int:
    token = describe("*", code)
    match token:
        case "*:0":
            return 1
        case "?:0":
            return 2
        case other:
            return len(other)

def main() -> None:
    println(classify(0))
    match fetch("k"):
        Ok(value) => println(value)
        Err(error) => println(error)
"#,
    )?;
    compile_generated_rust(&rust)?;
    let rust = compact(&rust);
    assert!(!rust.contains("letdescribe=describe.clone();"), "{rust}");
    assert!(rust.contains("letkey=key.clone();"), "{rust}");
    Ok(())
}

#[test]
fn loop_and_mut_receiver_shapes_are_rust_valid_issue1869() -> TestResult {
    let rust = generated_rust(
        r#"
class Cell:
    value: int
    def tag(mut self) -> None:
        self.value += 1

class Grid:
    pub cells: list[Cell]

def grow(mut pairs: list[tuple[list[int], int]]) -> int:
    for xs, n in pairs:
        xs.append(n)
    return len(pairs)

def main() -> None:
    mut pairs = [([1], 2)]
    println(grow(pairs))
    mut g = Grid(cells=[Cell(value=1)])
    for c in g.cells:
        if true:
            c.tag()
"#,
    )?;
    compile_generated_rust(&rust)?;
    let rust = compact(&rust);
    assert!(!rust.contains("for(mutxs,n)inpairs.iter_mut()"), "{rust}");
    assert!(rust.contains("g.cells.iter_mut()"), "{rust}");
    Ok(())
}

#[test]
fn mut_parameter_copy_in_generic_function_states_clone_issue1874() -> TestResult {
    let rust = generated_rust(
        r#"
def count[T](mut row: list[T]) -> int:
    return len(row)

pub def total[T](grid: list[list[T]]) -> int:
    mut n = 0
    for row in grid:
        n += count(row)
    return n
"#,
    )?;
    compile_generated_rust(&rust)?;
    let rust = compact(&rust);
    assert!(rust.contains("<T:Clone,>"), "{rust}");
    Ok(())
}

/// A loop over `zip(left, right)` that changes only the items of one operand iterates only that operand in place; the
/// other operand is read as a loop over it alone reads it, so an immutable list, a list of `int`, a list of `str` and a
/// `range` all build beside it, on either side.
#[test]
fn zip_loop_iterates_only_the_changed_operand_in_place() -> TestResult {
    let rust = generated_rust(
        r#"
def main() -> None:
    mut rows: list[list[int]] = [[1], [2]]
    extra: list[int] = [3, 4]
    for row, n in zip(rows, extra):
        row.append(n)
    mut more: list[int] = [5, 6]
    for row, n in zip(rows, more):
        row.append(n)
    for row, i in zip(rows, range(2)):
        row.append(i)
    names: list[str] = ["a", "bc"]
    for row, name in zip(rows, names):
        row.append(len(name))
    for i, row in zip(range(2), rows):
        row.append(i)
    println(len(rows[0]) + len(more) + len(extra))
"#,
    )?;
    compile_generated_rust(&rust)?;
    let rust = compact(&rust);
    assert!(!rust.contains("extra).iter_mut()"), "{rust}");
    assert!(!rust.contains(".iter_mut()).iter_mut()"), "{rust}");
    Ok(())
}

/// A loop whose derived items are changed in place reads each `Copy` element of a tuple item by value, as a loop over
/// the list alone does (#1869): over a dict's values and over a list read out of another list. A list of tuples zipped
/// beside a changed list is read as a loop over it alone reads it.
#[test]
fn in_place_derived_loop_items_copy_their_copy_tuple_elements() -> TestResult {
    let rust = generated_rust(
        r#"
def main() -> None:
    mut table: dict[str, tuple[list[int], int]] = {"x": ([1], 2)}
    for xs, n in table.values():
        xs.append(n)
    mut groups: list[list[tuple[list[int], int]]] = [[([1], 2)]]
    for xs, n in groups[0]:
        xs.append(n)
    pairs: list[tuple[list[int], int]] = [([1], 2)]
    mut rows: list[list[int]] = [[3]]
    for pair, row in zip(pairs, rows):
        row.append(pair[1])
    println(len(rows[0]) + len(groups[0][0][0]) + len(pairs))
"#,
    )?;
    compile_generated_rust(&rust)
}

/// A `mut self` method called on a loop item inside a larger expression (a call argument, a list element, an
/// f-string, a field of the item) iterates the list in place, as the same call written as its own statement does.
#[test]
fn mut_self_call_inside_a_larger_expression_iterates_in_place() -> TestResult {
    let rust = generated_rust(
        r#"
class Cell:
    pub value: int

    def bump(mut self) -> int:
        self.value += 1
        return self.value

class Grid:
    pub inner: Cell

def show(n: int) -> int:
    return n

def main() -> None:
    mut cells: list[Cell] = [Cell(value=1)]
    for c in cells:
        println(c.bump())
    for c in cells:
        show(c.bump())
    for c in cells:
        values = [c.bump()]
        println(len(values))
    for c in cells:
        println(f"{c.bump()}")
    for i, c in enumerate(cells):
        println(i + c.bump())
    mut grids: list[Grid] = [Grid(inner=Cell(value=2))]
    for g in grids:
        println(g.inner.bump())
    println(cells[0].value + grids[0].inner.value)
"#,
    )?;
    compile_generated_rust(&rust)?;
    let rust = compact(&rust);
    assert!(!rust.contains("incells.iter()"), "{rust}");
    assert!(!rust.contains("ingrids.iter()"), "{rust}");
    Ok(())
}

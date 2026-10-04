//! Checked programs whose `mut` parameters the checker now classifies by what the body does to them (#1561): a
//! `Generator` the callee advances, and parameters read through a pattern binding or a plain-`self` trait method. Each
//! test builds the generated Rust with rustc and runs it, so the accepted shapes are proved to build and to behave.

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

/// Generate a program, build it as an executable against the runtime this test binary links, run it, and return its
/// standard output, so both the build and the behavior are rustc's and the program's own.
fn build_and_run(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let rust = generate(source)?;
    let directory = tempfile::tempdir()?;
    let input = directory.path().join("main.rs");
    let binary = directory
        .path()
        .join(format!("mut_parameter_changes{}", std::env::consts::EXE_SUFFIX));
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
        "--crate-type=bin",
        "--crate-name=mut_parameter_changes",
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
    let build = command.arg(&input).arg("-o").arg(&binary).output()?;
    assert!(
        build.status.success(),
        "{}\n{rust}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = std::process::Command::new(&binary).output()?;
    assert!(run.status.success(), "{}\n{rust}", String::from_utf8_lossy(&run.stderr));
    Ok(String::from_utf8(run.stdout)?)
}

/// #1561: iterating a `Generator` parameter advances the caller's generator, directly and through a function that
/// passes it on, so the second call sees the next item. An immutable binding is refused for such a parameter
/// (`INCAN-T0117`), since the generator can be neither changed nor copied there.
#[test]
fn generator_parameter_advanced_by_the_callee_reaches_the_caller_issue1561() -> TestResult {
    let callees = r#"
def numbers(limit: int) -> Generator[int]:
    for value in range(limit):
        yield value

def first(mut g: Generator[int]) -> int:
    for v in g:
        return v
    return -1

def outer(mut g: Generator[int]) -> int:
    return first(g)
"#;
    let stdout = build_and_run(&format!(
        "{callees}\ndef main() -> None:\n    mut g = numbers(4)\n    println(first(g))\n    println(outer(g))\n    println(first(g))\n"
    ))?;
    assert_eq!(stdout, "0\n1\n2\n");
    let refused = generate(&format!(
        "{callees}\ndef main() -> None:\n    g = numbers(4)\n    println(first(g))\n"
    ));
    let Err(error) = refused else {
        return Err("an immutable generator passed to a parameter that advances it must be refused".into());
    };
    assert!(format!("{error:?}").contains("INCAN-T0117"), "{error:?}");
    Ok(())
}

/// #1561: a `mut` parameter the body only reads, through a `match` binding, a plain-`self` trait method or a method on
/// `Self` in a trait default, takes a copy of an immutable binding; the copy builds and the caller's value is
/// unchanged.
#[test]
fn parameter_read_through_a_pattern_or_a_trait_method_takes_a_copy_issue1561() -> TestResult {
    let stdout = build_and_run(
        r#"
trait Peek:
    def peek(self) -> int

    def same(self, mut other: Self) -> bool:
        return self.peek() == other.peek()

    def doubled(self) -> int:
        return self.peek() * 2

class Counter with Peek:
    n: int

    def peek(self) -> int:
        return self.n

def total(mut box: Option[list[int]]) -> int:
    match box:
        Some(xs) => return len(xs)
        None => return 0

def twice(mut c: Counter) -> int:
    return c.doubled()

def main() -> None:
    box = Some([0, 1])
    println(total(box))
    a = Counter(n=3)
    b = Counter(n=3)
    println(twice(a))
    println(a.same(b))
    println(a.peek() + b.peek())
"#,
    )?;
    assert_eq!(stdout, "2\n6\ntrue\n6\n");
    Ok(())
}

/// #1561: a `match`, `if let` or `while let` whose arm changes a `mut` parameter through a name its pattern binds
/// matches the parameter in place, so the change reaches the caller. That holds for the parameter itself, a field of
/// it, the variable of a loop over it, a name an enclosing arm bound, a value a `match` expression produces, an arm
/// with a guard, a copyable value bound beside the changed one, and a field of `self` in a `mut self` method. Before,
/// the pattern bound a `mut` copy (rustc E0507 over the parameter itself, the change silently lost through a field or a
/// loop variable).
#[test]
fn change_through_a_pattern_binding_reaches_the_caller_issue1561() -> TestResult {
    let stdout = build_and_run(
        r#"
model Holder:
    pub inner: Option[list[int]]

enum Slot:
    Pair(list[int], int)
    Empty

class Grid:
    pub rows: list[Option[list[int]]]

    def fill(mut self) -> None:
        for row in self.rows:
            if let Some(xs) = row:
                xs.append(7)
        match self.rows[0]:
            Some(xs) => println(len(xs))
            None => pass

def by_match(mut box: Option[list[int]]) -> None:
    match box:
        Some(xs) => xs.append(1)
        None => pass

def by_if_let(mut box: Option[list[int]]) -> None:
    if let Some(xs) = box:
        xs.append(2)

def by_while_let(mut box: Option[list[int]]) -> None:
    while let Some(xs) = box:
        xs.append(3)
        break

def by_field(mut h: Holder) -> None:
    match h.inner:
        Some(xs) => xs.append(4)
        None => pass

def by_loop_view(mut rows: list[Option[list[int]]]) -> None:
    for row in rows:
        if let Some(xs) = row:
            xs.append(5)

def by_nested(mut box: Option[Option[list[int]]]) -> None:
    match box:
        Some(inner) =>
            match inner:
                Some(xs) => xs.append(6)
                None => pass
        None => pass

def popped(mut box: Option[list[int]]) -> int:
    n = match box:
        Some(xs) => xs.pop()
        None => 0
    return n

def guarded(mut box: Option[list[int]], limit: int) -> None:
    match box:
        Some(xs) if len(xs) < limit => xs.append(len(xs))
        _ => pass

def paired(mut s: Slot) -> int:
    match s:
        Slot.Pair(items, n) =>
            items.append(n)
            return n + len(items)
        Slot.Empty => return 0

def main() -> None:
    mut box = Some([0])
    by_match(box)
    by_if_let(box)
    by_while_let(box)
    println(box)
    mut h = Holder(inner=Some([0]))
    by_field(h)
    println(h.inner)
    mut rows = [Some([0]), None]
    by_loop_view(rows)
    println(rows)
    mut nested = Some(Some([0]))
    by_nested(nested)
    println(nested)
    println(popped(box))
    println(box)
    guarded(box, 5)
    guarded(box, 1)
    println(box)
    mut s = Slot.Pair([1], 5)
    println(paired(s))
    println(paired(s))
    mut g = Grid(rows=[Some([1]), None])
    g.fill()
    println(g.rows)
"#,
    )?;
    assert_eq!(
        stdout,
        "Some([0, 1, 2, 3])\nSome([0, 4])\n[Some([0, 5]), None]\nSome(Some([0, 6]))\n3\nSome([0, 1, 2])\nSome([0, 1, 2, 3])\n7\n8\n2\n[Some([1, 7]), None]\n"
    );
    Ok(())
}

/// #1561: a method alias called on a `mut` parameter, which the callee holds by reference, calls the method it names,
/// for a model, a class whose alias names a `mut self` method, and a newtype. Before, the alias was left unresolved on
/// the reference and rustc refused the call (E0599).
#[test]
fn method_alias_on_a_mut_parameter_calls_its_target_issue1561() -> TestResult {
    let stdout = build_and_run(
        r#"
model Reading:
    n: int
    look = peek

    def peek(self) -> int:
        return self.n

class Counter:
    pub n: int
    grow = add

    def add(mut self) -> None:
        self.n += 1

type Meters = newtype int:
    def value(self) -> int:
        return self.0
    get = value

def show(mut r: Reading) -> int:
    return r.look()

def bump(mut c: Counter) -> None:
    c.grow()

def measure(mut m: Meters) -> int:
    return m.get()

def main() -> None:
    r = Reading(n=1)
    println(show(r))
    mut c = Counter(n=1)
    bump(c)
    bump(c)
    println(c.n)
    m = Meters(3)
    println(measure(m))
"#,
    )?;
    assert_eq!(stdout, "1\n3\n3\n");
    Ok(())
}

/// #1561: a change through a name a `match`, `if let` or `while let` pattern binds from a `mut` local, a field or list
/// element of one, or the variable of a loop over one changes that local, as RFC 129 requires of a writable place.
/// Before, the pattern bound a copy and the change was silently lost.
#[test]
fn change_through_a_pattern_binding_of_a_mut_local_reaches_it_issue1561() -> TestResult {
    let stdout = build_and_run(
        r#"
model Holder:
    pub inner: Option[list[int]]
    pub slots: list[Option[list[int]]]

def main() -> None:
    mut box = Some([1])
    match box:
        Some(xs) => xs.append(2)
        None => pass
    if let Some(xs) = box:
        xs.append(3)
    println(box)
    mut h = Holder(inner=Some([1]), slots=[Some([1])])
    if let Some(xs) = h.inner:
        xs.append(4)
    while let Some(xs) = h.slots[0]:
        xs.append(5)
        break
    println(h.inner)
    println(h.slots)
    mut rows = [Some([1]), None]
    match rows[0]:
        Some(xs) => xs.append(6)
        None => pass
    for row in rows:
        if let Some(xs) = row:
            xs.append(7)
    println(rows)
    kept = Some([1])
    match kept:
        Some(xs) => println(len(xs))
        None => pass
    println(kept)
"#,
    )?;
    assert_eq!(
        stdout,
        "Some([1, 2, 3])\nSome([1, 4])\n[Some([1, 5])]\n[Some([1, 6, 7]), None]\n1\nSome([1])\n"
    );
    Ok(())
}

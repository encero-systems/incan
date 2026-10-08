//! Standard-library builtins, carriers, and provider-crate items on the direct route, compared with legacy.

use super::*;

/// Compile one program on both routes, run each binary in its own copy of one scratch directory, and require the same
/// exit status, stdout, and stderr; the native observables are returned for exact assertions.
fn stdlib_parity(fixture: &DriverFixture, name: &str, program: &str) -> Result<Output, Box<dyn std::error::Error>> {
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
        "native stdlib compilation",
    );
    let legacy = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy)
            .output()?,
        "legacy stdlib compilation",
    );
    let legacy_run = root.join("legacy-run");
    let native_run = root.join("native-run");
    fs::create_dir_all(&legacy_run)?;
    fs::create_dir_all(&native_run)?;
    let expected = Command::new(legacy.join("oven/release").join(name))
        .current_dir(&legacy_run)
        .output()?;
    let actual = Command::new(native).current_dir(&native_run).output()?;
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(
        String::from_utf8_lossy(&actual.stderr),
        String::from_utf8_lossy(&expected.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&actual.stdout),
        String::from_utf8_lossy(&expected.stdout)
    );
    Ok(actual)
}

/// Run every standard-library case in one test process, so the driver graph is baked once for all of them.
///
/// Each case compares one program's exit status, stdout and stderr with legacy; the first difference fails the test
/// with that case's assertion.
#[test]
fn direct_route_stdlib_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    builtins_match_legacy(fixture)?;
    option_model_fields_match_legacy(fixture)?;
    Ok(())
}

/// min and max over each admitted list element, the empty-sequence panic, and read_file/write_file results and host
/// error text match legacy.
fn builtins_match_legacy(fixture: &DriverFixture) -> Result<(), Box<dyn std::error::Error>> {
    let values = stdlib_parity(
        fixture,
        "stdlib_builtins",
        r#"
def encoded(values: list[str]) -> str:
    return json_stringify(values)


def main() -> None:
    println(json_stringify([1, 2]))
    println(json_stringify([1.5, 2.0]))
    println(json_stringify([true, false]))
    println(encoded(["a\"b", "line\n"]))
    println(json_stringify([[1], [2, 3]]))
    ints = [3, -1, 4, -1, 5]
    floats = [2.5, -0.5, 9.0]
    flags = [true, false, true]
    words = ["pear", "apple", "zoo"]
    println(min(ints))
    println(max(ints))
    println(min(floats))
    println(max(floats))
    println(min(flags))
    println(max(flags))
    mut lowest = min(words)
    highest: str = max(words)
    println(f"{lowest} {highest} {len(words)}")
    lowest = max(words)
    println(lowest)
    match write_file("probe.txt", "hello file"):
        Ok(_) => println("written")
        Err(error) => println(f"write failed: {error}")
    path = "probe.txt"
    match read_file(path):
        Ok(data) => println(data)
        Err(error) => println(f"read failed: {error}")
    missing = read_file("missing/probe.txt")
    match missing:
        Ok(data) => println(f"unexpected: {data}")
        Err(error) => println(error)
    match write_file("missing/probe.txt", "text"):
        Ok(_) => println("unexpected write")
        Err(error) => println(error)
"#,
    )?;
    let stdout = String::from_utf8(values.stdout)?;
    assert!(stdout.starts_with("[1,2]\n[1.5,2.0]\n[true,false]\n[\"a\\\"b\",\"line\\n\"]\n[[1],[2,3]]\n-1\n5\n-0.5\n9.0\nfalse\ntrue\napple zoo 3\nzoo\nwritten\nhello file\n"), "{stdout}");
    let empty = stdlib_parity(
        fixture,
        "stdlib_empty_min",
        r#"
def main() -> None:
    values: list[int] = []
    println("before")
    println(max(values))
"#,
    )?;
    assert!(!empty.status.success());
    Ok(())
}

/// Option model fields of owned text, integer, and list payloads match legacy through defaulted construction, explicit
/// `Some` values, field reads, matches, and fallbacks, and a defaulted Option parameter matches legacy when omitted.
fn option_model_fields_match_legacy(fixture: &DriverFixture) -> Result<(), Box<dyn std::error::Error>> {
    let output = stdlib_parity(
        fixture,
        "option_model_fields",
        r#"
model Profile:
    name: str
    nick: Option[str] = None
    count: Option[int] = None
    tags: Option[list[int]] = None


def label(profile: Profile) -> str:
    nick = profile.nick
    match nick:
        Some(value) => return value
        None => return "anonymous"


def greeting(nick: Option[str] = None) -> str:
    match nick:
        Some(value) => return value
        None => return "nobody"


def main() -> None:
    first = Profile(name="ada")
    second = Profile(name="bob", nick=Some("b"), count=Some(3), tags=Some([1, 2]))
    count = second.count
    println(count.unwrap_or(0))
    missing = first.count
    println(missing.unwrap_or(-1))
    tags = second.tags
    match tags:
        Some(values) => println(len(values))
        None => println("no tags")
    println(second.name)
    println(label(first))
    println(label(second))
    println(greeting())
    println(greeting(Some("cy")))
"#,
    )?;
    assert_eq!(
        String::from_utf8(output.stdout)?,
        "3\n-1\n2\nbob\nanonymous\nb\nnobody\ncy\n"
    );
    Ok(())
}

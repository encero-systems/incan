//! The direct route's long tail of small statements, operators and places, each compared byte for byte with legacy.

use super::*;

/// Build one program natively and through legacy, requiring identical streams and exit codes; returns the native run.
fn compare_with_legacy(name: &str, text: &str) -> Result<Output, Box<dyn std::error::Error>> {
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
        "native tail compilation",
    );
    let legacy_root = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy tail compilation",
    );
    let expected = Command::new(legacy_root.join("oven/release").join(name)).output()?;
    let actual = Command::new(native).output()?;
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
    assert_eq!(actual.status.code(), expected.status.code());
    Ok(actual)
}

/// Require a successful native run whose output streams and exit code equal legacy's.
fn check_case(name: &str, text: &str) -> Result<(), Box<dyn std::error::Error>> {
    success(&compare_with_legacy(name, text)?, "native tail execution");
    Ok(())
}

/// Integer bit operators keep their operand type, shifts keep their left type and mask their count, and `~`
/// complements, exactly as legacy's infix Rust operators do.
#[test]
fn bit_operators_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_case(
        "bit_operators",
        r#"
def shift(value: int, count: int) -> int:
    return value << count

def main() -> None:
    a: u8 = 12
    b: u8 = 10
    both: u8 = a & b
    either: u8 = a | b
    differ: u8 = a ^ b
    flipped: u8 = ~b
    println(f"{both} {either} {differ} {flipped}")
    small: i16 = 10
    count: int = 2
    wide: int = small << 2
    narrow: i16 = small >> count
    byte: u8 = 12
    shifted: int = 1 << byte
    println(f"{wide} {narrow} {shifted}")
    mut flags: u8 = 0
    flags |= 3
    flags &= b
    flags ^= 1
    flags <<= 2
    flags >>= count
    println(flags)
    x = 6
    y = 3
    println(x & y)
    println(x | y)
    println(x ^ y)
    println(~(1 | 2))
    println(-17 >> 2)
    println(shift(1, 65))
    println(shift(3, 63))
"#,
    )
}

/// A one-argument dictionary `get` copies a present value into its Option or reports absence, as legacy does.
#[test]
fn optional_dictionary_get_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_case(
        "optional_dictionary_get",
        r#"
def lookup(counts: dict[str, int], word: str) -> str:
    match counts.get(word):
        Some(count) => return f"{word}={count}"
        None => return f"{word} missing"

def first_name(names: Dict[int, str], id: int) -> str:
    return names.get(id).unwrap_or("nobody")

def main() -> None:
    counts: dict[str, int] = {"the": 2, "cat": 1}
    println(lookup(counts, "the"))
    println(lookup(counts, "dog"))
    mut hits: Dict[str, int] = {}
    hits["a"] = 1
    println(hits.get("a").unwrap_or(0))
    println(hits.get("b").unwrap_or(0))
    names: Dict[int, str] = {1: "ada"}
    println(first_name(names, 1))
    println(first_name(names, 2))
    mut found = names.get(1)
    println(found.unwrap_or("none"))
    found = names.get(3)
    println(found.unwrap_or("none"))
"#,
    )
}

/// Carrier-pattern assertions bind payloads on success and fail through the same std.testing helpers as legacy.
#[test]
fn pattern_assertions_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_case(
        "pattern_assertions",
        r#"
def unwrap_value(value: Option[int]) -> int:
    assert value is Some(inner)
    return inner

def label(value: Option[str]) -> str:
    assert value is Some(text), "needs text"
    return text

def check(result: Result[int, str]) -> int:
    assert result is Ok(number)
    return number

def reason(result: Result[int, str]) -> str:
    assert result is Err(error)
    return error

def main() -> None:
    println(unwrap_value(Some(42)))
    assert Some(43) is Some(found)
    println(found)
    println(label(Some("x")))
    println(check(Ok(5)))
    println(reason(Err("bad")))
    empty: Option[int] = None
    assert empty is None
    assert Some(1) is Some(_)
    println("done")
"#,
    )?;
    for (name, source, message) in [
        (
            "pattern_assert_some",
            "def main() -> None:\n    println(\"before\")\n    empty: Option[int] = None\n    assert empty is Some(value)\n    println(value)\n",
            "AssertionError: expected Some, got None",
        ),
        (
            "pattern_assert_none",
            "def main() -> None:\n    value: Option[str] = Some(\"x\")\n    assert value is None\n",
            "AssertionError: expected None, got Some",
        ),
        (
            "pattern_assert_ok",
            "def main() -> None:\n    value: Result[int, str] = Err(\"no\")\n    assert value is Ok(_), \"custom failure\"\n",
            "AssertionError: custom failure",
        ),
        (
            "pattern_assert_err",
            "def main() -> None:\n    value: Result[int, str] = Ok(1)\n    assert value is Err(reason)\n    println(reason)\n",
            "AssertionError: expected Err, got Ok",
        ),
        (
            "pattern_assert_empty_message",
            "def main() -> None:\n    empty: Option[int] = None\n    assert empty is Some(value), \"\"\n    println(value)\n",
            "AssertionError: expected Some, got None",
        ),
    ] {
        let actual = compare_with_legacy(name, source)?;
        assert!(!actual.status.success());
        assert!(String::from_utf8_lossy(&actual.stderr).contains(message));
    }
    Ok(())
}

/// An `@rust.extern` declaration forwards to its bound generic Rust item, with the instantiation legacy infers.
#[test]
fn extern_delegation_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let actual = compare_with_legacy(
        "extern_delegation",
        r#"
rust.module("incan_std_testing")

@rust.extern
def fail_t(msg: str) -> None:
    ...

def assert_true(value: bool) -> None:
    if not value:
        fail_t("assertion failed: expected true")

def main() -> None:
    assert_true(true)
    println("before")
    assert_true(false)
    println("after")
"#,
    )?;
    assert_eq!(actual.stdout, b"before\n");
    assert!(!actual.status.success());
    Ok(())
}

/// A union with a list member keeps legacy's payload identity, injects either member and tests its tag.
#[test]
fn list_union_member_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_case(
        "list_union_member",
        r#"
def probe(value: int | List[int]) -> bool:
    println("checked")
    return isinstance(value, int)

def main() -> None:
    println(probe(1))
    println(probe([1, 2]))
    items: List[int] = [3]
    println(probe(items))
"#,
    )
}

/// List slices copy their selection through legacy's Python-style helper, including omitted, negative and stepped
/// components and the zero-step failure.
#[test]
fn list_slices_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_case(
        "list_slices",
        r#"
def program_result() -> list[int]:
    values = [40, 2]
    return values[0:1]

def show(values: list[int]) -> None:
    mut text = ""
    for value in values:
        text = text + f"{value},"
    println(text)

def main() -> None:
    show(program_result())
    values = [1, 2, 3, 4, 5]
    show(values[1:3])
    show(values[:2])
    show(values[3:])
    show(values[::2])
    show(values[-2:])
    show(values[::-1])
    start = 1
    show(values[start:start + 2])
    names = ["a", "b", "c"]
    tail = names[1:]
    println(tail[0])
    println(tail[1])
    copy = values[:]
    println(len(copy))
"#,
    )?;
    let actual = compare_with_legacy(
        "list_slice_zero_step",
        "def main() -> None:\n    values = [1, 2]\n    step = 0\n    part = values[::step]\n    println(len(part))\n",
    )?;
    assert!(!actual.status.success());
    Ok(())
}

/// A read-only text constant read by value owns a copy of its literal, as legacy's owned constant read does.
#[test]
fn text_constants_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_case(
        "text_constants",
        r#"
const LABEL: str = "orders"
const COUNT: int = 3

def make_widget(name: str) -> str:
    return f"widget:{name}"

def main() -> None:
    println(LABEL)
    println(make_widget(LABEL))
    println(f"{LABEL}-{COUNT}")
"#,
    )
}

/// An omitted list or dictionary default is built in the caller and borrowed by the call, as legacy passes it.
#[test]
fn collection_defaults_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_case(
        "collection_defaults",
        r#"
def keep(items: list[float] = []) -> int:
    return 1

def total(values: list[int] = [1, 2, 3]) -> int:
    mut sum = 0
    for value in values:
        sum += value
    return sum

def names(prefix: str, items: list[str] = ["a", "b"]) -> str:
    second = items[1]
    return prefix + second

def size(table: dict[str, int] = {"x": 1}) -> int:
    return len(table)

def main() -> None:
    println(keep())
    println(total())
    println(total([4]))
    println(names("n:"))
    println(size())
    for _index in range(2):
        println(total())
"#,
    )
}

/// Fields of a model element read out of a list copy scalars and clone owned values, as legacy's element reads do.
#[test]
fn list_element_fields_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_case(
        "list_element_fields",
        r#"
type Email = newtype str

model User:
    email: Email
    age: int
    name: str

def main() -> None:
    user = User(email=Email("ada@example.com"), age=36, name="Ada")
    mut users: list[User] = [user]
    users.append(User(email=Email("bo@example.com"), age=41, name="Bo"))
    println(users[0].email.0)
    println(users[1].age)
    println(users[1].name)
    first = users[0].name
    println(first + "!")
    println(len(users))
"#,
    )
}

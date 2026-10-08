//! The direct route's long tail of small statements, operators and places, each compared byte for byte with legacy.

use super::*;

/// Build one program natively and through legacy, requiring identical streams and exit codes; returns the native run.
fn compare_with_legacy(name: &str, text: &str) -> Result<Output, Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch(name)?;
    let source = root.join(format!("{name}.incn"));
    fs::write(&source, text)?;
    let native = root.join("native");
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
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

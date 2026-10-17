//! Live source parity for SDK reader defaults, returned layouts, cursors, and errors.

use std::process::Command;

use super::{corpus, driver_fixture, success};

const ORIGINAL: &str = include_str!(
    "../../../incan_test_support/fixtures/behavior/snapshots_traits_and_generics/std_fallible_adapter_loop.incn"
);

/// A real SDK default returns its closed generic layout and keeps the original reader's shared cursor.
#[test]
fn sdk_reader_default_return_and_cursor_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    reader_parity(
        "sdk-reader-default-cursor",
        r#"from std.io import BinaryReader, BytesIO

def main() -> None:
    reader = BytesIO(b"abcd")
    stream = reader.chunks(2)
    println(stream.chunk_size)
    match stream.reader.read_bytes(1):
        Ok(value) => println(len(value))
        Err(error) => println(error.detail)
    println(reader.tell())
"#,
        b"2\n1\n1\n",
    )
}

/// A declared BinaryReader binder closes the same SDK default using its actual checked concrete argument.
#[test]
fn sdk_reader_default_generic_consumer_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    reader_parity(
        "sdk-reader-default-generic",
        r#"from std.io import BinaryReader, BytesIO

def consume[R with BinaryReader](reader: R) -> None:
    stream = reader.chunks(2)
    println(stream.chunk_size)

def main() -> None:
    consume(BytesIO(b"abcd"))
"#,
        b"2\n",
    )
}

/// The unchanged full stream reaches the separately owned nominal callback-input boundary after its reader default.
#[test]
fn sdk_reader_default_original_reaches_nominal_callback() -> Result<(), Box<dyn std::error::Error>> {
    reader_callback_frontier("sdk-reader-original-callback-frontier", ORIGINAL, b"16\n4\ndone\n")
}

/// The zero-size stream keeps legacy's actual error while native refuses before callback execution.
#[test]
fn sdk_reader_default_zero_size_reaches_nominal_callback() -> Result<(), Box<dyn std::error::Error>> {
    reader_callback_frontier(
        "sdk-reader-zero-callback-frontier",
        &ORIGINAL.replace("chunks(16)", "chunks(0)"),
        b"read failed: chunk_size must be greater than zero\n",
    )
}

/// Build and execute the untouched legacy route, then require exactly the later native refusal, never full parity.
fn reader_callback_frontier(name: &str, source: &str, expected: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch(name)?;
    let input = root.join("main.incn");
    let legacy = root.join("legacy");
    super::fs::write(&input, source)?;
    let build = super::support::repo_command()
        .current_dir(&root)
        .arg("build")
        .arg(&input)
        .arg(&legacy)
        .output()?;
    success(&build, "normal frozen legacy full reader reference");
    let reference = Command::new(legacy.join("oven/release/main")).output()?;
    success(&reference, "normal frozen legacy full reader execution");
    assert_eq!(reference.stdout, expected);
    assert!(
        reference.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&reference.stderr)
    );
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let result = corpus::source_command(
        &fixture.driver_binary("debug"),
        &input,
        &root.join("native"),
        &fixture.sysroot,
        &closure,
    )
    .output()?;
    let diagnostics = String::from_utf8_lossy(&result.stderr);
    super::fs::write(root.join("native.stderr"), &result.stderr)?;
    super::fs::write(root.join("legacy.stdout"), &reference.stdout)?;
    assert!(!result.status.success(), "nominal callback input remains unadmitted");
    assert!(
        diagnostics.contains("unsupported Body IR closure input outside scalars, text, collection views and borrows"),
        "{diagnostics}"
    );
    assert!(
        !diagnostics.contains("nonlocal or trait-object Method chunks"),
        "{diagnostics}"
    );
    assert!(!diagnostics.contains("internal compiler error"), "{diagnostics}");
    Ok(())
}

/// Compare live routes and separately pin the recorded fixture contract using the same already compiled binaries.
fn reader_parity(name: &str, source: &str, expected: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch(name)?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &root,
        &fixture.sysroot,
        &fixture.formatting,
        source,
    )?;
    for (label, binary) in [
        ("legacy", root.join("builtin-parity/legacy/oven/release/main")),
        ("native", root.join("builtin-parity/native")),
    ] {
        let result = Command::new(binary).output()?;
        success(&result, &format!("{label} reader output contract"));
        assert_eq!(result.stdout, expected, "{label} reader output contract");
        assert!(
            result.stderr.is_empty(),
            "{label}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    Ok(())
}

/// Closed reader layouts follow imported generic functions by full canonical identity, including same-name controls.
#[test]
fn sdk_reader_default_imported_consumer_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    super::check_project_modules_case(
        "sdk_reader_default_imported_consumer",
        r#"from readers import run
from other import consume

def main() -> None:
    run()
    consume(9)
"#,
        &[
            (
                "readers.incn",
                r#"from std.io import BinaryReader, BytesIO

pub def consume[R with BinaryReader](reader: R) -> None:
    stream = reader.chunks(2)
    println(stream.chunk_size)

pub def run() -> None:
    consume(BytesIO(b"abcd"))
"#,
            ),
            (
                "other.incn",
                r#"pub def consume[T](_value: T) -> None:
    println(9)
"#,
            ),
        ],
        b"2\n9\n",
    )
}

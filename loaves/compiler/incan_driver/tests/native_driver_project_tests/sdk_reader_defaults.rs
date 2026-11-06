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

/// The unchanged full stream preserves its selected callback behavior through both current routes.
#[test]
fn sdk_reader_default_original_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    reader_parity("sdk-reader-original-callback", ORIGINAL, b"16\n4\ndone\n")
}

/// The zero-size stream preserves the exact reader error through both current routes.
#[test]
fn sdk_reader_default_zero_size_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    reader_parity(
        "sdk-reader-zero-callback",
        &ORIGINAL.replace("chunks(16)", "chunks(0)"),
        b"read failed: chunk_size must be greater than zero\n",
    )
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

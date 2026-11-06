//! Captured SDK error mappers retain their environment and nominal input through ordinary and generic frames.

use std::process::Command;

use super::{corpus, driver_fixture, success};

const ORIGINAL: &str = include_str!(
    "../../../incan_test_support/fixtures/behavior/snapshots_traits_and_generics/std_fallible_adapter_loop.incn"
);

/// Exact captured factory source shared with the session-assembly regression; execution assertions remain separate.
pub(super) const FACTORY: &str = r#"from std.io import BinaryReader, BytesIO


pub def consume[R with BinaryReader](reader: R) -> None:
    prefix = "read failed: "
    mapped = reader.chunks(16).map_err((error) => f"{prefix}{error.detail}")
    println("created")
    println(prefix)


def main() -> None:
    consume(BytesIO(b"0123456789abcdefghij"))
"#;

/// A shared callable view receives the real SDK error and keeps its captured prefix across repeated calls.
#[test]
fn captured_sdk_error_shared_view_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    callback_parity(
        "captured-sdk-error-shared-view",
        r#"from std.io import IoError


def render(mapper: (IoError) -> str, error: IoError) -> str:
    return mapper(error)


def main() -> None:
    prefix = "mapped: "
    mapper: (IoError) -> str = (error) => f"{prefix}{error.detail}"
    println(render(mapper, IoError(kind="bad", detail="first")))
    println(render(mapper, IoError(kind="bad", detail="second")))
    println(prefix)
"#,
        b"mapped: first\nmapped: second\nmapped: \n",
    )
}

/// A generic callable value and its local alias preserve the captured environment instead of selecting pointer storage.
#[test]
fn captured_sdk_error_generic_owner_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    callback_parity(
        "captured-sdk-error-generic-owner",
        r#"from std.io import IoError
from std.traits.callable import Callable1


def render[F with (Clone, Callable1[IoError, str])](mapper: F, error: IoError) -> str:
    saved = mapper
    return saved(error)


def main() -> None:
    prefix = "owned: "
    mapper: (IoError) -> str = (error) => f"{prefix}{error.detail}"
    println(render(mapper, IoError(kind="bad", detail="first")))
    println(render(mapper, IoError(kind="bad", detail="second")))
    println(prefix)
"#,
        b"owned: first\nowned: second\nowned: \n",
    )
}

/// An adopted trait closes both sides of a callback signature before lowering its default method frame.
#[test]
fn trait_default_callable_parameters_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    callback_parity(
        "trait-default-callable-parameters",
        r#"trait Mapping[T]:
    def item(self) -> T: ...
    def apply(self, transform: (T) -> T) -> T:
        return transform(self.item())


model Number with Mapping[int]:
    value: int
    def item(self) -> int:
        return self.value


def double(value: int) -> int:
    return value * 2


def main() -> None:
    println(Number(value=7).apply(double))
"#,
        b"14\n",
    )
}

/// A generic adopter keeps its independent owner frame when a trait default returns the trait's item type.
#[test]
fn generic_adopter_default_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    callback_parity(
        "generic-adopter-default",
        r#"trait ItemSource[T]:
    def item(self) -> T: ...
    def again(self) -> T:
        return self.item()


model Reader[T] with ItemSource[T]:
    value: T
    def item(self) -> T:
        return self.value


def main() -> None:
    println(Reader[int](value=7).again())
"#,
        b"7\n",
    )
}

/// Construct the captured SDK factory without polling, keeping its generic callable ABI separate from iteration.
#[test]
fn captured_reader_factory_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    callback_parity("captured-reader-factory", FACTORY, b"created\nread failed: \n")
}

/// The complete unchanged chunk stream executes through its captured mapper with the normal legacy output.
#[test]
fn captured_reader_original_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    callback_parity("captured-reader-original", ORIGINAL, b"16\n4\ndone\n")
}

/// The original's one changed chunk size executes its captured IoError mapper and preserves the actual SDK message.
#[test]
fn captured_reader_zero_size_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(ORIGINAL.matches("chunks(16)").count(), 1);
    callback_parity(
        "captured-reader-zero-size",
        &ORIGINAL.replacen("chunks(16)", "chunks(0)", 1),
        b"read failed: chunk_size must be greater than zero\n",
    )
}

/// Compare live routes and pin complete output, empty diagnostics and successful execution for each actual binary.
fn callback_parity(name: &str, source: &str, expected: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
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
        let output = Command::new(binary).output()?;
        success(&output, &format!("{label} captured callback execution"));
        assert_eq!(output.stdout, expected, "{label} captured callback output");
        assert!(
            output.stderr.is_empty(),
            "{label}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

use std::fs;
use std::process::Command;

fn run_source_case(source: &str) -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let source_path = dir.path().join("main.incn");
    fs::write(&source_path, source)?;

    let output = Command::new(env!("CARGO_BIN_EXE_incan"))
        .arg("--no-banner")
        .arg("run")
        .arg(&source_path)
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    assert!(
        output.status.success(),
        "encoding algorithm case failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[test]
fn std_encoding_algorithm_vectors_and_invalid_cases() -> Result<(), Box<dyn std::error::Error>> {
    run_source_case(
        r#"from std.encoding.base32 import b32decode, b32decode_lenient, b32encode, b32hexencode
from std.encoding._shared import EncodingError
from std.encoding.base58 import b58decode, b58decode_lenient, b58encode
from std.encoding.base64 import b64decode, b64decode_lenient, b64encode, urlsafe_b64encode
from std.encoding.base85 import a85decode_lenient, a85encode, b85decode, b85encode, z85decode, z85encode
from std.encoding.bech32 import Bech32Variant, bech32_decode, bech32_encode, bech32m_encode, decode as bech32_decode_any

def check_base64() -> None:
    assert b64encode(b"hello") == "aGVsbG8="
    assert urlsafe_b64encode(b"\xfb\xff") == "-_8="
    match b64decode_lenient("aG Vs\nbG8="):
        Ok(data) => assert data == b"hello"
        Err(err) => assert false, err.detail
    match b64decode("@@@="):
        Ok(_) => assert false, "invalid base64 unexpectedly decoded"
        Err(err) => assert err.kind == "invalid_character"
    match b64decode("A"):
        Ok(_) => assert false, "invalid-length base64 unexpectedly decoded"
        Err(err) => assert err.kind == "invalid_length"
    match b64decode("a=AA"):
        Ok(_) => assert false, "invalid-padding base64 unexpectedly decoded"
        Err(err) => assert err.kind == "invalid_padding"

def check_base32() -> None:
    assert b32encode(b"foo") == "MZXW6==="
    assert b32hexencode(b"foo") == "CPNMU==="
    match b32decode_lenient("mz xw6==="):
        Ok(data) => assert data == b"foo"
        Err(err) => assert false, err.detail
    match b32decode("MZXW6===="):
        Ok(_) => assert false, "invalid base32 unexpectedly decoded"
        Err(err) => assert err.kind == "invalid_length" or err.kind == "invalid_padding"
    match b32decode("MZ=XW6=="):
        Ok(_) => assert false, "misplaced-padding base32 unexpectedly decoded"
        Err(err) => assert err.kind == "invalid_padding"

def check_base58() -> None:
    assert b58encode(b"hello world") == "StV1DL6CwTryKyV"
    assert b58encode(b"\x00\x00") == "11"
    match b58decode_lenient(" StV1DL6CwTryKyV\n"):
        Ok(data) => assert data == b"hello world"
        Err(err) => assert false, err.detail
    match b58decode("0"):
        Ok(_) => assert false, "invalid base58 unexpectedly decoded"
        Err(err) => assert err.kind == "invalid_character"

def check_base85() -> None:
    assert a85encode(b"\x00\x00\x00\x00") == "z"
    match b85decode(b85encode(b"hello")):
        Ok(data) => assert data == b"hello"
        Err(err) => assert false, err.detail
    match a85decode_lenient("<~ z \n~>"):
        Ok(data) => assert data == b"\x00\x00\x00\x00"
        Err(err) => assert false, err.detail
    match z85encode(b"\x86\x4f\xd2\x6f"):
        Ok(text) => assert text == "Hello"
        Err(err) => assert false, err.detail
    match z85decode("Hello"):
        Ok(data) => assert data == b"\x86\x4f\xd2\x6f"
        Err(err) => assert false, err.detail
    match z85decode("Hell"):
        Ok(_) => assert false, "invalid-length z85 unexpectedly decoded"
        Err(err) => assert err.kind == "invalid_length"
    match b85decode("\t"):
        Ok(_) => assert false, "invalid-character base85 unexpectedly decoded"
        Err(err) => assert err.kind == "invalid_character"

def check_bech32() -> None:
    match bech32_encode("a", []):
        Ok(text) => assert text == "a12uel5l"
        Err(err) => assert false, err.detail
    match bech32_decode_any("A12UEL5L"):
        Ok(decoded) => assert decoded.hrp == "a" and len(decoded.data) == 0 and decoded.variant == Bech32Variant.Bech32
        Err(err) => assert false, err.detail
    match bech32m_encode("a", []):
        Ok(text) => assert text == "a1lqfn3a"
        Err(err) => assert false, err.detail
    match bech32_decode("a1lqfn3a"):
        Ok(_) => assert false, "bech32 accepted a bech32m checksum"
        Err(err) => assert err.kind == "invalid_checksum"

def main() -> None:
    check_base64()
    check_base32()
    check_base58()
    check_base85()
    check_bech32()
"#,
    )
}

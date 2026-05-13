use std::fs;
use std::process::Command;
use std::sync::Mutex;

static INCAN_RUN_LOCK: Mutex<()> = Mutex::new(());

fn run_module_case(module_path: &str, assertions: &str) -> Result<(), Box<dyn std::error::Error>> {
    let _guard = match INCAN_RUN_LOCK.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let module_source = fs::read_to_string(module_path)?;
    let dir = tempfile::tempdir()?;
    let source_path = dir.path().join("main.incn");
    fs::write(&source_path, format!("{module_source}\n\n{assertions}"))?;

    let output = Command::new(env!("CARGO_BIN_EXE_incan"))
        .arg("--no-banner")
        .arg("run")
        .arg(&source_path)
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    assert!(
        output.status.success(),
        "module case failed for {module_path}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[test]
fn base64_vectors_and_lenient_decode() -> Result<(), Box<dyn std::error::Error>> {
    run_module_case(
        "crates/incan_stdlib/stdlib/encoding/base64.incn",
        r#"
def main() -> None:
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
"#,
    )
}

#[test]
fn base32_vectors_and_lenient_decode() -> Result<(), Box<dyn std::error::Error>> {
    run_module_case(
        "crates/incan_stdlib/stdlib/encoding/base32.incn",
        r#"
def main() -> None:
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
"#,
    )
}

#[test]
fn base58_vectors_and_lenient_decode() -> Result<(), Box<dyn std::error::Error>> {
    run_module_case(
        "crates/incan_stdlib/stdlib/encoding/base58.incn",
        r#"
def main() -> None:
    assert b58encode(b"hello world") == "StV1DL6CwTryKyV"
    assert b58encode(b"\x00\x00") == "11"
    match b58decode_lenient(" StV1DL6CwTryKyV\n"):
        Ok(data) => assert data == b"hello world"
        Err(err) => assert false, err.detail
    match b58decode("0"):
        Ok(_) => assert false, "invalid base58 unexpectedly decoded"
        Err(err) => assert err.kind == "invalid_character"
"#,
    )
}

#[test]
fn base85_vectors_and_lenient_decode() -> Result<(), Box<dyn std::error::Error>> {
    run_module_case(
        "crates/incan_stdlib/stdlib/encoding/base85.incn",
        r#"
def main() -> None:
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
"#,
    )
}

#[test]
fn bech32_vectors_and_invalid_cases() -> Result<(), Box<dyn std::error::Error>> {
    run_module_case(
        "crates/incan_stdlib/stdlib/encoding/bech32.incn",
        r#"
def main() -> None:
    match bech32_encode("a", []):
        Ok(text) => assert text == "a12uel5l"
        Err(err) => assert false, err.detail
    match decode("A12UEL5L"):
        Ok(decoded) => assert decoded.hrp == "a" and len(decoded.data) == 0 and decoded.variant == Bech32Variant.Bech32
        Err(err) => assert false, err.detail
    match bech32m_encode("a", []):
        Ok(text) => assert text == "a1lqfn3a"
        Err(err) => assert false, err.detail
    match bech32_decode("a1lqfn3a"):
        Ok(_) => assert false, "bech32 accepted a bech32m checksum"
        Err(err) => assert err.kind == "invalid_checksum"
"#,
    )
}

//! Integration tests for the Incan compiler frontend

include!("support/integration_tests_root.rs");

mod codegen_tests {
    include!("support/integration_tests_codegen_tests.rs");

    /// Verify that source programs can retain the public SHA-256 hasher across method calls.
    #[test]
    fn test_std_hash_storable_sha256_hasher_issue969() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.hash import Sha256Hasher, sha256

model StructuralSink:
    hasher: Sha256Hasher

    def append(mut self, chunk: bytes) -> None:
        self.hasher.update(chunk)

    def finalize(mut self) -> bytes:
        return self.hasher.finalize_bytes()

def main() -> None:
    mut sink = StructuralSink(hasher=sha256.new())
    sink.append(b"a")
    sink.append(b"bc")
    println(sink.finalize() == sha256.digest(b"abc"))
"#;
        let output = incan_command()
            .args(["run", "-c", source])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "storable SHA-256 hasher program failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "true");
        Ok(())
    }

    #[test]
    fn test_std_hash_compile_and_run_digest_file_and_error_paths() -> Result<(), Box<dyn std::error::Error>> {
        // Keep std.hash's generated-project dependencies in the root Cargo graph so CI fetches them before this smoke
        // runs the generated project under CARGO_NET_OFFLINE.
        use blake2::Digest as _;
        assert_eq!(blake2::Blake2s256::digest(b"abc").len(), 32);
        assert_eq!(blake3::hash(b"abc").as_bytes().len(), 32);
        assert_eq!(md5_010::Md5::digest(b"abc").len(), 16);
        assert_eq!(sha1::Sha1::digest(b"abc").len(), 20);
        assert_eq!(sha2::Sha256::digest(b"abc").len(), 32);
        assert_eq!(sha3::Sha3_256::digest(b"abc").len(), 32);
        let mut xxh32 = xxhash_rust::xxh32::Xxh32::default();
        xxh32.update(b"abc");
        assert_ne!(xxh32.digest(), 0);
        let mut xxh64 = xxhash_rust::xxh64::Xxh64::default();
        xxh64.update(b"abc");
        assert_ne!(xxh64.digest(), 0);
        let mut xxh3 = xxhash_rust::xxh3::Xxh3Default::new();
        xxh3.update(b"abc");
        assert_ne!(xxh3.digest(), 0);

        let payload = std::env::temp_dir().join(format!("incan_std_hash_integration_{}.txt", std::process::id()));
        std::fs::write(&payload, b"abc")?;

        let source = format!(
            r#"
from std.hash import (
    blake2b,
    blake2s,
    blake3,
    HashError,
    file_digest,
    file_hash_u32,
    file_hash_u64,
    file_hash_u128,
    md5,
    reader_digest,
    reader_hash_u32,
    reader_hash_u64,
    reader_hash_u128,
    sha1,
    sha224,
    sha256,
    sha384,
    sha512,
    sha3_224,
    sha3_256,
    sha3_384,
    sha3_512,
    shake128,
    shake256,
    xxh32,
    xxh64,
    xxh3_64,
    xxh3_128,
)
from std.fs import Path
from std.io import BytesIO

def run() -> Result[None, HashError]:
    sha1_digest = sha1.digest(b"abc")
    println(len(sha1_digest))
    println(sha1_digest == b"\xa9\x99\x3e\x36\x47\x06\x81\x6a\xba\x3e\x25\x71\x78\x50\xc2\x6c\x9c\xd0\xd8\x9d")
    println(len(md5.digest(b"abc")))
    println(md5.digest(b"abc") == b"\x90\x01\x50\x98\x3c\xd2\x4f\xb0\xd6\x96\x3f\x7d\x28\xe1\x7f\x72")
    println(len(sha224.digest(b"abc")))
    println(len(sha384.digest(b"abc")))
    println(len(sha512.digest(b"abc")))
    println(len(sha3_224.digest(b"abc")))
    println(len(sha3_256.digest(b"abc")))
    println(len(sha3_384.digest(b"abc")))
    println(len(sha3_512.digest(b"abc")))
    println(len(blake2b.digest(b"abc")))
    println(len(blake2s.digest(b"abc")))
    println(len(blake3.digest(b"abc")))

    mut legacy = sha1.new()
    legacy.update(b"a")
    legacy.update(b"bc")
    println(legacy.finalize_bytes() == sha1_digest)

    digest = sha256.digest(b"abc")
    println(len(digest))

    mut h = sha256.new()
    h.update(b"a")
    h.update(b"bc")
    println(h.finalize_bytes() == digest)

    mut fast = xxh3_64.new()
    fast.update(b"a")
    fast.update(b"bc")
    println(fast.finalize_u64() == xxh3_64.hash_u64(b"abc"))

    println(len(shake128.digest(b"abc", 8)?))
    println(len(shake256.digest(b"abc", 8)?))
    match shake128.digest(b"abc", 0):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)

    path = Path("{payload}")
    missing_path = Path("{missing_payload}")
    match path.open("rb"):
        Ok(file) => println(file_digest(file, "sha256", 1)? == digest)
        Err(err) => return Err(HashError(kind=err.kind, algorithm="open", detail=err.detail))
    println(file_digest(path, "sha1", 1)? == sha1_digest)
    println(file_digest(path, "sha256", 1)? == digest)
    println(len(file_digest(path, "shake128", 1, 8)?))
    println(len(file_digest(path, "shake256", 2, 8)?))
    println(file_hash_u32(path, "xxh32", 1)? == xxh32.hash_u32(b"abc"))
    println(file_hash_u64(path, "xxh3_64", 1)? == xxh3_64.hash_u64(b"abc"))
    println(file_hash_u64(path, "xxh64", 2)? == xxh64.hash_u64(b"abc"))
    println(file_hash_u128(path, "xxh3_128", 2)? == xxh3_128.hash_u128(b"abc"))
    println(reader_digest(BytesIO(b"abc"), "sha256", 1)? == digest)
    println(len(reader_digest(BytesIO(b"abc"), "shake256", 2, 8)?))
    println(reader_hash_u32(BytesIO(b"abc"), "xxh32", 2)? == xxh32.hash_u32(b"abc"))
    println(reader_hash_u64(BytesIO(b"abc"), "xxh3_64", 2)? == xxh3_64.hash_u64(b"abc"))
    println(reader_hash_u64(BytesIO(b"abc"), "xxh64", 2)? == xxh64.hash_u64(b"abc"))
    println(reader_hash_u128(BytesIO(b"abc"), "xxh3_128", 2)? == xxh3_128.hash_u128(b"abc"))

    match file_hash_u64(path, "sha256", 1):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    match file_hash_u64(path, "unknown", 1):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    match reader_hash_u64(BytesIO(b"abc"), "sha256", 1):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    match reader_hash_u64(BytesIO(b"abc"), "unknown", 1):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    match file_digest(path, "shake128", 1):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    match file_digest(path, "sha256", 0):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    match reader_digest(BytesIO(b"abc"), "sha256", 0):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    match file_digest(missing_path, "sha256", 1):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    return Ok(None)

def main() -> None:
    match run():
        Ok(_) => pass
        Err(err) => println(err.message())
"#,
            payload = payload.display(),
            missing_payload = payload.with_extension("missing").display(),
        );
        let output = incan_command()
            .args(["run", "-c", source.as_str()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        let _ = std::fs::remove_file(&payload);
        assert!(
            output.status.success(),
            "incan run std.hash smoke failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec![
                "20",
                "true",
                "16",
                "true",
                "28",
                "48",
                "64",
                "28",
                "32",
                "48",
                "64",
                "64",
                "32",
                "32",
                "true",
                "32",
                "true",
                "true",
                "8",
                "8",
                "invalid_length",
                "true",
                "true",
                "true",
                "8",
                "8",
                "true",
                "true",
                "true",
                "true",
                "true",
                "8",
                "true",
                "true",
                "true",
                "true",
                "unsupported_width",
                "unknown_algorithm",
                "unsupported_width",
                "unknown_algorithm",
                "invalid_length",
                "invalid_chunk_size",
                "invalid_chunk_size",
                "not_found"
            ],
            "unexpected std.hash output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_std_checksum_compile_and_run_crc32_vectors() -> Result<(), Box<dyn std::error::Error>> {
        // Keep std.checksum's generated-project dependency in the root Cargo graph so CI fetches it before this smoke
        // runs the generated project under CARGO_NET_OFFLINE.
        assert_eq!(crc32fast::hash(b"abc"), 891568578);

        let source = r#"
from std.checksum import crc32

def main() -> None:
    println(crc32.value(b"abc") == 891568578)
    println(crc32.digest(b"abc") == b"\x35\x24\x41\xc2")

    mut h = crc32.new()
    h.update(b"a")
    h.update(b"bc")
    println(h.finalize_u32() == crc32.value(b"abc"))

    mut bytes = crc32.new()
    bytes.update(b"abc")
    println(bytes.finalize_bytes() == crc32.digest(b"abc"))
"#;
        let output = incan_command()
            .args(["run", "-c", source])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run std.checksum smoke failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec!["true", "true", "true", "true"],
            "unexpected std.checksum output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_std_hash_hmac_compile_and_run_rfc4231_vectors() -> Result<(), Box<dyn std::error::Error>> {
        // Keep std.hash's keyed-MAC dependency in the root Cargo graph so CI fetches it before this smoke runs the
        // generated project under CARGO_NET_OFFLINE.
        use hmac::Mac as _;
        let mut probe = hmac::Hmac::<sha2::Sha256>::new_from_slice(b"key")?;
        probe.update(b"abc");
        assert_eq!(probe.finalize_reset().into_bytes().len(), 32);
        probe.update(b"def");
        assert_eq!(probe.finalize().into_bytes().len(), 32);

        // Vectors are RFC 4231's published values for HMAC-SHA256. They are the point of this test: round-tripping
        // our own output against our own input would prove only self-consistency, which is exactly the weakness a
        // keyed MAC exists to fix.
        let source = r#"
from std.hash import hmac_sha256

def main() -> None:
    # RFC 4231 test case 1
    println(hmac_sha256.digest(b"\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b", b"\x48\x69\x20\x54\x68\x65\x72\x65") == b"\xb0\x34\x4c\x61\xd8\xdb\x38\x53\x5c\xa8\xaf\xce\xaf\x0b\xf1\x2b\x88\x1d\xc2\x00\xc9\x83\x3d\xa7\x26\xe9\x37\x6c\x2e\x32\xcf\xf7")
    println(hmac_sha256.verify(b"\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b", b"\x48\x69\x20\x54\x68\x65\x72\x65", b"\xb0\x34\x4c\x61\xd8\xdb\x38\x53\x5c\xa8\xaf\xce\xaf\x0b\xf1\x2b\x88\x1d\xc2\x00\xc9\x83\x3d\xa7\x26\xe9\x37\x6c\x2e\x32\xcf\xf7"))

    # test case 2, short key
    println(hmac_sha256.digest(b"\x4a\x65\x66\x65", b"\x77\x68\x61\x74\x20\x64\x6f\x20\x79\x61\x20\x77\x61\x6e\x74\x20\x66\x6f\x72\x20\x6e\x6f\x74\x68\x69\x6e\x67\x3f") == b"\x5b\xdc\xc1\x46\xbf\x60\x75\x4e\x6a\x04\x24\x26\x08\x95\x75\xc7\x5a\x00\x3f\x08\x9d\x27\x39\x83\x9d\xec\x58\xb9\x64\xec\x38\x43")
    println(hmac_sha256.verify(b"\x4a\x65\x66\x65", b"\x77\x68\x61\x74\x20\x64\x6f\x20\x79\x61\x20\x77\x61\x6e\x74\x20\x66\x6f\x72\x20\x6e\x6f\x74\x68\x69\x6e\x67\x3f", b"\x5b\xdc\xc1\x46\xbf\x60\x75\x4e\x6a\x04\x24\x26\x08\x95\x75\xc7\x5a\x00\x3f\x08\x9d\x27\x39\x83\x9d\xec\x58\xb9\x64\xec\x38\x43"))

    # test case 3, 0xdd payload
    println(hmac_sha256.digest(b"\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa", b"\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd") == b"\x77\x3e\xa9\x1e\x36\x80\x0e\x46\x85\x4d\xb8\xeb\xd0\x91\x81\xa7\x29\x59\x09\x8b\x3e\xf8\xc1\x22\xd9\x63\x55\x14\xce\xd5\x65\xfe")
    println(hmac_sha256.verify(b"\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa", b"\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd", b"\x77\x3e\xa9\x1e\x36\x80\x0e\x46\x85\x4d\xb8\xeb\xd0\x91\x81\xa7\x29\x59\x09\x8b\x3e\xf8\xc1\x22\xd9\x63\x55\x14\xce\xd5\x65\xfe"))

    # test case 4, incrementing key
    println(hmac_sha256.digest(b"\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\x0f\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19", b"\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd") == b"\x82\x55\x8a\x38\x9a\x44\x3c\x0e\xa4\xcc\x81\x98\x99\xf2\x08\x3a\x85\xf0\xfa\xa3\xe5\x78\xf8\x07\x7a\x2e\x3f\xf4\x67\x29\x66\x5b")
    println(hmac_sha256.verify(b"\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\x0f\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19", b"\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd", b"\x82\x55\x8a\x38\x9a\x44\x3c\x0e\xa4\xcc\x81\x98\x99\xf2\x08\x3a\x85\xf0\xfa\xa3\xe5\x78\xf8\x07\x7a\x2e\x3f\xf4\x67\x29\x66\x5b"))

    # test case 6, key longer than the block size
    println(hmac_sha256.digest(b"\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa", b"\x54\x65\x73\x74\x20\x55\x73\x69\x6e\x67\x20\x4c\x61\x72\x67\x65\x72\x20\x54\x68\x61\x6e\x20\x42\x6c\x6f\x63\x6b\x2d\x53\x69\x7a\x65\x20\x4b\x65\x79\x20\x2d\x20\x48\x61\x73\x68\x20\x4b\x65\x79\x20\x46\x69\x72\x73\x74") == b"\x60\xe4\x31\x59\x1e\xe0\xb6\x7f\x0d\x8a\x26\xaa\xcb\xf5\xb7\x7f\x8e\x0b\xc6\x21\x37\x28\xc5\x14\x05\x46\x04\x0f\x0e\xe3\x7f\x54")
    println(hmac_sha256.verify(b"\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa", b"\x54\x65\x73\x74\x20\x55\x73\x69\x6e\x67\x20\x4c\x61\x72\x67\x65\x72\x20\x54\x68\x61\x6e\x20\x42\x6c\x6f\x63\x6b\x2d\x53\x69\x7a\x65\x20\x4b\x65\x79\x20\x2d\x20\x48\x61\x73\x68\x20\x4b\x65\x79\x20\x46\x69\x72\x73\x74", b"\x60\xe4\x31\x59\x1e\xe0\xb6\x7f\x0d\x8a\x26\xaa\xcb\xf5\xb7\x7f\x8e\x0b\xc6\x21\x37\x28\xc5\x14\x05\x46\x04\x0f\x0e\xe3\x7f\x54"))

    # A tag from the right data under the wrong key must not verify.
    println(hmac_sha256.verify(b"wrong-key", b"\x48\x69\x20\x54\x68\x65\x72\x65", b"\xb0\x34\x4c\x61\xd8\xdb\x38\x53\x5c\xa8\xaf\xce\xaf\x0b\xf1\x2b\x88\x1d\xc2\x00\xc9\x83\x3d\xa7\x26\xe9\x37\x6c\x2e\x32\xcf\xf7"))

    # Streaming the message in pieces must agree with the one-shot tag.
    mut signer = hmac_sha256.new(b"Jefe")
    signer.update(b"what do ya want ")
    signer.update(b"for nothing?")
    println(signer.finalize_bytes() == b"\x5b\xdc\xc1\x46\xbf\x60\x75\x4e\x6a\x04\x24\x26\x08\x95\x75\xc7\x5a\x00\x3f\x08\x9d\x27\x39\x83\x9d\xec\x58\xb9\x64\xec\x38\x43")
"#;
        let output = incan_command()
            .args(["run", "-c", source])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run std.hash hmac smoke failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec![
                "true", "true", "true", "true", "true", "true", "true", "true", "true", "true", "false", "true"
            ],
            "unexpected std.hash hmac output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_std_io_compile_and_run_bytesio_core_and_numeric_helpers() -> Result<(), Box<dyn std::error::Error>> {
        // Keep std.io's generated-project dependency in the root Cargo graph so CI fetches it before this smoke runs
        // the generated project under CARGO_NET_OFFLINE.
        let mut cache_anchor = [0u8; 4];
        <byteorder::LittleEndian as byteorder::ByteOrder>::write_u32(&mut cache_anchor, 258);
        assert_eq!(cache_anchor, [2, 1, 0, 0]);

        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
from std.derives.collection import FallibleIterator
from std.io import BinaryReader, BytesIO, Endian, IoError
from std.traits.callable import Callable1, Callable2

model NumberStream with FallibleIterator[int, str]:
    items: list[int]
    index: int
    fail_at: Option[int]

    def __next__(mut self) -> Result[Option[int], str]:
        match self.fail_at:
            Some(index) =>
                if self.index == index:
                    return Err("boom")
            None => pass
        if self.index >= len(self.items):
            return Ok(None)
        item = self.items[self.index]
        self.index += 1
        return Ok(Some(item))

def double(value: int) -> int:
    return value * 2

def is_even(value: int) -> bool:
    return value % 2 == 0

def expand(value: int) -> list[int]:
    return [value, value + 10]

def expand_empty(value: int) -> list[int]:
    return []

def observe_item(value: int) -> None:
    println(f"seen:{value}")

def observe_error(error: str) -> None:
    println(f"source-error:{error}")

def prefix_error(error: str) -> str:
    return f"mapped:{error}"

def add(left: int, right: int) -> int:
    return left + right

@derive(Clone)
model Multiplier with Callable1[int, int]:
    factor: int

    def __call__(self, value: int) -> int:
        return value * self.factor

@derive(Clone)
model ScaledAdd with Callable2[int, int, int]:
    factor: int

    def __call__(self, left: int, right: int) -> int:
        return left + right * self.factor

model TracingStream with FallibleIterator[int, str]:
    items: list[int]
    index: int
    fail_at: Option[int]

    def __next__(mut self) -> Result[Option[int], str]:
        println(f"poll:{self.index}")
        match self.fail_at:
            Some(index) =>
                if self.index == index:
                    return Err("trace-boom")
            None => pass
        if self.index >= len(self.items):
            return Ok(None)
        item = self.items[self.index]
        self.index += 1
        return Ok(Some(item))

def trace_map(value: int) -> int:
    println(f"map-callback:{value}")
    return value * 2

def trace_filter(value: int) -> bool:
    println(f"filter-callback:{value}")
    return true

def trace_expand(value: int) -> list[int]:
    println(f"flat-map-callback:{value}")
    return [value, value + 10]

def trace_inspect(value: int) -> None:
    println(f"inspect-callback:{value}")

def trace_inspect_error(error: str) -> None:
    println(f"inspect-error-callback:{error}")

def trace_map_error(error: str) -> str:
    println(f"map-error-callback:{error}")
    return f"mapped:{error}"

def exercise_fallible_adapters() -> None:
    traced = TracingStream(items=[1, 2], index=0, fail_at=Some(2)).map(trace_map).filter(trace_filter).flat_map(trace_expand).take(10).inspect(trace_inspect).inspect_err(trace_inspect_error).map_err(trace_map_error)
    println("pipeline:constructed")
    match traced.collect():
        Ok(_) => println("bad")
        Err(error) => println(f"pipeline-error:{error}")

    match NumberStream(items=[1, 2, 3], index=0, fail_at=None).map(double).collect():
        Ok(values) => println(f"map:{values[0]}:{values[1]}:{values[2]}")
        Err(error) => println(error)

    match NumberStream(items=[1, 2], index=0, fail_at=None).map(Multiplier(factor=3)).collect():
        Ok(values) => println(f"model-map:{values[0]}:{values[1]}")
        Err(error) => println(error)

    offset = 4
    match NumberStream(items=[1, 2], index=0, fail_at=None).map((value) => value + offset).collect():
        Ok(values) => println(f"closure-map:{values[0]}:{values[1]}")
        Err(error) => println(error)

    match NumberStream(items=[1, 2, 3, 4], index=0, fail_at=None).filter(is_even).collect():
        Ok(values) => println(f"filter:{values[0]}:{values[1]}")
        Err(error) => println(error)

    match NumberStream(items=[1, 2], index=0, fail_at=None).flat_map(expand).collect():
        Ok(values) => println(f"flat:{values[0]}:{values[1]}:{values[2]}:{values[3]}")
        Err(error) => println(error)

    match NumberStream(items=[1, 2], index=0, fail_at=None).flat_map(expand_empty).collect():
        Ok(values) => println(f"flat-empty:{len(values)}")
        Err(error) => println(error)

    match NumberStream(items=[1, 2], index=0, fail_at=Some(1)).take(1).collect():
        Ok(values) => println(f"take:{values[0]}")
        Err(error) => println(error)

    match NumberStream(items=[3, 4], index=0, fail_at=None).inspect(observe_item).collect():
        Ok(values) => println(f"inspect:{len(values)}")
        Err(error) => println(error)

    errors = NumberStream(items=[7, 8], index=0, fail_at=Some(1)).inspect_err(observe_error).map_err(prefix_error)
    match errors.collect():
        Ok(_) => println("bad")
        Err(error) => println(f"error:{error}")

    match NumberStream(items=[1, 2, 3], index=0, fail_at=None).fold(0, add):
        Ok(value) => println(f"fold:{value}")
        Err(error) => println(error)

    match NumberStream(items=[1, 2, 3], index=0, fail_at=None).fold(0, ScaledAdd(factor=2)):
        Ok(value) => println(f"model-fold:{value}")
        Err(error) => println(error)

    match NumberStream(items=[4, 5], index=0, fail_at=Some(1)).inspect(observe_item).fold(0, add):
        Ok(_) => println("bad")
        Err(error) => println(f"fold-error:{error}")

    println("take-zero:start")
    match TracingStream(items=[9], index=0, fail_at=Some(0)).take(0).collect():
        Ok(values) => println(f"take-zero:{len(values)}")
        Err(error) => println(error)
    println("take-zero:end")

model FailingReader with BinaryReader:
    def read_bytes(self, size: int) -> Result[bytes, IoError]:
        return Err(IoError(kind="other", detail="read failed", operation="read_bytes"))

def propagate_reader_error() -> Result[None, IoError]:
    for _ in FailingReader().chunks(2)?:
        pass
    return Ok(None)

def error_kind(err: IoError) -> str:
    return err.kind

def map_reader_error() -> Result[None, str]:
    for _ in FailingReader().chunks(2).map_err(error_kind)?:
        pass
    return Ok(None)

def run() -> Result[None, IoError]:
    exercise_fallible_adapters()
    buf = BytesIO(b"abc\0rest")
    first = buf.read(2)?
    println(len(first))
    println(buf.tell())
    buf.rewind()?
    nul: u8 = 0
    letter_t: u8 = 116
    until = buf.read_until(nul)?
    println(len(until))
    println(buf.remaining())
    println(buf.skip_until(letter_t)?)
    println(buf.remaining())
    match buf.read_exact(1):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)

    for chunk in BytesIO(b"abcde").chunks(2)?:
        println(len(chunk))
    for _ in BytesIO(b"").chunks(2)?:
        println("bad")
    match FailingReader().chunks(0).__next__():
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    match propagate_reader_error():
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    match map_reader_error():
        Ok(_) => println("bad")
        Err(kind) => println(kind)

    out = BytesIO()
    u32_value: u32 = 258
    i16_value: i16 = -2
    u128_value: u128 = 42
    f64_value: f64 = 1.5
    out.write(u32_value, Endian.Little)?
    out.write(i16_value, Endian.Big)?
    out.write(u128_value, Endian.Big)?
    out.write(f64_value, Endian.Little)?
    println(len(out.getvalue()))
    out.rewind()?
    read_u32: u32 = out.read(Endian.Little)?
    read_i16: i16 = out.read(Endian.Big)?
    read_u128: u128 = out.read(Endian.Big)?
    read_f64: f64 = out.read(Endian.Little)?
    println(read_u32)
    println(read_i16)
    println(read_u128)
    println(read_f64 == f64_value)

    rewrite = BytesIO(b"abcd")
    rewrite.seek(1, 0)?
    xy: bytes = b"XY"
    rewrite.write(xy)?
    rewrite.truncate(Some(3))?
    println(len(rewrite.getvalue()))
    println(rewrite.remaining())
    return Ok(None)

def main() -> None:
    match run():
        Ok(_) => pass
        Err(err) => println(err.message())
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .env(
                "INCAN_STDLIB",
                repo_root().join("loaves/stdlib"),
            )
            .output()?;
        assert!(
            output.status.success(),
            "incan run std.io smoke failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec![
                "pipeline:constructed",
                "poll:0",
                "map-callback:1",
                "filter-callback:2",
                "flat-map-callback:2",
                "inspect-callback:2",
                "inspect-callback:12",
                "poll:1",
                "map-callback:2",
                "filter-callback:4",
                "flat-map-callback:4",
                "inspect-callback:4",
                "inspect-callback:14",
                "poll:2",
                "inspect-error-callback:trace-boom",
                "map-error-callback:trace-boom",
                "pipeline-error:mapped:trace-boom",
                "map:2:4:6",
                "model-map:3:6",
                "closure-map:5:6",
                "filter:2:4",
                "flat:1:11:2:12",
                "flat-empty:0",
                "take:1",
                "seen:3",
                "seen:4",
                "inspect:2",
                "source-error:boom",
                "error:mapped:boom",
                "fold:6",
                "model-fold:12",
                "seen:4",
                "fold-error:boom",
                "take-zero:start",
                "take-zero:0",
                "take-zero:end",
                "2",
                "2",
                "4",
                "4",
                "4",
                "0",
                "unexpected_eof",
                "2",
                "2",
                "1",
                "invalid_input",
                "other",
                "other",
                "30",
                "258",
                "-2",
                "42",
                "true",
                "3",
                "0"
            ],
            "unexpected std.io output:\n{stdout}"
        );
        Ok(())
    }
}

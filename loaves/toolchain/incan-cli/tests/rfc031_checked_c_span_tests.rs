#![allow(dead_code)] // Split test binaries intentionally use different parts of shared support fragments.

//! RFC 031 pub-import integration tests.
//!
//! These regressions each drive a full `incan build` of a library plus one or more consumers, so they are the most
//! expensive cases in the suite. They live in their own libtest root because the Oven replay partitioner packs whole
//! roots: kept alongside the rest of the frontend integration tests they made one indivisible root that no split
//! could shorten.

include!("support/rfc031_pub_import_integration_tests_root.rs");

use support::strip_ansi_escapes;

mod rfc031_pub_import_integration_tests {
    include!("support/rfc031_pub_import_integration_tests_rfc031_pub_import_integration_tests.rs");

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_copies_a_sqlite_error_view_with_an_explicit_bound() -> Result<(), Box<dyn std::error::Error>> {
        let sqlite_header = sqlite_header_path()?;
        let tmp = tempfile::tempdir()?;
        let source = sqlite_checked_c_source(&sqlite_header);
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"sqlite_checked_c_facade\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");
        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            output.status.success(),
            "expected SQLite to use an owned output handle, a borrowed error view, and bounded copied text.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let checkout = support::repo_root();
        let emitted = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["--emit-rust", main_path.to_string_lossy().as_ref(), "--strict"])
            .output()?;
        assert!(
            emitted.status.success(),
            "expected the bounded SQLite view bridge to lower and emit strict Rust.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&emitted.stdout),
            String::from_utf8_lossy(&emitted.stderr)
        );
        let generated = String::from_utf8_lossy(&emitted.stdout);
        assert!(
            generated.contains("__incan_checked_c_copy_utf8"),
            "expected the generated SQLite façade to use the compiler-private bounded copy helper:\n{generated}"
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_models_a_sqlite_caller_owned_byte_buffer_through_a_declared_shim()
    -> Result<(), Box<dyn std::error::Error>> {
        let sqlite_header = sqlite_header_path()?;
        let tmp = tempfile::tempdir()?;
        let shim_header = tmp.path().join("sqlite_span_bridge.h");
        std::fs::write(
            &shim_header,
            format!(
                "#include \"{}\"\n#include <stddef.h>\n#include <stdint.h>\nsize_t incan_sqlite_random_bytes(uint8_t *destination, size_t destination_capacity);\n",
                sqlite_header.display()
            ),
        )?;
        let source = format!(
            r#"from std.interop import c

binding SQLiteBuffer:
    header = "{}"
    link = c.system_library("sqlite3")

    symbol random_bytes(destination: c.MutPtr[c.u8], destination_capacity: c.Size) -> c.Size:
        native = "incan_sqlite_random_bytes"
        bounds = {{ destination: destination_capacity }}

def random_bytes() -> Result[bytes, str]:
    mut destination = c.mutable_bytes_span(b"\0\0\0\0\0\0\0\0")
    unsafe:
        written = SQLiteBuffer.random_bytes(destination.as_mut_ptr(), destination.byte_capacity())
        return destination.into_bytes(written)
"#,
            shim_header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"sqlite_checked_c_span\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");

        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            output.status.success(),
            "expected the SQLite shim boundary to retain one checked byte buffer contract.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let checkout = support::repo_root();
        let emitted = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["--emit-rust", main_path.to_string_lossy().as_ref(), "--strict"])
            .output()?;
        assert!(
            emitted.status.success(),
            "expected the SQLite caller-owned buffer bridge to emit strict Rust.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&emitted.stdout),
            String::from_utf8_lossy(&emitted.stderr)
        );
        let generated = String::from_utf8_lossy(&emitted.stdout);
        assert!(
            generated.contains("__incan_checked_c_finish_span")
                && generated.contains("*mut u8")
                && generated.contains("-> usize"),
            "expected a bounded byte buffer and exact c.Size carrier, got:\n{generated}"
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_models_an_accelerate_f32_span_through_a_declared_framework_shim()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let shim_header = tmp.path().join("accelerate_span_bridge.h");
        std::fs::write(
            &shim_header,
            "#include <stddef.h>\n#define INCAN_ACCELERATE_OK 0\nint incan_accelerate_sum_f32(const float *values, size_t value_count, float *output);\n",
        )?;
        let source = format!(
            r#"from std.interop import c

binding Accelerate:
    header = "{}"
    link = c.framework("Accelerate")

    enum Status:
        OK: c.i32 = INCAN_ACCELERATE_OK

    symbol sum(values: c.ConstPtr[c.f32], value_count: c.Size, output: c.Out[c.f32]) -> c.i32:
        native = "incan_accelerate_sum_f32"
        bounds = {{ values: value_count }}

        outcome Status.OK:
            initializes = [output]

def sum(values: list[f32]) -> Result[f32, str]:
    source = c.f32_span(values)
    unsafe:
        output = c.out[c.f32]()
        status = Accelerate.sum(source.as_const_ptr(), source.element_count(), output)
        if status == Accelerate.Status.OK:
            return Ok(output.take())
        return Err("Accelerate sum failed")
"#,
            shim_header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"accelerate_checked_c_span\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");

        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            output.status.success(),
            "expected the Accelerate-shaped shim boundary to retain one checked f32 span and output contract.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let checkout = support::repo_root();
        let emitted = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["--emit-rust", main_path.to_string_lossy().as_ref(), "--strict"])
            .output()?;
        assert!(
            emitted.status.success(),
            "expected the Accelerate f32 span bridge to emit strict Rust.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&emitted.stdout),
            String::from_utf8_lossy(&emitted.stderr)
        );
        let generated = String::from_utf8_lossy(&emitted.stdout);
        assert!(
            generated.contains("*const f32")
                && generated.contains("fn from_incan_value(value: f32)")
                && generated.contains("fn take(self) -> f32")
                && generated.contains("#[link(name = \"Accelerate\", kind = \"framework\")]")
                && !generated.contains("i64::try_from(value)"),
            "expected an exact f32 span/output carrier and source-selected framework link without i64 normalization, got:\n{generated}"
        );

        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_supports_checked_byte_spans_and_caller_owned_buffers() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let header = tmp.path().join("span_fixture.h");
        std::fs::write(
            &header,
            "#include <stddef.h>\n#include <stdint.h>\nsize_t fixture_copy_prefix(const uint8_t *source, size_t source_length, uint8_t *destination, size_t destination_capacity);\n",
        )?;
        let source = format!(
            r#"from std.interop import c

binding Fixture:
    header = "{}"
    link = c.system_library("c")

    symbol copy_prefix(
        source: c.ConstPtr[c.u8],
        source_length: c.Size,
        destination: c.MutPtr[c.u8],
        destination_capacity: c.Size,
    ) -> c.Size:
        native = "fixture_copy_prefix"
        bounds = {{
            source: source_length,
            destination: destination_capacity,
        }}

def copy_bounded(data: bytes) -> Result[bytes, str]:
    source = c.bytes_span(data)
    mut destination = c.mutable_bytes_span(b"\0\0\0\0")
    unsafe:
        written = Fixture.copy_prefix(
            source.as_const_ptr(),
            source.byte_length(),
            destination.as_mut_ptr(),
            destination.byte_capacity(),
        )
        return Ok(destination.into_bytes(written)?)

def main() -> Result[None, str]:
    assert copy_bounded(b"abc")? == b"abc"
    return Ok(None)
"#,
            header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"checked_c_spans\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");

        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            output.status.success(),
            "expected bounded byte spans and a caller-owned output buffer to typecheck.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let checkout = support::repo_root();
        let emitted = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["--emit-rust", main_path.to_string_lossy().as_ref(), "--strict"])
            .output()?;
        assert!(
            emitted.status.success(),
            "expected bounded byte spans and caller-owned buffer finishing to emit strict Rust.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&emitted.stdout),
            String::from_utf8_lossy(&emitted.stderr)
        );
        let generated = String::from_utf8_lossy(&emitted.stdout);
        assert!(
            generated.contains("__incan_checked_c_finish_span"),
            "expected generated Rust to validate the returned count before returning caller-owned storage:\n{generated}"
        );
        assert!(
            generated.contains(".as_mut_ptr()") && generated.contains(".as_ptr()"),
            "expected generated Rust to extract only the checked pointer forms:\n{generated}"
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_rejects_unpaired_or_immutable_checked_byte_buffer_arguments()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let header = tmp.path().join("span_fixture.h");
        std::fs::write(
            &header,
            "#include <stddef.h>\n#include <stdint.h>\nsize_t fixture_copy_prefix(const uint8_t *source, size_t source_length, uint8_t *destination, size_t destination_capacity);\n",
        )?;
        let source = format!(
            r#"from std.interop import c

binding Fixture:
    header = "{}"
    link = c.system_library("c")

    symbol copy_prefix(
        source: c.ConstPtr[c.u8],
        source_length: c.Size,
        destination: c.MutPtr[c.u8],
        destination_capacity: c.Size,
    ) -> c.Size:
        native = "fixture_copy_prefix"
        bounds = {{
            source: source_length,
            destination: destination_capacity,
        }}

def reject_unchecked_pair(data: bytes) -> None:
    source = c.bytes_span(data)
    other = c.bytes_span(b"not the same owner")
    destination = c.mutable_bytes_span(b"\0\0\0\0")
    unsafe:
        Fixture.copy_prefix(
            source.as_const_ptr(),
            other.byte_length(),
            destination.as_mut_ptr(),
            destination.byte_capacity(),
        )
"#,
            header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"checked_c_span_rejections\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");
        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            !output.status.success(),
            "expected invalid checked byte-span source to be rejected.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let diagnostics = String::from_utf8_lossy(&output.stderr);
        assert!(
            diagnostics.contains("to come from the same checked immutable byte span"),
            "expected the declared pointer/length relationship diagnostic, got:\n{diagnostics}"
        );
        assert!(
            diagnostics.contains("requires a mutable borrow of 'destination'"),
            "expected an immutable caller-owned buffer to be rejected, got:\n{diagnostics}"
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_rejects_checked_byte_span_escape_and_reuse() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let source = r#"from std.interop import c

def reject_escape_and_reuse(data: bytes) -> Result[None, str]:
    source = c.bytes_span(data)
    escaped = [source]
    mut destination = c.mutable_bytes_span(b"\0\0\0\0")
    unsafe:
        completed = destination.into_bytes(0)?
        destination.into_bytes(0)?
    return Ok(None)
"#;
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"checked_c_span_escape_rejections\"\n\n[sdk]\nprofile = \"minimal\"\n",
            source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");
        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            !output.status.success(),
            "expected an escaped or reused checked byte carrier to be rejected.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let diagnostics = String::from_utf8_lossy(&output.stderr);
        assert!(
            diagnostics.contains("has no ordinary value surface"),
            "expected a checked byte-carrier escape diagnostic, got:\n{diagnostics}"
        );
        assert!(
            diagnostics.contains("was already consumed"),
            "expected a checked mutable byte-buffer consumption diagnostic, got:\n{diagnostics}"
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_preserves_checked_c_f32_scalar_identity() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let header = tmp.path().join("float_fixture.h");
        std::fs::write(&header, "float fixture_absolute_f32(float value);\n")?;
        let source = format!(
            r#"from std.interop import c

binding FloatFixture:
    header = "{}"
    link = c.system_library("m")

    symbol absolute(value: c.f32) -> c.f32:
        native = "fixture_absolute_f32"

def absolute(value: f32) -> f32:
    unsafe:
        return FloatFixture.absolute(value)

def main() -> None:
    value: f32 = 1.5
    assert absolute(value) == value
"#,
            header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"checked_c_f32_scalar\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");
        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            output.status.success(),
            "expected an exact checked c.f32 signature to typecheck.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let checkout = support::repo_root();
        let emitted = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["--emit-rust", main_path.to_string_lossy().as_ref(), "--strict"])
            .output()?;
        assert!(
            emitted.status.success(),
            "expected an exact checked c.f32 signature to emit strict Rust.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&emitted.stdout),
            String::from_utf8_lossy(&emitted.stderr)
        );
        assert!(
            String::from_utf8_lossy(&emitted.stdout).contains("f32"),
            "expected generated Rust to retain the f32 ABI carrier"
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_preserves_exact_checked_c_scalar_carriers() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let header = tmp.path().join("scalar_fixture.h");
        std::fs::write(
            &header,
            "#include <stddef.h>\n#include <stdint.h>\nint8_t fixture_i8(int8_t value);\nuint8_t fixture_u8(uint8_t value);\nint16_t fixture_i16(int16_t value);\nuint16_t fixture_u16(uint16_t value);\nint32_t fixture_i32(int32_t value);\nuint32_t fixture_u32(uint32_t value);\nint64_t fixture_i64(int64_t value);\nuint64_t fixture_u64(uint64_t value);\n__int128 fixture_i128(__int128 value);\nunsigned __int128 fixture_u128(unsigned __int128 value);\nfloat fixture_f32(float value);\ndouble fixture_f64(double value);\nsize_t fixture_size(size_t value);\n",
        )?;
        let source = format!(
            r#"from std.interop import c

binding Scalars:
    header = "{}"
    link = c.system_library("c")

    symbol i8(value: c.i8) -> c.i8:
        native = "fixture_i8"
    symbol u8(value: c.u8) -> c.u8:
        native = "fixture_u8"
    symbol i16(value: c.i16) -> c.i16:
        native = "fixture_i16"
    symbol u16(value: c.u16) -> c.u16:
        native = "fixture_u16"
    symbol i32(value: c.i32) -> c.i32:
        native = "fixture_i32"
    symbol u32(value: c.u32) -> c.u32:
        native = "fixture_u32"
    symbol i64(value: c.i64) -> c.i64:
        native = "fixture_i64"
    symbol u64(value: c.u64) -> c.u64:
        native = "fixture_u64"
    symbol i128(value: c.i128) -> c.i128:
        native = "fixture_i128"
    symbol u128(value: c.u128) -> c.u128:
        native = "fixture_u128"
    symbol f32(value: c.f32) -> c.f32:
        native = "fixture_f32"
    symbol f64(value: c.f64) -> c.f64:
        native = "fixture_f64"
    symbol size(value: c.Size) -> c.Size:
        native = "fixture_size"

def exact_i8(value: i8) -> i8:
    unsafe:
        return Scalars.i8(value)

def exact_u8(value: u8) -> u8:
    unsafe:
        return Scalars.u8(value)

def exact_i16(value: i16) -> i16:
    unsafe:
        return Scalars.i16(value)

def exact_u16(value: u16) -> u16:
    unsafe:
        return Scalars.u16(value)

def exact_i32(value: i32) -> i32:
    unsafe:
        return Scalars.i32(value)

def exact_u32(value: u32) -> u32:
    unsafe:
        return Scalars.u32(value)

def exact_i64(value: i64) -> i64:
    unsafe:
        return Scalars.i64(value)

def exact_u64(value: u64) -> u64:
    unsafe:
        return Scalars.u64(value)

def exact_i128(value: i128) -> i128:
    unsafe:
        return Scalars.i128(value)

def exact_u128(value: u128) -> u128:
    unsafe:
        return Scalars.u128(value)

def exact_f32(value: f32) -> f32:
    unsafe:
        return Scalars.f32(value)

def exact_f64(value: f64) -> f64:
    unsafe:
        return Scalars.f64(value)

def exact_size(value: usize) -> usize:
    unsafe:
        return Scalars.size(value)
"#,
            header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"checked_c_exact_scalars\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");
        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            output.status.success(),
            "expected checked fixed-width C carriers to retain exact Incan representations.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let checkout = support::repo_root();
        let emitted = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["--emit-rust", main_path.to_string_lossy().as_ref(), "--strict"])
            .output()?;
        assert!(
            emitted.status.success(),
            "expected exact checked C carriers to emit strict Rust.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&emitted.stdout),
            String::from_utf8_lossy(&emitted.stderr)
        );
        let generated = String::from_utf8_lossy(&emitted.stdout);
        for rust_type in [
            "i8", "u8", "i16", "u16", "i32", "u32", "i64", "u64", "i128", "u128", "f32", "f64", "usize",
        ] {
            assert!(
                generated.contains(&format!("__incan_arg_0: {rust_type}")),
                "expected generated Rust to preserve exact {rust_type} checked-C carrier, got:\n{generated}"
            );
        }
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_supports_checked_f32_spans_for_paired_numeric_pointers() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let header = tmp.path().join("float_span_fixture.h");
        std::fs::write(
            &header,
            "#include <stddef.h>\nfloat fixture_sum_f32(const float *values, size_t value_count);\nsize_t fixture_fill_f32(float *destination, size_t destination_capacity);\n",
        )?;
        let source = format!(
            r#"from std.interop import c

binding FloatSpanFixture:
    header = "{}"
    link = c.system_library("m")

    symbol sum(values: c.ConstPtr[c.f32], value_count: c.Size) -> c.f32:
        native = "fixture_sum_f32"
        bounds = {{ values: value_count }}

    symbol fill(destination: c.MutPtr[c.f32], destination_capacity: c.Size) -> c.Size:
        native = "fixture_fill_f32"
        bounds = {{ destination: destination_capacity }}

def sum(values: list[f32]) -> f32:
    source = c.f32_span(values)
    unsafe:
        return FloatSpanFixture.sum(source.as_const_ptr(), source.element_count())

def fill() -> Result[list[f32], str]:
    mut destination = c.mutable_f32_span([0.0, 0.0, 0.0])
    unsafe:
        written = FloatSpanFixture.fill(destination.as_mut_ptr(), destination.element_capacity())
        return destination.into_f32s(written)
"#,
            header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"checked_c_f32_spans\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");
        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            output.status.success(),
            "expected paired checked f32 span calls to typecheck.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let checkout = support::repo_root();
        let bindings = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args([
                "inspect",
                "bindings",
                main_path.to_string_lossy().as_ref(),
                "--format",
                "json",
            ])
            .output()?;
        assert!(
            bindings.status.success(),
            "expected binding inspection to project the checked f32 span association.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&bindings.stdout),
            String::from_utf8_lossy(&bindings.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&bindings.stdout)?;
        let sum = report["bindings"][0]["symbols"]
            .as_array()
            .and_then(|symbols| symbols.iter().find(|symbol| symbol["name"] == serde_json::json!("sum")))
            .ok_or("binding inspection did not retain FloatSpanFixture.sum")?;
        assert_eq!(
            sum["buffers"],
            serde_json::json!([{
                "pointer_parameter": "values",
                "length_parameter": "value_count",
                "element": "c.f32",
            }]),
            "binding inspection must project the descriptor-owned span association"
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_rejects_unpaired_checked_f32_span_arguments() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let header = tmp.path().join("float_span_fixture.h");
        std::fs::write(
            &header,
            "#include <stddef.h>\nfloat fixture_sum_f32(const float *values, size_t value_count);\n",
        )?;
        let source = format!(
            r#"from std.interop import c

binding FloatSpanFixture:
    header = "{}"
    link = c.system_library("m")

    symbol sum(values: c.ConstPtr[c.f32], value_count: c.Size) -> c.f32:
        native = "fixture_sum_f32"
        bounds = {{ values: value_count }}

def reject_mismatched_owner(left: list[f32], right: list[f32]) -> f32:
    source = c.f32_span(left)
    other = c.f32_span(right)
    unsafe:
        return FloatSpanFixture.sum(source.as_const_ptr(), other.element_count())
"#,
            header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"checked_c_f32_span_rejection\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");
        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            !output.status.success(),
            "expected independently owned f32 pointer/count arguments to be rejected.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("same checked immutable f32 span"),
            "expected the declared f32 pointer/count relationship diagnostic, got:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn sqlite_checked_c_tooling_projects_the_shared_descriptor() -> Result<(), Box<dyn std::error::Error>> {
        let sqlite_header = sqlite_header_path()?;
        let tmp = tempfile::tempdir()?;
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"sqlite_checked_c_tooling\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &sqlite_checked_c_source(&sqlite_header),
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");
        let checkout = support::repo_root();
        let entry_path = main_path.to_string_lossy();

        let bindings = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["inspect", "bindings", entry_path.as_ref(), "--format", "json"])
            .output()?;
        assert!(
            bindings.status.success(),
            "expected binding inspection to reuse the checked SQLite descriptor.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&bindings.stdout),
            String::from_utf8_lossy(&bindings.stderr)
        );
        let binding_report: serde_json::Value = serde_json::from_slice(&bindings.stdout)?;
        let sqlite = binding_report["bindings"]
            .as_array()
            .and_then(|bindings| {
                bindings
                    .iter()
                    .find(|binding| binding["name"] == serde_json::json!("SQLite"))
            })
            .ok_or("binding inspection did not retain the SQLite descriptor")?;
        assert_eq!(sqlite["link_capability"], serde_json::json!("system_library"));
        assert_eq!(sqlite["resources"][0]["name"], serde_json::json!("Database"));
        assert_eq!(sqlite["resources"][0]["release"], serde_json::json!("close"));
        let error_message = sqlite["symbols"]
            .as_array()
            .and_then(|symbols| {
                symbols
                    .iter()
                    .find(|symbol| symbol["name"] == serde_json::json!("error_message"))
            })
            .ok_or("binding inspection did not retain SQLite.error_message")?;
        assert_eq!(
            error_message["parameters"][0]["type"]["access"],
            serde_json::json!("borrowed")
        );
        assert_eq!(error_message["return_type"]["kind"], serde_json::json!("pointer"));
        assert_eq!(error_message["return_type"]["mutable"], serde_json::json!(false));
        assert_eq!(
            error_message["return_type"]["pointee"]["spelling"],
            serde_json::json!("c.c_char")
        );

        let codegraph = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["inspect", "codegraph", entry_path.as_ref(), "--format", "jsonl"])
            .output()?;
        assert!(
            codegraph.status.success(),
            "expected codegraph to reuse the checked SQLite descriptor.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&codegraph.stdout),
            String::from_utf8_lossy(&codegraph.stderr)
        );
        let records = String::from_utf8(codegraph.stdout)?
            .lines()
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<Result<Vec<_>, _>>()?;
        let binding = records
            .iter()
            .find(|record| {
                record["record"] == serde_json::json!("c_binding") && record["name"] == serde_json::json!("SQLite")
            })
            .ok_or("codegraph did not retain the checked SQLite binding")?;
        assert_eq!(binding["provenance"], serde_json::json!("checked"));
        let error_call = records
            .iter()
            .find(|record| {
                record["record"] == serde_json::json!("c_binding_call")
                    && record["binding"] == serde_json::json!("SQLite")
                    && record["symbol"] == serde_json::json!("error_message")
            })
            .ok_or("codegraph did not retain the checked SQLite.error_message call")?;
        assert_eq!(error_call["binding_id"], binding["id"]);
        assert_eq!(error_call["unsafe_acknowledged"], serde_json::json!(true));
        assert_eq!(error_call["provenance"], serde_json::json!("checked"));
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )),
        ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
    )]
    fn consumer_check_reports_checked_c_signature_mismatch_at_the_binding() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let fixture_header = tmp.path().join("fixture.h");
        std::fs::write(&fixture_header, "long fixture_abs(int value);\n")?;
        let source = format!(
            "from std.interop import c\n\nbinding Fixture:\n    header = \"{}\"\n    link = c.system_library(\"c\")\n\n    symbol absolute(value: c.i32) -> c.i32:\n        native = \"fixture_abs\"\n\ndef main() -> None:\n    pass\n",
            fixture_header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"checked_c_signature_mismatch\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");

        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            !output.status.success(),
            "expected a mismatched C signature to fail before code generation.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let diagnostics = strip_ansi_escapes(&String::from_utf8_lossy(&output.stderr));
        assert!(
            diagnostics.contains("C binding `Fixture` verification failed"),
            "expected a binding-anchored verifier diagnostic, got:\n{diagnostics}"
        );
        assert!(
            diagnostics.contains("Incan C signature mismatch"),
            "expected the Clang signature assertion detail, got:\n{diagnostics}"
        );
        Ok(())
    }
}

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
    fn build_lib_fails_early_for_invalid_helper_binding_manifest() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("invalid_helper_vocab_project");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"widgets_core\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub def make_widget(name: str) -> str:\n  return name\n",
        )?;
        write_vocab_companion_crate_with_source(
            &producer_root,
            "vocab_companion",
            "widgets_vocab_companion",
            "use incan_vocab::{HelperBinding, LibraryManifest, VocabRegistration};\n\npub fn library_vocab() -> VocabRegistration {\n    VocabRegistration::new().with_library_manifest(LibraryManifest {\n        helper_bindings: vec![HelperBinding {\n            key: \"filter\".to_string(),\n            exported_name: \"filter\".to_string(),\n        }],\n        ..LibraryManifest::default()\n    })\n}\n",
        )?;

        let producer_build = bake_library_provider(&producer_root)?;
        assert!(
            !producer_build.status.success(),
            "expected `build --lib` to fail for invalid helper binding.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&producer_build.stderr));
        assert!(
            stderr.contains("unknown exported symbol `filter`"),
            "expected helper-binding validation failure, got:\n{stderr}"
        );
        Ok(())
    }

    #[test]
    fn consumer_check_uses_serialized_vocab_metadata_for_keyword_activation() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("widgets_assert_vocab_project");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"widgets_assert_core\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub def make_widget(name: str) -> str:\n  return name\n",
        )?;
        write_vocab_companion_crate_with_assert_keyword(&producer_root, "vocab_companion", "widgets_vocab_companion")?;

        let producer_build = run_build_lib(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected `build --lib` with assert vocab companion to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );

        let consumer_root = tmp.path().join("consumer_with_vocab_keyword");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::write(
            consumer_root.join("loaf.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nwidgets = { path = \"../widgets_assert_vocab_project\" }\n",
        )?;
        let consumer_main = consumer_root.join("src/main.incn");
        std::fs::write(
            &consumer_main,
            "import pub::widgets\n\ndef main() -> None:\n  assert true\n",
        )?;

        let check_output = run_check(&consumer_main)?;
        assert!(
            check_output.status.success(),
            "expected consumer check to parse/typecheck assert keyword from serialized vocab metadata.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&check_output.stdout),
            String::from_utf8_lossy(&check_output.stderr)
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
    fn consumer_check_activates_standard_checked_c_vocab() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let fixture_header = tmp.path().join("fixture.h");
        std::fs::write(
            &fixture_header,
            "typedef struct fixture_pair { int left; int right; } fixture_pair;\n#define FIXTURE_OK 0\nint abs(int value);\n",
        )?;
        let source = format!(
            "from std.interop import c\n\nbinding Fixture:\n    header = \"{}\"\n    link = c.system_library(\"c\")\n\n    symbol absolute(value: c.i32) -> c.i32:\n        native = \"abs\"\n\n    enum Status:\n        OK: c.i32 = FIXTURE_OK\n\n    struct Pair:\n        native = \"fixture_pair\"\n        left: c.i32 = left\n        right: c.i32 = right\n\ndef absolute(value: int) -> int:\n    unsafe:\n        return Fixture.absolute(value)\n\ndef main() -> None:\n    assert Fixture.Status.OK == 0\n    assert absolute(-7) == 7\n",
            fixture_header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"standard_interop_vocab_consumer\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");

        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            output.status.success(),
            "expected the standard C binding vocabulary to lower and typecheck.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let checkout = support::repo_root();
        let run_output = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["run", main_path.to_string_lossy().as_ref()])
            .output()?;
        assert!(
            run_output.status.success(),
            "expected a checked C call to compile and run.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&run_output.stdout),
            String::from_utf8_lossy(&run_output.stderr)
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
    fn consumer_check_resolves_a_manifest_declared_package_relative_c_header() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let header = tmp.path().join("interop/include/fixture.h");
        std::fs::create_dir_all(header.parent().ok_or("fixture header has no parent")?)?;
        std::fs::write(&header, "int fixture_abs(int value);\n")?;
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"package_relative_c_header\"\n\n[sdk]\nprofile = \"minimal\"\n\n[interop.c]\nschema = 1\n\n[[interop.c.targets]]\ntarget = \"aarch64-apple-darwin\"\nheaders = [\"interop/include/fixture.h\"]\n",
            "from std.interop import c\n\nbinding Fixture:\n    header = \"interop/include/fixture.h\"\n    link = c.system_library(\"c\")\n\n    symbol absolute(value: c.i32) -> c.i32:\n        native = \"fixture_abs\"\n\ndef main() -> None:\n    pass\n",
        )?;
        let output = run_check_against_checkout_sdk(&main_path, &tmp.path().join("generated-cargo-target"))?;
        assert!(
            output.status.success(),
            "expected a package-relative checked C header to resolve through [interop.c].\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
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
    fn consumer_check_verifies_checked_c_resource_and_output_contracts() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let fixture_header = tmp.path().join("fixture.h");
        std::fs::write(
            &fixture_header,
            "typedef unsigned long size_t;\nvoid free(void *);\nint posix_memalign(void **, size_t, size_t);\nunsigned int rand_r(unsigned int *);\n#define FIXTURE_OK 0\n",
        )?;
        let source = format!(
            "from std.interop import c\n\nbinding Fixture:\n    header = \"{}\"\n    link = c.system_library(\"c\")\n\n    resource Memory:\n        native = \"void\"\n        release = close\n\n    symbol close(handle: c.Owned[Memory]) -> None:\n        native = \"free\"\n\n    enum Status:\n        OK: c.i32 = FIXTURE_OK\n\n    symbol open(output: c.Out[c.Owned[Memory]], alignment: c.Size, size: c.Size) -> c.i32:\n        native = \"posix_memalign\"\n\n        outcome Status.OK:\n            initializes = [output]\n\n    symbol random(seed: c.InOut[c.u32]) -> c.u32:\n        native = \"rand_r\"\n\ndef main() -> None:\n    unsafe:\n        handle = c.out[c.Owned[Memory]]()\n        status = Fixture.open(handle, 8, 8)\n        if status == Fixture.Status.OK:\n            resource = handle.take()\n            Fixture.close(resource)\n\n        seed_value: u32 = 7\n        seed = c.inout(seed_value)\n        Fixture.random(seed)\n        seed.take()\n",
            fixture_header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"checked_c_resource_contracts\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");

        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            output.status.success(),
            "expected checked C resource and output contracts to lower and verify.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let checkout = support::repo_root();
        let run_output = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["run", main_path.to_string_lossy().as_ref()])
            .output()?;
        assert!(
            run_output.status.success(),
            "expected checked C resources and output slots to compile, run, and release.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&run_output.stdout),
            String::from_utf8_lossy(&run_output.stderr)
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
    fn consumer_run_passes_checked_c_string_to_a_pointer_parameter() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let fixture_header = tmp.path().join("checked_c_string_fixture.h");
        std::fs::write(
            &fixture_header,
            "typedef unsigned long size_t;\nsize_t strlen(const char *value);\n",
        )?;
        let source = format!(
            r#"from std.interop import c

binding LibC:
    header = "{}"
    link = c.system_library("c")

    symbol string_length(value: c.ConstPtr[c.c_char]) -> c.Size:
        native = "strlen"

def checked_length(value: str) -> Result[usize, str]:
    text = c.cstr(value)?
    unsafe:
        return Ok(LibC.string_length(text.as_const_ptr()))

def main() -> Result[None, str]:
    assert checked_length("incan")? == 5
    match c.cstr("not{nul}allowed"):
        Err(_) => return Ok(None)
        Ok(_) => return Err("expected c.cstr to reject an interior terminator")
"#,
            fixture_header.display(),
            nul = '\0',
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"checked_c_string\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");

        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            output.status.success(),
            "expected a checked C string to typecheck.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let checkout = support::repo_root();
        let run_output = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["run", main_path.to_string_lossy().as_ref()])
            .output()?;
        assert!(
            run_output.status.success(),
            "expected a checked C string to compile and run.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&run_output.stdout),
            String::from_utf8_lossy(&run_output.stderr)
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
    fn consumer_check_hides_owned_c_resources_behind_an_incan_facade() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let fixture_header = tmp.path().join("fixture.h");
        std::fs::write(
            &fixture_header,
            r#"typedef struct fixture_file FILE;
int fclose(FILE *);
FILE *tmpfile(void);
int fflush(FILE *);
int fileno(FILE *);
int close(int);
"#,
        )?;
        let source = format!(
            r#"from std.interop import c

binding CFile:
    header = "{}"
    link = c.system_library("c")

    resource File:
        native = "FILE"
        release = close

    symbol close(file: c.Owned[File]) -> c.i32:
        native = "fclose"

    symbol open() -> Option[c.Owned[File]]:
        native = "tmpfile"

    symbol flush(file: c.BorrowedMut[File]) -> c.i32:
        native = "fflush"

    symbol descriptor(file: c.Borrowed[File]) -> c.i32:
        native = "fileno"

    symbol close_descriptor(descriptor: c.i32) -> c.i32:
        native = "close"

def temporary_descriptor() -> int:
    unsafe:
        if let Some(open_file) = CFile.open():
            mut file = open_file
            if CFile.flush(file) != 0:
                return -1
            return CFile.descriptor(file)
        return -1

def temporary_file_is_released() -> bool:
    descriptor = temporary_descriptor()
    if descriptor < 0:
        return False
    unsafe:
        return CFile.close_descriptor(descriptor) == -1

def main() -> None:
    assert temporary_file_is_released()
"#,
            fixture_header.display()
        );
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"checked_c_resource_facade\"\n\n[sdk]\nprofile = \"minimal\"\n",
            &source,
        )?;
        let generated_cargo_target = tmp.path().join("generated-cargo-target");

        let output = run_check_against_checkout_sdk(&main_path, &generated_cargo_target)?;
        assert!(
            output.status.success(),
            "expected a same-module Incan façade to hide an owned C resource.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let checkout = support::repo_root();
        let run_output = super::incan_command()
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .env("INCAN_LOCK_PREHEAT", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .args(["run", main_path.to_string_lossy().as_ref()])
            .output()?;
        assert!(
            run_output.status.success(),
            concat!(
                "expected the Incan façade to borrow, flush, and release its encapsulated C resource.\n",
                "stdout:\n{}\nstderr:\n{}",
            ),
            String::from_utf8_lossy(&run_output.stdout),
            String::from_utf8_lossy(&run_output.stderr)
        );
        Ok(())
    }
}

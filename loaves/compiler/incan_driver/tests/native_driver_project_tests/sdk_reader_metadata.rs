//! Native generic reader metadata controls through the actual driver and adapter.

use std::path::{Path, PathBuf};

use super::{DriverFixture, bake, corpus, driver_fixture, fs, prepare_source_driver, success, support};

/// Actual native metadata rejects malformed generic aliases and trait arguments before Rust errors appear.
#[test]
fn sdk_reader_default_native_metadata_rejects_corruption() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("sdk-reader-native-metadata")?;
    let binary = metadata_driver(fixture, &root)?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    for mode in [
        "genuine",
        "missing-arity",
        "extra-arity",
        "model-bound",
        "parameter-kind",
        "argument-cycle",
        "borrowed-argument",
        "field-type",
        "trait-bound",
    ] {
        let source = root.join(format!("{mode}.incn"));
        let destination = root.join(format!("{mode}-native"));
        fs::write(
            &source,
            format!(
                "# reader-plan-control: {mode}\nfrom std.io import BinaryReader, BytesIO\n\ndef main() -> None:\n    reader = BytesIO(b\"abcd\")\n    stream = reader.chunks(2)\n    println(stream.chunk_size)\n"
            ),
        )?;
        let result = corpus::source_command(&binary, &source, &destination, &fixture.sysroot, &closure).output()?;
        fs::write(root.join(format!("{mode}.stdout")), &result.stdout)?;
        fs::write(root.join(format!("{mode}.stderr")), &result.stderr)?;
        fs::write(root.join(format!("{mode}.status")), result.status.to_string())?;
        if mode == "genuine" {
            success(&result, "genuine SDK reader plan through actual adapter metadata");
            continue;
        }
        let diagnostics = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success(), "{mode} must refuse");
        assert!(diagnostics.contains("unsupported"), "{mode}: {diagnostics}");
        let reason = match mode {
            "missing-arity" | "extra-arity" => "stdlib layout type argument count differs from metadata",
            "model-bound" | "trait-bound" => "a type argument has no native impl for the callee's bounds",
            "parameter-kind" => "stdlib layout generic parameter is not a type",
            "argument-cycle" => "recursive stdlib layout type arguments",
            "borrowed-argument" => "borrowed stdlib layout argument has no retained lifetime contract",
            "field-type" => "stdlib layout field differs from metadata",
            _ => return Err(format!("unknown malformed metadata case {mode}").into()),
        };
        assert!(diagnostics.contains(reason), "{mode}: {diagnostics}");
        // rustc can print a public re-export; the hook still selects the exact io declaration and call.
        if mode == "model-bound" {
            assert!(
                diagnostics.contains("incan_stdlib_system::") && diagnostics.contains("::ReaderChunks`"),
                "{mode}: {diagnostics}"
            );
        }
        if mode == "trait-bound" {
            assert!(
                diagnostics.contains("incan_stdlib_system::") && diagnostics.contains("::BinaryReader::chunks`"),
                "{mode}: {diagnostics}"
            );
        }
        for forbidden in [
            "E0107",
            "E0106",
            "E0277",
            "E0391",
            "E0747",
            "internal compiler error",
            "panicked at",
        ] {
            assert!(!diagnostics.contains(forbidden), "{mode}: {diagnostics}");
        }
    }
    Ok(())
}

/// Copy the actual driver and add one post-lowering test hook; source markers keep output reuse case-specific.
fn metadata_driver(fixture: &DriverFixture, root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let driver = prepare_source_driver(root, &support::repo_root())?;
    let frontend_path = driver.join("src/frontend.rs");
    let original = fs::read_to_string(&frontend_path)?;
    let binding = "    let plan = {";
    let adapter = "    crate::adapter::compile(";
    assert_eq!(
        original.matches(binding).count(),
        1,
        "actual source plan binding changed"
    );
    assert_eq!(original.matches(adapter).count(), 1, "actual adapter entry changed");
    let instrumented = original.replacen(binding, "    let mut plan = {", 1).replacen(
        adapter,
        "    crate::reader_plan_controls::apply(&mut plan, Path::new(&arguments[0]))?;\n    crate::adapter::compile(",
        1,
    );
    fs::write(frontend_path, instrumented)?;
    let main = driver.join("src/main.rs");
    fs::write(
        &main,
        format!("{}\nmod reader_plan_controls;\n", fs::read_to_string(&main)?),
    )?;
    let plan_source = fs::read_to_string(support::repo_root().join("loaves/compiler/incan_mir_plan/src/lib.incn"))?;
    let controls = if plan_source.contains("pub library_type_arguments:") {
        include_str!("reader_plan_controls.rs")
    } else {
        "/// Older plan schemas cannot reach the new metadata controls.\npub(crate) fn apply(_plan: &mut crate::plan::Plan, _source: &std::path::Path) -> Result<(), String> { Ok(()) }\n"
    };
    fs::write(driver.join("src/reader_plan_controls.rs"), controls)?;
    bake(&root.join("library"), &fixture.home)?;
    bake(&root.join("lowering"), &fixture.home)?;
    bake(&driver, &fixture.home)?;
    Ok(driver.join("target/rust/debug/incan-rustc-driver"))
}

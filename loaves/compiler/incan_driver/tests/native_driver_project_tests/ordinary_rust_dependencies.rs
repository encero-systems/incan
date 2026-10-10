//! Ordinary native roots at the normal Rust caller boundary, independent of SDK inventory coverage.

use super::{Command, Output, Path, fs, success, support};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A caller links its own native loaf, reuses completed output, and observes native edits and restored inputs.
#[test]
fn ordinary_rust_caller_dependencies_execute_reuse_and_invalidate() -> TestResult {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path().canonicalize()?;
    for project in ["provider", "native", "caller"] {
        fs::create_dir_all(root.join(project).join("src"))?;
    }
    for (relative, contents) in [
        (
            "provider/loaf.toml",
            "[project]\nname='ordinary_provider'\nversion='1.0.0'\n",
        ),
        (
            "provider/src/lib.incn",
            "pub def value() -> int:\n    \"\"\"Return the checked provider value.\"\"\"\n    return 42\n",
        ),
        (
            "native/loaf.toml",
            "[project]\nname='ordinary_native'\nversion='1.0.0'\n[rust]\nname='ordinary_native'\ntype='lib'\nedition='2024'\n",
        ),
        ("native/src/lib.rs", "pub const VALUE: i64 = 7;\n"),
        (
            "caller/loaf.toml",
            "[project]\nname='ordinary_caller'\nversion='1.0.0'\n[dependencies]\nordinary_provider={loaf='ordinary_provider',path='../provider'}\nordinary_native={loaf='ordinary_native',path='../native'}\n[[rust.bin]]\nname='ordinary_caller'\npath='src/main.rs'\n",
        ),
        (
            "caller/src/main.rs",
            "use ordinary_provider::caller::incan::value;\nfn main() { println!(\"{}\", value() + ordinary_native::VALUE); }\n",
        ),
        ("lock.json", r#"{"schema":"incan.oven.loaf-resolution/2","units":[]}"#),
        (
            "graph.json",
            r#"{"index_commit":"0000000000000000000000000000000000000000","registry_lock":"lock.json","facets":[{"project":"native","features":[],"domain":"target"}]}"#,
        ),
    ] {
        fs::write(root.join(relative), contents)?;
    }
    let caller = root.join("caller");
    let bake = |project: &Path, require_reuse: bool, missing_index: bool| -> TestResult<Output> {
        let mut command = support::cli_project::configured_incan_command(project, &["oven", "bake", "--project", "."]);
        support::configure_explicit_oven_bake_command(&mut command)?;
        command
            .env("INCAN_SDK_NATIVE_COMPILER_GRAPH", root.join("graph.json"))
            .env("INCAN_SDK_NATIVE_INDEX", &root)
            .env("INCAN_SDK_NATIVE_BLOBS", &root);
        if require_reuse {
            command.env("INCAN_TEST_REQUIRE_COMPLETED_BAKE_REUSE", "1");
        }
        if missing_index {
            command.env_remove("INCAN_SDK_NATIVE_INDEX");
        }
        Ok(command.output()?)
    };
    success(
        &bake(&root.join("provider"), false, false)?,
        "ordinary Incan provider bake",
    );
    success(&bake(&caller, false, false)?, "ordinary native caller bake");
    assert_native_dependency_work(&caller)?;
    let native = caller.join("target/rust/debug/ordinary_caller");
    let execute = |expected: &[u8]| -> TestResult {
        let output = Command::new(&native).output()?;
        success(&output, "ordinary native caller execution");
        assert_eq!(output.stdout, expected);
        Ok(())
    };
    execute(b"49\n")?;
    success(&bake(&caller, true, false)?, "completed ordinary caller reuse");
    let original_output = fs::read(&native)?;
    let dependency = root.join("native/src/lib.rs");
    fs::write(&dependency, "pub const VALUE: i64 = 8;\n")?;
    assert!(
        !bake(&caller, true, false)?.status.success(),
        "native source edits must invalidate completed reuse"
    );
    let refused = bake(&caller, false, true)?;
    assert!(!refused.status.success());
    assert!(
        String::from_utf8(refused.stderr)?
            .contains("configured Rust-unit native graph requires INCAN_SDK_NATIVE_INDEX")
    );
    assert_eq!(
        fs::read(&native)?,
        original_output,
        "failed preparation must preserve the previous output"
    );
    success(&bake(&caller, false, false)?, "changed ordinary native dependency bake");
    assert_native_dependency_work(&caller)?;
    execute(b"50\n")?;
    fs::write(&dependency, "pub const VALUE: i64 = 7;\n")?;
    success(
        &bake(&caller, true, false)?,
        "restored ordinary native dependency reuse",
    );
    assert_eq!(fs::read(&native)?, original_output);
    execute(b"49\n")?;
    Ok(())
}

/// The producer selects only this native root and compiles at most its missing current generation.
fn assert_native_dependency_work(caller: &Path) -> TestResult {
    let report: serde_json::Value = serde_json::from_slice(&fs::read(
        caller.join("target/rust/dependencies/debug/native-loaf-preparation.json"),
    )?)?;
    assert_eq!(report["selected_units"], 1);
    let compiled = report["compiled"].as_array().ok_or("missing native work counts")?;
    assert!(compiled.len() <= 1);
    for unit in compiled {
        assert_eq!(unit, "ordinary_native 1.0.0 target debug");
    }
    assert_eq!(report["index_blob_bytes"], 0);
    Ok(())
}

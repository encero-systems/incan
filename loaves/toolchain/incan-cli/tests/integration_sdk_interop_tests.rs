#![allow(dead_code)] // Split test binaries intentionally use different parts of shared support fragments.

//! Integration tests for the Incan compiler frontend

include!("support/integration_tests_root.rs");

/// Exercise compiled SDK codecs through both a user re-export and an isolated root module import.
#[test]
fn std_toml_manifest_and_lock_roundtrip_through_compiled_sdk() -> Result<(), Box<dyn std::error::Error>> {
    for (source, expected_stdout, uses_facade) in [
        (
            fs::read_to_string(incan_test_support::fixture("valid/std_toml_typed_lookup.incn"))?
                .replace("from std.toml import", "from codec import"),
            "TOML typed lookup, strict kinds, paths, and locations passed",
            true,
        ),
        (
            fs::read_to_string(incan_test_support::fixture("valid/std_toml_surface.incn"))?
                .replace("from std.toml import", "from codec import"),
            "TOML manifest, lock, datetime, and located errors passed",
            true,
        ),
        (
            fs::read_to_string(incan_test_support::fixture("valid/std_toml_module_import.incn"))?.to_owned(),
            "TOML root module-only roundtrip passed",
            false,
        ),
    ] {
        let temporary = tempfile::tempdir()?;
        let source_dir = temporary.path().join("src");
        fs::create_dir_all(&source_dir)?;
        fs::write(
            temporary.path().join("loaf.toml"),
            "[project]\nname = \"std_toml_native_surface\"\nversion = \"0.1.0\"\n",
        )?;
        if uses_facade {
            fs::write(
                source_dir.join("codec.incn"),
                "pub from std.toml import TomlValue, TomlError, TomlKind, TomlErrorKind, TomlDatetime, parse, deserialize, serialize, serialize_pretty, locate\n",
            )?;
        }
        let main = source_dir.join("main.incn");
        fs::write(&main, source)?;

        let mut bake = incan_command();
        bake.current_dir(temporary.path())
            .args(["oven", "bake", "--project"])
            .arg(temporary.path());
        support::configure_explicit_oven_bake_command(&mut bake)?;
        let baked = bake.output()?;
        assert!(
            baked.status.success(),
            "TOML native bake failed:\n{}\n{}",
            String::from_utf8_lossy(&baked.stdout),
            String::from_utf8_lossy(&baked.stderr)
        );
        let output = incan_command()
            .current_dir(temporary.path())
            .args(["run", "--locked"])
            .arg(main)
            .output()?;
        assert!(
            output.status.success(),
            "TOML native assertions failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout)).trim(),
            expected_stdout
        );
        let generated = temporary.path().join("target/incan/std_toml_native_surface");
        assert!(
            !generated.join("src/__incan_std/toml.rs").exists(),
            "the consumer must select compiled std.toml rather than materialize its implementation"
        );
        let provider = compiled_sdk_provider_artifact_root(&generated, "incan_stdlib_data")?;
        assert!(
            provider.join("src/toml.rs").is_file(),
            "compiled stdlib-data must own the TOML implementation"
        );
    }
    Ok(())
}

/// Native Rust checking must enforce imported bounds after Incan accepts scalar and collection instantiations.
#[test]
fn imported_rust_generic_bounds_remain_native_obligations() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let project_root = temporary.path().join("project");
    let source_dir = project_root.join("src");
    fs::create_dir_all(&source_dir)?;
    fs::write(
        project_root.join("loaf.toml"),
        "[project]\nname = \"foreign_bounds\"\nversion = \"0.1.0\"\n[rust-dependencies]\nserde = \"1.0\"\n",
    )?;
    fs::write(
        source_dir.join("bounds.incn"),
        fs::read_to_string(incan_test_support::fixture("valid/rust_generic_bounds.incn"))?,
    )?;
    let main = source_dir.join("main.incn");
    fs::write(
        &main,
        "from bounds import identity, Reader\ndef main() -> None:\n    assert identity(\"demo\") == \"demo\"\n    assert Reader().identity([\"linux\"]) == [\"linux\"]\n",
    )?;
    // Exercise a noncanonical project spelling independently of the host temporary-directory layout.
    #[cfg(unix)]
    let bake_root = {
        let alias = temporary.path().join("project-alias");
        std::os::unix::fs::symlink(fs::canonicalize(&project_root)?, &alias)?;
        alias
    };
    #[cfg(not(unix))]
    let bake_root = project_root.clone();
    let mut bake = incan_command();
    bake.current_dir(&project_root)
        .args(["oven", "bake", "--project"])
        .arg(&bake_root);
    support::configure_explicit_oven_bake_command(&mut bake)?;
    let baked = bake.output()?;
    assert!(
        baked.status.success(),
        "native bound bake failed:\n{}\n{}",
        String::from_utf8_lossy(&baked.stdout),
        String::from_utf8_lossy(&baked.stderr)
    );
    let ran = incan_command()
        .current_dir(&project_root)
        .args(["run", "--locked"])
        .arg(&main)
        .output()?;
    assert!(
        ran.status.success(),
        "native bound assertions failed:\n{}\n{}",
        String::from_utf8_lossy(&ran.stdout),
        String::from_utf8_lossy(&ran.stderr)
    );

    fs::write(
        &main,
        "from bounds import identity\nmodel Plain:\n    value: str\ndef main() -> None:\n    identity(Plain(value=\"demo\"))\n",
    )?;
    let mut bake = incan_command();
    bake.current_dir(&project_root)
        .args(["oven", "bake", "--project"])
        .arg(&bake_root);
    support::configure_explicit_oven_bake_command(&mut bake)?;
    let rejected = bake.output()?;
    let diagnostic = format!(
        "{}\n{}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(
        !rejected.status.success(),
        "a model without Deserialize must not satisfy the foreign bound"
    );
    assert!(
        diagnostic.contains("E0277") && diagnostic.contains("Deserialize") && diagnostic.contains("Plain"),
        "expected native trait-obligation failure, got:\n{diagnostic}"
    );
    assert!(
        !diagnostic.contains("violates generic bound"),
        "foreign obligations must reach native checking: {diagnostic}"
    );
    Ok(())
}

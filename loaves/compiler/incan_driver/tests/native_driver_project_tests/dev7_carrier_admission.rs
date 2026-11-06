//! Genuine nested carrier controls at the real frontend, Incan lowering, and native validator boundaries.

use super::*;

/// Genuine source plans validate their checked JSON root and reject corrupt native header contracts before rustc.
#[test]
fn checked_json_roots_require_exact_native_forwarding_contracts() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let (_caller, binary) = baked_carrier_caller(fixture, "checked-json-root-admission")?;
    let output = driver_command(&binary).arg("checked-json").output()?;
    success(&output, "checked JSON root admission controls");
    assert_eq!(
        output.stdout,
        b"checked JSON admission: 1 genuine plan and 8 native forwarding ABI refusals passed\n"
    );
    Ok(())
}

/// Bake a test-only caller including the actual driver validator and require positive and independent negative cases.
#[test]
fn checked_nested_carriers_preserve_projection_and_layout_admission() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let (_caller, binary) = baked_carrier_caller(fixture, "dev7-carrier-admission")?;
    let output = driver_command(&binary).output()?;
    success(&output, "nested carrier admission controls");
    assert_eq!(
        output.stdout,
        b"nested carrier admission: 6 positive, 23 Body IR negative, and 8 native plan negative controls passed\n"
    );
    Ok(())
}

/// Require instantiated stdlib Result returns to retain their final owner's normal and subsequent-call unwind drops.
#[test]
fn instantiated_stdlib_results_retain_owned_cleanup() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let (_caller, binary) = baked_carrier_caller(fixture, "instantiated-stdlib-owned-results")?;
    let output = driver_command(&binary).arg("owned-result").output()?;
    success(&output, "instantiated stdlib result ownership");
    assert_eq!(
        output.stdout,
        b"instantiated result ownership: normal and unwind drops passed\n"
    );
    Ok(())
}

/// Return the baked executable with its scratch owner so the caller keeps it alive until execution completes.
pub(super) fn baked_carrier_caller(
    fixture: &DriverFixture,
    scratch: &str,
) -> Result<(DriverScratch, PathBuf), Box<dyn std::error::Error>> {
    let caller = fixture.scratch(scratch)?;
    let repo = support::repo_root();
    fs::create_dir_all(caller.join("src"))?;
    copy_tree(&fixture.driver.join("src"), &caller.join("src/native"))?;
    let mut modules = String::new();
    for name in [
        "ast_build",
        "bodies",
        "callees",
        "captured_generators",
        "checked_json",
        "cloneable_closures",
        "closures",
        "comparison_bridges",
        "declarations",
        "error",
        "field_metadata",
        "frozen_values",
        "futures",
        "plan",
        "shared_callables",
        "spans",
        "statics",
        "terminators",
        "types",
        "validation",
        "values",
    ] {
        let source = format!("native/{name}.rs");
        modules.push_str(&format!("#[path = {source:?}]\nmod {name};\n"));
    }
    fs::write(
        caller.join("src/main.rs"),
        include_str!("dev7_carrier_admission_caller.rs").replace("// DRIVER_MODULES", &modules),
    )?;
    fs::write(
        caller.join("src/instantiated_owned_results_caller.rs"),
        include_str!("instantiated_owned_results_caller.rs"),
    )?;
    fs::write(
        caller.join("src/checked_json_admission_caller.rs"),
        include_str!("checked_json_admission_caller.rs"),
    )?;
    fs::write(
        caller.join("src/numeric_boundaries_caller.rs"),
        include_str!("numeric_boundaries_caller.rs"),
    )?;
    fs::write(
        caller.join("src/numeric_boundaries_sources.rs"),
        include_str!("numeric_boundaries_sources.rs"),
    )?;
    fs::copy(
        repo.join("loaves/stdlib/core/src/result.incn"),
        caller.join("src/result.incn"),
    )?;
    let lowering = fixture_dependency_path(&caller, &fixture.root.join("lowering"))?;
    let frontend = fixture_dependency_path(&caller, &repo.join("loaves/compiler/incan_frontend"))?;
    let semantics = fixture_dependency_path(&caller, &repo.join("loaves/kernel/incan_semantics_core"))?;
    fs::write(
        caller.join("loaf.toml"),
        format!(
            r#"[project]
name = "dev7-carrier-admission"
version = "0.1.0"
private = true

[dependencies]
incan_mir_lowering = {{ loaf = "incan_mir_lowering", path = "{lowering}" }}
incan_frontend = {{ loaf = "incan_frontend", path = "{frontend}", default-features = false }}
incan_semantics_core = {{ loaf = "incan_semantics_core", path = "{semantics}" }}
thiserror = {{ loaf = "crates-io/thiserror", version = "2" }}

[[rust.bin]]
name = "dev7_carrier_admission"
path = "src/main.rs"
unstable_features = ["rustc_private"]
toolchain_components = ["rustc-dev"]
sysroot_dependencies = ["rustc_driver"]
"#
        ),
    )?;
    bake(&caller, &fixture.home)?;
    let binary = caller.join("target/rust/debug/dev7_carrier_admission");
    Ok((caller, binary))
}

/// Repeated error formatting retains one live String owner before transferring both outputs to the f-string array.
#[test]
fn validation_error_formatting_retains_single_owned_cleanup() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let (_caller, binary) = baked_carrier_caller(fixture, "validation-error-format-owned-results")?;
    let output = driver_command(&binary).arg("validation-format").output()?;
    success(&output, "validation error formatting ownership");
    assert_eq!(
        output.stdout,
        b"validation formatting ownership: second-call cleanup and array transfer passed\n"
    );
    Ok(())
}

/// A retained mutable enum reference must match its original declaration index and name.
#[test]
fn mutable_enum_references_require_exact_declaration_identity() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let (_caller, binary) = baked_carrier_caller(fixture, "mutable-enum-reference-admission")?;
    let output = driver_command(&binary).arg("mutable-enum").output()?;
    success(&output, "mutable enum reference admission");
    Ok(())
}

/// An exclusive tuple reference may move but cannot be duplicated by an implicit Copy operand.
#[test]
fn mutable_tuple_references_preserve_exclusive_transfer() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let (_caller, binary) = baked_carrier_caller(fixture, "mutable-tuple-reference-admission")?;
    let output = driver_command(&binary).arg("mutable-tuple").output()?;
    success(&output, "mutable tuple reference admission");
    Ok(())
}

/// Malformed nested collection shapes return typed admission errors in locals and callable signatures.
#[test]
fn nested_collection_leaf_arity_is_checked_before_projection() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let (_caller, binary) = baked_carrier_caller(fixture, "nested-collection-arity")?;
    let output = driver_command(&binary).arg("collection-arity").output()?;
    success(&output, "nested collection arity admission controls");
    assert_eq!(
        output.stdout,
        b"nested collection arity: genuine source plan and 44 typed native refusals passed\n"
    );
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

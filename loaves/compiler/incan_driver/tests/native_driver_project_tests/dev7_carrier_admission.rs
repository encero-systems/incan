//! Genuine nested carrier controls at the real frontend, Incan lowering, and native validator boundaries.

use super::*;

/// Bake a test-only caller including the actual driver validator and require positive and independent negative cases.
#[test]
fn checked_nested_carriers_preserve_projection_and_layout_admission() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let binary = baked_carrier_caller(fixture, "dev7-carrier-admission")?;
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
    let binary = baked_carrier_caller(fixture, "instantiated-stdlib-owned-results")?;
    let output = driver_command(&binary).arg("owned-result").output()?;
    success(&output, "instantiated stdlib result ownership");
    assert_eq!(
        output.stdout,
        b"instantiated result ownership: normal and unwind drops passed\n"
    );
    Ok(())
}

/// Bake the shared caller once for one test's isolated scratch directory and return its executable path.
fn baked_carrier_caller(fixture: &DriverFixture, scratch: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let caller = fixture.scratch(scratch)?;
    let repo = support::repo_root();
    fs::create_dir_all(caller.join("src"))?;
    copy_tree(&fixture.driver.join("src"), &caller.join("src/native"))?;
    let mut modules = String::new();
    for name in [
        "bodies",
        "callees",
        "captured_generators",
        "closures",
        "declarations",
        "error",
        "frozen_values",
        "plan",
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
    Ok(caller.join("target/rust/debug/dev7_carrier_admission"))
}

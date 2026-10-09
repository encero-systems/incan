//! Genuine nested carrier controls at the real frontend, Incan lowering, and native validator boundaries.

use super::*;

/// Bake a test-only caller including the actual driver validator and require positive and independent negative cases.
#[test]
fn checked_nested_carriers_preserve_projection_and_layout_admission() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let caller = fixture.scratch("dev7-carrier-admission")?;
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
        "plan",
        "spans",
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
sysroot_dependencies = ["rustc_driver", "rustc_interface"]
"#
        ),
    )?;
    bake(&caller, &fixture.home)?;
    let output = driver_command(&caller.join("target/rust/debug/dev7_carrier_admission")).output()?;
    success(&output, "nested carrier admission controls");
    assert_eq!(
        output.stdout,
        b"nested carrier admission: 6 positive, 23 Body IR negative, and 8 native plan negative controls passed\n"
    );
    Ok(())
}

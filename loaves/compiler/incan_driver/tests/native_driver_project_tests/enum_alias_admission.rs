//! Target-integrity controls across the actual Incan lowering caller boundary (#1337, #1698).

use super::*;

/// Bake one caller against the prepared lowering provider and require every positive and negative target control.
#[test]
fn checked_enum_aliases_preserve_canonical_target_admission() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let caller = fixture.scratch("enum-alias-admission")?;
    let repo = support::repo_root();
    fs::create_dir_all(caller.join("src"))?;
    fs::write(
        caller.join("src/main.rs"),
        include_str!("enum_alias_admission_caller.rs"),
    )?;
    let lowering = fixture_dependency_path(&caller, &fixture.root.join("lowering"))?;
    let frontend = fixture_dependency_path(&caller, &repo.join("loaves/compiler/incan_frontend"))?;
    let semantics = fixture_dependency_path(&caller, &repo.join("loaves/kernel/incan_semantics_core"))?;
    fs::write(
        caller.join("loaf.toml"),
        format!(
            r#"[project]
name = "enum-alias-admission"
version = "0.1.0"
private = true

[dependencies]
incan_mir_lowering = {{ loaf = "incan_mir_lowering", path = "{lowering}" }}
incan_frontend = {{ loaf = "incan_frontend", path = "{frontend}", default-features = false }}
incan_semantics_core = {{ loaf = "incan_semantics_core", path = "{semantics}" }}

[[rust.bin]]
name = "enum_alias_admission"
path = "src/main.rs"
"#,
        ),
    )?;
    bake(&caller, &fixture.home)?;
    let output = Command::new(caller.join("target/rust/debug/enum_alias_admission")).output()?;
    success(&output, "canonical enum target admission controls");
    assert_eq!(
        output.stdout,
        b"enum target admission: 3 positive and 14 negative controls passed\n"
    );
    Ok(())
}

//! The ordinary library compile writes its own checked-body count companion.

use std::path::Path;
use std::{fs, process::Command};

use super::compile_unit;

/// Exercise the actual provider compile, including an activation with no concrete callable.
#[test]
fn ordinary_provider_writes_actual_body_counts() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = Command::new("rustc").args(["--print", "sysroot"]).output()?;
    assert!(
        runtime.status.success(),
        "the selected compiler did not report its sysroot"
    );
    let sysroot = String::from_utf8(runtime.stdout)?;
    for (name, source, evaluations) in [
        (
            "provider_with_body",
            "pub def value() -> int:\n    \"\"\"Return the provider value.\"\"\"\n    return 7\n",
            1,
        ),
        ("provider_without_body", "const VALUE: int = 7\n", 0),
    ] {
        let scratch = tempfile::tempdir()?;
        let root = scratch.path();
        let unit = describe_provider(root, name, source)?;
        let output = root.join(format!("lib{name}.rlib"));
        let counts = root.join("checked-body-counts.crumb");
        let arguments = [
            unit.to_string_lossy().into_owned(),
            name.to_owned(),
            output.to_string_lossy().into_owned(),
            sysroot.trim().to_owned(),
            "--no-codegraph".to_owned(),
            "--checked-body-counts".to_owned(),
            counts.to_string_lossy().into_owned(),
        ];
        compile_unit(&arguments, false, None)?;
        assert!(output.is_file(), "the ordinary provider did not finish");
        let encoded = fs::read(counts)?;
        assert_eq!(
            internal_models::decode_provider_body_tally(&encoded, &arguments[0], &arguments[6])?,
            evaluations
        );
        assert!(
            internal_models::decode_provider_body_tally(&encoded, &arguments[0], &"another activation".into()).is_err()
        );
    }
    Ok(())
}

/// Describe the smallest ordinary library to the same reader used by Oven's provider command.
fn describe_provider(root: &Path, name: &str, source: &str) -> std::io::Result<std::path::PathBuf> {
    let entry = root.join("lib.incn");
    fs::write(&entry, source)?;
    let unit = root.join("provider.unit.json");
    let document = serde_json::json!({
        "schema": "incan.unit/1",
        "kind": "library",
        "source": entry,
        "project_root": root,
        "source_root": root,
        "package": name,
        "version": "0.1.0",
        "profile": "debug",
        "declared_features": [],
        "active_features": [],
        "loaves": [],
        "rust_crates": [],
        "contract_models": [],
        "c_headers": [],
        "externs": [],
        "search": [],
        "rust_abi": [],
    });
    fs::write(&unit, document.to_string())?;
    Ok(unit)
}

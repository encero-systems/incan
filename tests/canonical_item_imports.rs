//! Native import spellings must select the same canonical function while preserving module bindings.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::process::Command;

mod support;

/// Build and execute each spelling in a fresh two-file project so another import cannot supply its missing symbol.
#[test]
fn canonical_item_imports_link_and_run_through_oven() -> Result<(), Box<dyn Error>> {
    for (label, import, expression, helper_path) in [
        ("plain", "import helper::value", "value()", "helper.incn"),
        ("aliased", "import helper::value as answer", "answer()", "helper.incn"),
        ("from", "from helper import value", "value()", "helper.incn"),
        (
            "from_alias",
            "from helper import value as answer",
            "answer()",
            "helper.incn",
        ),
        (
            "nested_item",
            "import package::helper::value",
            "value()",
            "package/helper.incn",
        ),
        (
            "nested_module_alias",
            "import package::helper as helpers",
            "helpers.value()",
            "package/helper.incn",
        ),
    ] {
        let project = tempfile::tempdir()?;
        let source_dir = project.path().join("src");
        let helper = source_dir.join(helper_path);
        let helper_parent = helper.parent().ok_or("helper source has no parent")?;
        fs::create_dir_all(helper_parent)?;
        fs::write(&helper, "pub def value() -> int:\n    return 42\n")?;
        fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname = \"canonical_import_probe\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(
            source_dir.join("main.incn"),
            format!(
                "{import}\n\ndef main() -> None:\n    result = {expression}\n    assert result == 42\n    println(result)\n"
            ),
        )?;

        let mut bake = project_command(project.path());
        bake.args(["oven", "bake", "--project", "."]);
        support::configure_explicit_oven_bake_command(&mut bake)?;
        let baked = bake.output()?;
        assert!(
            baked.status.success(),
            "{label}: native bake failed:\n{}\n{}",
            String::from_utf8_lossy(&baked.stdout),
            String::from_utf8_lossy(&baked.stderr)
        );
        let output = project_command(project.path())
            .args(["run", "--locked", "src/main.incn"])
            .output()?;
        assert!(
            output.status.success(),
            "{label}: locked execution failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "42", "{label}");
    }
    Ok(())
}

/// Retain the caller's SDK and shared Cargo selection while keeping generated project output under its temp root.
fn project_command(project: &Path) -> Command {
    let mut command = Command::new(support::incan_binary());
    command
        .current_dir(project)
        .env("INCAN_NO_BANNER", "1")
        .env_remove("INCAN_INTERNAL_PROJECT_ROOT")
        .env_remove("INCAN_INTERNAL_MANIFEST_OVERRIDE");
    command
}

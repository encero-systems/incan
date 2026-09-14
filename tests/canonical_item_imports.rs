//! Native import spellings must select the same canonical function while preserving module bindings.

use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::Path;
use std::process::Command;

mod support;

/// One import spelling, the module that exercises it, and the helper source it has to reach.
struct Spelling {
    module: &'static str,
    import: &'static str,
    expression: &'static str,
    helper_path: &'static str,
}

const SPELLINGS: &[Spelling] = &[
    Spelling {
        module: "case_plain",
        import: "import helper::value",
        expression: "value()",
        helper_path: "helper.incn",
    },
    Spelling {
        module: "case_aliased",
        import: "import helper::value as answer",
        expression: "answer()",
        helper_path: "helper.incn",
    },
    Spelling {
        module: "case_from",
        import: "from helper import value",
        expression: "value()",
        helper_path: "helper.incn",
    },
    Spelling {
        module: "case_from_alias",
        import: "from helper import value as answer",
        expression: "answer()",
        helper_path: "helper.incn",
    },
    Spelling {
        module: "case_nested_item",
        import: "import package::helper::value",
        expression: "value()",
        helper_path: "package/helper.incn",
    },
    Spelling {
        module: "case_nested_module_alias",
        import: "import package::helper as helpers",
        expression: "helpers.value()",
        helper_path: "package/helper.incn",
    },
];

/// Every spelling links and runs, each in its own module so none can supply another's symbol.
///
/// This used to bake and run a separate two-file project per spelling, for the isolation: a shared project might
/// let one import satisfy another's missing symbol, which would make the whole test vacuous. Six projects bought
/// that isolation at six native bakes — each re-establishing the same dependency closure, because the six
/// manifests were identical.
///
/// Module scope buys the same isolation for one bake. Each spelling lives in its own module holding only its own
/// import, and `an_import_does_not_reach_across_modules` proves that a module which loses its import stops
/// resolving rather than borrowing a sibling's. That negative is what makes the merge safe: without it, a later
/// refactor could fold the cases together and quietly leave this test asserting nothing.
#[test]
fn canonical_item_imports_link_and_run_through_oven() -> Result<(), Box<dyn Error>> {
    let project = tempfile::tempdir()?;
    write_project(project.path(), None)?;

    let mut bake = project_command(project.path());
    bake.args(["oven", "bake", "--project", "."]);
    support::configure_explicit_oven_bake_command(&mut bake)?;
    let baked = bake.output()?;
    assert!(
        baked.status.success(),
        "native bake failed:\n{}\n{}",
        String::from_utf8_lossy(&baked.stdout),
        String::from_utf8_lossy(&baked.stderr)
    );

    let output = project_command(project.path())
        .args(["run", "--locked", "src/main.incn"])
        .output()?;
    assert!(
        output.status.success(),
        "locked execution failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "42");
    Ok(())
}

/// A module that drops its own import stops resolving, rather than reaching a sibling module's.
///
/// This is the isolation the per-spelling projects used to provide physically. Import resolution is decided in the
/// frontend, so `check` settles it without a bake; asserting it once on one spelling is enough, because the
/// property belongs to module scope rather than to any particular import form.
#[test]
fn an_import_does_not_reach_across_modules() -> Result<(), Box<dyn Error>> {
    let project = tempfile::tempdir()?;
    write_project(project.path(), Some("case_plain"))?;

    let checked = project_command(project.path())
        .args(["check", "src/main.incn"])
        .output()?;
    let diagnostics = format!(
        "{}{}",
        String::from_utf8_lossy(&checked.stdout),
        String::from_utf8_lossy(&checked.stderr)
    );
    assert!(
        !checked.status.success(),
        "a module without its import must not resolve through a sibling's: {diagnostics}"
    );
    assert!(
        diagnostics.contains("Unknown symbol 'value'"),
        "the refusal must name the unresolved symbol: {diagnostics}"
    );
    Ok(())
}

/// Write the whole probe project, optionally stripping one named module's import to exercise isolation.
///
/// Every spelling gets its own module file holding a single import, and `main` calls all six under distinct
/// aliases so one module's binding cannot stand in for another's.
fn write_project(root: &Path, strip_import_from: Option<&str>) -> Result<(), Box<dyn Error>> {
    let source_dir = root.join("src");
    fs::create_dir_all(&source_dir)?;
    fs::write(
        root.join("loaf.toml"),
        "[project]\nname = \"canonical_import_probe\"\nversion = \"0.1.0\"\n",
    )?;
    let helper_paths = SPELLINGS
        .iter()
        .map(|spelling| spelling.helper_path)
        .collect::<BTreeSet<_>>();
    for helper_path in helper_paths {
        let helper = source_dir.join(helper_path);
        let helper_parent = helper.parent().ok_or("helper source has no parent")?;
        fs::create_dir_all(helper_parent)?;
        fs::write(&helper, "pub def value() -> int:\n    return 42\n")?;
    }
    for spelling in SPELLINGS {
        let import = if strip_import_from == Some(spelling.module) {
            String::new()
        } else {
            format!("{}\n\n", spelling.import)
        };
        fs::write(
            source_dir.join(format!("{}.incn", spelling.module)),
            format!("{import}pub def check() -> int:\n    return {}\n", spelling.expression),
        )?;
    }
    let imports = SPELLINGS
        .iter()
        .map(|spelling| format!("from {} import check as {}_check\n", spelling.module, spelling.module))
        .collect::<String>();
    let assertions = SPELLINGS
        .iter()
        .map(|spelling| format!("    assert {}_check() == 42\n", spelling.module))
        .collect::<String>();
    fs::write(
        source_dir.join("main.incn"),
        format!("{imports}\ndef main() -> None:\n{assertions}    println(42)\n"),
    )?;
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

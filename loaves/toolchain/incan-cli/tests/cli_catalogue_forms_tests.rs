//! Issue #1385: a canonical form in the capability catalog compiles as written.
//!
//! `loaves/stdlib/core/src/features.incn` is the source the feature inventory reference is generated from, so a form
//! that does not compile reaches users as documentation of an API that does not exist in that shape. It is also the
//! corpus `example_capability_coverage.rs` measures the examples against, and a form nobody can write is a form no
//! example can honestly demonstrate. `TimeDelta(days=1)` was such a form: `TimeDelta` has three required fields, and
//! the real spelling is the static constructor `TimeDelta.days(1)`.
//!
//! This root reads the live catalog rather than restating its forms, so it cannot drift from what the reference
//! publishes. An entry is admitted here when its forms are self-contained: the `from ... import ...` forms supply
//! the names, and every other form is an expression that can stand on its own. The generated program places the
//! import forms at module top and each remaining form in expression position, then builds and runs it through Oven.

use std::fs;
use std::process::Command;

use incan_test_support::cli_project::{assert_success, run_explicit_oven_bake, run_incan, write_minimal_project};
use regex::Regex;

/// Catalog entries whose canonical forms are self-contained enough to compile as one program.
///
/// Each entry names the `const <name>_forms` binding in the live catalog. An entry belongs here once every one
/// of its non-import forms is a complete expression under the names its own import forms bring into scope; entries
/// whose forms are declarations, fragments, or `?`-propagating calls that need a `Result`-returning context stay
/// out until they are.
const SELF_CONTAINED_ENTRIES: &[&str] = &["std_datetime_forms"];

/// Read one `const <name>: FrozenList[str] = [...]` form list out of the live catalog.
fn catalogue_forms(name: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let catalogue = fs::read_to_string(incan_test_support::repo_root().join("loaves/stdlib/core/src/features.incn"))?;
    let declaration = Regex::new(&format!(r#"(?s)const\s+{name}:\s*FrozenList\[str\]\s*=\s*\[(.*?)\]\n"#))?;
    let quoted = Regex::new(r#""((?:[^"\\]|\\.)*)""#)?;
    let body = declaration
        .captures(&catalogue)
        .and_then(|found| found.get(1))
        .ok_or_else(|| format!("the catalog declares no `{name}` form list"))?;
    let forms = quoted
        .captures_iter(body.as_str())
        .filter_map(|form| form.get(1).map(|value| value.as_str().replace("\\\"", "\"")))
        .collect::<Vec<_>>();
    if forms.is_empty() {
        return Err(format!("`{name}` publishes no forms").into());
    }
    Ok(forms)
}

/// Assemble one program from an entry's forms: imports at module top, every other form as an expression statement.
///
/// The forms are statements of their own rather than `_ = form` bindings because `_` is an ordinary immutable name
/// in Incan, so a second `_ = ...` would be a reassignment error unrelated to the form under test.
fn program_for(forms: &[String]) -> String {
    let (imports, expressions): (Vec<&String>, Vec<&String>) = forms
        .iter()
        .partition(|form| form.starts_with("from ") || form.starts_with("import "));
    let mut program = String::new();
    for import in imports {
        program.push_str(import);
        program.push('\n');
    }
    program.push_str("\ndef main() -> None:\n");
    for form in expressions {
        program.push_str("    ");
        program.push_str(form);
        program.push('\n');
    }
    program.push_str("    println(\"catalogue forms ok\")\n");
    program
}

/// Build and run every self-contained entry's forms as a program through Oven.
#[test]
fn self_contained_catalogue_forms_compile_as_written_issue1385() -> Result<(), Box<dyn std::error::Error>> {
    for entry in SELF_CONTAINED_ENTRIES {
        let forms = catalogue_forms(entry)?;
        let source = program_for(&forms);

        let tmp = tempfile::tempdir()?;
        let project_name = "catalog_forms";
        write_minimal_project(tmp.path(), project_name, "")?;
        fs::write(tmp.path().join("src/main.incn"), &source)?;

        // Under the compiler suite the build resolves through the suite's stdlib Loaf and needs no Cargo. Outside it a
        // fresh project has no inspection authority until it is baked once, so the standalone run bakes first.
        if !incan_test_support::oven_compiler_suite_is_active() {
            let bake = run_explicit_oven_bake(tmp.path())?;
            assert_success(&bake, &format!("prepare the `{entry}` catalog-forms fixture"));
        }
        let build = run_incan(tmp.path(), &["build", "src/main.incn"])?;
        assert!(
            build.status.success(),
            "the `{entry}` catalog forms must compile as written.\nprogram:\n{source}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build.stdout),
            String::from_utf8_lossy(&build.stderr)
        );

        let binary = tmp
            .path()
            .join("target/incan")
            .join(project_name)
            .join("oven/release")
            .join(project_name);
        let run = Command::new(&binary).output()?;
        assert!(
            run.status.success(),
            "the `{entry}` catalog-forms program must run.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        );
        assert_eq!(String::from_utf8(run.stdout)?.trim(), "catalog forms ok");
    }
    Ok(())
}

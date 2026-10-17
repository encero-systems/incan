//! Actual native session assembly and conversion authority controls.

use super::*;

/// The unchanged facade reaches the actual private session and Incan planner, retaining inputs if either boundary
/// refuses.
#[test]
fn original_facade_session_and_incan_plan_keep_carriers() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let project = fixture.scratch("original-facade-source-plan")?;
    let original = incan_test_support::behavior_fixtures::discover(
        &support::fixtures_dir().join("behavior/cli_modules_and_declarations"),
    )?
    .into_iter()
    .find(|fixture| fixture.name == "facade_reexport_alias_target")
    .ok_or("missing original facade")?;
    let sources = project.join("source");
    fs::create_dir_all(&sources)?;
    incan_test_support::behavior_fixtures::materialize(&original, &sources)?;
    let binary = bake_conversion_assembly_caller(fixture, &project)?;
    let output = driver_command(&binary)
        .arg("--source-plan-probe")
        .arg(sources.join("src/main.incn"))
        .arg(project.join("actual-body-ir.json"))
        .output()?;
    fs::write(project.join("probe.stdout"), &output.stdout)?;
    fs::write(project.join("probe.stderr"), &output.stderr)?;
    success(&output, "original facade actual session and Incan planner");
    Ok(())
}

/// A conversion import cannot veto ordinary cross-module layouts whose conversions are never called.
#[test]
fn unused_foreign_newtype_conversion_chain_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_modules_case(
        "unused_foreign_newtype_conversion_chain",
        r#"from owners import Port
from std.environ import get_as

type Wrapper = newtype Port
type Envelope = newtype Wrapper


def hold(value: Option[Envelope]) -> None:
    """Keep ordinary storage live without invoking conversion or constructing its payload."""
    pass


def main() -> None:
    hold(None)
"#,
        &[("owners.incn", "pub type Port = newtype int\n")],
        b"",
    )
}

/// Conversion import activation is retained once by the actual native session and cannot be injected by a fragment.
#[test]
fn composed_conversion_assembly_retains_one_authentic_contract() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let project = fixture.scratch("composed-conversion-assembly")?;
    let sources = project.join("sources");
    write_conversion_sources(&sources)?;
    let binary = bake_conversion_assembly_caller(fixture, &project)?;
    let output = driver_command(&binary).arg(&sources).output()?;
    success(&output, "composed conversion actual assembly authority");
    assert_eq!(
        output.stdout,
        b"conversion assembly: 5 selected, 1 foreign chain, 1 inactive, 2 missing-authority controls, 5 authority refusals, and 4 source-header refusals passed\n"
    );
    Ok(())
}

/// Keep each import context in its own module graph; the original fixture is copied without source changes.
fn write_conversion_sources(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let declarations = "type Port = newtype int\ntype WrappedPort = newtype Port\n\ndef main() -> None:\n    pass\n";
    for (name, imports) in [
        ("inactive", ""),
        ("direct", "from std.traits.convert import TryFrom\n\n"),
        ("qualified", "import std.traits.convert as convert\n\n"),
        ("dependency", "import dependency\n\n"),
    ] {
        let directory = root.join(name);
        fs::create_dir_all(&directory)?;
        fs::write(directory.join("main.incn"), format!("{imports}{declarations}"))?;
    }
    fs::write(
        root.join("dependency/dependency.incn"),
        "import std.traits.convert as convert\n\npub def unused() -> None:\n    pass\n",
    )?;
    fs::create_dir_all(root.join("original"))?;
    fs::create_dir_all(root.join("dependency_owners"))?;
    fs::write(
        root.join("dependency_owners/main.incn"),
        "from std.environ import get_as\nfrom owners import WrappedPort\n\ndef main() -> None:\n    pass\n",
    )?;
    fs::write(
        root.join("dependency_owners/owners.incn"),
        "pub type Port = newtype int\npub type WrappedPort = newtype Port\n",
    )?;
    fs::create_dir_all(root.join("foreign_chain"))?;
    fs::write(
        root.join("foreign_chain/main.incn"),
        "from owners import Port\nfrom std.environ import get_as\n\ntype Wrapper = newtype Port\ntype Envelope = newtype Wrapper\n\ndef main() -> None:\n    pass\n",
    )?;
    fs::write(root.join("foreign_chain/owners.incn"), "pub type Port = newtype int\n")?;
    fs::write(
        root.join("original/main.incn"),
        include_str!(
            "../../../incan_test_support/fixtures/behavior/codegen_models_and_collections/environ_get_as_composed_newtype.incn"
        ),
    )?;
    Ok(())
}

/// Copy the actual source-assembly functions, omitting only the unrelated native compile entry and its adapter.
fn bake_conversion_assembly_caller(
    fixture: &DriverFixture,
    project: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    bake_session_assembly_caller(fixture, project, include_str!("composed_conversion_assembly_caller.rs"))
}

/// Bake a caller against the actual private session assembly, exposing only its checked output for assertions.
pub(super) fn bake_session_assembly_caller(
    fixture: &DriverFixture,
    project: &Path,
    entry_source: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let caller = project.join("caller");
    fs::create_dir_all(caller.join("src"))?;
    let original = fs::read_to_string(fixture.driver.join("src/frontend.rs"))?;
    let boundary = original
        .find("\n/// Exact caller-declared native libraries")
        .ok_or("native frontend assembly boundary missing")?;
    let mut assembly = original[..boundary].to_owned();
    assembly.push_str(
        r#"
/// Test-only view of the actual private session assembly without duplicating its selection policy.
pub fn collect_for_test(path: &Path) -> Result<(Vec<BodyIrModule>, Vec<String>, Vec<String>, usize), String> {
    let path = path.to_path_buf();
    let program = incan_frontend::compiler_stack::run_on_compiler_stack(move || checked_modules(&path, None))?;
    Ok((program.modules, program.sources, program.files, program.entry))
}
"#,
    );
    fs::write(caller.join("src/frontend.rs"), assembly)?;
    fs::write(caller.join("src/main.rs"), entry_source)?;
    write_source_header_controls(&caller, &fixture.driver)?;
    let repo = support::repo_root();
    let lowering = fixture_dependency_path(&caller, &fixture.root.join("lowering"))?;
    let frontend = fixture_dependency_path(&caller, &repo.join("loaves/compiler/incan_frontend"))?;
    let semantics = fixture_dependency_path(&caller, &repo.join("loaves/kernel/incan_semantics_core"))?;
    let driver = fixture_dependency_path(&caller, &repo.join("loaves/compiler/incan_driver"))?;
    let oven_model = fixture_dependency_path(&caller, &repo.join("loaves/oven/oven_model"))?;
    fs::write(
        caller.join("loaf.toml"),
        format!(
            r#"[project]
name = "composed-conversion-assembly"
version = "0.1.0"
private = true

[dependencies]
incan_mir_lowering = {{ loaf = "incan_mir_lowering", path = "{lowering}" }}
incan_frontend = {{ loaf = "incan_frontend", path = "{frontend}", default-features = false }}
incan_semantics_core = {{ loaf = "incan_semantics_core", path = "{semantics}" }}
incan_driver = {{ loaf = "incan_driver", path = "{driver}", default-features = false, features = ["cli", "rust_inspect", "std_async", "std_testing", "std_decorators"] }}
oven_model = {{ loaf = "oven_model", path = "{oven_model}" }}
tracing = {{ loaf = "crates-io/tracing", version = "0.1" }}
serde_json = {{ loaf = "crates-io/serde_json", version = "1" }}
thiserror = {{ loaf = "crates-io/thiserror", version = "2" }}

[[rust.bin]]
name = "composed_conversion_assembly"
path = "src/main.rs"
"#
        ),
    )?;
    bake(&caller, &fixture.home)?;
    Ok(caller.join("target/rust/debug/composed_conversion_assembly"))
}

/// Test the actual adapter policy on a genuine plan without importing unrelated rustc AST construction.
fn write_source_header_controls(caller: &Path, driver: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let source = fs::read_to_string(driver.join("src/checked_json.rs"))?;
    let controls = if let Some(start) = source.find("/// Source conversion grants") {
        let end = source[start..]
            .find("/// Declare std TryFrom<String>")
            .ok_or("source conversion validator boundary missing")?
            + start;
        format!(
            "use super::{{Plan, PlanType, TestResult}};\nuse incan_mir_lowering::caller::incan::ModelDeclaration;\nuse super::error::PlanError;\n{}\n{}",
            &source[start..end],
            include_str!("composed_conversion_header_controls.rs"),
        )
    } else {
        // Old source has no new header policy; earlier genuine Body IR assertions still execute in the red caller.
        "pub fn check(_: &super::Plan) -> super::TestResult<()> { Err(\"source string conversion validator absent\".into()) }\npub fn check_foreign(_: &super::Plan) -> super::TestResult<()> { Err(\"source string conversion validator absent\".into()) }".to_owned()
    };
    // Keep the real adapter error type; only the dependency-free source policy functions are extracted above.
    fs::write(caller.join("src/source_headers.rs"), controls)?;
    fs::copy(driver.join("src/error.rs"), caller.join("src/error.rs"))?;
    Ok(())
}

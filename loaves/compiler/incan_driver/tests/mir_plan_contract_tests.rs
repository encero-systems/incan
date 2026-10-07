//! Contract checks for the Incan-owned native plan and the unbootstrapped driver boundary.

use incan_driver::build::caller_facet::select_checked_caller_exports;
use incan_frontend::library_exports::collect_checked_public_exports;
use incan_frontend::typechecker::TypeChecker;
use incan_frontend::{lexer, parser};
use incan_provider::FeatureSelection;
use incan_test_support as support;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// Check the public plan at the same front-end boundary the project Rust-unit baker uses.
#[test]
fn scalar_plan_exports_are_public_and_representable() -> Result<(), Box<dyn std::error::Error>> {
    let path = support::repo_root().join("loaves/compiler/incan_mir_plan/src/lib.incn");
    let source = fs::read_to_string(&path)?;
    let tokens = lexer::lex(&source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let exports = collect_checked_public_exports(&program, &checker);
    let requested: BTreeSet<_> = [
        "SourceSpan",
        "PlanType",
        "TupleElement",
        "tuple_element_type",
        "Parameter",
        "Local",
        "Projection",
        "Place",
        "Constant",
        "OperandKind",
        "Operand",
        "BinaryOp",
        "UnaryOp",
        "RvalueKind",
        "Rvalue",
        "StatementKind",
        "Statement",
        "CalleeKind",
        "Callee",
        "Unwind",
        "TerminatorKind",
        "Terminator",
        "BasicBlock",
        "Function",
        "Plan",
        "ModelDeclaration",
        "EnumDeclaration",
        "EnumVariant",
        "ExternalFunction",
        "place",
        "integer",
        "copy",
        "assign",
        "block",
        "scalar_example",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let selection = select_checked_caller_exports("incan_mir_plan", &requested, &exports)?;
    assert_eq!(selection.exports.len(), requested.len());
    Ok(())
}

/// A declared driver still requires its sibling Incan plan dependency before baking.
#[test]
fn a_native_driver_unit_requires_an_incan_dependency() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let project = temporary.path();
    fs::create_dir_all(project.join("src"))?;
    fs::write(project.join("src/main.rs"), "fn main() {}\n")?;
    fs::write(
        project.join("loaf.toml"),
        r#"
[project]
name = "unseeded-driver"
[[rust.bin]]
name = "unseeded-driver"
path = "src/main.rs"
unstable_features = ["rustc_private"]
toolchain_components = ["rustc-dev"]
sysroot_dependencies = ["rustc_driver"]
"#,
    )?;
    let result = incan_driver::build::bake::bake_oven_project_targets(project, &FeatureSelection::default(), None);
    let error = match result {
        Err(error) => error.to_string(),
        Ok(_) => return Err("an unseeded driver was unexpectedly baked".into()),
    };
    assert!(error.contains("must declare an Incan"), "{error}");
    assert!(!Path::new(project).join("target").exists());
    Ok(())
}

/// One bake builds a real native caller of the complete scalar plan, including source-named enum payloads.
#[test]
fn hand_filled_scalar_plans_cross_the_oven_caller_boundary() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let library = temporary.path().join("library");
    let consumer = temporary.path().join("consumer");
    fs::create_dir_all(library.join("src"))?;
    fs::create_dir_all(consumer.join("src"))?;
    let repo = support::repo_root();
    fs::copy(
        repo.join("loaves/compiler/incan_mir_plan/loaf.toml"),
        library.join("loaf.toml"),
    )?;
    fs::copy(
        repo.join("loaves/compiler/incan_mir_plan/src/lib.incn"),
        library.join("src/lib.incn"),
    )?;
    let fixture = repo.join("loaves/compiler/incan_driver/tests/fixtures/mir_plan_caller");
    let manifest = fs::read_to_string(fixture.join("loaf.toml"))?.replace("../../../../incan_mir_plan", "../library");
    fs::write(consumer.join("loaf.toml"), manifest)?;
    fs::copy(fixture.join("src/main.rs"), consumer.join("src/main.rs"))?;
    // The fixture's own plan module names only `incan_mir_plan`; the driver's names the lowering Loaf that
    // re-exports the plan, which this consumer does not depend on.
    fs::copy(fixture.join("src/plan.rs"), consumer.join("src/plan.rs"))?;
    let baked = support::repo_command()
        .args(["oven", "bake", "--project"])
        .arg(&consumer)
        .env("INCAN_HOME", temporary.path().join("home"))
        .output()?;
    assert!(baked.status.success(), "{}", String::from_utf8_lossy(&baked.stderr));
    let run = std::process::Command::new(consumer.join("target/rust/debug/mir-plan-caller")).output()?;
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    assert_eq!(
        String::from_utf8(run.stdout)?,
        "plan: functions=2 target=1 external=native_output::caller::incan::print_int\nplan: functions=2 target=99 external=native_output::caller::incan::print_int\n"
    );
    Ok(())
}

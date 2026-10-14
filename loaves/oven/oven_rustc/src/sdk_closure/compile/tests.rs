//! Real native compilation controls for dependency ordering, parallel ownership and failure isolation.

use super::{CompileContext, Error, PreparedUnit, Schedule, SdkCompiledClosure, compile_units_with_jobs};
use crate::sdk_closure::{SdkLockedEdge, compiler_closure_digest};
use oven_store::store::{OvenStore, OvenStoreLimits};
use std::path::Path;

/// Prepare a tiny real Rust library with explicit renamed dependency edges and stable source identity.
fn fixture(root: &Path, name: &str, source: &str, dependencies: &[(&str, &str)]) -> Result<PreparedUnit, Error> {
    let project = root.join(name);
    std::fs::create_dir_all(project.join("src"))?;
    std::fs::write(project.join("src/lib.rs"), source)?;
    let manifest = format!(
        "[project]\nname='crates-io/{name}'\nversion='1.0.0'\n[rust]\nname='{name}'\ntype='lib'\nedition='2024'\n"
    );
    std::fs::write(project.join("loaf.toml"), &manifest)?;
    let mut unit = crate::sdk_closure::tests::unit(&format!("crates-io/{name}"), "target", &manifest, &[])?;
    unit.root = project;
    unit.binding.archive_digest = oven_store::digest_source_tree(&unit.root)?;
    unit.binding.edges = Some(
        dependencies
            .iter()
            .map(|(alias, name)| SdkLockedEdge {
                dependency_key: alias.to_string(),
                loaf: format!("crates-io/{name}"),
                version: "1.0.0".to_string(),
                domain: "target".to_string(),
            })
            .collect(),
    );
    Ok(unit)
}

/// Run the production executor with an explicit worker bound and an isolated canonical Store.
fn execute(root: &Path, units: &[PreparedUnit], jobs: usize) -> Result<SdkCompiledClosure, Error> {
    std::fs::create_dir_all(root)?;
    let rustc = crate::rustc::resolve_active_rustc()?;
    let target = crate::rustc::rustc_host_target(&rustc)?;
    let toolchain = crate::rustc::rustc_identity(&rustc)?;
    let store = OvenStore::new(
        root.join("store"),
        OvenStoreLimits::new(32_000_000, 32_000_000, 32_000_000),
    );
    let context = CompileContext {
        rustc: &rustc,
        target: &target,
        toolchain: &toolchain,
        output: root,
        store: &store,
        compiler_digest: compiler_closure_digest(&rustc, &target)?,
        compiler_executable: oven_store::store::digest_regular_file(&rustc.canonicalize()?)?.1,
        profile: "debug",
        unit_codegen: &[],
    };
    compile_units_with_jobs(units, &context, jobs)
}

/// Independent cold compilations preserve native identities and stable inspection edges at one or four workers.
#[test]
fn serial_and_parallel_native_compilation_preserve_identities_and_reuse() -> Result<(), Error> {
    let root = tempfile::tempdir()?;
    let units = vec![
        fixture(
            root.path(),
            "parent",
            "pub fn value() -> u8 { renamed_left::value() + renamed_right::value() }",
            &[("renamed_left", "left"), ("renamed_right", "right")],
        )?,
        fixture(root.path(), "left", "pub fn value() -> u8 { 20 }", &[])?,
        fixture(root.path(), "right", "pub fn value() -> u8 { 22 }", &[])?,
    ];
    let serial = execute(&root.path().join("serial"), &units, 1)?;
    let parallel = execute(&root.path().join("parallel"), &units, 4)?;
    serial.require_complete()?;
    parallel.require_complete()?;
    assert_eq!(serial.report.compiled, parallel.report.compiled);
    assert_eq!(parallel.report.compiled.len(), 3);
    for (before, after) in serial.units.iter().zip(&parallel.units) {
        assert_eq!(before.entry_identity(), after.entry_identity());
        assert_eq!(before.compiled_identity(), after.compiled_identity());
        assert_eq!(before.inspection["deps"], after.inspection["deps"]);
        assert_eq!(std::fs::read(&before.output)?, std::fs::read(&after.output)?);
    }
    let parent = parallel.units.last().ok_or("parent missing")?;
    assert_eq!(
        parent.inspection["deps"],
        serde_json::json!([{"crate":0,"name":"renamed_left"},{"crate":1,"name":"renamed_right"}])
    );
    let repeat = execute(&root.path().join("parallel"), &units, 4)?;
    repeat.require_complete()?;
    assert!(repeat.report.compiled.is_empty());
    assert_eq!(repeat.report.reused.len(), 3);
    assert_eq!(
        repeat.units.last().ok_or("repeat parent missing")?.entry_identity(),
        parent.entry_identity()
    );
    Ok(())
}

/// Compiler errors and missing facts block only their descendants; successful inspection indices stay compact.
#[test]
fn native_failures_and_refusals_preserve_independent_branches() -> Result<(), Error> {
    let root = tempfile::tempdir()?;
    let mut refused = fixture(root.path(), "refused", "compile_error!(\"must not compile\");", &[])?;
    refused.build_script = true;
    let units = vec![
        fixture(root.path(), "broken", "compile_error!(\"intentional failure\");", &[])?,
        refused,
        fixture(root.path(), "good", "pub fn value() -> u8 { 42 }", &[])?,
        fixture(
            root.path(),
            "bad_parent",
            "compile_error!(\"must not compile parent\");",
            &[("broken", "broken")],
        )?,
        fixture(
            root.path(),
            "refused_parent",
            "compile_error!(\"must not compile parent\");",
            &[("refused", "refused")],
        )?,
        fixture(
            root.path(),
            "good_parent",
            "pub fn value() -> u8 { renamed::value() }",
            &[("renamed", "good")],
        )?,
    ];
    let result = execute(&root.path().join("output"), &units, 4)?;
    assert_eq!(result.report.refused.len(), 1);
    assert_eq!(result.report.failed.len(), 3);
    assert_eq!(result.report.compiled.len(), 2);
    assert!(
        result
            .report
            .failed
            .iter()
            .any(|error| error.contains("intentional failure"))
    );
    assert_eq!(
        result
            .report
            .failed
            .iter()
            .filter(|error| error.contains("dependency was refused or failed"))
            .count(),
        2
    );
    assert_eq!(result.units[0].binding.loaf, "crates-io/good");
    assert_eq!(
        result.units[1].inspection["deps"],
        serde_json::json!([{"crate":0,"name":"renamed"}])
    );
    assert!(result.require_complete().is_err());
    Ok(())
}

/// A cyclic lock is rejected before source compilation or Store publication begins.
#[test]
fn native_dependency_cycles_fail_before_execution() -> Result<(), Error> {
    let root = tempfile::tempdir()?;
    let units = vec![
        fixture(root.path(), "left", "pub fn value() {}", &[("right", "right")])?,
        fixture(root.path(), "right", "pub fn value() {}", &[("left", "left")])?,
    ];
    let error = match Schedule::new(&units) {
        Ok(_) => return Err("cyclic graph was accepted".into()),
        Err(error) => error,
    };
    assert!(error.to_string().contains("cycle"));
    assert!(!root.path().join("store").exists());
    Ok(())
}

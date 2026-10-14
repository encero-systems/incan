//! Real local graph controls for shared worker execution and original-owner assembly.

use super::{Error, LocalFacetSelection, SdkCompiledClosure, compile_local_graph};
use crate::sdk_closure::SdkClosureReport;
use oven_store::store::{OvenStore, OvenStoreLimits};
use std::collections::BTreeMap;
use std::path::Path;

/// Write an authored local source and named aliases; poisoned Cargo metadata must never be consulted.
fn fixture(root: &Path, name: &str, source: &str, dependencies: &[(&str, &str)]) -> Result<LocalFacetSelection, Error> {
    let project = root.join(name);
    std::fs::create_dir_all(project.join("src"))?;
    let mut declaration = format!(
        "[project]\nname='{name}'\nversion='1.0.0'\n[rust]\nname='{name}'\ntype='lib'\nedition='2024'\n[dependencies]\n"
    );
    for (alias, dependency) in dependencies {
        declaration.push_str(&format!("{alias}={{loaf='{dependency}',path='../{dependency}'}}\n"));
    }
    std::fs::write(project.join("loaf.toml"), declaration)?;
    std::fs::write(project.join("src/lib.rs"), source)?;
    std::fs::write(project.join("Cargo.toml"), "forbidden metadata")?;
    Ok(LocalFacetSelection {
        project: name.into(),
        features: Vec::new(),
        domain: "target".to_string(),
        target: None,
        target_predicates: Vec::new(),
    })
}

/// Run the production shared coordinator with a chosen process bound and retained Store.
fn execute(
    root: &Path,
    output: &Path,
    selections: &[LocalFacetSelection],
    jobs: usize,
    closure: &mut SdkCompiledClosure,
) -> Result<(), Error> {
    let rustc = crate::rustc::resolve_active_rustc()?;
    let store = OvenStore::new(
        output.join("store"),
        OvenStoreLimits::new(32_000_000, 32_000_000, 32_000_000),
    );
    compile_local_graph(closure, selections, root, output, &rustc, "debug", &store, jobs)
}

/// Begin an empty producer closure with no preselected owner or auxiliary target.
fn empty() -> SdkCompiledClosure {
    SdkCompiledClosure {
        report: SdkClosureReport::default(),
        units: Vec::new(),
        auxiliary_targets: BTreeMap::new(),
    }
}

/// Parallel completion must preserve serial receipts, bytes, aliases and reuse without duplicating owners.
#[test]
fn serial_and_parallel_local_graphs_preserve_native_owners() -> Result<(), Error> {
    let root = tempfile::tempdir()?;
    let selections = vec![
        fixture(
            root.path(),
            "parent",
            "pub fn value() -> u8 { renamed_left::value() + renamed_right::value() }",
            &[("renamed_left", "left"), ("renamed_right", "right")],
        )?,
        fixture(root.path(), "left", "pub fn value() -> u8 { 20 }", &[])?,
        fixture(root.path(), "right", "pub fn value() -> u8 { 22 }", &[])?,
    ];
    let mut serial = empty();
    let mut parallel = empty();
    execute(root.path(), &root.path().join("serial"), &selections, 1, &mut serial)?;
    execute(
        root.path(),
        &root.path().join("parallel"),
        &selections,
        4,
        &mut parallel,
    )?;
    serial.require_complete()?;
    parallel.require_complete()?;
    assert_eq!(serial.report.compiled, parallel.report.compiled);
    assert_eq!(parallel.units.len(), 3);
    for (left, right) in serial.units.iter().zip(&parallel.units) {
        assert_eq!(left.entry_identity(), right.entry_identity());
        assert_eq!(left.compiled_identity(), right.compiled_identity());
        assert_eq!(left.inspection["deps"], right.inspection["deps"]);
        assert_eq!(std::fs::read(left.output())?, std::fs::read(right.output())?);
    }
    let mut repeat = empty();
    execute(root.path(), &root.path().join("parallel"), &selections, 4, &mut repeat)?;
    repeat.require_complete()?;
    assert!(repeat.report.compiled.is_empty());
    assert_eq!(repeat.report.reused.len(), 3);
    let identities = parallel
        .units
        .iter()
        .map(|u| u.entry_identity().to_string())
        .collect::<Vec<_>>();
    parallel.report = SdkClosureReport::default();
    execute(
        root.path(),
        &root.path().join("parallel"),
        &selections,
        4,
        &mut parallel,
    )?;
    parallel.require_complete()?;
    assert_eq!(parallel.report.reused.len(), 3);
    assert!(parallel.report.compiled.is_empty());
    assert_eq!(
        parallel
            .units
            .iter()
            .map(|u| u.entry_identity().to_string())
            .collect::<Vec<_>>(),
        identities
    );
    std::fs::write(root.path().join("left/src/lib.rs"), "pub fn value() -> u8 { 21 }")?;
    parallel.report = SdkClosureReport::default();
    execute(
        root.path(),
        &root.path().join("parallel"),
        &selections,
        4,
        &mut parallel,
    )?;
    assert_eq!(parallel.report.failed.len(), 2);
    assert!(
        parallel
            .report
            .failed
            .iter()
            .any(|e| e.contains("conflicts with an already selected binding"))
    );
    assert_eq!(
        parallel
            .units
            .iter()
            .map(|u| u.entry_identity().to_string())
            .collect::<Vec<_>>(),
        identities
    );
    Ok(())
}

/// Failed local producers block their consumers while independent branches retain compact inspection indices.
#[test]
fn local_failures_preserve_independent_parallel_branches() -> Result<(), Error> {
    let root = tempfile::tempdir()?;
    let selections = vec![
        fixture(
            root.path(),
            "broken",
            "compile_error!(\"intentional local failure\");",
            &[],
        )?,
        fixture(
            root.path(),
            "blocked",
            "compile_error!(\"consumer must not compile\");",
            &[("broken", "broken")],
        )?,
        fixture(root.path(), "good", "pub fn value() -> u8 { 42 }", &[])?,
        fixture(
            root.path(),
            "parent",
            "pub fn value() -> u8 { alias::value() }",
            &[("alias", "good")],
        )?,
    ];
    let mut closure = empty();
    execute(root.path(), &root.path().join("output"), &selections, 4, &mut closure)?;
    assert_eq!(closure.report.failed.len(), 2);
    assert_eq!(closure.report.compiled.len(), 2);
    assert!(
        closure
            .report
            .failed
            .iter()
            .any(|e| e.contains("intentional local failure"))
    );
    assert!(
        closure
            .report
            .failed
            .iter()
            .any(|e| e.contains("local dependency was refused or failed"))
    );
    assert_eq!(
        closure.units[1].inspection["deps"],
        serde_json::json!([{"crate":0,"name":"alias"}])
    );
    Ok(())
}

/// Cyclic local graphs are refused before any compilation or publication can begin.
#[test]
fn local_cycles_refuse_before_worker_execution() -> Result<(), Error> {
    let root = tempfile::tempdir()?;
    let selections = vec![
        fixture(root.path(), "left", "", &[("right", "right")])?,
        fixture(root.path(), "right", "", &[("left", "left")])?,
    ];
    let output = root.path().join("output");
    let error = execute(root.path(), &output, &selections, 4, &mut empty())
        .err()
        .ok_or("cycle accepted")?;
    assert!(error.to_string().contains("cycle"));
    assert!(!output.exists());
    Ok(())
}

/// Host macro dependencies and target libraries with the same source name retain their exact domains.
#[test]
fn local_parallel_macro_edges_preserve_host_and_target_domains() -> Result<(), Error> {
    let root = tempfile::tempdir()?;
    let helper = fixture(root.path(), "helper", "pub fn value() -> u8 { 42 }", &[])?;
    let mut host_helper = fixture(root.path(), "helper", "pub fn value() -> u8 { 42 }", &[])?;
    host_helper.domain = "host".to_string();
    let mut macro_unit = fixture(
        root.path(),
        "local_macro",
        r#"
extern crate proc_macro;
#[proc_macro]
pub fn answer(_: proc_macro::TokenStream) -> proc_macro::TokenStream {
    proc_macro::TokenStream::from(proc_macro::TokenTree::Literal(proc_macro::Literal::u8_unsuffixed(helper::value())))
}
"#,
        &[("helper", "helper")],
    )?;
    macro_unit.domain = "host".to_string();
    let manifest = root.path().join("local_macro/loaf.toml");
    std::fs::write(
        &manifest,
        std::fs::read_to_string(&manifest)?.replace("type='lib'", "type='proc-macro'"),
    )?;
    let consumer = fixture(
        root.path(),
        "consumer",
        "pub fn value() -> u8 { local_macro::answer!() + helper::value() }",
        &[("helper", "helper"), ("local_macro", "local_macro")],
    )?;
    let selections = vec![consumer, macro_unit, helper, host_helper];
    let mut closure = empty();
    execute(root.path(), &root.path().join("output"), &selections, 4, &mut closure)?;
    closure.require_complete()?;
    assert_eq!(closure.units.len(), 4);
    let macro_unit = closure
        .units
        .iter()
        .find(|unit| unit.binding.loaf == "local_macro")
        .ok_or("macro missing")?;
    assert_eq!(macro_unit.physical_edges[0].destination().binding.domain, "host");
    let consumer = closure
        .units
        .iter()
        .find(|unit| unit.binding.loaf == "consumer")
        .ok_or("consumer missing")?;
    for edge in &consumer.physical_edges {
        assert_eq!(
            edge.destination().binding.domain,
            if edge.alias() == "local_macro" {
                "host"
            } else {
                "target"
            }
        );
    }
    Ok(())
}

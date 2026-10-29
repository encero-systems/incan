//! Experimental executable entry into the existing adapter; all plan semantics live in the Incan producer.
//!
//! This is temporary packaging glue for #1337's binary-only rustc-private boundary. Delete it when an Incan
//! caller can import the adapter through the compiler authoring API. It performs no source checking or lowering.

use std::path::{Path, PathBuf};

use native_plan_spike::Scenario;

/// Construct a plan in Incan, verify the selected native toolchain, and submit it unchanged to the normal adapter.
/// The manifest supplies the same explicit runtime closure used by ordinary compilation, never an ambient search.
pub fn run(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    // ---- Invocation and Incan-owned scenario ----
    let [mode, value, source, output, manifest] = arguments else {
        return Err("usage: --spike-plan MODE VALUE SOURCE OUTPUT TOOLCHAIN_JSON".into());
    };
    let scenario = match mode.as_str() {
        "constant" => Scenario::Constant,
        "invalid" => Scenario::InvalidReturn,
        "arithmetic" => Scenario::Arithmetic,
        "branch" => Scenario::Branch,
        "record" => Scenario::Record,
        _ => return Err("unknown plan spike mode".into()),
    };

    // ---- Verified toolchain and explicit dependency closure ----
    let manifest = Path::new(manifest);
    let root = manifest.parent().ok_or("toolchain manifest has no parent")?;
    let selected: serde_json::Value = serde_json::from_slice(&std::fs::read(manifest)?)?;
    let sysroot = selected["sysroot"].as_str().ok_or("missing toolchain sysroot")?;
    let identity = crate::identity::verify(&root.join(sysroot))?;
    let mut externs = Vec::new();
    for item in selected["externs"].as_array().ok_or("missing toolchain externs")? {
        let name = item["name"].as_str().ok_or("missing extern name")?;
        let path = item["path"].as_str().ok_or("missing extern path")?;
        externs.push((name.to_owned(), root.join(path)));
    }
    let search: Vec<PathBuf> = selected["search"]
        .as_array()
        .ok_or("missing toolchain search paths")?
        .iter()
        .map(|value| value.as_str().map(|path| root.join(path)).ok_or("invalid search path"))
        .collect::<Result<_, _>>()?;

    // ---- Incan construction and unchanged native validation ----
    let plan = native_plan_spike::build_plan(scenario, value.parse()?, source.clone());
    crate::adapter::compile(
        plan,
        "injected_program",
        Path::new(output),
        &identity,
        &externs,
        &search,
        &[],
        None,
        incan_driver::unit::UnitKind::Program,
        true,
        None,
    )?;
    Ok(())
}

//! Native-only preparation for an explicit compiler test dependency selection.

use std::path::Path;

use oven_model::manifest::DependencySpec;
use oven_rustc::native_loaf::{
    NativeLoafClosure, NativeLoafConsumerPreparation, NativeLoafConsumerRequest, prepare_declared_native_loafs,
};

/// Prepare exact native records in a shared worktree-owned cache without publishing checked library components.
pub(super) fn prepare(
    compiler_root: &Path,
    graph: &Path,
    index: &Path,
    blobs: &Path,
    rustc: &Path,
    dependencies: &[DependencySpec],
    declaration_owner: &Path,
) -> Result<NativeLoafConsumerPreparation, Box<dyn std::error::Error>> {
    let output = compiler_root.join("target/compiler-development/native-loafs");
    let target = oven_rustc::rustc::rustc_host_target(rustc)?;
    Ok(prepare_declared_native_loafs(&NativeLoafConsumerRequest {
        graph,
        index,
        blobs,
        output: &output,
        rustc,
        target: &target,
        profile: "debug",
        dependencies,
        declaration_owner,
        domain: "target",
    })?)
}

/// Retain actual preparation work and selected physical closure counts beside the native test report.
pub(super) fn write_report(
    output: &Path,
    preparation: Option<&NativeLoafConsumerPreparation>,
    closure: &NativeLoafClosure,
) -> Result<(), Box<dyn std::error::Error>> {
    let report = serde_json::json!({
        "schema": "incan.compiler-test-native-loaf-preparation/1",
        "native": preparation.map(NativeLoafConsumerPreparation::report),
        "prepared_native_units": preparation.map_or(0, |prepared| prepared.report().prepared_units),
        "selected_native_units": closure.graph().units().len(),
        "roots": closure.roots(),
        "checked_library_publications": 0,
    });
    std::fs::write(
        output.join("native-loaf-preparation.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(())
}

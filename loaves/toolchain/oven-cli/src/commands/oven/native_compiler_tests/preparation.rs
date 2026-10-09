//! Native-only preparation for an explicit compiler test dependency selection.

use std::path::Path;

use oven_rustc::native_loaf::{NativeLoafClosure, NativeLoafPreparation, prepare_resolved_native_loafs};

/// Prepare exact native records in a shared worktree-owned cache without publishing checked library components.
pub(super) fn prepare(
    compiler_root: &Path,
    graph: &Path,
    index: &Path,
    blobs: &Path,
    rustc: &Path,
) -> Result<NativeLoafPreparation, Box<dyn std::error::Error>> {
    let output = compiler_root.join("target/compiler-development/native-loafs");
    let target = oven_rustc::rustc::rustc_host_target(rustc)?;
    Ok(prepare_resolved_native_loafs(
        graph, index, blobs, &output, rustc, &target, "debug",
    )?)
}

/// Retain actual preparation work and selected physical closure counts beside the native test report.
pub(super) fn write_report(
    output: &Path,
    preparation: Option<&NativeLoafPreparation>,
    closure: &NativeLoafClosure,
) -> Result<(), Box<dyn std::error::Error>> {
    let report = serde_json::json!({
        "schema": "incan.compiler-test-native-loaf-preparation/1",
        "native": preparation.map(NativeLoafPreparation::report),
        "prepared_native_units": preparation.map_or(0, |prepared| prepared.graph().units().len()),
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

//! Observe materialized package execution and packaging refusal on the real compiler routes.

use std::error::Error;
use std::fs;

use incan::library_manifest::{LibraryManifest, digest_provider_artifact};

use crate::package_project::{bake, command, project, success};

/// Concrete process observations for the two stable package-boundary corpus rows.
#[derive(Debug)]
pub(crate) struct PackageBoundaryObservation {
    /// Output from the fresh native consumer after provider source removal.
    pub(crate) native_stdout: Vec<u8>,
    /// Output from non-linking execution of the same consumer source.
    pub(crate) replacement_stdout: Vec<u8>,
    /// Whether the missing-representation request incorrectly succeeded.
    pub(crate) refusal_success: bool,
    /// Captured output that must remain empty when package admission refuses.
    pub(crate) refusal_stdout: Vec<u8>,
    /// Diagnostic naming the package, its version and the missing representation.
    pub(crate) refusal_stderr: String,
    /// Whether the refused fresh consumer incorrectly obtained a completed receipt.
    pub(crate) refusal_receipt_exists: bool,
}

/// Publish one provider, remove its sources, and observe both execution routes before testing a missing descriptor.
///
/// The final descriptor omission is a negative replacement-input fixture. It makes no claim that the modified
/// package still satisfies native sealed-artifact integrity.
pub(crate) fn observe_package_boundary(source: &str) -> Result<PackageBoundaryObservation, Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let provider = temporary.path().join("provider");
    let consumer = temporary.path().join("consumer");
    project(
        &provider,
        "widgets",
        "lib.incn",
        "pub def build() -> int:\n    return 42\n",
        "",
    )?;
    bake(&provider)?;
    fs::remove_dir_all(provider.join("src"))?;
    fs::remove_file(provider.join("loaf.toml"))?;
    let artifact = provider.join("target/lib");
    let before = digest_provider_artifact(&artifact)?;
    let dependencies = "\n[dependencies]\nwidgets = { path = \"../provider\" }\n";
    project(&consumer, "consumer", "main.incn", source, dependencies)?;
    bake(&consumer)?;
    let native = success(
        command(&consumer).args(["run", "--locked", "src/main.incn"]).output()?,
        "native package corpus execution",
    )?;
    let report = consumer.join("replacement.json");
    let replacement = success(
        command(&consumer)
            .args([
                "build",
                "src/main.incn",
                "--backend",
                "replacement",
                "--report",
                "json",
                "--report-output",
            ])
            .arg(&report)
            .output()?,
        "non-linking package corpus execution",
    )?;
    let report: serde_json::Value = serde_json::from_slice(&fs::read(report)?)?;
    if !report["replacement_execution"]["package_declarations_decoded"]
        .as_u64()
        .is_some_and(|count| count > 0)
        || !consumer.join(".incan/backend/receipt.json").is_file()
    {
        return Err("package execution did not retain decoded declaration and completed receipt evidence".into());
    }
    if digest_provider_artifact(&artifact)? != before
        || provider.join("src").exists()
        || provider.join("loaf.toml").exists()
    {
        return Err("package execution changed the published artifact or recreated dependency source".into());
    }
    let manifest_path = artifact.join("widgets.incnlib");
    let mut manifest = LibraryManifest::read_from_path(&manifest_path)?;
    manifest.contract_metadata.executable_representation = None;
    manifest.write_to_path(&manifest_path)?;
    let missing = temporary.path().join("missing");
    project(
        &missing,
        "missing",
        "main.incn",
        "from pub::widgets import build\n\ndef main() -> None:\n    println(\"must not print\")\n    if False:\n        println(build())\n",
        dependencies,
    )?;
    let refused = command(&missing)
        .args(["build", "src/main.incn", "--backend", "replacement"])
        .output()?;
    Ok(PackageBoundaryObservation {
        native_stdout: native.stdout,
        replacement_stdout: replacement.stdout,
        refusal_success: refused.status.success(),
        refusal_stdout: refused.stdout,
        refusal_stderr: String::from_utf8(refused.stderr)?,
        refusal_receipt_exists: missing.join(".incan/backend/receipt.json").exists(),
    })
}

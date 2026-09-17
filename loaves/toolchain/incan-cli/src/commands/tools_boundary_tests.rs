//! The one `tools` test that needs `incan build --lib`: it publishes a library through this crate's build command
//! and then asks `oven_cli`'s registry inspection what a consumer of that library may see. The handler lives in
//! `oven-cli`; the build command lives here, so the test that needs both lives here.

use std::fs;
use std::path::Path;

use crate::commands::build::build_library;
use incan_driver::build::BuildCommandOptions;
use incan_driver::build_report::BuildReportOptions;
use oven_cli::ExitCode;
use oven_cli::commands::tools::{RegistryInspectionFormat, inspect_registry};
use oven_model::lock::{CargoFeatureSelection, IncanLock, compute_deps_fingerprint};

/// A consumer of a compiled library sees the library's public registries and none of its private ones: the
/// producer publishes through `incan build --lib`, the consumer's inspection reads the sealed metadata.
#[test]
fn inspect_registry_reads_only_public_facts_from_a_library_consumer() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let producer_root = tmp.path().join("registrylib");
    let producer_src = producer_root.join("src");
    fs::create_dir_all(&producer_src)?;
    fs::write(
        producer_root.join("loaf.toml"),
        "[project]\nname = \"registrylib\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        producer_src.join("lib.incn"),
        r#"
from std.registry import Registry, SubjectKind, describe

@derive(Clone, Eq)
pub type FunctionId = newtype str

@derive(Descriptor)
pub model FunctionSpec:
    pub summary: str

pub static public_functions: Registry[FunctionId, FunctionSpec] = Registry.define(
    subjects=[SubjectKind.Function],
)
static private_functions: Registry[FunctionId, FunctionSpec] = Registry.define(
    subjects=[SubjectKind.Function],
)

@describe(public_functions, FunctionId("public"), FunctionSpec(summary="public"))
pub def public_function() -> None:
    pass

@describe(private_functions, FunctionId("private"), FunctionSpec(summary="private"))
def private_function() -> None:
    pass
"#,
    )?;
    write_test_incan_lock(&producer_root)?;
    let producer_entry = producer_src.join("lib.incn");
    let exit = build_library(
        producer_entry.to_str(),
        None,
        BuildCommandOptions::default(),
        BuildReportOptions::default(),
    )?;
    assert_eq!(exit, ExitCode::SUCCESS);

    let consumer_root = tmp.path().join("consumer");
    let consumer_src = consumer_root.join("src");
    fs::create_dir_all(&consumer_src)?;
    fs::write(
        consumer_root.join("loaf.toml"),
        "[project]\nname = \"consumer\"\nversion = \"0.1.0\"\n\n[dependencies]\nregistrylib = { path = \"../registrylib\" }\n",
    )?;
    fs::write(
        consumer_src.join("main.incn"),
        "from pub::registrylib import FunctionId\n\ndef main() -> None:\n    assert FunctionId(\"consumer\") == FunctionId(\"consumer\")\n",
    )?;

    assert_eq!(
        inspect_registry(
            "lib::public_functions",
            Some(&consumer_root),
            RegistryInspectionFormat::Json,
        )?,
        ExitCode::SUCCESS
    );
    let private_error = match inspect_registry(
        "lib::private_functions",
        Some(&consumer_root),
        RegistryInspectionFormat::Json,
    ) {
        Ok(code) => {
            return Err(format!("consumer inspection unexpectedly exposed a private registry: {code:?}").into());
        }
        Err(error) => error,
    };
    assert!(
        private_error.to_string().contains("was not found"),
        "expected private registry to be absent from consumer inspection, got: {private_error}"
    );
    Ok(())
}

/// Write the minimal `oven.lock` a library build expects beside the fixture's manifest.
fn write_test_incan_lock(project_root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let cargo_lock_payload = fs::read_to_string(oven_model::toolchain_layout::development_root().join("Cargo.lock"))?;
    let features = CargoFeatureSelection::default();
    let fingerprint = compute_deps_fingerprint(&[], &[], &features, Some(project_root));
    IncanLock::new(
        incan_lang::version::INCAN_VERSION,
        fingerprint,
        features,
        cargo_lock_payload,
    )
    .write(&project_root.join("oven.lock"))?;
    Ok(())
}

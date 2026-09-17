//! The frozen fixture project the store tests publish, shared with the crates above: on under `cfg(test)` and the
//! `test_support` feature, which the dependants' dev-dependencies turn on.

use std::fs;
use std::path::Path;

use crate::store::{OvenArtifactKind, OvenArtifactPublishRequest};
use crate::{OvenImportRequest, import_frozen_project};

/// A publish request for the frozen fixture project, shared with sibling store modules' tests.
pub fn request(
    project: &Path,
    domain: &str,
    payload: &[u8],
) -> Result<OvenArtifactPublishRequest, Box<dyn std::error::Error>> {
    let receipt = import_frozen_project(&OvenImportRequest::new(
        project,
        "aarch64-apple-darwin",
        "rustc 1.96.0",
        "release",
        Vec::new(),
    ))?;
    Ok(OvenArtifactPublishRequest {
        receipt,
        domain: domain.to_string(),
        kind: OvenArtifactKind::Engine,
        payload: payload.to_vec(),
        materialized_files: Vec::new(),
        materialized_directories: Vec::new(),
    })
}

/// Write the frozen fixture project a receipt can be computed from, shared with sibling store modules' tests.
pub fn write_project(root: &Path) -> Result<(), std::io::Error> {
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"store_fixture\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(root.join("Cargo.lock"), "version = 4\n")?;
    Ok(())
}

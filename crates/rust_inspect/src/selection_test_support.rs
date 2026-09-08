//! Small explicit projects for physical-boundary tests; no Cargo workspace is synthesized.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

use crate::selection::{InspectionSourceInput, SelectedInspectionInputs, ValidatedInspectionProject};
use crate::{RustMetadataError, RustWorkspace};

/// Own source and output separately so inspection cannot write into its selected inputs.
pub(crate) struct InspectionFixture {
    pub source: tempfile::TempDir,
    pub output: tempfile::TempDir,
    pub inputs: SelectedInspectionInputs,
}

/// Bind exact bytes using the production physical digest format.
pub(crate) fn byte_digest(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

impl InspectionFixture {
    /// Prepare a selected no-std crate whose metadata does not need any ambient sysroot.
    pub(crate) fn new(source: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let source_dir = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let root = source_dir.path().canonicalize()?;
        let module = root.join("lib.rs");
        fs::write(&module, source)?;
        fs::write(root.join("Cargo.toml"), "invalid cargo manifest: do not read\n")?;
        let project_json = serde_json::to_vec(&serde_json::json!({ "crates": [{
            "display_name": "demo", "root_module": module, "edition": "2021", "deps": [],
            "cfg": [], "env": {}, "is_workspace_member": false,
            "source": { "include_dirs": [root], "exclude_dirs": [] }
        }] }))?;
        let target_spec_json = br#"{"data-layout":"e-m:o-i64:64-i128:128-n32:64-S128","arch":"aarch64"}"#.to_vec();
        let inputs = SelectedInspectionInputs {
            project_digest: byte_digest(&project_json),
            project_json,
            sources: vec![InspectionSourceInput {
                root: root.clone(),
                digest: crate::loader::digest_oven_source_tree(&root)?,
            }],
            target_spec_digest: byte_digest(&target_spec_json),
            target_spec_json,
            toolchain_version: "1.98.0".to_string(),
            query_roots: BTreeMap::from([("demo".to_string(), module)]),
        };
        Ok(Self {
            source: source_dir,
            output,
            inputs,
        })
    }

    /// Update an intentionally edited test projection's expected binding, without changing source authority.
    pub(crate) fn set_project(&mut self, value: serde_json::Value) -> Result<(), Box<dyn std::error::Error>> {
        self.inputs.project_json = serde_json::to_vec(&value)?;
        self.inputs.project_digest = byte_digest(&self.inputs.project_json);
        Ok(())
    }

    /// Load only this fixture's selected projection and explicit output root.
    pub(crate) fn load(&self) -> Result<RustWorkspace, RustMetadataError> {
        let validated = ValidatedInspectionProject::validate(self.inputs.clone())?;
        RustWorkspace::load_selected(&validated, self.output.path(), &|_| {})
    }

    /// Return the selected root module's canonical path.
    pub(crate) fn module(&self) -> Result<PathBuf, Box<dyn std::error::Error>> {
        Ok(self.source.path().canonicalize()?.join("lib.rs"))
    }
}

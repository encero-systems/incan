//! Physical validation of an already selected inspection projection.
//!
//! This is the compiler's rust-analyzer adapter, not the Oven host ABI or a dependency resolver. The caller supplies
//! the projection and expected content bindings from admission, and retains the corresponding source/artifact leases.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use ra_ap_ide_db::base_db::target::TargetData;
use ra_ap_project_model::ProjectJsonData;
use ra_ap_project_model::toolchain_info::target_data::TargetSpec;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::RustMetadataError;
use crate::loader::digest_oven_source_tree;

/// One immutable physical tree selected by the caller's admission boundary.
#[derive(Debug, Clone)]
pub struct InspectionSourceInput {
    pub root: PathBuf,
    /// Exact source-tree digest from the selected artifact, using Oven's portable source-tree encoding.
    pub digest: String,
}

/// Inputs for the bounded physical inspection operation.
///
/// Expected digests must come from the admitted selection, not from untrusted candidate payloads. Every sysroot,
/// generated source and dependency unit must already be in `project_json`. Query aliases identify explicit root
/// modules; no package-name lookup or registry discovery is performed. Distinct units sharing a root module are
/// currently refused because this ingress does not yet carry an exact rust-analyzer unit handle (#991, #1037).
#[derive(Debug, Clone)]
pub struct SelectedInspectionInputs {
    pub project_json: Vec<u8>,
    pub project_digest: String,
    pub sources: Vec<InspectionSourceInput>,
    /// Selected rustc target-spec JSON, including `data-layout` and `arch`.
    pub target_spec_json: Vec<u8>,
    pub target_spec_digest: String,
    pub toolchain_version: String,
    pub query_roots: BTreeMap<String, PathBuf>,
}

/// A structurally valid projection whose declared physical inputs matched their expected bytes.
///
/// Validation does not establish provider authority or acquire leases. The caller must keep the admitted inputs
/// immutable for the loaded database's lifetime. No deserialization or boolean bypass can construct this value.
#[derive(Debug, Clone)]
pub struct ValidatedInspectionProject {
    inputs: SelectedInspectionInputs,
    fingerprint: String,
}

/// Return a hard physical-input diagnostic associated with one declared path.
fn invalid(path: &Path, message: impl Into<String>) -> RustMetadataError {
    RustMetadataError::InvalidSelectedInput {
        path: path.to_path_buf(),
        message: message.into(),
    }
}

/// Require a SHA-256 content binding for one supplied byte sequence.
fn verify_digest(bytes: &[u8], expected: &str, label: &str) -> Result<(), RustMetadataError> {
    let actual = format!("sha256:{}", hex::encode(Sha256::digest(bytes)));
    if actual != expected {
        return Err(invalid(
            Path::new(label),
            format!("expected {expected}, found {actual}"),
        ));
    }
    Ok(())
}

/// Require an absolute canonical path under a declared input root, without adding implicit roots.
fn selected_path(path: &Path, roots: &[InspectionSourceInput]) -> Result<PathBuf, RustMetadataError> {
    if !path.is_absolute() {
        return Err(invalid(path, "selected paths must be absolute"));
    }
    let canonical = path.canonicalize()?;
    if canonical != path || !roots.iter().any(|root| canonical.starts_with(&root.root)) {
        return Err(invalid(
            path,
            "path is not canonical or is outside the selected input roots",
        ));
    }
    Ok(canonical)
}

/// Read one required absolute path field from a selected JSON object.
fn path_field(value: &Value, field: &str, roots: &[InspectionSourceInput]) -> Result<PathBuf, RustMetadataError> {
    let path = value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(Path::new(field), "required selected path is absent"))?;
    selected_path(Path::new(path), roots)
}

/// Reject cyclic supplied indices before rust-analyzer can silently discard one of their edges.
///
/// This checks the declared projection only; it never chooses or repairs dependency edges.
fn validate_edge_cycles(crates: &[Value]) -> Result<(), RustMetadataError> {
    let mut incoming = vec![0usize; crates.len()];
    let mut dependents = vec![Vec::new(); crates.len()];
    for (index, unit) in crates.iter().enumerate() {
        let dependencies = unit["deps"]
            .as_array()
            .ok_or_else(|| invalid(Path::new("rust-project.json"), "dependency array absent"))?;
        incoming[index] = dependencies.len();
        for dependency in dependencies {
            let selected = dependency["crate"]
                .as_u64()
                .and_then(|index| usize::try_from(index).ok())
                .filter(|index| *index < crates.len())
                .ok_or_else(|| invalid(Path::new("rust-project.json"), "invalid dependency index"))?;
            dependents[selected].push(index);
        }
    }
    let mut ready = incoming
        .iter()
        .enumerate()
        .filter_map(|(index, count)| (*count == 0).then_some(index))
        .collect::<Vec<_>>();
    let mut visited = 0usize;
    while let Some(index) = ready.pop() {
        visited += 1;
        for dependent in &dependents[index] {
            incoming[*dependent] -= 1;
            if incoming[*dependent] == 0 {
                ready.push(*dependent);
            }
        }
    }
    if visited != crates.len() {
        return Err(invalid(
            Path::new("rust-project.json"),
            "selected projection contains a dependency cycle",
        ));
    }
    Ok(())
}

impl ValidatedInspectionProject {
    /// Check supplied bytes and the physical projection contract before any database or process is created.
    ///
    /// This verifies declarations already selected by the caller; it never repairs a missing edge, source or target.
    pub fn validate(mut inputs: SelectedInspectionInputs) -> Result<Self, RustMetadataError> {
        verify_digest(&inputs.project_json, &inputs.project_digest, "rust-project.json")?;
        verify_digest(&inputs.target_spec_json, &inputs.target_spec_digest, "target-spec.json")?;
        let _: ProjectJsonData = serde_json::from_slice(&inputs.project_json)
            .map_err(|error| invalid(Path::new("rust-project.json"), error.to_string()))?;
        let _: TargetSpec = serde_json::from_slice(&inputs.target_spec_json)
            .map_err(|error| invalid(Path::new("target-spec.json"), error.to_string()))?;
        semver::Version::parse(&inputs.toolchain_version)
            .map_err(|error| invalid(Path::new("toolchain-version"), error.to_string()))?;
        if inputs.sources.is_empty() || inputs.query_roots.is_empty() {
            return Err(RustMetadataError::SelectedInputUnavailable {
                path: PathBuf::from("rust-project.json"),
            });
        }
        inputs.sources.sort_by(|a, b| a.root.cmp(&b.root));
        for (index, input) in inputs.sources.iter().enumerate() {
            if !input.root.is_absolute() || input.root.canonicalize()? != input.root || !input.root.is_dir() {
                return Err(invalid(
                    &input.root,
                    "selected source root must be a canonical absolute directory",
                ));
            }
            if inputs.sources[..index]
                .iter()
                .any(|other| input.root.starts_with(&other.root))
            {
                return Err(invalid(&input.root, "duplicate or overlapping selected source roots"));
            }
        }
        let document: Value = serde_json::from_slice(&inputs.project_json)
            .map_err(|error| invalid(Path::new("rust-project.json"), error.to_string()))?;
        let object = document
            .as_object()
            .ok_or_else(|| invalid(Path::new("rust-project.json"), "expected an object"))?;
        // Sysroot crates are ordinary explicit units here. Accepting the convenience sysroot fields would reopen
        // rust-analyzer's discovery path; runnables and proc-macro execution need separate granted host operations.
        for (key, value) in object {
            if key != "crates" && !value.is_null() {
                return Err(invalid(
                    Path::new("rust-project.json"),
                    format!("unsupported selected projection field `{key}`"),
                ));
            }
        }
        let crates = document["crates"]
            .as_array()
            .ok_or_else(|| invalid(Path::new("rust-project.json"), "selected units are absent"))?;
        if crates.is_empty() {
            return Err(RustMetadataError::SelectedInputUnavailable {
                path: PathBuf::from("rust-project.json"),
            });
        }
        let mut roots = BTreeSet::new();
        for unit in crates {
            let root = path_field(unit, "root_module", &inputs.sources)?;
            if !root.is_file() {
                return Err(invalid(&root, "root module is not a regular file"));
            }
            if !roots.insert(root.clone()) {
                return Err(RustMetadataError::UnsupportedSelectedOperation {
                    operation: "multiple inspection units sharing a source root module (#991, #1037)",
                });
            }
            if unit.get("is_proc_macro").and_then(Value::as_bool).unwrap_or(false)
                || unit.get("proc_macro_dylib_path").is_some_and(|value| !value.is_null())
            {
                return Err(RustMetadataError::UnsupportedSelectedOperation {
                    operation: "selected proc-macro execution (#991, #1037)",
                });
            }
            if unit.get("target").is_some_and(|value| !value.is_null()) {
                return Err(RustMetadataError::UnsupportedSelectedOperation {
                    operation: "per-unit target discovery; supply resolved cfg and target facts (#991, #1037)",
                });
            }
            if unit.get("is_workspace_member").and_then(Value::as_bool) != Some(false) {
                return Err(invalid(
                    &root,
                    "is_workspace_member must be false; cfg must come only from selected inputs",
                ));
            }
            if unit.get("cfg").and_then(Value::as_array).is_none() {
                return Err(invalid(&root, "explicit resolved cfg is required"));
            }
            let unit_object = unit
                .as_object()
                .ok_or_else(|| invalid(&root, "unit must be an object"))?;
            for (key, value) in unit_object {
                if !matches!(
                    key.as_str(),
                    "display_name"
                        | "root_module"
                        | "edition"
                        | "version"
                        | "deps"
                        | "cfg"
                        | "target"
                        | "env"
                        | "is_workspace_member"
                        | "source"
                        | "is_proc_macro"
                        | "proc_macro_dylib_path"
                ) && !value.is_null()
                {
                    return Err(invalid(&root, format!("unsupported selected unit field `{key}`")));
                }
            }
            let source = unit
                .get("source")
                .ok_or_else(|| invalid(&root, "explicit source include/exclude directories are required"))?;
            for field in ["include_dirs", "exclude_dirs"] {
                let paths = source
                    .get(field)
                    .and_then(Value::as_array)
                    .ok_or_else(|| invalid(&root, format!("source.{field} is required")))?;
                if field == "include_dirs" && paths.is_empty() {
                    return Err(invalid(&root, "selected source include directories are empty"));
                }
                for path in paths {
                    let path = path
                        .as_str()
                        .ok_or_else(|| invalid(&root, "source directory must be a string"))?;
                    if !selected_path(Path::new(path), &inputs.sources)?.is_dir() {
                        return Err(invalid(
                            Path::new(path),
                            "selected source include/exclude path is not a directory",
                        ));
                    }
                }
            }
            let deps = unit
                .get("deps")
                .and_then(Value::as_array)
                .ok_or_else(|| invalid(&root, "explicit dependency edges are required"))?;
            let mut aliases = BTreeSet::new();
            for dependency in deps {
                let index = dependency
                    .get("crate")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| invalid(&root, "dependency unit index is absent"))?;
                if usize::try_from(index).ok().is_none_or(|index| index >= crates.len()) {
                    return Err(invalid(&root, "dependency references an absent selected unit"));
                }
                let alias = dependency
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid(&root, "dependency alias is absent"))?;
                if !aliases.insert(alias) {
                    return Err(invalid(&root, "duplicate dependency alias"));
                }
            }
            if let Some(out_dir) = unit.get("env").and_then(|env| env.get("OUT_DIR")) {
                let path = out_dir
                    .as_str()
                    .ok_or_else(|| invalid(&root, "OUT_DIR must be a string"))?;
                if !selected_path(Path::new(path), &inputs.sources)?.is_dir() {
                    return Err(invalid(Path::new(path), "selected OUT_DIR is not a directory"));
                }
            }
        }
        validate_edge_cycles(crates)?;
        for (alias, root) in &inputs.query_roots {
            if alias.is_empty() || alias.contains("::") || !roots.contains(root) {
                return Err(invalid(
                    root,
                    format!("query binding `{alias}` does not identify one selected root module"),
                ));
            }
        }
        let mut hash = Sha256::new();
        // Length-delimited serialization prevents concatenation ambiguity and retains exact exposure/target bindings.
        let binding = (
            &inputs.project_digest,
            &inputs.target_spec_digest,
            &inputs.toolchain_version,
            &inputs.query_roots,
            inputs
                .sources
                .iter()
                .map(|input| (&input.root, &input.digest))
                .collect::<Vec<_>>(),
        );
        hash.update(
            serde_json::to_vec(&binding)
                .map_err(|error| invalid(Path::new("inspection-binding"), error.to_string()))?,
        );
        let result = Self {
            inputs,
            fingerprint: hex::encode(hash.finalize()),
        };
        result.verify_inputs()?;
        Ok(result)
    }

    /// Recheck selected trees before and after physical loading; admission retains their leases afterward.
    pub(crate) fn verify_inputs(&self) -> Result<(), RustMetadataError> {
        for input in &self.inputs.sources {
            if input.root.canonicalize()? != input.root || !fs::symlink_metadata(&input.root)?.is_dir() {
                return Err(invalid(
                    &input.root,
                    "selected source root changed its physical binding",
                ));
            }
            let actual =
                digest_oven_source_tree(&input.root).map_err(|error| invalid(&input.root, error.to_string()))?;
            if actual != input.digest {
                return Err(invalid(
                    &input.root,
                    format!(
                        "selected source digest changed: expected {}, found {actual}",
                        input.digest
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Whether a temporary output path would enter one of the selected inputs.
    pub(crate) fn contains_input_path(&self, path: &Path) -> bool {
        self.inputs.sources.iter().any(|input| path.starts_with(&input.root))
    }

    /// Return the exact admitted projection bytes for temporary materialization.
    pub(crate) fn payload(&self) -> &[u8] {
        &self.inputs.project_json
    }
    /// Decode the existing rust-analyzer projection format after structural validation.
    pub(crate) fn project_data(&self) -> Result<ProjectJsonData, RustMetadataError> {
        serde_json::from_slice(&self.inputs.project_json)
            .map_err(|error| invalid(Path::new("rust-project.json"), error.to_string()))
    }
    /// Return selected target facts without invoking rustc or discovering a toolchain.
    pub(crate) fn target_data(&self) -> Result<TargetData, RustMetadataError> {
        let spec: TargetSpec = serde_json::from_slice(&self.inputs.target_spec_json)
            .map_err(|error| invalid(Path::new("target-spec.json"), error.to_string()))?;
        Ok(TargetData {
            arch: spec.arch.into(),
            data_layout: spec.data_layout.into_boxed_str(),
        })
    }
    /// Return the selected compiler version used by rust-analyzer's semantic model.
    pub(crate) fn toolchain_version(&self) -> &str {
        &self.inputs.toolchain_version
    }
    /// Return exact query aliases selected by the caller.
    pub(crate) fn query_roots(&self) -> &BTreeMap<String, PathBuf> {
        &self.inputs.query_roots
    }
    /// Return the complete physical projection binding used for metadata reuse.
    pub(crate) fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

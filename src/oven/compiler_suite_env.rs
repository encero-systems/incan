//! Typed process-boundary capability used by stored Oven compiler-suite children.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use serde::{Deserialize, Serialize};

/// Environment variable carrying the generated-code warning-check closure.
pub(crate) const OVEN_COMPILER_SUITE_CAPABILITY_ENV: &str = "INCAN_OVEN_COMPILER_SUITE_CAPABILITY";
/// Environment variable carrying the separate vocabulary-companion closure.
pub(crate) const OVEN_COMPILER_SUITE_VOCAB_CAPABILITY_ENV: &str = "INCAN_OVEN_COMPILER_SUITE_VOCAB_CAPABILITY";
/// Stable activation/compiler marker retained for narrow test helpers that need only the selected Rustc path.
pub(crate) const OVEN_COMPILER_SUITE_RUSTC_ENV: &str = "INCAN_OVEN_COMPILER_SUITE_RUSTC";
const OVEN_COMPILER_SUITE_CAPABILITY_SCHEMA_VERSION: u32 = 1;
const MAX_COMPILER_SUITE_DIRECT_RUSTC_INPUTS: usize = 1024;

/// Explicit process capabilities for one stored compiler-suite root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OvenCompilerSuiteTargetCapabilities {
    pub(crate) generated_rust_closure: bool,
}

impl OvenCompilerSuiteTargetCapabilities {
    /// Resolve the generated-code capability for one receipt-bound root's source path.
    pub(crate) fn for_target(source_relative_path: &str) -> Self {
        let generated_rust_closure = source_relative_path != "tests/toolchain_installer_tests.rs";
        Self { generated_rust_closure }
    }
}

/// One complete, receipt-selected direct-Rustc closure exported to a suite child.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenCompilerSuiteCapability {
    schema_version: u32,
    pub(crate) rustc: PathBuf,
    pub(crate) dependency_search_paths: Vec<PathBuf>,
    pub(crate) externs: BTreeMap<String, PathBuf>,
}

impl OvenCompilerSuiteCapability {
    /// Construct the complete typed direct-Rustc closure selected for one suite child.
    pub(crate) fn new(
        rustc: PathBuf,
        dependency_search_paths: Vec<PathBuf>,
        externs: BTreeMap<String, PathBuf>,
    ) -> Self {
        Self {
            schema_version: OVEN_COMPILER_SUITE_CAPABILITY_SCHEMA_VERSION,
            rustc,
            dependency_search_paths,
            externs,
        }
    }

    /// Encode one capability for a child process without indexed environment-key conventions.
    pub(crate) fn encode(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|error| format!("could not encode compiler-suite capability: {error}"))
    }

    /// Decode one complete capability and reject schema drift before any consumer uses a partial closure.
    pub(crate) fn decode(payload: &str) -> Result<Self, String> {
        let capability = serde_json::from_str::<Self>(payload)
            .map_err(|error| format!("invalid compiler-suite capability: {error}"))?;
        if capability.schema_version != OVEN_COMPILER_SUITE_CAPABILITY_SCHEMA_VERSION {
            return Err(format!(
                "unsupported compiler-suite capability schema {}",
                capability.schema_version
            ));
        }
        if capability.dependency_search_paths.len() > MAX_COMPILER_SUITE_DIRECT_RUSTC_INPUTS
            || capability.externs.len() > MAX_COMPILER_SUITE_DIRECT_RUSTC_INPUTS
        {
            return Err(format!(
                "compiler-suite capability exceeds the {MAX_COMPILER_SUITE_DIRECT_RUSTC_INPUTS}-input limit"
            ));
        }
        Ok(capability)
    }

    /// Read one optional typed capability from the current child environment.
    pub(crate) fn from_environment(name: &str) -> Result<Option<Self>, String> {
        let Some(payload) = std::env::var_os(name).filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        Self::decode(&payload.to_string_lossy()).map(Some)
    }
}

/// One exact physical file supplied by an admitted vocabulary helper closure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenVocabSupportFile {
    /// Stable manifest or checked workspace-output label, independent of delivery directories.
    pub(crate) label: String,
    pub(crate) path: PathBuf,
    pub(crate) digest: String,
}

/// The parent-selected files and arguments for one vocabulary compilation target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenVocabSupportClosure {
    pub(crate) dependency_search_paths: Vec<PathBuf>,
    pub(crate) externs: BTreeMap<String, PathBuf>,
    pub(crate) files: Vec<OvenVocabSupportFile>,
}

/// Vocabulary capability with retained file evidence, distinct from the path-only warning-check capability.
///
/// The caller obtains these records from its admitted manifest/materialization and retains the corresponding owners.
/// Decoding does not select a graph or grant access to neighboring files. Every recorded input is checked before a
/// vocabulary cache lookup; an old path-only capability cannot supply this contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenCompilerSuiteVocabCapability {
    schema_version: u32,
    pub(crate) rustc: OvenVocabSupportFile,
    pub(crate) intent: crate::oven::OvenBuildIntent,
    pub(crate) host: OvenVocabSupportClosure,
    pub(crate) auxiliary_targets: BTreeMap<String, OvenVocabSupportClosure>,
}

impl OvenVocabSupportFile {
    /// Record the explicitly selected compiler executable; artifact digests instead come from their admitted records.
    pub(crate) fn selected_compiler(path: PathBuf) -> Result<Self, String> {
        let digest = Self::digest(&path)?;
        Ok(Self {
            label: "rustc".to_string(),
            path,
            digest,
        })
    }

    /// Hash one selected regular file with bounded memory, without enumerating its containing directory.
    fn digest(path: &Path) -> Result<String, String> {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|error| format!("selected vocabulary file {}: {error}", path.display()))?;
        if !metadata.is_file() {
            return Err(format!(
                "selected vocabulary input is not a regular file: {}",
                path.display()
            ));
        }
        let mut file = std::fs::File::open(path)
            .map_err(|error| format!("cannot read selected vocabulary file {}: {error}", path.display()))?;
        let mut digest = Sha256::new();
        let mut buffer = [0; 65536];
        loop {
            let length = file
                .read(&mut buffer)
                .map_err(|error| format!("cannot hash selected vocabulary file {}: {error}", path.display()))?;
            if length == 0 {
                break;
            }
            digest.update(&buffer[..length]);
        }
        Ok(format!("sha256:{}", hex::encode(digest.finalize())))
    }

    /// Verify an already selected regular file without discovering other inputs beside it.
    pub(crate) fn verify(&self) -> Result<(), String> {
        if self.label.is_empty() || !self.path.is_absolute() {
            return Err("selected vocabulary file lacks an absolute path or logical label".to_string());
        }
        let metadata = std::fs::symlink_metadata(&self.path)
            .map_err(|error| format!("selected vocabulary file {}: {error}", self.path.display()))?;
        if !metadata.is_file() {
            return Err(format!(
                "selected vocabulary input is not a regular file: {}",
                self.path.display()
            ));
        }
        let actual = Self::digest(&self.path)?;
        if actual != self.digest {
            return Err(format!(
                "selected vocabulary file {} changed: expected {}, found {actual}",
                self.path.display(),
                self.digest
            ));
        }
        Ok(())
    }
}

impl OvenVocabSupportClosure {
    /// Join already materialized arguments to their admitted file records, without discovering a dependency closure.
    pub(crate) fn from_selected_files(
        dependency_search_paths: Vec<PathBuf>,
        externs: BTreeMap<String, PathBuf>,
        files: impl IntoIterator<Item = OvenVocabSupportFile>,
    ) -> Result<Self, String> {
        let mut selected = BTreeMap::<PathBuf, OvenVocabSupportFile>::new();
        for file in files {
            if !externs.values().any(|path| path == &file.path)
                && !dependency_search_paths
                    .iter()
                    .any(|path| file.path.parent() == Some(path.as_path()))
            {
                continue;
            }
            if let Some(previous) = selected.get(&file.path) {
                if previous != &file {
                    return Err(format!("conflicting vocabulary evidence for {}", file.path.display()));
                }
            } else {
                selected.insert(file.path.clone(), file);
            }
        }
        Ok(Self {
            dependency_search_paths,
            externs,
            files: selected.into_values().collect(),
        })
    }

    /// Copy only the selected files into private search directories; Rustc cannot discover unbound neighbors there.
    ///
    /// The caller retains the temporary directory through compilation. Recheck the copied bytes as well as the source
    /// binding so a same-path replacement during the copy cannot become a cache-authorized input.
    pub(crate) fn stage(&self, root: &Path) -> Result<Self, String> {
        self.verified_projection()?;
        let mut staged_paths = BTreeMap::new();
        let mut directories = BTreeMap::new();
        let mut files = Vec::new();
        for file in &self.files {
            let parent = file.path.parent().ok_or("selected vocabulary input has no parent")?;
            let next = root.join(format!("group-{}", directories.len()));
            let directory = directories.entry(parent.to_path_buf()).or_insert(next);
            std::fs::create_dir_all(&*directory).map_err(|error| error.to_string())?;
            let path = directory.join(
                file.path
                    .file_name()
                    .ok_or("selected vocabulary input has no filename")?,
            );
            std::fs::copy(&file.path, &path)
                .map_err(|error| format!("cannot stage vocabulary file {}: {error}", file.path.display()))?;
            let staged = OvenVocabSupportFile {
                label: file.label.clone(),
                path: path.clone(),
                digest: file.digest.clone(),
            };
            staged.verify()?;
            staged_paths.insert(file.path.clone(), path);
            files.push(staged);
        }
        let externs = self
            .externs
            .iter()
            .map(|(name, path)| {
                staged_paths
                    .get(path)
                    .cloned()
                    .map(|path| (name.clone(), path))
                    .ok_or_else(|| format!("selected vocabulary extern `{name}` was not staged"))
            })
            .collect::<Result<_, _>>()?;
        let dependency_search_paths = self
            .dependency_search_paths
            .iter()
            .map(|path| {
                directories
                    .get(path)
                    .cloned()
                    .ok_or_else(|| format!("selected vocabulary search group {} was not staged", path.display()))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            dependency_search_paths,
            externs,
            files,
        })
    }

    /// Validate explicit file/argument correspondence and return its delivery-independent cache projection.
    fn verified_projection(&self) -> Result<serde_json::Value, String> {
        if self.files.is_empty() || self.files.len() > MAX_COMPILER_SUITE_DIRECT_RUSTC_INPUTS {
            return Err("selected vocabulary support file binding is missing or exceeds its input limit".to_string());
        }
        let mut paths = BTreeMap::new();
        let mut files = BTreeMap::new();
        for file in &self.files {
            file.verify()?;
            if paths.insert(&file.path, &file.label).is_some()
                || files
                    .insert(
                        &file.label,
                        serde_json::json!({
                            "digest": file.digest, "filename": file.path.file_name().and_then(|name| name.to_str())
                                .ok_or("selected vocabulary file has no UTF-8 Rustc filename")?,
                        }),
                    )
                    .is_some()
            {
                return Err("selected vocabulary support repeats a physical file or logical label".to_string());
            }
        }
        let mut externs = BTreeMap::new();
        for (name, path) in &self.externs {
            if name.is_empty() || !name.chars().all(|ch| ch == '_' || ch.is_ascii_alphanumeric()) {
                return Err(format!("selected vocabulary extern has invalid name `{name}`"));
            }
            let label = paths
                .get(path)
                .ok_or_else(|| format!("selected vocabulary extern `{name}` has no exact file binding"))?;
            externs.insert(name, label);
        }
        for required in ["incan_vocab", "serde_json"] {
            if !self.externs.contains_key(required) {
                return Err(format!("selected vocabulary closure lacks required `{required}`"));
            }
        }
        for file in &self.files {
            if !self.externs.values().any(|path| path == &file.path)
                && !self
                    .dependency_search_paths
                    .iter()
                    .any(|path| file.path.parent() == Some(path.as_path()))
            {
                return Err(format!(
                    "selected vocabulary file has no argument binding: {}",
                    file.path.display()
                ));
            }
        }
        let mut search_groups = Vec::new();
        for path in &self.dependency_search_paths {
            if !path.is_absolute() || !path.is_dir() {
                return Err(format!(
                    "selected vocabulary search path is invalid: {}",
                    path.display()
                ));
            }
            let labels = self
                .files
                .iter()
                .filter(|file| file.path.parent() == Some(path.as_path()))
                .map(|file| &file.label)
                .collect::<std::collections::BTreeSet<_>>();
            if labels.is_empty() {
                return Err(format!(
                    "selected vocabulary search path has no bound files: {}",
                    path.display()
                ));
            }
            search_groups.push(labels);
        }
        Ok(serde_json::json!({"files": files, "externs": externs, "search_groups": search_groups}))
    }
}

impl OvenCompilerSuiteVocabCapability {
    /// Retain evidence already selected by the parent; validation is required before either caching or execution.
    pub(crate) fn new(
        rustc: OvenVocabSupportFile,
        intent: crate::oven::OvenBuildIntent,
        host: OvenVocabSupportClosure,
        auxiliary_targets: BTreeMap<String, OvenVocabSupportClosure>,
    ) -> Self {
        Self {
            schema_version: 1,
            rustc,
            intent,
            host,
            auxiliary_targets,
        }
    }

    /// Verify all selected bytes and bind target/toolchain/profile/features plus extraction mode to cache reuse.
    pub(crate) fn verified_cache_identity(&self, mode: &str) -> Result<String, String> {
        if self.schema_version != 1 {
            return Err(format!(
                "unsupported vocabulary capability schema {}",
                self.schema_version
            ));
        }
        if self.intent.target.is_empty() || self.intent.toolchain.is_empty() || self.intent.profile.is_empty() {
            return Err("selected vocabulary target, toolchain and profile binding is required".to_string());
        }
        self.rustc.verify()?;
        let host = self.host.verified_projection()?;
        let mut auxiliary = BTreeMap::new();
        for (target, closure) in &self.auxiliary_targets {
            if target.is_empty() || target == &self.intent.target {
                return Err("selected vocabulary auxiliary target is empty or duplicates the host".to_string());
            }
            auxiliary.insert(target, closure.verified_projection()?);
        }
        let projection = serde_json::json!({
            "format": 1, "rustc": self.rustc.digest, "intent": self.intent,
            "host": host, "auxiliary": auxiliary, "mode": mode,
        });
        serde_json::to_vec(&projection)
            .map(|bytes| crate::oven::digest_bytes(&bytes))
            .map_err(|error| format!("cannot encode selected vocabulary inputs: {error}"))
    }

    /// Encode the complete parent-selected vocabulary capability.
    pub(crate) fn encode(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|error| format!("cannot encode vocabulary capability: {error}"))
    }

    /// Read an explicit scheduler capability; legacy/missing file bindings are never reconstructed from directories.
    pub(crate) fn from_environment() -> Result<Option<Self>, String> {
        let Some(payload) =
            std::env::var_os(OVEN_COMPILER_SUITE_VOCAB_CAPABILITY_ENV).filter(|value| !value.is_empty())
        else {
            return Ok(None);
        };
        serde_json::from_str(&payload.to_string_lossy())
            .map(Some)
            .map_err(|error| format!("selected vocabulary capability lacks valid file evidence: {error}"))
    }
}

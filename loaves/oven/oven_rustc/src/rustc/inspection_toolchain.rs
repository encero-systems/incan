//! Compiler-bound Rust library sources retained by an original Store owner, independently of SDK inventories.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oven_store::store::{
    OvenArtifactKind, OvenArtifactMaterializedDirectory, OvenArtifactMaterializedFile, OvenArtifactPublishRequest,
    OvenStore, OvenStoreExecutionPayload, digest_regular_file,
};
use oven_store::{OvenGeneratedProjectRequest, OvenReceipt, digest_bytes, receipt_generated_project};
use serde::{Deserialize, Serialize};

use super::{OvenRustcError, rustc_cfg_snapshot, rustc_host_target, rustc_identity, rustc_sysroot};

const DOMAIN: &str = "rust-inspection-toolchain";
const SOURCE: &str = "rust-src/library";

/// Portable compiler facts; paths are original owner coordinates rather than serialized authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CompilerFacts {
    executable_digest: String,
    std_digest: String,
    identity: String,
    host: String,
    target: String,
    cfg: Vec<String>,
}

/// Complete regular-file and empty-directory inventory of the producer's Rust library sources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SourceInventory {
    files: BTreeMap<String, (u64, String)>,
    empty_directories: Vec<String>,
}

/// Immutable source and target evidence bound by both the receipt and its materialized Store inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ToolchainPayload {
    schema_version: u32,
    compiler: CompilerFacts,
    sources: SourceInventory,
}

/// Original Store admission for the selected compiler's Rust sources and target cfg.
///
/// This capability can only be constructed by the producer below. A serialized payload or a path catalog cannot
/// select new owners. It grants source inspection, never native macro execution.
pub struct OvenRustInspectionToolchain {
    owner: Arc<OvenStoreExecutionPayload>,
    receipt: OvenReceipt,
    payload: ToolchainPayload,
    rustc: PathBuf,
    sysroot: PathBuf,
    source_root: PathBuf,
}

impl OvenRustInspectionToolchain {
    /// Revalidate original compiler bytes, its selected binary std closure and the complete retained source owner.
    pub fn verify(&self) -> Result<(), OvenRustcError> {
        self.owner.verify_admitted_payload()?;
        if self.owner.manifest.kind != OvenArtifactKind::RustInspectionToolchain
            || self.owner.manifest.domain != DOMAIN
            || self.owner.original_native_receipt() != Some(&self.receipt)
            || self.owner.payload != encode(&self.payload)?
            || digest_regular_file(&self.rustc)?.1 != self.payload.compiler.executable_digest
            || crate::sdk_closure::compiler_closure_digest(&self.rustc, &self.payload.compiler.target)
                .map_err(invalid)?
                != self.payload.compiler.std_digest
        {
            return Err(invalid(
                "retained Rust inspection toolchain differs from its original producer",
            ));
        }
        let files = self.owner.admitted_materialized_files();
        if files.len() != self.payload.sources.files.len()
            || files.iter().any(|file| {
                file.relative_path
                    .strip_prefix(&format!("{SOURCE}/"))
                    .and_then(|relative| self.payload.sources.files.get(relative))
                    != Some(&(file.logical_bytes, file.digest.clone()))
            })
            || self
                .owner
                .admitted_materialized_directories()
                .iter()
                .map(|directory| directory.relative_path.clone())
                .collect::<Vec<_>>()
                != self
                    .payload
                    .sources
                    .empty_directories
                    .iter()
                    .map(|relative| format!("{SOURCE}/{relative}"))
                    .collect::<Vec<_>>()
        {
            return Err(invalid(
                "retained Rust inspection source inventory differs from its producer",
            ));
        }
        Ok(())
    }

    /// Match an explicit compiler and target before projecting this original source capability.
    pub fn verify_compiler(&self, rustc: &Path, target: &str) -> Result<(), OvenRustcError> {
        if fs::canonicalize(rustc).map_err(|source| io(rustc, source))? != self.rustc
            || target != self.payload.compiler.target
        {
            return Err(invalid(
                "Rust inspection toolchain belongs to another compiler or target",
            ));
        }
        self.verify()
    }

    /// Borrow the immutable source root after verifying this capability at the semantic boundary.
    pub fn source_root(&self) -> &Path {
        &self.source_root
    }

    /// Borrow the original compiler's binary sysroot coordinate, separately verified by its binary closure.
    pub fn sysroot(&self) -> &Path {
        &self.sysroot
    }

    /// Borrow exact target cfg captured from the selected compiler during publication.
    pub fn cfg(&self) -> &[String] {
        &self.payload.compiler.cfg
    }

    /// Return the source-owning artifact identity for cache fingerprints and diagnostic evidence.
    pub fn identity(&self) -> &str {
        &self.owner.manifest.identity
    }
}

/// Publish and retain complete Rust library sources for one explicit compiler and target without Cargo or an SDK.
///
/// The receipt selects source bytes and compiler facts independently of application inputs or build profiles. Store
/// publication reuses the same identity when these inputs match. The producer checks input freshness again after
/// materialization; consumers then use the immutable copy rather than depending on the installed source tree.
pub fn prepare_rust_inspection_toolchain(
    store: &OvenStore,
    rustc: &Path,
    target: &str,
) -> Result<OvenRustInspectionToolchain, OvenRustcError> {
    let rustc = fs::canonicalize(rustc).map_err(|source| io(rustc, source))?;
    let sysroot = rustc_sysroot(&rustc)?;
    if !sysroot.is_absolute() {
        return Err(invalid("Rust inspection requires an absolute compiler sysroot"));
    }
    let sysroot = fs::canonicalize(&sysroot).map_err(|source| io(&sysroot, source))?;
    let source = sysroot.join("lib/rustlib/src/rust/library");
    if fs::canonicalize(&source).map_err(|error| io(&source, error))? != source {
        return Err(invalid("Rust inspection source coordinate contains a linked ancestor"));
    }
    if rustc.parent().and_then(Path::parent) != Some(sysroot.as_path()) {
        return Err(invalid(
            "Rust inspection compiler and binary sysroot have different original roots",
        ));
    }
    let payload = ToolchainPayload {
        schema_version: 1,
        compiler: compiler_facts(&rustc, target)?,
        sources: source_inventory(&source)?,
    };
    if !payload.sources.files.contains_key("core/src/lib.rs")
        || !payload.sources.files.contains_key("std/src/lib.rs")
        || !payload.sources.files.contains_key("alloc/src/lib.rs")
    {
        return Err(invalid(
            "Rust inspection requires complete core, alloc and std library sources",
        ));
    }
    let bytes = encode(&payload)?;
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            &source,
            DOMAIN,
            "0.0.0",
            target,
            &payload.compiler.identity,
            "inspection",
            Vec::new(),
        )
        .with_generated_source("core", source.join("core/src/lib.rs"))
        .with_build_unit_input("rust-inspection-toolchain", digest_bytes(&bytes)),
    )
    .map_err(invalid)?;
    let request = OvenArtifactPublishRequest {
        receipt: receipt.clone(),
        domain: DOMAIN.to_string(),
        kind: OvenArtifactKind::RustInspectionToolchain,
        payload: bytes,
        materialized_files: payload
            .sources
            .files
            .keys()
            .map(|relative| OvenArtifactMaterializedFile {
                source_path: source.join(relative),
                relative_path: format!("{SOURCE}/{relative}"),
            })
            .collect(),
        materialized_directories: payload
            .sources
            .empty_directories
            .iter()
            .map(|relative| OvenArtifactMaterializedDirectory {
                source_path: source.join(relative),
                relative_path: format!("{SOURCE}/{relative}"),
            })
            .collect(),
    };
    let expected = store.manifest_for_publication(&request)?;
    let identities = [expected.identity.clone()];
    let mut selected = match store.try_select_payloads_for_execution(&identities)? {
        Some(owners) => owners,
        None => {
            store.publish(&request)?;
            store.select_payloads_for_execution(&identities)?
        }
    };
    if compiler_facts(&rustc, target)? != payload.compiler || source_inventory(&source)? != payload.sources {
        return Err(invalid(
            "Rust inspection compiler or sources changed during publication",
        ));
    }
    let owner = Arc::new(
        selected
            .pop()
            .ok_or_else(|| invalid("Rust inspection publication returned no original owner"))?,
    );
    if owner.manifest != expected {
        return Err(invalid(
            "Rust inspection owner differs from the exact prospective publication",
        ));
    }
    let result = OvenRustInspectionToolchain {
        source_root: owner.artifact_root.join(SOURCE),
        owner,
        receipt,
        payload,
        rustc,
        sysroot,
    };
    result.verify()?;
    Ok(result)
}

/// Capture compiler identity, binary closure and exact target cfg without ambient loader paths.
fn compiler_facts(rustc: &Path, target: &str) -> Result<CompilerFacts, OvenRustcError> {
    let snapshot = rustc_cfg_snapshot(rustc, Some(target))?;
    let mut cfg = snapshot.flags;
    for (name, values) in snapshot.values {
        for value in values {
            cfg.push(format!("{name}={}", serde_json::to_string(&value).map_err(invalid)?));
        }
    }
    cfg.sort();
    Ok(CompilerFacts {
        executable_digest: digest_regular_file(rustc)?.1,
        std_digest: crate::sdk_closure::compiler_closure_digest(rustc, target).map_err(invalid)?,
        identity: rustc_identity(rustc)?,
        host: rustc_host_target(rustc)?,
        target: target.to_string(),
        cfg,
    })
}

/// Inventory every source file and empty directory, rejecting links, special files and nonportable path spellings.
fn source_inventory(root: &Path) -> Result<SourceInventory, OvenRustcError> {
    let mut result = SourceInventory {
        files: BTreeMap::new(),
        empty_directories: Vec::new(),
    };
    collect_source(root, root, &mut result)?;
    result.empty_directories.sort();
    Ok(result)
}

/// Recursively capture one complete source tree without following links or ignoring source declarations.
fn collect_source(root: &Path, path: &Path, result: &mut SourceInventory) -> Result<(), OvenRustcError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| io(path, source))?;
    if metadata.file_type().is_symlink() || !(metadata.is_file() || metadata.is_dir()) {
        return Err(invalid(format!(
            "Rust inspection source must be a regular file or directory: {}",
            path.display()
        )));
    }
    let relative = path.strip_prefix(root).map_err(invalid)?;
    let relative = relative
        .components()
        .map(|component| {
            let std::path::Component::Normal(name) = component else {
                return Err(invalid("Rust source path is not relative"));
            };
            let text = name.to_str().ok_or_else(|| invalid("Rust source path is not UTF-8"))?;
            if text.contains('\\') {
                return Err(invalid("Rust source path contains a nonportable separator"));
            }
            Ok(text.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?
        .join("/");
    if metadata.is_file() {
        result.files.insert(relative, digest_regular_file(path)?);
    } else {
        let children = fs::read_dir(path)
            .map_err(|source| io(path, source))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| io(path, source))?;
        if children.is_empty() && !relative.is_empty() {
            result.empty_directories.push(relative);
        }
        for child in children {
            collect_source(root, &child.path(), result)?;
        }
    }
    Ok(())
}

/// Encode portable producer evidence while preserving serialization errors in the native error domain.
fn encode(payload: &ToolchainPayload) -> Result<Vec<u8>, OvenRustcError> {
    serde_json::to_vec(payload).map_err(invalid)
}

/// Attach the concrete file coordinate to a filesystem failure.
fn io(path: &Path, source: std::io::Error) -> OvenRustcError {
    OvenRustcError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Refuse an unbound semantic source or compiler input.
fn invalid(error: impl std::fmt::Display) -> OvenRustcError {
    OvenRustcError::InvalidInput {
        field: "Rust inspection toolchain",
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oven_store::store::OvenStoreLimits;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A real compiler's complete source publication reuses its identity and refuses original-owner damage.
    #[test]
    fn compiler_bound_sources_reuse_original_store_identity_and_refuse_damage() -> TestResult {
        let root = tempfile::tempdir()?;
        let store = OvenStore::new(
            root.path().join("store"),
            OvenStoreLimits::new(1 << 30, 1 << 30, 1 << 30),
        );
        let rustc = super::super::resolve_active_rustc()?;
        let target = rustc_host_target(&rustc)?;
        let started = std::time::Instant::now();
        let mut first = prepare_rust_inspection_toolchain(&store, &rustc, &target)?;
        let first_seconds = started.elapsed().as_secs_f64();
        // Original exact selection needs no new physical admission, even with no headroom for publication.
        let bounded = OvenStore::new(store.root(), OvenStoreLimits::new(1, 1, 1 << 30));
        let started = std::time::Instant::now();
        let repeat = prepare_rust_inspection_toolchain(&bounded, &rustc, &target)?;
        println!(
            "compiler-bound source preparation first={first_seconds:.6}s repeat={:.6}s files={}",
            started.elapsed().as_secs_f64(),
            first.payload.sources.files.len()
        );
        assert_eq!(first.identity(), repeat.identity());
        assert_eq!(first.source_root(), repeat.source_root());
        assert!(first.source_root().starts_with(root.path().canonicalize()?));
        assert!(!first.source_root().starts_with(first.sysroot()));
        assert!(first.cfg().iter().any(|cfg| cfg.starts_with("target_arch=")));
        assert!(first.verify_compiler(&rustc, "another-target").is_err());
        let other = root.path().join("another-rustc");
        fs::write(&other, "different compiler")?;
        assert!(first.verify_compiler(&other, &target).is_err());
        let source = first.source_root().join("core/src/lib.rs");
        let backup = root.path().join("original-core.rs");
        fs::rename(&source, &backup)?;
        assert!(first.verify().is_err());
        fs::write(&source, "pub const SUBSTITUTED: bool = true;")?;
        assert!(first.verify().is_err());
        fs::remove_file(&source)?;
        fs::rename(&backup, &source)?;
        first.verify()?;
        let extra = first.source_root().join("unlisted.rs");
        fs::write(&extra, "pub const UNLISTED: bool = true;")?;
        assert!(first.verify().is_err());
        fs::remove_file(extra)?;
        first.verify()?;
        Arc::get_mut(&mut first.owner)
            .ok_or("unexpected original owner clone")?
            .payload
            .push(b' ');
        assert!(first.verify().is_err());
        Ok(())
    }

    /// Complete source capture detects file changes and empty directories and refuses linked input shapes.
    #[test]
    fn complete_source_inventory_binds_changes_and_refuses_links() -> TestResult {
        let root = tempfile::tempdir()?;
        fs::write(root.path().join("lib.rs"), "before")?;
        let first = source_inventory(root.path())?;
        fs::write(root.path().join("lib.rs"), "after!")?;
        assert_ne!(first, source_inventory(root.path())?);
        fs::create_dir(root.path().join("empty"))?;
        assert_eq!(source_inventory(root.path())?.empty_directories, vec!["empty"]);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.path().join("lib.rs"), root.path().join("alias.rs"))?;
            assert!(source_inventory(root.path()).is_err());
        }
        Ok(())
    }
}

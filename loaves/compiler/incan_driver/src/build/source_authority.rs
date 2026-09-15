//! Digesting the sources, locks and build trees a bake's authority is keyed on, and where its receipts live.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::{fs, io};

use crate::build::bake::discover_oven_executable_entrypoints;
use crate::build::package_loafs::read_packaged_library_loaf_manifest;
use crate::build::{
    OvenBakeProjectTarget, OvenProjectPlanMode, ProjectSourceAuthorityDigester, oven_bake_project_target_identity,
};
use crate::error::{CliError, CliResult};
use crate::project::{effective_project_manifest_for_exact_root, resolve_source_root};
use incan_frontend::library_manifest::digest_cargo_path_source_tree_with_cache;
use incan_frontend::library_manifest_index::{
    LibraryArtifactKind, LibraryManifestIndexEntry, load_provider_dependency_artifact,
};
use oven_model::lock::{IncanLock, LOCK_FILENAME};
use oven_model::manifest::{DependencySource, GitReference, LOAF_MANIFEST_FILENAME, ProjectManifest};
use oven_model::oven_interop::locked_oven_interop_targets;
use oven_rustc::interop::{
    default_interop_execution_receipt_path, load_interop_execution_receipt, validate_interop_execution_receipt,
};
use oven_rustc::legacy_cargo::digest_local_cargo_workspace_authority;
use oven_store::{digest_bytes, digest_project_source_tree};

impl ProjectSourceAuthorityDigester {
    /// Digest the exact build-input graph for one project without observing generated or unrelated files.
    pub fn digest(&mut self, project_root: &Path) -> CliResult<String> {
        self.digest_project_node(project_root, &mut HashSet::new())
    }

    /// Drop every memoized project-tree digest so the next scan re-reads each project from disk.
    ///
    /// A memoized digest is only as current as the tree it was read from. Publishing the canonical lock writes one
    /// file that belongs to every member's build inputs at once, so a child digest taken before that write would keep
    /// describing a tree that no longer exists. Rust crate and source-closure digests are unaffected: a lock
    /// publication does not touch them.
    pub fn forget_project_tree_digests(&mut self) {
        self.project_digests.clear();
    }

    /// Return how many cache-miss project-tree scans this digester performed for one canonical root.
    #[cfg(test)]
    pub fn project_scan_count(&self, project_root: &Path) -> usize {
        fs::canonicalize(project_root)
            .ok()
            .and_then(|root| self.project_scan_counts.get(&root).copied())
            .unwrap_or_default()
    }

    /// Record one cache-miss project-tree scan for this canonical root.
    #[cfg(test)]
    pub fn record_project_scan(&mut self, canonical_root: &Path) {
        *self
            .project_scan_counts
            .entry(canonical_root.to_path_buf())
            .or_default() += 1;
    }

    /// Ignore cache-miss scan accounting outside tests.
    #[cfg(not(test))]
    pub fn record_project_scan(&mut self, _canonical_root: &Path) {}

    /// Digest one local Rust package together with only the Cargo-workspace facts that it actually inherits.
    pub fn digest_rust_path_crate_authority(
        package_root: &Path,
        rust_source_closure_digests: &mut BTreeMap<PathBuf, String>,
    ) -> CliResult<String> {
        let source_tree =
            digest_cargo_path_source_tree_with_cache(package_root, rust_source_closure_digests).map_err(|error| {
                CliError::failure(format!(
                    "Oven Alpha cannot digest Rust path dependency source authority at {}: {error}",
                    package_root.display()
                ))
            })?;
        let mut records = BTreeMap::from([("package-source-tree", source_tree)]);
        if let Some(workspace_authority) = digest_local_cargo_workspace_authority(package_root).map_err(|error| {
            CliError::failure(format!(
                "Oven Alpha cannot resolve Rust path dependency workspace authority at {}: {error}",
                package_root.display()
            ))
        })? {
            records.insert("inherited-cargo-workspace", workspace_authority);
        }
        let payload = serde_json::to_vec(&records).map_err(|error| {
            CliError::failure(format!(
                "failed to serialize Rust path dependency source authority at {}: {error}",
                package_root.display()
            ))
        })?;
        Ok(digest_bytes(&payload))
    }

    /// Digest one reachable Incan node and its named dependency edges.
    pub fn digest_project_node(&mut self, root: &Path, visiting: &mut HashSet<PathBuf>) -> CliResult<String> {
        let canonical_root = fs::canonicalize(root).map_err(|error| {
            CliError::failure(format!(
                "Oven Alpha cannot resolve project source authority at {}: {error}",
                root.display()
            ))
        })?;
        if let Some(digest) = self.project_digests.get(&canonical_root) {
            return Ok(digest.clone());
        }
        if !visiting.insert(canonical_root.clone()) {
            return Err(CliError::failure(format!(
                "Oven Alpha project source authority contains a cyclic path dependency at {}",
                canonical_root.display()
            )));
        }
        self.record_project_scan(&canonical_root);
        let manifest = effective_project_manifest_for_exact_root(&canonical_root)?;
        let mut records = BTreeMap::from([
            (
                "project-build-inputs".to_string(),
                digest_baked_project_build_tree(&canonical_root, &manifest)?,
            ),
            (
                "project-dependency-selections".to_string(),
                digest_baked_project_dependency_selections(&manifest)?,
            ),
        ]);
        let mut library_dependencies = manifest.library_dependencies().iter().collect::<Vec<_>>();
        library_dependencies.sort_by_key(|(name, _)| *name);
        for (name, dependency) in library_dependencies {
            let child_manifest = dependency.path.join(LOAF_MANIFEST_FILENAME);
            let child_digest = match fs::symlink_metadata(&child_manifest) {
                Ok(_) => self.digest_project_node(&dependency.path, visiting)?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    let artifact_root = dependency.path.join("target/lib");
                    let entry = load_provider_dependency_artifact(name, &artifact_root);
                    let artifact = match entry {
                        LibraryManifestIndexEntry::Loaded { metadata, .. }
                            if metadata.kind == LibraryArtifactKind::Materialized =>
                        {
                            metadata
                        }
                        LibraryManifestIndexEntry::Loaded { .. } => {
                            return Err(CliError::failure(format!(
                                "Oven Alpha cannot bind source-free pub::{name}: its checked package artifact is not materialized"
                            )));
                        }
                        LibraryManifestIndexEntry::Failed(failure) => {
                            return Err(CliError::failure(format!(
                                "Oven Alpha cannot bind source-free pub::{name} at {}: {failure}",
                                dependency.path.display()
                            )));
                        }
                    };
                    let package = read_packaged_library_loaf_manifest(&artifact)?.ok_or_else(|| {
                        CliError::failure(format!(
                            "Oven Alpha cannot bind source-free pub::{name} at {} without its sealed package Loaf; rebake or reinstall that provider",
                            dependency.path.display()
                        ))
                    })?;
                    package.source_authority_digest
                }
                Err(error) => {
                    return Err(CliError::failure(format!(
                        "Oven Alpha cannot inspect the source manifest for pub::{name} at {}: {error}",
                        child_manifest.display()
                    )));
                }
            };
            records.insert(format!("incan-dependency:{name}"), child_digest);
        }

        let mut rust_path_dependencies =
            manifest
                .rust_dependencies()
                .iter()
                .filter_map(|(name, dependency)| match &dependency.source {
                    DependencySource::Path { path } => Some(("normal", name, path)),
                    DependencySource::Registry | DependencySource::Git { .. } => None,
                })
                .chain(manifest.rust_dev_dependencies().iter().filter_map(
                    |(name, dependency)| match &dependency.source {
                        DependencySource::Path { path } => Some(("dev", name, path)),
                        DependencySource::Registry | DependencySource::Git { .. } => None,
                    },
                ))
                .collect::<Vec<_>>();
        rust_path_dependencies.sort_by(|left, right| (left.0, left.1).cmp(&(right.0, right.1)));
        for (kind, name, dependency) in rust_path_dependencies {
            let canonical_dependency = fs::canonicalize(dependency).map_err(|error| {
                CliError::failure(format!(
                    "Oven Alpha cannot resolve Rust path dependency source authority at {}: {error}",
                    dependency.display()
                ))
            })?;
            let child_digest = if let Some(digest) = self.rust_crate_digests.get(&canonical_dependency) {
                digest.clone()
            } else {
                let digest = Self::digest_rust_path_crate_authority(
                    &canonical_dependency,
                    &mut self.rust_source_closure_digests,
                )?;
                self.rust_crate_digests.insert(canonical_dependency, digest.clone());
                digest
            };
            records.insert(format!("rust-{kind}-dependency:{name}"), child_digest);
        }

        let payload = serde_json::to_vec(&records).map_err(|error| {
            CliError::failure(format!("failed to serialize Oven project source authority: {error}"))
        })?;
        let digest = digest_bytes(&payload);
        visiting.remove(&canonical_root);
        self.project_digests.insert(canonical_root, digest.clone());
        Ok(digest)
    }
}

/// Digest the exact build-input graph for a completed project output without observing generated or unrelated files.
///
/// Each ordinary call owns a fresh memo, preserving the existing behavior outside explicit project bake orchestration.
pub fn digest_baked_project_source_authority(project_root: &Path) -> CliResult<String> {
    ProjectSourceAuthorityDigester::default().digest(project_root)
}

/// Digest one effective project's portable dependency selections without walking dependency source twice.
///
/// Named graph edges below bind path dependencies to their authored content. This record binds the remaining
/// identity facts, including RFC 077 inherited features and registry or Git selection, while intentionally omitting
/// machine-local path spelling so equivalent relocated worktrees can reuse a completed output.
fn digest_baked_project_dependency_selections(manifest: &ProjectManifest) -> CliResult<String> {
    let mut records = BTreeMap::new();
    for (name, dependency) in manifest.library_dependencies() {
        let mut features = dependency.features.clone();
        features.sort();
        features.dedup();
        records.insert(
            format!("incan:{name}"),
            format!(
                "{}|{}|{}|{}",
                dependency.library_name,
                dependency.default_features,
                dependency.optional,
                features.join(",")
            ),
        );
    }
    for (kind, dependencies) in [
        ("normal", manifest.rust_dependencies()),
        ("dev", manifest.rust_dev_dependencies()),
    ] {
        for (name, dependency) in dependencies {
            let mut features = dependency.features.clone();
            features.sort();
            features.dedup();
            let source = match &dependency.source {
                DependencySource::Registry => "registry".to_string(),
                DependencySource::Path { .. } => "path-tree".to_string(),
                DependencySource::Git { url, reference } => match reference {
                    GitReference::Branch(branch) => format!("git:{url}:branch:{branch}"),
                    GitReference::Tag(tag) => format!("git:{url}:tag:{tag}"),
                    GitReference::Rev(revision) => format!("git:{url}:rev:{revision}"),
                },
            };
            records.insert(
                format!("rust-{kind}:{name}"),
                format!(
                    "{}|{}|{}|{}|{}|{}|{}",
                    dependency.crate_name,
                    dependency.package.as_deref().unwrap_or(""),
                    dependency.version.as_deref().unwrap_or(""),
                    dependency.default_features,
                    dependency.optional,
                    features.join(","),
                    source
                ),
            );
        }
    }
    let payload = serde_json::to_vec(&records).map_err(|error| {
        CliError::failure(format!(
            "failed to serialize Oven project dependency selections: {error}"
        ))
    })?;
    Ok(digest_bytes(&payload))
}

/// Resolve the one canonical semantic lock that governs a project bake.
///
/// Workspace members share the root RFC 077 lock. Reading a member-local file instead would let a stale side file
/// hide changes to the real workspace authority, or make equivalent members disagree about the lock they consumed.
pub fn canonical_baked_project_lock_path(project_root: &Path) -> CliResult<PathBuf> {
    let workspace = oven_model::workspace::WorkspaceGraph::discover(project_root)
        .map_err(|error| CliError::failure(format!("failed to resolve Oven project workspace: {error}")))?;
    Ok(workspace
        .map(|workspace| workspace.root().join(LOCK_FILENAME))
        .unwrap_or_else(|| project_root.join(LOCK_FILENAME)))
}

/// Load the derived dependency fingerprint from the canonical project or workspace lock, when present.
pub fn baked_project_lock_dependencies_fingerprint(project_root: &Path) -> CliResult<Option<String>> {
    let lock_path = canonical_baked_project_lock_path(project_root)?;
    if !lock_path.is_file() {
        return Ok(None);
    }
    IncanLock::load(&lock_path)
        .map(|lock| Some(lock.deps_fingerprint))
        .map_err(|error| CliError::failure(error.to_string()))
}

/// Hash the canonical lock fields that can change build meaning while excluding derived and structural fields.
///
/// Lock format 1 and 2 decode into the same semantic authority projection. An explicit bake is allowed to migrate
/// that representation, so recording the format here would make the publisher reject the state it just wrote.
///
/// A workspace lock is one root file shared by every member, but a member's build authority is only its own entry:
/// the features, providers, and SDK selections locked for that member, plus the workspace-level Cargo fields. Hashing
/// every sibling's entry too would change a member's authority each time another member is baked or added, which
/// refuses every previously baked provider in the workspace and makes staged member bakes impossible (#1414). A member
/// that has no entry yet is digested as if the lock were absent, so its authority is the same before and after the
/// root lock first appears without it.
fn digest_baked_project_lock_authority(lock_path: &Path, project_root: &Path) -> CliResult<Option<String>> {
    let lock = IncanLock::load(lock_path).map_err(|error| CliError::failure(error.to_string()))?;
    let mut semantic = lock.semantic;
    if !semantic.workspace_members.is_empty() {
        let workspace_root = lock_path.parent().unwrap_or(lock_path);
        let member_root = oven_model::lock::portable_project_path(workspace_root, project_root);
        let Some(member) = semantic
            .workspace_members
            .into_iter()
            .find(|member| member.member_root == member_root)
        else {
            return Ok(None);
        };
        semantic = oven_model::lock::SemanticLockState {
            workspace_members: vec![member],
            ..oven_model::lock::SemanticLockState::default()
        };
    }
    // A bake refreshes compiler-owned SDK identity records to the active release cohort. Those records are already
    // bound by the compiler/runtime receipt inputs, so treating them as authored project authority would make the
    // publisher reject its own lock refresh. Package, feature, custom-provider, Oven, workspace, and Cargo-lock
    // selections remain part of this lock authority.
    semantic.sdk = None;
    semantic
        .providers
        .retain(|provider| !provider.identity.starts_with("incan_stdlib_"));
    for member in &mut semantic.workspace_members {
        member.sdk = None;
        member
            .providers
            .retain(|provider| !provider.identity.starts_with("incan_stdlib_"));
    }
    let projection = serde_json::json!({
        "cargo_features": lock.cargo_features,
        "semantic": semantic,
        "cargo_lock_payload": lock.cargo_lock_payload,
    });
    serde_json::to_vec(&projection)
        .map(|bytes| Some(digest_bytes(&bytes)))
        .map_err(|error| CliError::failure(format!("failed to serialize canonical Oven lock authority: {error}")))
}

/// Hash only the declared Incan project inputs that can affect a normal build.
fn digest_baked_project_build_tree(project_root: &Path, manifest: &ProjectManifest) -> CliResult<String> {
    /// Record one regular authority file under a caller-selected portable key.
    fn append_named_file(path: &Path, record_key: String, records: &mut BTreeMap<String, String>) -> CliResult<()> {
        let metadata = fs::symlink_metadata(path).map_err(|error| {
            CliError::failure(format!(
                "Oven Alpha cannot read project build authority at {}: {error}",
                path.display()
            ))
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(CliError::failure(format!(
                "Oven Alpha project build authority must use regular files, found {}",
                path.display()
            )));
        }
        let digest = digest_bytes(&fs::read(path).map_err(|error| {
            CliError::failure(format!(
                "Oven Alpha cannot hash project build authority at {}: {error}",
                path.display()
            ))
        })?);
        if records.insert(record_key.clone(), digest).is_some() {
            return Err(CliError::failure(format!(
                "Oven Alpha project build authority contains duplicate path `{record_key}`"
            )));
        }
        Ok(())
    }

    /// Record one regular project input by its portable path and content digest.
    fn append_file(root: &Path, path: &Path, records: &mut BTreeMap<String, String>) -> CliResult<()> {
        let relative = path
            .strip_prefix(root)
            .map_err(|_| CliError::failure(format!("project build authority escaped {}", root.display())))?
            .to_string_lossy()
            .replace('\\', "/");
        append_named_file(path, relative, records)
    }

    /// Traverse a regular source directory deterministically and record its input files.
    ///
    /// `already_recorded` names files this traversal must not re-record: the manifest and lock file are always
    /// recorded separately above under their own dedicated (and, for the lock, semantically filtered) digest, but a
    /// flat-layout project whose source root is the project root itself would otherwise walk straight over them here
    /// too, producing a spurious duplicate-path failure.
    fn collect_directory(
        root: &Path,
        directory: &Path,
        records: &mut BTreeMap<String, String>,
        already_recorded: &HashSet<PathBuf>,
    ) -> CliResult<()> {
        let mut entries = fs::read_dir(directory)
            .map_err(|error| {
                CliError::failure(format!(
                    "Oven Alpha cannot read project build authority at {}: {error}",
                    directory.display()
                ))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                CliError::failure(format!(
                    "Oven Alpha cannot read project build authority at {}: {error}",
                    directory.display()
                ))
            })?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                CliError::failure(format!(
                    "Oven Alpha cannot read project build authority at {}: {error}",
                    path.display()
                ))
            })?;
            if metadata.file_type().is_symlink() {
                return Err(CliError::failure(format!(
                    "Oven Alpha project build authority does not allow symlinks: {}",
                    path.display()
                )));
            }
            if metadata.is_dir() {
                // Tool-owned output directories are never part of the build authority. A project whose
                // source root is the project root itself (no dedicated `src/`) would otherwise scan its
                // own `.incan`/`target` output back into the digest, making an explicit bake refuse to
                // publish because the authority it just computed changed while writing that same output.
                if matches!(
                    path.file_name().and_then(|name| name.to_str()),
                    Some(".git" | ".incan" | ".ralph-cache" | "target")
                ) {
                    continue;
                }
                collect_directory(root, &path, records, already_recorded)?;
            } else if !already_recorded.contains(&path) {
                append_file(root, &path, records)?;
            }
        }
        Ok(())
    }

    let mut records = BTreeMap::new();
    let mut already_recorded = HashSet::new();
    let manifest_path = project_root.join(LOAF_MANIFEST_FILENAME);
    append_file(project_root, &manifest_path, &mut records)?;
    already_recorded.insert(manifest_path);
    let lockfile = canonical_baked_project_lock_path(project_root)?;
    if lockfile.is_file() {
        if let Some(authority) = digest_baked_project_lock_authority(&lockfile, project_root)? {
            records.insert(LOCK_FILENAME.to_string(), authority);
        }
        already_recorded.insert(lockfile);
    }
    let source_root = resolve_source_root(project_root, Some(manifest));
    let source_metadata = fs::symlink_metadata(&source_root).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha project build authority requires {}: {error}",
            source_root.display()
        ))
    })?;
    if source_metadata.file_type().is_symlink() || !source_metadata.is_dir() {
        return Err(CliError::failure(format!(
            "Oven Alpha project build authority requires a regular source directory at {}",
            source_root.display()
        )));
    }
    let canonical_project_root = fs::canonicalize(project_root).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot resolve project build authority root {}: {error}",
            project_root.display()
        ))
    })?;
    let canonical_source_root = fs::canonicalize(&source_root).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot resolve project source authority at {}: {error}",
            source_root.display()
        ))
    })?;
    if canonical_source_root.starts_with(&canonical_project_root) {
        collect_directory(project_root, &source_root, &mut records, &already_recorded)?;
    } else {
        records.insert(
            "configured-source-root".to_string(),
            digest_project_source_tree(&canonical_source_root).map_err(|error| {
                CliError::failure(format!(
                    "Oven Alpha cannot digest configured source authority at {}: {error}",
                    canonical_source_root.display()
                ))
            })?,
        );
    }

    for (relative, entrypoint) in discover_oven_executable_entrypoints(manifest)? {
        let canonical_entrypoint = fs::canonicalize(&entrypoint).map_err(|error| {
            CliError::failure(format!(
                "Oven Alpha cannot resolve declared executable source authority at {}: {error}",
                entrypoint.display()
            ))
        })?;
        if canonical_entrypoint.starts_with(&canonical_source_root) {
            continue;
        }
        if !canonical_entrypoint.starts_with(&canonical_project_root) {
            return Err(CliError::failure(format!(
                "Oven Alpha declared executable source authority escaped project root {}: {}",
                project_root.display(),
                entrypoint.display()
            )));
        }
        append_named_file(&entrypoint, format!("declared-executable:{relative}"), &mut records)?;
    }

    if let Some(configured_path) = manifest.vocab.as_ref().and_then(|vocab| vocab.crate_path.as_deref()) {
        let companion_root = if Path::new(configured_path).is_absolute() {
            PathBuf::from(configured_path)
        } else {
            project_root.join(configured_path)
        };
        let companion_root = fs::canonicalize(&companion_root).map_err(|error| {
            CliError::failure(format!(
                "Oven Alpha cannot resolve vocabulary companion source authority at {}: {error}",
                companion_root.display()
            ))
        })?;
        let companion_digest = digest_project_source_tree(&companion_root).map_err(|error| {
            CliError::failure(format!(
                "Oven Alpha cannot digest vocabulary companion source authority at {}: {error}",
                companion_root.display()
            ))
        })?;
        records.insert("vocab-companion".to_string(), companion_digest);
    }

    for (index, configured_path) in manifest.contract_model_bundle_paths().iter().enumerate() {
        let path = if Path::new(configured_path).is_absolute() {
            PathBuf::from(configured_path)
        } else {
            project_root.join(configured_path)
        };
        append_named_file(&path, format!("contract-model-bundle:{index}"), &mut records)?;
    }

    let locked_interop_targets = locked_oven_interop_targets(manifest).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot lock declared interop inputs for project source authority: {error}"
        ))
    })?;
    for target in locked_interop_targets {
        let locked_payload = serde_json::to_vec(&target).map_err(|error| {
            CliError::failure(format!(
                "failed to serialize locked Oven interop target `{}`: {error}",
                target.target
            ))
        })?;
        records.insert(
            format!("oven-interop-locked-target:{}", target.target),
            digest_bytes(&locked_payload),
        );

        let receipt_path = default_interop_execution_receipt_path(project_root, &target.target);
        let receipt_metadata = match fs::symlink_metadata(&receipt_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(CliError::failure(format!(
                    "Oven Alpha cannot inspect selected interop receipt {}: {error}",
                    receipt_path.display()
                )));
            }
        };
        if receipt_metadata.file_type().is_symlink() || !receipt_metadata.is_file() {
            return Err(CliError::failure(format!(
                "Oven Alpha selected interop receipt must be a regular file: {}",
                receipt_path.display()
            )));
        }
        let receipt = load_interop_execution_receipt(&receipt_path).map_err(CliError::failure)?;
        validate_interop_execution_receipt(&target, &receipt).map_err(|error| {
            CliError::failure(format!(
                "Oven Alpha selected interop receipt {} is stale: {error}",
                receipt_path.display()
            ))
        })?;
        records.insert(
            format!("oven-interop-execution-receipt:{}", target.target),
            receipt.identity,
        );
    }

    let payload = serde_json::to_vec(&records)
        .map_err(|error| CliError::failure(format!("failed to serialize Oven project build authority: {error}")))?;
    Ok(digest_bytes(&payload))
}

/// Return the durable receipt path for one explicitly baked target/profile.
///
/// Conventional library and main targets retain their historical paths. Other scripts use the complete digest of
/// their portable target identity, so two declared executables can never overwrite one another's lineage.
pub fn project_bake_receipt_path(
    project_root: &Path,
    target: OvenBakeProjectTarget,
    entrypoint: &Path,
    profile: &str,
) -> CliResult<PathBuf> {
    let target_identity = oven_bake_project_target_identity(project_root, target, entrypoint)?;
    let file_name = if target_identity == target.as_str() {
        format!("{}-{profile}-receipt.json", target.as_str())
    } else {
        format!(
            "executable-{}-{profile}-receipt.json",
            digest_bytes(target_identity.as_bytes()).trim_start_matches("sha256:")
        )
    };
    Ok(oven_store::default_receipt_path(project_root).with_file_name(file_name))
}

/// Return the private pre-interop receipt path for one Rust target, entrypoint, and profile.
///
/// This never shares the ordinary explicit-bake receipt namespace: the automatic bootstrap deliberately excludes
/// publisher-only development dependencies and is valid only as the base later extended by a selected native plan.
pub fn interop_bootstrap_receipt_path(
    project_root: &Path,
    rust_target: &str,
    target: OvenBakeProjectTarget,
    entrypoint: &Path,
    profile: &str,
) -> CliResult<PathBuf> {
    let target_identity = oven_bake_project_target_identity(project_root, target, entrypoint)?;
    let identity = digest_bytes(format!("{rust_target}\0{target_identity}").as_bytes())
        .trim_start_matches("sha256:")
        .to_string();
    Ok(oven_store::default_receipt_path(project_root)
        .with_file_name(format!("interop-bootstrap-{identity}-{profile}-receipt.json")))
}

/// Return the durable receipt path appropriate to one prepared Oven command mode.
pub fn prepared_oven_receipt_path(
    project_root: &Path,
    mode: OvenProjectPlanMode,
    rust_target: &str,
    entrypoint: &Path,
    profile: &str,
) -> CliResult<PathBuf> {
    if mode == OvenProjectPlanMode::InteropBootstrap {
        interop_bootstrap_receipt_path(
            project_root,
            rust_target,
            OvenBakeProjectTarget::Executable,
            entrypoint,
            profile,
        )
    } else {
        Ok(oven_store::default_receipt_path(project_root))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::test_support::{fixture_project_output_publication, fixture_project_output_publication_for};
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;

    use crate::build::bake::discover_oven_bake_project_targets;
    use crate::build::library_outputs::packaged_library_loaf_store_root;
    use crate::build::output_materialization::{
        select_coherent_library_outputs, warn_for_completed_output_lock_fingerprint_drift,
    };
    use crate::build::output_paths::packaged_library_metadata_files;
    use crate::build::output_selection::{
        matching_baked_project_outputs_with_source_authority, select_baked_project_output,
        select_current_debug_project_outputs,
    };
    use crate::build::package_loafs::write_packaged_library_loaf_manifest;
    use crate::build::plan_authority::read_verified_caller_owned_provider_receipt;
    use crate::build::publication::publish_project_output_loaf;
    use crate::build::reuse::try_reuse_baked_project;
    use crate::build::{
        OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION, OvenBakeProjectTarget, OvenPackagedLibraryLoafManifest,
        OvenPackagedLibraryLoafProfile, OvenProjectBakeAuthorityContext,
    };
    use incan_core::version::INCAN_VERSION;
    use incan_frontend::library_manifest::LibraryManifest;
    use incan_frontend::library_manifest::published_layout::{
        oven_library_dependency_declares_package_loaf, packaged_library_loaf_manifest_path,
    };
    use incan_provider::FeatureSelection;
    use oven_model::lock::{
        CargoFeatureSelection, IncanLock, LockedProvider, LockedSdkComponent, LockedSdkState, SemanticLockState,
    };
    use oven_model::manifest::{LOAF_MANIFEST_FILENAME, ProjectManifest};
    use oven_model::oven_interop::locked_oven_interop_targets;
    use oven_rustc::interop::{
        OvenInteropCapabilitySelection, default_interop_execution_receipt_path, receipt_interop_execution,
        write_interop_execution_receipt,
    };
    use oven_rustc::plan::OvenPackagedLibraryLoafEntry;
    use oven_rustc::rustc::{
        OvenProjectInspectionAuthorityRef, resolve_active_rustc, rustc_host_target, rustc_identity,
    };
    use oven_store::store::{OvenArtifactKind, OvenArtifactPublishRequest, OvenStore};
    use oven_store::{OvenGeneratedProjectRequest, digest_bytes, receipt_generated_project, write_receipt};

    #[test]
    fn caller_owned_provider_authority_prefers_the_explicit_library_receipt() -> Result<(), Box<dyn std::error::Error>>
    {
        let project = tempfile::tempdir()?;
        let library_source = project.path().join("generated/library.rs");
        let script_source = project.path().join("generated/checker.rs");
        fs::create_dir_all(library_source.parent().ok_or("library source has no parent")?)?;
        fs::write(&library_source, "pub fn library() {}\n")?;
        fs::write(&script_source, "fn main() {}\n")?;

        let library_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "provider",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc fixture",
                "release",
                Vec::new(),
            )
            .with_generated_source("generated-root", &library_source),
        )?;
        let script_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "provider",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc fixture",
                "release",
                Vec::new(),
            )
            .with_generated_source("generated-root", &script_source),
        )?;
        assert_ne!(library_receipt.identity, script_receipt.identity);

        write_receipt(
            &library_receipt,
            project_bake_receipt_path(
                project.path(),
                OvenBakeProjectTarget::Library,
                &project.path().join("src/lib.incn"),
                "release",
            )?,
        )?;
        write_receipt(&script_receipt, oven_store::default_receipt_path(project.path()))?;

        let selected = read_verified_caller_owned_provider_receipt(project.path(), "release")
            .ok_or("library receipt should be selected")?;
        assert_eq!(selected.identity, library_receipt.identity);
        Ok(())
    }

    #[test]
    fn baked_project_source_authority_tracks_transitive_path_source_not_its_output()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let provider = project.path().join("provider");
        fs::create_dir_all(project.path().join("src"))?;
        fs::create_dir_all(provider.join("src"))?;
        fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nprovider = { path = \"provider\" }\n",
        )?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        fs::write(provider.join("loaf.toml"), "[project]\nname = \"provider\"\n")?;
        fs::write(provider.join("src/lib.incn"), "pub def value() -> int:\n    return 1\n")?;
        let initial = digest_baked_project_source_authority(project.path())?;

        fs::create_dir_all(provider.join(".ralph-cache/loafs"))?;
        fs::write(provider.join(".ralph-cache/loafs/mutable"), "not source authority")?;
        assert_eq!(initial, digest_baked_project_source_authority(project.path())?);

        fs::create_dir_all(project.path().join("docs"))?;
        fs::write(
            project.path().join("docs/architecture.png"),
            "documentation is not a build input",
        )?;
        assert_eq!(initial, digest_baked_project_source_authority(project.path())?);

        fs::write(provider.join("src/lib.incn"), "pub def value() -> int:\n    return 2\n")?;
        assert_ne!(initial, digest_baked_project_source_authority(project.path())?);
        Ok(())
    }

    #[test]
    fn baked_project_source_authority_tracks_rust_path_crate_without_an_incan_manifest()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let rust_workspace = project.path().join("rust-workspace");
        let rust_crate = rust_workspace.join("rust-helper");
        let rust_leaf = rust_workspace.join("rust-leaf");
        fs::create_dir_all(project.path().join("src"))?;
        fs::create_dir_all(rust_crate.join("src"))?;
        fs::create_dir_all(rust_leaf.join("src"))?;
        fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname = \"consumer\"\n\n[rust-dependencies.rust_helper]\npath = \"rust-workspace/rust-helper\"\n",
        )?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        fs::write(
            rust_workspace.join("Cargo.toml"),
            "[workspace]\nmembers = [\"rust-helper\", \"rust-leaf\"]\nresolver = \"2\"\n\n[workspace.package]\nversion = \"0.1.0\"\n\n[workspace.dependencies]\nitoa = \"1\"\n",
        )?;
        fs::write(
            rust_crate.join("Cargo.toml"),
            "[package]\nname = \"rust_helper\"\nversion.workspace = true\nedition = \"2024\"\n\n[dependencies]\nitoa.workspace = true\nrust_leaf = { path = \"../rust-leaf\" }\n",
        )?;
        fs::write(rust_crate.join("src/lib.rs"), "pub fn value() -> i64 { 1 }\n")?;
        fs::write(
            rust_leaf.join("Cargo.toml"),
            "[package]\nname = \"rust_leaf\"\nversion.workspace = true\nedition = \"2024\"\n",
        )?;
        fs::write(rust_leaf.join("src/lib.rs"), "pub fn leaf() -> i64 { 1 }\n")?;

        let initial = digest_baked_project_source_authority(project.path())?;
        fs::create_dir_all(rust_crate.join("target/debug"))?;
        fs::write(rust_crate.join("target/debug/libhelper.rlib"), "mutable output")?;
        assert_eq!(initial, digest_baked_project_source_authority(project.path())?);

        fs::write(
            rust_workspace.join("Cargo.toml"),
            "[workspace]\nmembers = [\"rust-helper\", \"rust-leaf\"]\nresolver = \"2\"\n\n[workspace.package]\nversion = \"0.2.0\"\n\n[workspace.dependencies]\nitoa = \"1\"\n",
        )?;
        let inherited_workspace_changed = digest_baked_project_source_authority(project.path())?;
        assert_ne!(initial, inherited_workspace_changed);

        fs::write(rust_crate.join("src/lib.rs"), "pub fn value() -> i64 { 2 }\n")?;
        assert_ne!(
            inherited_workspace_changed,
            digest_baked_project_source_authority(project.path())?
        );
        let direct_source_changed = digest_baked_project_source_authority(project.path())?;
        fs::write(rust_leaf.join("src/lib.rs"), "pub fn leaf() -> i64 { 2 }\n")?;
        assert_ne!(
            direct_source_changed,
            digest_baked_project_source_authority(project.path())?,
            "a transitive sibling Cargo path dependency must remain part of the project source authority"
        );
        Ok(())
    }

    #[test]
    fn baked_project_source_authority_binds_dependency_content_to_its_named_edge()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let left = project.path().join("deps/left");
        let right = project.path().join("deps/right");
        fs::create_dir_all(project.path().join("src"))?;
        fs::create_dir_all(left.join("src"))?;
        fs::create_dir_all(right.join("src"))?;
        fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nleft = { path = \"deps/left\" }\nright = { path = \"deps/right\" }\n",
        )?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let left_manifest = "[project]\nname = \"left_provider\"\n";
        let right_manifest = "[project]\nname = \"right_provider\"\n";
        let left_source = "pub def value() -> int:\n    return 1\n";
        let right_source = "pub def value() -> int:\n    return 2\n";
        fs::write(left.join("loaf.toml"), left_manifest)?;
        fs::write(left.join("src/lib.incn"), left_source)?;
        fs::write(right.join("loaf.toml"), right_manifest)?;
        fs::write(right.join("src/lib.incn"), right_source)?;
        let initial = digest_baked_project_source_authority(project.path())?;

        fs::write(left.join("loaf.toml"), right_manifest)?;
        fs::write(left.join("src/lib.incn"), right_source)?;
        fs::write(right.join("loaf.toml"), left_manifest)?;
        fs::write(right.join("src/lib.incn"), left_source)?;

        assert_ne!(
            initial,
            digest_baked_project_source_authority(project.path())?,
            "swapping the same reachable child trees between named dependency slots must invalidate the root"
        );
        Ok(())
    }

    #[test]
    fn baked_project_source_authority_tracks_every_declared_model_bundle_file() -> Result<(), Box<dyn std::error::Error>>
    {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("src"))?;
        fs::create_dir_all(project.path().join("contracts"))?;
        fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname = \"consumer\"\n\n[tool.incan.metadata]\nmodel-bundles = [\"contracts/a.json\", \"contracts/b.json\"]\n",
        )?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        fs::write(project.path().join("contracts/a.json"), "{\"model\":\"a-one\"}\n")?;
        fs::write(project.path().join("contracts/b.json"), "{\"model\":\"b-one\"}\n")?;
        let initial = digest_baked_project_source_authority(project.path())?;

        fs::write(project.path().join("contracts/b.json"), "{\"model\":\"b-two\"}\n")?;
        assert_ne!(initial, digest_baked_project_source_authority(project.path())?);
        fs::write(project.path().join("contracts/b.json"), "{\"model\":\"b-one\"}\n")?;
        fs::write(project.path().join("contracts/a.json"), "{\"model\":\"a-two\"}\n")?;
        assert_ne!(initial, digest_baked_project_source_authority(project.path())?);
        Ok(())
    }

    #[test]
    fn baked_project_source_authority_tracks_the_declared_vocab_companion() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("src"))?;
        fs::create_dir_all(project.path().join("vocab_companion/src"))?;
        fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname = \"consumer\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        fs::write(
            project.path().join("src/lib.incn"),
            "pub def value() -> int:\n    return 1\n",
        )?;
        fs::write(
            project.path().join("vocab_companion/Cargo.toml"),
            "[package]\nname = \"fixture_vocab\"\nversion = \"0.1.0\"\n",
        )?;
        let companion_source = project.path().join("vocab_companion/src/lib.rs");
        fs::write(&companion_source, "pub fn library_vocab() { }\n")?;
        let initial = digest_baked_project_source_authority(project.path())?;

        fs::write(&companion_source, "pub fn library_vocab() { let _changed = true; }\n")?;
        assert_ne!(
            initial,
            digest_baked_project_source_authority(project.path())?,
            "a changed declared vocabulary companion must invalidate its completed provider output"
        );
        Ok(())
    }

    #[test]
    fn baked_project_source_authority_uses_the_configured_source_root() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("src"))?;
        fs::create_dir_all(project.path().join("library"))?;
        fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname = \"consumer\"\n\n[build]\nsource-root = \"library\"\n",
        )?;
        fs::write(project.path().join("src/ignored.incn"), "pub const IGNORED: int = 1\n")?;
        let configured_source = project.path().join("library/lib.incn");
        fs::write(&configured_source, "pub def value() -> int:\n    return 1\n")?;
        let initial = digest_baked_project_source_authority(project.path())?;

        fs::write(project.path().join("src/ignored.incn"), "pub const IGNORED: int = 2\n")?;
        assert_eq!(
            initial,
            digest_baked_project_source_authority(project.path())?,
            "the conventional src directory must not become authority when the manifest selects another source root"
        );
        fs::write(&configured_source, "pub def value() -> int:\n    return 2\n")?;
        assert_ne!(
            initial,
            digest_baked_project_source_authority(project.path())?,
            "a change below the configured source root must invalidate the completed project output"
        );
        Ok(())
    }

    #[test]
    fn baked_project_source_authority_tracks_declared_scripts_outside_the_source_root()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir(project.path().join("library"))?;
        fs::create_dir(project.path().join("scripts"))?;
        fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname = \"consumer\"\n\n[project.scripts]\ncli = \"scripts/cli.incn\"\ncli_alias = \"scripts/cli.incn\"\n\n[build]\nsource-root = \"library\"\n",
        )?;
        fs::write(
            project.path().join("library/lib.incn"),
            "pub def value() -> int:\n    return 1\n",
        )?;
        let script = project.path().join("scripts/cli.incn");
        fs::write(&script, "def main() -> None:\n    println(1)\n")?;
        let initial = digest_baked_project_source_authority(project.path())?;

        fs::write(&script, "def main() -> None:\n    println(2)\n")?;

        assert_ne!(
            initial,
            digest_baked_project_source_authority(project.path())?,
            "a declared executable outside the configured source root must invalidate completed outputs"
        );
        Ok(())
    }

    #[test]
    fn baked_project_source_authority_tracks_fresh_interop_inputs_and_selected_receipt()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("src"))?;
        fs::create_dir_all(project.path().join("interop/include"))?;
        fs::write(
            project.path().join("loaf.toml"),
            r#"[project]
name = "consumer"

[interop.c]
schema = 1

[[interop.c.targets]]
target = "aarch64-apple-darwin"
toolchain = { capability = "apple-clang", version = ">=17, <19" }
headers = ["interop/include/bridge.h"]
"#,
        )?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let header = project.path().join("interop/include/bridge.h");
        fs::write(&header, "int incan_bridge(void);\n")?;
        let manifest = ProjectManifest::load(&project.path().join("loaf.toml"))?;
        let locked = locked_oven_interop_targets(&manifest)?;
        let target = locked.first().ok_or("expected one locked interop target")?;
        let first_receipt = receipt_interop_execution(
            target,
            Some(OvenInteropCapabilitySelection {
                capability: "apple-clang".to_string(),
                version: "17.0.6".to_string(),
                identity: "sha256:clang-17".to_string(),
            }),
            None,
        )?;
        let receipt_path = default_interop_execution_receipt_path(project.path(), &target.target);
        write_interop_execution_receipt(&first_receipt, &receipt_path)?;
        let initial = digest_baked_project_source_authority(project.path())?;

        let second_receipt = receipt_interop_execution(
            target,
            Some(OvenInteropCapabilitySelection {
                capability: "apple-clang".to_string(),
                version: "18.0.0".to_string(),
                identity: "sha256:clang-18".to_string(),
            }),
            None,
        )?;
        write_interop_execution_receipt(&second_receipt, &receipt_path)?;
        assert_ne!(
            initial,
            digest_baked_project_source_authority(project.path())?,
            "a different valid selected execution receipt must invalidate a completed output"
        );

        write_interop_execution_receipt(&first_receipt, &receipt_path)?;
        fs::write(&header, "int incan_bridge_changed(void);\n")?;
        assert!(
            digest_baked_project_source_authority(project.path()).is_err(),
            "fresh interop input identities must reject the now-stale selected receipt"
        );
        let changed_manifest = ProjectManifest::load(&project.path().join("loaf.toml"))?;
        let changed_locked = locked_oven_interop_targets(&changed_manifest)?;
        let changed_target = changed_locked
            .first()
            .ok_or("expected one changed locked interop target")?;
        let changed_receipt = receipt_interop_execution(
            changed_target,
            Some(OvenInteropCapabilitySelection {
                capability: "apple-clang".to_string(),
                version: "17.0.6".to_string(),
                identity: "sha256:clang-17".to_string(),
            }),
            None,
        )?;
        write_interop_execution_receipt(&changed_receipt, &receipt_path)?;
        assert_ne!(
            initial,
            digest_baked_project_source_authority(project.path())?,
            "a declared interop header mutation must invalidate a completed output after reselection"
        );
        Ok(())
    }

    #[test]
    fn baked_workspace_member_source_authority_uses_only_the_canonical_root_lock()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let member = workspace.path().join("member");
        let provider = workspace.path().join("provider");
        let rust_helper = workspace.path().join("rust-helper");
        fs::create_dir_all(member.join("src"))?;
        fs::create_dir_all(provider.join("src"))?;
        fs::create_dir_all(rust_helper.join("src"))?;
        fs::write(
            workspace.path().join("loaf.toml"),
            "[workspace]\nmembers = [\"member\"]\n\n[workspace.dependencies]\nprovider = { path = \"provider\" }\n\n[workspace.rust-dependencies]\nrust_helper = { path = \"rust-helper\" }\n",
        )?;
        fs::write(
            member.join("loaf.toml"),
            "[project]\nname = \"member\"\nversion = \"0.1.0\"\n\n[project.scripts]\nmain = \"src/main.incn\"\n\n[dependencies]\nprovider = { workspace = true }\n\n[rust-dependencies]\nrust_helper = { workspace = true }\n",
        )?;
        fs::write(member.join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        fs::write(provider.join("loaf.toml"), "[project]\nname = \"provider\"\n")?;
        fs::write(provider.join("src/lib.incn"), "pub def value() -> int:\n    return 1\n")?;
        fs::write(
            rust_helper.join("Cargo.toml"),
            "[package]\nname = \"rust_helper\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(rust_helper.join("src/lib.rs"), "pub fn value() -> i64 { 1 }\n")?;
        IncanLock::new(
            incan_core::version::INCAN_VERSION,
            "sha256:canonical-one".to_string(),
            CargoFeatureSelection::default(),
            "version = 4\n".to_string(),
        )
        .write(&workspace.path().join("oven.lock"))?;
        fs::write(member.join("oven.lock"), "obsolete-member-lock-one\n")?;

        let initial = digest_baked_project_source_authority(&member)?;
        fs::write(member.join("oven.lock"), "obsolete-member-lock-two\n")?;
        assert_eq!(
            initial,
            digest_baked_project_source_authority(&member)?,
            "a workspace member must ignore a non-authoritative member-local lock"
        );

        IncanLock::new(
            incan_core::version::INCAN_VERSION,
            "sha256:canonical-two".to_string(),
            CargoFeatureSelection::default(),
            "version = 4\n".to_string(),
        )
        .write(&workspace.path().join("oven.lock"))?;
        assert_eq!(
            initial,
            digest_baked_project_source_authority(&member)?,
            "a derived dependency fingerprint must not replace the canonical semantic lock authority"
        );

        IncanLock::new(
            incan_core::version::INCAN_VERSION,
            "sha256:canonical-two".to_string(),
            CargoFeatureSelection::default(),
            "version = 4\n\n[[package]]\nname = \"changed\"\nversion = \"1.0.0\"\n".to_string(),
        )
        .write(&workspace.path().join("oven.lock"))?;
        assert_ne!(
            initial,
            digest_baked_project_source_authority(&member)?,
            "a canonical lock payload change must invalidate completed project authority"
        );

        let changed_lock = digest_baked_project_source_authority(&member)?;
        fs::write(provider.join("src/lib.incn"), "pub def value() -> int:\n    return 2\n")?;
        assert_ne!(
            changed_lock,
            digest_baked_project_source_authority(&member)?,
            "workspace-inherited Incan provider source must remain part of completed-output authority"
        );
        fs::write(provider.join("src/lib.incn"), "pub def value() -> int:\n    return 1\n")?;
        fs::write(rust_helper.join("src/lib.rs"), "pub fn value() -> i64 { 2 }\n")?;
        assert_ne!(
            changed_lock,
            digest_baked_project_source_authority(&member)?,
            "workspace-inherited Rust path source must remain part of completed-output authority"
        );
        Ok(())
    }

    #[test]
    fn baked_project_source_authority_ignores_lock_format_migration_issue1194() -> Result<(), Box<dyn std::error::Error>>
    {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("src"))?;
        fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname = \"lock_migration_fixture\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let lock_path = project.path().join("oven.lock");
        let mut lock = IncanLock::new(
            incan_core::version::INCAN_VERSION,
            "sha256:lock-migration".to_string(),
            CargoFeatureSelection::default(),
            "version = 4\n".to_string(),
        );
        lock.format = 1;
        lock.write(&lock_path)?;
        let format_one = digest_baked_project_source_authority(project.path())?;

        lock.format = 2;
        lock.incan_version = "0.5.1-rc2".to_string();
        lock.write(&lock_path)?;
        assert_eq!(
            format_one,
            digest_baked_project_source_authority(project.path())?,
            "a structural or compiler-cohort lock refresh must not invalidate unchanged authored project input"
        );
        Ok(())
    }

    #[test]
    fn baked_project_source_authority_ignores_sdk_cohort_refresh_issue1194() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("src"))?;
        fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname = \"sdk_cohort_fixture\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let lock_path = project.path().join("oven.lock");
        let mut lock = IncanLock::new_with_semantic(
            incan_core::version::INCAN_VERSION,
            "sha256:sdk-cohort".to_string(),
            CargoFeatureSelection::default(),
            SemanticLockState {
                sdk: Some(LockedSdkState {
                    identity: "incan@0.5.0".to_string(),
                    inventory_digest: "sha256:old-sdk".to_string(),
                    profile: "default".to_string(),
                    components: vec![LockedSdkComponent {
                        id: "stdlib-core".to_string(),
                        version: "0.5.0".to_string(),
                        reason: "mandatory".to_string(),
                    }],
                }),
                providers: vec![
                    LockedProvider {
                        identity: "incan_stdlib_core@0.5.0#sha256:old[]".to_string(),
                        participation: "used".to_string(),
                        namespace_claims: BTreeSet::new(),
                        used_modules: BTreeSet::new(),
                        implementation_facets: Vec::new(),
                        backend_requirements: BTreeSet::new(),
                    },
                    LockedProvider {
                        identity: "example_provider@1.0.0#sha256:stable[]".to_string(),
                        participation: "used".to_string(),
                        namespace_claims: BTreeSet::new(),
                        used_modules: BTreeSet::new(),
                        implementation_facets: Vec::new(),
                        backend_requirements: BTreeSet::new(),
                    },
                ],
                ..SemanticLockState::default()
            },
            "version = 4\n".to_string(),
        );
        lock.write(&lock_path)?;
        let initial = digest_baked_project_source_authority(project.path())?;

        lock.incan_version = "0.5.1-rc2".to_string();
        let sdk = lock.semantic.sdk.as_mut().ok_or("fixture lost SDK state")?;
        sdk.identity = "incan@0.5.1-rc2".to_string();
        sdk.inventory_digest = "sha256:new-sdk".to_string();
        sdk.components[0].version = "0.5.1-rc2".to_string();
        lock.semantic.providers[0].identity = "incan_stdlib_core@0.5.1-rc2#sha256:new[]".to_string();
        lock.write(&lock_path)?;
        assert_eq!(
            initial,
            digest_baked_project_source_authority(project.path())?,
            "a compiler-owned SDK cohort refresh must not invalidate unchanged authored project input"
        );

        lock.semantic.providers[1].identity = "example_provider@1.0.0#sha256:changed[]".to_string();
        lock.write(&lock_path)?;
        assert_ne!(
            initial,
            digest_baked_project_source_authority(project.path())?,
            "a non-SDK provider selection must remain part of project authority"
        );
        Ok(())
    }

    #[test]
    fn completed_output_selection_prefers_current_receipt_then_lock_and_keeps_stale_fallback()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let store_dir = tempfile::tempdir()?;
        let store = OvenStore::new(
            store_dir.path(),
            oven_store::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        fs::create_dir(project.path().join("src"))?;
        fs::write(project.path().join("loaf.toml"), "[project]\nname = \"fixture\"\n")?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let lock_path = project.path().join("oven.lock");
        let write_lock = |fingerprint: &str| {
            IncanLock::new(
                incan_core::version::INCAN_VERSION,
                fingerprint.to_string(),
                CargoFeatureSelection::default(),
                "version = 4\n".to_string(),
            )
            .write(&lock_path)
        };
        write_lock("sha256:old")?;
        let (old_receipt, old_payload, old_files) =
            fixture_project_output_publication(project.path(), "release", "old")?;
        publish_project_output_loaf(&store, &old_receipt, &old_payload, &old_files)?;
        write_lock("sha256:current")?;
        let (receipt, payload, files) = fixture_project_output_publication(project.path(), "release", "current")?;
        publish_project_output_loaf(&store, &receipt, &payload, &files)?;
        let entrypoint = project.path().join("src/main.incn");
        let select = || {
            select_baked_project_output(
                &store,
                project.path(),
                &entrypoint,
                OvenBakeProjectTarget::Executable,
                "release",
            )
        };
        let selected = select()?.ok_or("missing output without local receipt")?;
        assert_eq!(
            selected.payload.lock_dependencies_fingerprint.as_deref(),
            Some("sha256:current")
        );
        let receipt_path = project_bake_receipt_path(
            project.path(),
            OvenBakeProjectTarget::Executable,
            &entrypoint,
            "release",
        )?;
        write_receipt(&receipt, &receipt_path)?;
        write_lock("sha256:old")?;
        let selected = select()?.ok_or("missing current lineage with stale lock")?;
        assert_eq!(selected.payload.receipt_identity, receipt.identity);
        warn_for_completed_output_lock_fingerprint_drift(project.path(), [&selected])?;
        Ok(())
    }

    #[test]
    fn completed_output_library_selection_keeps_one_cohort_across_profiles() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let store_dir = tempfile::tempdir()?;
        let store = OvenStore::new(
            store_dir.path(),
            oven_store::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        fs::create_dir(project.path().join("src"))?;
        fs::write(project.path().join("loaf.toml"), "[project]\nname = \"fixture\"\n")?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let mut current_receipts = Vec::new();
        for profile in ["debug", "release"] {
            for cohort in ["old", "current"] {
                let (receipt, mut payload, files) =
                    fixture_project_output_publication(project.path(), profile, cohort)?;
                payload.lock_dependencies_fingerprint = Some(format!("sha256:{cohort}"));
                publish_project_output_loaf(&store, &receipt, &payload, &files)?;
                if profile == "debug" && cohort == "current" {
                    current_receipts.push(receipt);
                }
            }
        }
        let (receipt, mut duplicate, files) =
            fixture_project_output_publication(project.path(), "release", "old_duplicate")?;
        duplicate.lock_dependencies_fingerprint = Some("sha256:old".to_string());
        publish_project_output_loaf(&store, &receipt, &duplicate, &files)?;
        let entrypoint = project.path().join("src/main.incn");
        let authority = digest_baked_project_source_authority(project.path())?;
        let mut groups = Vec::new();
        for profile in ["debug", "release"] {
            let mut candidates = matching_baked_project_outputs_with_source_authority(
                &store,
                project.path(),
                &entrypoint,
                OvenBakeProjectTarget::Executable,
                profile,
                &authority,
                None,
            )?;
            // Model opposing hash order across profiles; both profiles must still choose one cohort.
            candidates.sort_by_key(|output| {
                (output.payload.lock_dependencies_fingerprint.as_deref() == Some("sha256:current"))
                    == (profile == "debug")
            });
            groups.push(candidates);
        }
        let outputs = select_coherent_library_outputs(groups, &current_receipts, Some("sha256:old"))?
            .ok_or("missing coherent cohort")?;
        assert_eq!(outputs.len(), 2);
        assert_eq!(
            outputs[0].payload.lock_dependencies_fingerprint.as_deref(),
            Some("sha256:current"),
            "a current debug receipt outranks a stale lock even with duplicate stale release outputs and no local release receipt"
        );
        assert_eq!(
            outputs[0].payload.lock_dependencies_fingerprint,
            outputs[1].payload.lock_dependencies_fingerprint
        );
        let mut disjoint = Vec::new();
        for (profile, fingerprint) in [("debug", "sha256:old"), ("release", "sha256:current")] {
            let candidates = matching_baked_project_outputs_with_source_authority(
                &store,
                project.path(),
                &entrypoint,
                OvenBakeProjectTarget::Executable,
                profile,
                &authority,
                None,
            )?;
            disjoint.push(
                candidates
                    .into_iter()
                    .filter(|output| output.payload.lock_dependencies_fingerprint.as_deref() == Some(fingerprint))
                    .collect(),
            );
        }
        let Err(error) = select_coherent_library_outputs(disjoint, &[], None) else {
            return Err("disjoint profile cohorts must require an explicit bake".into());
        };
        assert!(error.message.contains("no coherent completed library output cohort"));
        Ok(())
    }

    #[test]
    fn current_debug_output_scan_selects_exact_lineage_and_rejects_a_stale_only_cohort()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir(project.path().join("src"))?;
        fs::write(project.path().join("loaf.toml"), "[project]\nname = \"fixture\"\n")?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let (receipt, payload, files) = fixture_project_output_publication(project.path(), "debug", "current")?;
        write_receipt(
            &receipt,
            project_bake_receipt_path(
                project.path(),
                OvenBakeProjectTarget::Executable,
                &project.path().join("src/main.incn"),
                "debug",
            )?,
        )?;
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(
            store_root.path(),
            oven_store::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        let _published = publish_project_output_loaf(&store, &receipt, &payload, &files)?;

        let generated_source = project.path().join("target/fixture/generated-current.rs");
        let unrelated_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "fixture",
                "0.1.0",
                receipt.intent.target.clone(),
                receipt.intent.toolchain.clone(),
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated_source)
            .with_build_unit_input("unrelated-lineage", "true"),
        )?;
        store.publish(&OvenArtifactPublishRequest {
            receipt: unrelated_receipt,
            domain: format!("incan-release-{INCAN_VERSION}"),
            kind: OvenArtifactKind::ProjectOutput,
            payload: b"malformed unrelated project output".to_vec(),
            materialized_files: Vec::new(),
        })?;
        let mut stale_payload = payload.clone();
        stale_payload.source_authority_digest = digest_bytes(b"stale authored source");
        let stale = publish_project_output_loaf(&store, &receipt, &stale_payload, &files)?;
        drop(stale);
        let targets = discover_oven_bake_project_targets(project.path())?;
        let selected = select_current_debug_project_outputs(
            &store,
            project.path(),
            &targets,
            &digest_baked_project_source_authority(project.path())?,
            &receipt.intent.target,
            &receipt.intent.toolchain,
        )?
        .ok_or("valid exact debug output was poisoned by an unrelated malformed payload")?;
        assert_eq!(selected.len(), 1);
        drop(selected);

        store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: format!("incan-release-{INCAN_VERSION}"),
            kind: OvenArtifactKind::ProjectOutput,
            payload: b"malformed exact project output".to_vec(),
            materialized_files: Vec::new(),
        })?;
        let selected = select_current_debug_project_outputs(
            &store,
            project.path(),
            &targets,
            &digest_baked_project_source_authority(project.path())?,
            &receipt.intent.target,
            &receipt.intent.toolchain,
        )?
        .ok_or("a stale or malformed sibling poisoned the valid exact output")?;
        assert_eq!(selected.len(), 1);
        drop(selected);

        let stale_only_root = tempfile::tempdir()?;
        let stale_only = OvenStore::new(
            stale_only_root.path(),
            oven_store::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        let stale = publish_project_output_loaf(&stale_only, &receipt, &stale_payload, &files)?;
        drop(stale);
        let stale_result = select_current_debug_project_outputs(
            &stale_only,
            project.path(),
            &targets,
            &digest_baked_project_source_authority(project.path())?,
            &receipt.intent.target,
            &receipt.intent.toolchain,
        );
        let Err(error) = stale_result else {
            return Err("a same-receipt cohort with no source-current output did not fail closed".into());
        };
        assert!(error.message.contains("disagrees with its source-current lineage"));
        Ok(())
    }

    #[test]
    fn current_debug_output_scan_prefers_the_authority_every_target_shares() -> Result<(), Box<dyn std::error::Error>> {
        // A store restored from a cache can hold two exact generations of every output for unchanged sources, each
        // naming the project inspection authority its own bake sealed. Picking per target by identity alone can mix
        // the generations; the scan must pick the one authority every target can satisfy.
        let project = tempfile::tempdir()?;
        fs::create_dir(project.path().join("src"))?;
        fs::write(project.path().join("loaf.toml"), "[project]\nname = \"fixture\"\n")?;
        fs::write(project.path().join("src/lib.incn"), "def helper() -> None:\n    pass\n")?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let authority = |identity: &str| OvenProjectInspectionAuthorityRef {
            identity: identity.to_string(),
            receipt_identity: format!("{identity}-receipt"),
            build_unit_identity: format!("{identity}-build-unit"),
        };
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(
            store_root.path(),
            oven_store::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        let library_entrypoint = project
            .path()
            .join(OvenBakeProjectTarget::Library.source_relative_path());
        let mut library_generations = Vec::new();
        for identity in ["sha256:authority-a", "sha256:authority-b"] {
            let (receipt, payload, files) = fixture_project_output_publication_for(
                project.path(),
                "debug",
                "library",
                OvenBakeProjectTarget::Library,
                authority(identity),
            )?;
            write_receipt(
                &receipt,
                project_bake_receipt_path(
                    project.path(),
                    OvenBakeProjectTarget::Library,
                    &library_entrypoint,
                    "debug",
                )?,
            )?;
            let published = publish_project_output_loaf(&store, &receipt, &payload, &files)?;
            library_generations.push((published.identity.clone(), identity));
            drop(published);
        }
        assert_ne!(
            library_generations[0].0, library_generations[1].0,
            "two generations with different authorities must publish as distinct Loafs"
        );
        // An identity-ordered pick would take the smallest library output; give the executable only the generation
        // that names the other authority, so the targets agree only when the scan prefers the shared lineage.
        library_generations.sort();
        let shared = library_generations
            .last()
            .map(|(_, identity)| *identity)
            .ok_or("no library generation was published")?;
        let executable_entrypoint = project
            .path()
            .join(OvenBakeProjectTarget::Executable.source_relative_path());
        let (receipt, payload, files) = fixture_project_output_publication_for(
            project.path(),
            "debug",
            "executable",
            OvenBakeProjectTarget::Executable,
            authority(shared),
        )?;
        write_receipt(
            &receipt,
            project_bake_receipt_path(
                project.path(),
                OvenBakeProjectTarget::Executable,
                &executable_entrypoint,
                "debug",
            )?,
        )?;
        drop(publish_project_output_loaf(&store, &receipt, &payload, &files)?);

        let targets = discover_oven_bake_project_targets(project.path())?;
        let selected = select_current_debug_project_outputs(
            &store,
            project.path(),
            &targets,
            &digest_baked_project_source_authority(project.path())?,
            &receipt.intent.target,
            &receipt.intent.toolchain,
        )?
        .ok_or("the two-target project with exact outputs was not selected")?;
        assert_eq!(selected.len(), 2);
        for (target, output) in &selected {
            assert_eq!(
                output
                    .payload
                    .inspection_authority
                    .as_ref()
                    .map(|authority| authority.identity.as_str()),
                Some(shared),
                "{} output must name the authority every target shares",
                target.as_str()
            );
        }
        Ok(())
    }

    #[test]
    fn baked_library_reuse_requires_completed_project_outputs_beside_package_loafs()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let artifact_root = project.path().join("target/lib");
        let generated_source = artifact_root.join("src/lib.rs");
        fs::create_dir_all(project.path().join("src"))?;
        fs::create_dir_all(generated_source.parent().ok_or("generated source has no parent")?)?;
        fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(
            project.path().join("src/lib.incn"),
            "pub def value() -> int:\n    return 1\n",
        )?;
        fs::write(&generated_source, "pub fn value() -> i32 { 1 }\n")?;
        let library_manifest = LibraryManifest::new("fixture", "0.1.0");
        let library_manifest_path = artifact_root.join("fixture.incnlib");
        library_manifest.write_to_path(&library_manifest_path)?;

        let rustc = resolve_active_rustc()?;
        let target = rustc_host_target(&rustc)?;
        let toolchain = rustc_identity(&rustc)?;
        let limits = oven_store::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024);
        let package_store = OvenStore::new(packaged_library_loaf_store_root(&artifact_root), limits);
        let mut profiles = BTreeMap::new();
        for profile in ["debug", "release"] {
            let receipt = receipt_generated_project(
                &OvenGeneratedProjectRequest::new(
                    &artifact_root,
                    "fixture",
                    "0.1.0",
                    target.clone(),
                    toolchain.clone(),
                    profile,
                    Vec::new(),
                )
                .with_generated_source("generated-root", &generated_source)
                .with_build_unit_input("compiler-version", INCAN_VERSION),
            )?;
            let stored = package_store.publish(&OvenArtifactPublishRequest {
                receipt: receipt.clone(),
                domain: "fixture".to_string(),
                kind: OvenArtifactKind::ProjectPayload,
                payload: format!("fixture-{profile}-payload").into_bytes(),
                materialized_files: Vec::new(),
            })?;
            let output = artifact_root.join(format!("oven/{profile}/libfixture.rlib"));
            fs::create_dir_all(output.parent().ok_or("library output has no parent")?)?;
            fs::write(&output, format!("fixture-{profile}-library"))?;
            profiles.insert(
                profile.to_string(),
                OvenPackagedLibraryLoafProfile {
                    receipt: receipt.clone(),
                    entries: vec![OvenPackagedLibraryLoafEntry {
                        receipt,
                        identity: stored.identity,
                        kind: OvenArtifactKind::ProjectPayload,
                        base_loaf_identity: None,
                    }],
                    library_relative_path: format!("oven/{profile}/libfixture.rlib"),
                    library_digest: digest_bytes(&fs::read(output)?),
                },
            );
        }
        let manifest = OvenPackagedLibraryLoafManifest {
            schema_version: OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION,
            source_authority_digest: digest_baked_project_source_authority(project.path())?,
            compiler_version: INCAN_VERSION.to_string(),
            metadata_files: packaged_library_metadata_files(&library_manifest_path, &library_manifest, &artifact_root)?,
            profiles,
        };
        write_packaged_library_loaf_manifest(&artifact_root, &manifest)?;
        assert!(oven_library_dependency_declares_package_loaf(project.path()));
        let consumer_store = OvenStore::new(project.path().join("consumer-store"), limits);
        let targets = vec![(OvenBakeProjectTarget::Library, project.path().join("src/lib.incn"))];
        let mut authority_context = OvenProjectBakeAuthorityContext::default();

        assert!(
            try_reuse_baked_project(
                project.path(),
                &targets,
                &consumer_store,
                &FeatureSelection::default(),
                &mut authority_context,
            )?
            .is_none(),
            "a package Loaf alone cannot skip producing this project's completed outputs"
        );
        fs::remove_file(packaged_library_loaf_manifest_path(&artifact_root))?;
        assert!(!oven_library_dependency_declares_package_loaf(project.path()));
        Ok(())
    }

    #[test]
    fn oven_interop_bootstrap_receipt_isolated_from_explicit_bake_receipts() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let entrypoint = project.path().join("src/main.incn");
        let explicit =
            project_bake_receipt_path(project.path(), OvenBakeProjectTarget::Executable, &entrypoint, "debug")?;
        let bootstrap = interop_bootstrap_receipt_path(
            project.path(),
            "aarch64-apple-darwin",
            OvenBakeProjectTarget::Executable,
            &entrypoint,
            "debug",
        )?;

        assert_ne!(bootstrap, explicit);
        assert!(
            bootstrap
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("interop-bootstrap-") && name.ends_with("-debug-receipt.json")),
            "unexpected bootstrap receipt path: {}",
            bootstrap.display()
        );
        Ok(())
    }

    #[test]
    fn warm_project_reuse_rejects_missing_headers_before_deep_scan() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let entrypoint = project.path().join("src/main.incn");
        let generated_source = project.path().join("target/incan/fixture/src/main.rs");
        fs::create_dir_all(entrypoint.parent().ok_or("entrypoint has no parent")?)?;
        fs::create_dir_all(generated_source.parent().ok_or("generated source has no parent")?)?;
        fs::write(
            project.path().join(LOAF_MANIFEST_FILENAME),
            "[project]\nname = \"fixture\"\n",
        )?;
        fs::write(&entrypoint, "def main() -> None:\n    pass\n")?;
        fs::write(&generated_source, "fn main() {}\n")?;
        let rustc = resolve_active_rustc()?;
        let target = rustc_host_target(&rustc)?;
        let toolchain = rustc_identity(&rustc)?;
        let mut receipts = Vec::new();
        for profile in ["debug", "release"] {
            let receipt = receipt_generated_project(
                &OvenGeneratedProjectRequest::new(
                    project.path(),
                    "fixture",
                    "0.1.0",
                    target.clone(),
                    toolchain.clone(),
                    profile,
                    Vec::new(),
                )
                .with_generated_source("generated-root", &generated_source),
            )?;
            write_receipt(
                &receipt,
                project_bake_receipt_path(project.path(), OvenBakeProjectTarget::Executable, &entrypoint, profile)?,
            )?;
            receipts.push(receipt);
        }
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(
            store_root.path(),
            oven_store::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        let targets = vec![(OvenBakeProjectTarget::Executable, entrypoint)];
        let mut context = OvenProjectBakeAuthorityContext::default();
        assert!(
            try_reuse_baked_project(
                project.path(),
                &targets,
                &store,
                &FeatureSelection::default(),
                &mut context,
            )?
            .is_none()
        );
        assert_eq!(context.source_digester.project_scan_count(project.path()), 0);
        assert!(context.initial_project_source_authority.is_none());

        for receipt in receipts {
            store.publish(&OvenArtifactPublishRequest {
                receipt,
                domain: format!("incan-release-{INCAN_VERSION}"),
                kind: OvenArtifactKind::ProjectOutput,
                payload: b"malformed candidate after cheap header gate".to_vec(),
                materialized_files: Vec::new(),
            })?;
        }
        let mut context = OvenProjectBakeAuthorityContext::default();
        assert!(
            try_reuse_baked_project(
                project.path(),
                &targets,
                &store,
                &FeatureSelection::default(),
                &mut context,
            )?
            .is_none()
        );
        assert_eq!(context.source_digester.project_scan_count(project.path()), 0);
        assert!(
            context.initial_project_source_authority.is_none(),
            "a failed cache probe must not preserve pre-refresh authority for final publication"
        );
        Ok(())
    }
}

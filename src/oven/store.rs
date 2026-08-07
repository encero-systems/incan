//! Bounded, lease-aware storage for immutable Oven Alpha artifacts.
//!
//! This store is intentionally separate from generated Cargo targets. It owns versioned Oven artifacts only, reports
//! logical artifact bytes and measured physical file allocation separately, and refuses publication when its active
//! leases leave no safe way to satisfy capacity policy.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{OvenBuildIntent, OvenReceipt, digest_bytes};

/// Version of the persistent Oven artifact-store layout.
pub const OVEN_STORE_SCHEMA_VERSION: u32 = 3;
const ENTRIES_DIRECTORY: &str = "entries";
const STAGING_DIRECTORY: &str = "staging";
const MANIFEST_FILE: &str = "artifact.json";
const PAYLOAD_FILE: &str = "payload";
const MATERIALIZED_DIRECTORY: &str = "artifacts";
const ACCESS_FILE: &str = "last-used";
const ACTIVE_LOCK_FILE: &str = ".active.lock";
const MANAGER_LOCK_FILE: &str = ".manager.lock";
const LEGACY_CARGO_STAGING_DIRECTORY: &str = "legacy-cargo-staging";
const LEGACY_CARGO_PUBLISHER_LOCK_FILE: &str = ".publisher.lock";
const LEGACY_CARGO_STAGING_PREFIX: &str = ".legacy-cargo-";
static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Policy enforced before an Oven artifact becomes visible in the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenStoreLimits {
    /// Maximum measured physical file allocation retained by all published artifacts.
    pub max_physical_bytes: u64,
    /// Maximum measured physical file allocation retained by one compatibility domain.
    pub max_domain_physical_bytes: u64,
    /// Maximum logical artifact bytes retained by one compatibility domain.
    pub max_domain_logical_bytes: u64,
}

impl OvenStoreLimits {
    /// Construct an explicit capacity policy; zero values intentionally reject every non-empty artifact.
    #[must_use]
    pub const fn new(max_physical_bytes: u64, max_domain_physical_bytes: u64, max_domain_logical_bytes: u64) -> Self {
        Self {
            max_physical_bytes,
            max_domain_physical_bytes,
            max_domain_logical_bytes,
        }
    }
}

/// Semantic role of a payload stored by Oven Alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OvenArtifactKind {
    /// Publisher-prepared reusable engine or verified build unit.
    Engine,
    /// Project-specific plan, overlay, or small composition payload.
    ProjectPayload,
    /// Verified direct-rustc artifact plan consumed by a later executor stage.
    DirectRustcPlan,
    /// Publisher-prepared compiler test executable and matching CLI, executed later without Cargo.
    CompilerTestSuite,
    /// One independently admitted direct-rustc compiler-suite shard referenced by a small suite index.
    CompilerTestSuiteShard,
    /// One bounded compiler-test dependency foundation composed by receipt-bound root shards.
    CompilerTestSuiteFoundation,
    /// One independently policy-bounded compiler-Loaf data partition required by a stored suite child.
    CompilerTestSuiteToolchainData,
}

/// Content descriptor retained in a published Oven artifact manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenArtifactPayload {
    /// Content-derived payload digest.
    pub digest: String,
    /// Logical payload byte count, excluding manifests, locks, and filesystem allocation overhead.
    pub logical_bytes: u64,
}

/// One source file copied into the immutable, store-owned artifact payload.
#[derive(Debug, Clone)]
pub struct OvenArtifactMaterializedFile {
    /// Read-only source file selected and verified by the publisher before publication.
    pub source_path: PathBuf,
    /// Portable path below the entry's store-owned artifact root.
    pub relative_path: String,
}

/// Content descriptor for one file retained below a store-owned artifact root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenArtifactMaterializedFileManifest {
    /// Safe path below the entry's immutable artifact root.
    pub relative_path: String,
    /// Content-derived file digest.
    pub digest: String,
    /// Logical content byte count for this materialized file.
    pub logical_bytes: u64,
    /// Whether this regular artifact file is intended to be directly executable.
    ///
    /// This is identity-bearing because a native test or CLI artifact with the same bytes but no execute
    /// permission is not executable on Unix hosts.
    #[serde(default)]
    pub executable: bool,
}

/// Immutable manifest for one published Oven artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenArtifactManifest {
    /// Persistent store schema version.
    pub schema_version: u32,
    /// Content-addressed artifact identity.
    pub identity: String,
    /// Receipt identity that authorized this artifact.
    pub receipt_identity: String,
    /// Reusable compiler/SDK/provider identity that authorizes selection across compatible project receipts.
    pub build_unit_identity: String,
    /// Compatibility domain used for capacity policy and selection.
    pub domain: String,
    /// Semantic artifact role.
    pub kind: OvenArtifactKind,
    /// Target/toolchain/profile/feature intent inherited from the authorizing receipt.
    pub intent: OvenBuildIntent,
    /// Immutable payload identity and logical size.
    pub payload: OvenArtifactPayload,
    /// Exact dependency or native artifact files copied beneath the store-owned artifact root.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub materialized_files: Vec<OvenArtifactMaterializedFileManifest>,
}

/// Request to publish one immutable Oven artifact.
#[derive(Debug, Clone)]
pub struct OvenArtifactPublishRequest {
    /// Frozen receipt that authorizes the artifact's project and build intent.
    pub receipt: OvenReceipt,
    /// Stable compatibility-domain name, such as a target-specific engine identity.
    pub domain: String,
    /// Semantic role of the payload.
    pub kind: OvenArtifactKind,
    /// Exact immutable payload bytes.
    pub payload: Vec<u8>,
    /// Files copied into the store-owned artifact root together with the immutable payload.
    pub materialized_files: Vec<OvenArtifactMaterializedFile>,
}

/// Measured accounting for one store entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OvenStoreEntry {
    /// Verified immutable manifest.
    pub manifest: OvenArtifactManifest,
    /// Filesystem location owned by this store.
    pub path: PathBuf,
    /// Logical bytes for the primary payload and immutable materialized files.
    pub logical_bytes: u64,
    /// Measured allocated file bytes attributed to this entry during a stable store-wide scan.
    ///
    /// Byte-identical immutable files may be hard-linked by one related batch. Their allocation is attributed once
    /// across the inspection rather than pretending every link consumes separate disk blocks.
    pub physical_bytes: u64,
    /// Last successful selection/publication time used for LRU pruning.
    pub last_used_unix_seconds: u64,
}

/// One verified immutable payload retained with its active execution lease.
///
/// A suite scheduler keeps a vector of these values alive for its complete batch. That makes every indexed shard
/// lease-protected before the first child starts, so policy-driven publication or pruning cannot remove a later
/// shard between test roots.
pub struct OvenStoreExecutionPayload {
    /// Verified immutable manifest authorizing this execution input.
    pub manifest: OvenArtifactManifest,
    /// Store-owned root containing the materialized immutable closure.
    pub artifact_root: PathBuf,
    /// Verified immutable payload bytes.
    pub payload: Vec<u8>,
    _lease: OvenStoreLease,
}

impl OvenStoreExecutionPayload {
    /// Consume this selected payload while retaining the execution lease for the caller's complete use of it.
    #[must_use]
    pub fn into_parts(self) -> (OvenArtifactManifest, PathBuf, Vec<u8>, OvenStoreLease) {
        (self.manifest, self.artifact_root, self.payload, self._lease)
    }
}

impl OvenStoreEntry {
    /// Return the immutable store-owned root containing files materialized with this entry.
    #[must_use]
    pub fn materialized_root(&self) -> PathBuf {
        self.path.join(MATERIALIZED_DIRECTORY)
    }
}

/// Complete physical/logical accounting snapshot for one Oven store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OvenStoreInspection {
    /// Store layout schema version.
    pub schema_version: u32,
    /// Store root.
    pub root: PathBuf,
    /// Enforced capacity policy.
    pub limits: OvenStoreLimits,
    /// Sum of primary payload and immutable materialized-file bytes.
    pub logical_bytes: u64,
    /// Sum of measured allocated file bytes.
    pub physical_bytes: u64,
    /// Physical bytes held by inactive entries that are safe candidates for policy-driven reclamation.
    pub reclaimable_physical_bytes: u64,
    /// Physical bytes retained because an active consumer lease prevents unsafe pruning.
    pub active_lease_physical_bytes: u64,
    /// Individually measured immutable entries.
    pub entries: Vec<OvenStoreEntry>,
}

/// Result of a policy-driven or explicit Oven-store prune operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OvenStorePruneReport {
    /// Store layout schema version.
    pub schema_version: u32,
    /// Whether this is a non-mutating policy preview rather than an applied prune.
    pub dry_run: bool,
    /// Physical bytes before pruning.
    pub before_physical_bytes: u64,
    /// Physical bytes after pruning.
    pub after_physical_bytes: u64,
    /// Logical primary-payload and materialized-file bytes removed with pruned immutable entries.
    pub removed_logical_bytes: u64,
    /// Identities removed under the current policy.
    pub removed_entries: Vec<String>,
    /// Identities retained because their advisory active lease was held.
    pub skipped_active_entries: Vec<String>,
}

/// Failure while validating, publishing, selecting, measuring, or pruning Oven store content.
#[derive(Debug, thiserror::Error)]
pub enum OvenStoreError {
    /// A caller supplied an unsupported identity, domain, or empty artifact payload.
    #[error("invalid Oven store {field}: {message}")]
    InvalidInput { field: &'static str, message: String },
    /// A store file could not be read or written.
    #[error("Oven store I/O failed at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    /// An immutable manifest could not be encoded or decoded.
    #[error("Oven store manifest error at {path}: {message}")]
    Manifest { path: PathBuf, message: String },
    /// An entry's manifest and payload fail integrity verification.
    #[error("Oven store integrity failure for `{identity}`: {message}")]
    Integrity { identity: String, message: String },
    /// Capacity policy cannot admit an artifact without deleting an active entry or exceeding an allowance.
    #[error("Oven store capacity blocked for domain `{domain}`: {message}")]
    CapacityBlocked { domain: String, message: String },
    /// The named legacy publisher holds private staging capacity, so an unrelated publication cannot safely grow
    /// the same bounded store.
    #[error("Oven store legacy_cargo publisher staging is active at {path}; retry publication after it completes")]
    LegacyPublisherStagingActive { path: PathBuf },
}

/// Root handle for a bounded Oven artifact store.
#[derive(Debug, Clone)]
pub struct OvenStore {
    root: PathBuf,
    limits: OvenStoreLimits,
}

/// Validated batch member retained while the store serializes one related publication.
struct PreparedOvenArtifactPublication<'a> {
    request: &'a OvenArtifactPublishRequest,
    manifest: OvenArtifactManifest,
    materialized_files: Vec<ValidatedMaterializedFile>,
    logical_bytes: u64,
}

/// Fully written but not-yet-visible member of one related Oven publication batch.
struct StagedOvenArtifactPublication {
    staging: PathBuf,
    manifest: OvenArtifactManifest,
    logical_bytes: u64,
    physical_bytes: u64,
}

/// Held shared advisory lease that protects one selected Oven artifact from pruning.
pub struct OvenStoreLease {
    file: File,
}

impl OvenStore {
    /// Open a store with explicit retained physical and logical capacity policy.
    #[must_use]
    pub fn new(root: impl AsRef<Path>, limits: OvenStoreLimits) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            limits,
        }
    }

    /// Return the compiler-owned store root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Return the immutable capacity policy applied to every admission in this store.
    #[must_use]
    pub fn limits(&self) -> &OvenStoreLimits {
        &self.limits
    }

    /// Publish an immutable payload after capacity admission and atomic same-filesystem staging.
    pub fn publish(&self, request: &OvenArtifactPublishRequest) -> Result<OvenArtifactManifest, OvenStoreError> {
        self.publish_with_legacy_cargo_publisher_permission(request, false)
    }

    /// Publish one immutable result owned by the explicit `legacy_cargo` transition publisher.
    ///
    /// The caller must already have reserved the complete aggregate allowance through
    /// [`Self::reserve_legacy_cargo_publisher_capacity`]. This narrow entry point lets that owner finish its atomic
    /// hand-off while ordinary publishers refuse to overlap its private staging allocation.
    pub(crate) fn publish_from_legacy_cargo(
        &self,
        request: &OvenArtifactPublishRequest,
    ) -> Result<OvenArtifactManifest, OvenStoreError> {
        self.publish_with_legacy_cargo_publisher_permission(request, true)
    }

    /// Implement one publication, admitting the active legacy publisher only through its named transition boundary.
    fn publish_with_legacy_cargo_publisher_permission(
        &self,
        request: &OvenArtifactPublishRequest,
        allow_legacy_cargo_publisher: bool,
    ) -> Result<OvenArtifactManifest, OvenStoreError> {
        let domain = normalized_domain(&request.domain)?;
        if request.payload.is_empty() {
            return Err(OvenStoreError::InvalidInput {
                field: "payload",
                message: "payload must not be empty".to_string(),
            });
        }
        let materialized_files = validated_materialized_files(&request.materialized_files)?;
        let logical_bytes = request_logical_bytes(&request.payload, &materialized_files)?;
        if logical_bytes > self.limits.max_domain_logical_bytes {
            return Err(OvenStoreError::CapacityBlocked {
                domain,
                message: format!(
                    "logical artifact bytes {logical_bytes} exceed the per-domain allowance {}",
                    self.limits.max_domain_logical_bytes
                ),
            });
        }

        let manifest = artifact_manifest(request, domain.clone(), &materialized_files)?;
        self.ensure_layout()?;
        let manager = open_lock(&self.root.join(MANAGER_LOCK_FILE))?;
        manager.lock().map_err(|source| OvenStoreError::Io {
            path: self.root.join(MANAGER_LOCK_FILE),
            source,
        })?;
        self.reclaim_stale_staging()?;
        if !allow_legacy_cargo_publisher {
            self.reject_active_legacy_cargo_publisher()?;
        }

        let entry_path = self.entry_root(&manifest.identity);
        if entry_path.exists() {
            let verified = verify_entry(&entry_path)?;
            if verified.manifest != manifest {
                return Err(OvenStoreError::Integrity {
                    identity: manifest.identity,
                    message: "existing identity maps to different immutable manifest content".to_string(),
                });
            }
            touch_entry(&entry_path)?;
            return Ok(verified.manifest);
        }
        if let Some((existing, existing_path)) = self.reusable_existing_manifest(&manifest)? {
            touch_entry(&existing_path)?;
            return Ok(existing);
        }

        let estimated_physical = conservative_physical_reservation(&manifest)?;
        if estimated_physical > self.limits.max_domain_physical_bytes {
            return Err(OvenStoreError::CapacityBlocked {
                domain,
                message: format!(
                    "conservative physical reservation {estimated_physical} exceeds the per-domain allowance {}",
                    self.limits.max_domain_physical_bytes
                ),
            });
        }
        self.prune_for_admission(&domain, logical_bytes, estimated_physical)?;

        let staging = self.staging_root(&manifest.identity);
        fs::create_dir(&staging).map_err(|source| OvenStoreError::Io {
            path: staging.clone(),
            source,
        })?;
        let mut shared_materialized_files = BTreeMap::new();
        let publication = write_staged_entry(
            &staging,
            &manifest,
            &request.payload,
            &materialized_files,
            &mut shared_materialized_files,
        );
        if let Err(error) = publication {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }

        // `write_staged_entry` has just rechecked every source digest while writing and synchronizing the new
        // files. Capacity admission needs the actual allocation and shape, not a second full content walk of this
        // private, not-yet-visible entry. Selection and inspection still perform complete digest verification after
        // publication, while this avoids a redundant multi-gigabyte cold-publication hash pass.
        let staged = measure_staged_entry_for_admission(&staging)?;
        if staged.physical_bytes > self.limits.max_domain_physical_bytes {
            let _ = fs::remove_dir_all(&staging);
            return Err(OvenStoreError::CapacityBlocked {
                domain,
                message: format!(
                    "measured physical artifact bytes {} exceed the per-domain allowance {}",
                    staged.physical_bytes, self.limits.max_domain_physical_bytes
                ),
            });
        }
        self.prune_for_admission(&staged.manifest.domain, staged.logical_bytes, staged.physical_bytes)?;
        fs::rename(&staging, &entry_path).map_err(|source| OvenStoreError::Io {
            path: entry_path.clone(),
            source,
        })?;
        sync_directory(self.entries_root())?;
        Ok(manifest)
    }

    /// Return a content-equivalent reusable entry while the caller holds the manager lock.
    ///
    /// A receipt identifies the project invocation that first published an artifact, but direct Rustc selection is
    /// explicitly authorized by a reusable build unit. Re-publishing identical immutable bytes for another
    /// compatible receipt would waste the bounded compatibility-domain allowance and create ambiguous candidates.
    /// The retained manifest preserves the original receipt as provenance; the caller's receipt is independently
    /// checked before it may select that build unit.
    fn reusable_existing_manifest(
        &self,
        candidate: &OvenArtifactManifest,
    ) -> Result<Option<(OvenArtifactManifest, PathBuf)>, OvenStoreError> {
        let root = self.entries_root();
        if !root.exists() {
            return Ok(None);
        }
        for entry in fs::read_dir(&root).map_err(|source| OvenStoreError::Io {
            path: root.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| OvenStoreError::Io {
                path: root.clone(),
                source,
            })?;
            let path = entry.path();
            if !path.is_dir() {
                return Err(OvenStoreError::Integrity {
                    identity: path.display().to_string(),
                    message: "entries root contains a non-directory item".to_string(),
                });
            }
            let existing = verify_entry_manifest(&path)?;
            if reusable_manifest_equivalent(&existing, candidate) {
                return Ok(Some((existing, path)));
            }
        }
        Ok(None)
    }

    /// Validate one prospective immutable publication and return its content-addressed manifest without writing it.
    ///
    /// A compiler-suite index must name every required shard or foundation before the related batch is
    /// committed. This applies the same receipt, file-integrity, portable-path, and single-artifact domain checks
    /// as [`Self::publish_batch`], but deliberately makes no layout, lease, or capacity mutation. The final batch
    /// is still the sole admission and visibility decision.
    pub fn manifest_for_publication(
        &self,
        request: &OvenArtifactPublishRequest,
    ) -> Result<OvenArtifactManifest, OvenStoreError> {
        let domain = normalized_domain(&request.domain)?;
        if request.payload.is_empty() {
            return Err(OvenStoreError::InvalidInput {
                field: "payload",
                message: "payload must not be empty".to_string(),
            });
        }
        let materialized_files = validated_materialized_files(&request.materialized_files)?;
        let logical_bytes = request_logical_bytes(&request.payload, &materialized_files)?;
        if logical_bytes > self.limits.max_domain_logical_bytes {
            return Err(OvenStoreError::CapacityBlocked {
                domain,
                message: format!(
                    "logical artifact bytes {logical_bytes} exceed the per-domain allowance {}",
                    self.limits.max_domain_logical_bytes
                ),
            });
        }
        artifact_manifest(request, domain, &materialized_files)
    }

    /// Admit a related immutable artifact batch across one or more compatibility domains.
    ///
    /// A compiler-suite index is only useful with its complete shard set. This method therefore stages, measures, and
    /// capacity-admits the complete batch before making members visible. Dependencies are synchronized first and the
    /// index is committed last, so an interrupted process may leave reclaimable unreferenced members but can never
    /// expose a selectable partial suite.
    pub fn publish_batch(
        &self,
        requests: &[OvenArtifactPublishRequest],
    ) -> Result<Vec<OvenArtifactManifest>, OvenStoreError> {
        self.publish_batch_with_legacy_cargo_publisher_permission(requests, false)
    }

    /// Publish a related batch from the explicitly named `legacy_cargo` transition publisher.
    ///
    /// This is intentionally not a general bypass: the caller has to reserve the full transient aggregate before
    /// creating its private staging and ordinary publications are rejected for that interval.
    pub(crate) fn publish_batch_from_legacy_cargo(
        &self,
        requests: &[OvenArtifactPublishRequest],
    ) -> Result<Vec<OvenArtifactManifest>, OvenStoreError> {
        self.publish_batch_with_legacy_cargo_publisher_permission(requests, true)
    }

    /// Implement a related publication while allowing only the active named transition publisher to overlap its
    /// reserved private staging allocation.
    fn publish_batch_with_legacy_cargo_publisher_permission(
        &self,
        requests: &[OvenArtifactPublishRequest],
        allow_legacy_cargo_publisher: bool,
    ) -> Result<Vec<OvenArtifactManifest>, OvenStoreError> {
        self.publish_batch_with_legacy_cargo_publisher_permission_and_commit_hook(
            requests,
            allow_legacy_cargo_publisher,
            || Ok(()),
        )
    }

    /// Implement one related publication with an internal hook at the compiler-suite authority commit point.
    ///
    /// Production supplies a no-op hook. Focused tests interrupt this exact boundary after durable members but before
    /// the index rename, proving that the only executable authority is committed last.
    fn publish_batch_with_legacy_cargo_publisher_permission_and_commit_hook(
        &self,
        requests: &[OvenArtifactPublishRequest],
        allow_legacy_cargo_publisher: bool,
        before_authority_commit: impl FnOnce() -> Result<(), OvenStoreError>,
    ) -> Result<Vec<OvenArtifactManifest>, OvenStoreError> {
        if requests.is_empty() {
            return Err(OvenStoreError::InvalidInput {
                field: "publication batch",
                message: "must contain at least one immutable artifact".to_string(),
            });
        }
        let mut prepared = Vec::with_capacity(requests.len());
        let mut identities = BTreeSet::new();
        let mut pending_by_domain = BTreeMap::<String, (u64, u64)>::new();
        for request in requests {
            let domain = normalized_domain(&request.domain)?;
            if request.payload.is_empty() {
                return Err(OvenStoreError::InvalidInput {
                    field: "payload",
                    message: "payload must not be empty".to_string(),
                });
            }
            let materialized_files = validated_materialized_files(&request.materialized_files)?;
            let logical_bytes = request_logical_bytes(&request.payload, &materialized_files)?;
            if logical_bytes > self.limits.max_domain_logical_bytes {
                return Err(OvenStoreError::CapacityBlocked {
                    domain,
                    message: format!(
                        "logical artifact bytes {logical_bytes} exceed the per-domain allowance {}",
                        self.limits.max_domain_logical_bytes
                    ),
                });
            }
            let manifest = artifact_manifest(request, domain.clone(), &materialized_files)?;
            if !identities.insert(manifest.identity.clone()) {
                return Err(OvenStoreError::InvalidInput {
                    field: "publication batch",
                    message: format!("must not repeat immutable identity {}", manifest.identity),
                });
            }
            let pending = pending_by_domain.entry(domain).or_default();
            pending.0 = pending.0.saturating_add(logical_bytes);
            prepared.push(PreparedOvenArtifactPublication {
                request,
                manifest,
                materialized_files,
                logical_bytes,
            });
        }
        self.ensure_layout()?;
        let manager = open_lock(&self.root.join(MANAGER_LOCK_FILE))?;
        manager.lock().map_err(|source| OvenStoreError::Io {
            path: self.root.join(MANAGER_LOCK_FILE),
            source,
        })?;
        self.reclaim_stale_staging()?;
        if !allow_legacy_cargo_publisher {
            self.reject_active_legacy_cargo_publisher()?;
        }

        let mut pending = Vec::new();
        for publication in &prepared {
            let existing = self.entry_root(&publication.manifest.identity);
            if existing.exists() {
                let verified = verify_entry(&existing)?;
                if verified.manifest != publication.manifest {
                    return Err(OvenStoreError::Integrity {
                        identity: publication.manifest.identity.clone(),
                        message: "existing identity maps to different immutable manifest content".to_string(),
                    });
                }
                touch_entry(&existing)?;
            } else {
                pending.push(publication);
            }
        }
        if pending.is_empty() {
            return Ok(prepared.into_iter().map(|publication| publication.manifest).collect());
        }

        // Only genuinely new entries consume admission capacity. Existing identities were already verified and
        // touched above, so a retry of one member of a related batch does not charge its domain twice.
        pending_by_domain.clear();
        let mut reserved_materialized_files = BTreeSet::new();
        let mut reserved_by_domain = BTreeMap::<String, BTreeSet<(String, bool)>>::new();
        let mut estimated_physical = 0_u64;
        for publication in &pending {
            let domain = pending_by_domain
                .entry(publication.manifest.domain.clone())
                .or_default();
            domain.0 = domain.0.saturating_add(publication.logical_bytes);
            estimated_physical =
                estimated_physical.saturating_add(conservative_physical_reservation_with_shared_materialized_files(
                    &publication.manifest,
                    &mut reserved_materialized_files,
                )?);
            let domain_reservation = conservative_physical_reservation_with_shared_materialized_files(
                &publication.manifest,
                reserved_by_domain
                    .entry(publication.manifest.domain.clone())
                    .or_default(),
            )?;
            let domain = pending_by_domain
                .entry(publication.manifest.domain.clone())
                .or_default();
            domain.1 = domain.1.saturating_add(domain_reservation);
        }
        for (domain, (logical, physical)) in &pending_by_domain {
            if *logical > self.limits.max_domain_logical_bytes || *physical > self.limits.max_domain_physical_bytes {
                return Err(OvenStoreError::CapacityBlocked {
                    domain: domain.clone(),
                    message: format!(
                        "related batch bytes logical={logical} physical={physical} exceed the compatibility-domain allowance"
                    ),
                });
            }
        }
        self.prune_for_related_admission(&pending_by_domain, estimated_physical)?;

        let mut staged = Vec::with_capacity(pending.len());
        let mut shared_materialized_files = BTreeMap::new();
        for publication in pending {
            let staging = self.staging_root(&publication.manifest.identity);
            if let Err(source) = fs::create_dir(&staging) {
                cleanup_batch_staging(&staged);
                return Err(OvenStoreError::Io { path: staging, source });
            }
            if let Err(error) = write_staged_entry(
                &staging,
                &publication.manifest,
                &publication.request.payload,
                &publication.materialized_files,
                &mut shared_materialized_files,
            ) {
                let _ = fs::remove_dir_all(&staging);
                cleanup_batch_staging(&staged);
                return Err(error);
            }
            let measurement = match measure_staged_entry_for_admission(&staging) {
                Ok(measurement) => measurement,
                Err(error) => {
                    let _ = fs::remove_dir_all(&staging);
                    cleanup_batch_staging(&staged);
                    return Err(error);
                }
            };
            staged.push(StagedOvenArtifactPublication {
                staging,
                manifest: publication.manifest.clone(),
                logical_bytes: measurement.logical_bytes,
                physical_bytes: 0,
            });
        }
        let measured_physical = assign_unique_staged_physical_bytes(&mut staged)?;
        let mut measured_by_domain = BTreeMap::<String, (u64, u64)>::new();
        for publication in &staged {
            let domain = measured_by_domain
                .entry(publication.manifest.domain.clone())
                .or_default();
            domain.0 = domain.0.saturating_add(publication.logical_bytes);
            domain.1 = domain.1.saturating_add(publication.physical_bytes);
        }
        if let Some((domain, (logical, physical))) = measured_by_domain.iter().find(|(_, (logical, physical))| {
            *logical > self.limits.max_domain_logical_bytes || *physical > self.limits.max_domain_physical_bytes
        }) {
            cleanup_batch_staging(&staged);
            return Err(OvenStoreError::CapacityBlocked {
                domain: domain.clone(),
                message: format!(
                    "measured related-batch bytes logical={logical} physical={physical} exceed the compatibility-domain allowance"
                ),
            });
        }
        if let Err(error) = self.prune_for_related_admission(&measured_by_domain, measured_physical) {
            cleanup_batch_staging(&staged);
            return Err(error);
        }

        let authority_count = staged
            .iter()
            .filter(|publication| publication.manifest.kind == OvenArtifactKind::CompilerTestSuite)
            .count();
        if authority_count > 1 {
            cleanup_batch_staging(&staged);
            return Err(OvenStoreError::InvalidInput {
                field: "publication batch",
                message: "must not contain more than one compiler-suite authority index".to_string(),
            });
        }
        // A compiler-suite index is the sole execution authority for its referenced shards, foundations, and
        // toolchain data. Commit every member first, synchronize those directory entries, and only then expose the
        // index. A crash can therefore leave reclaimable unreferenced members, never a selectable partial suite.
        staged.sort_by_key(|publication| publication.manifest.kind == OvenArtifactKind::CompilerTestSuite);
        let mut published = Vec::with_capacity(staged.len());
        let mut before_authority_commit = Some(before_authority_commit);
        for publication in &staged {
            if publication.manifest.kind == OvenArtifactKind::CompilerTestSuite {
                if let Err(error) = sync_directory(self.entries_root()) {
                    for path in &published {
                        let _ = fs::remove_dir_all(path);
                    }
                    cleanup_batch_staging(&staged);
                    return Err(error);
                }
                if let Some(commit_hook) = before_authority_commit.take()
                    && let Err(error) = commit_hook()
                {
                    for path in &published {
                        let _ = fs::remove_dir_all(path);
                    }
                    cleanup_batch_staging(&staged);
                    return Err(error);
                }
            }
            let destination = self.entry_root(&publication.manifest.identity);
            if let Err(source) = fs::rename(&publication.staging, &destination) {
                for path in &published {
                    let _ = fs::remove_dir_all(path);
                }
                cleanup_batch_staging(&staged);
                return Err(OvenStoreError::Io {
                    path: destination,
                    source,
                });
            }
            published.push(destination);
        }
        sync_directory(self.entries_root())?;
        Ok(prepared.into_iter().map(|publication| publication.manifest).collect())
    }

    /// Select and integrity-check one immutable entry, then retain a shared active lease for its caller.
    pub fn select(&self, identity: &str) -> Result<(OvenStoreEntry, OvenStoreLease), OvenStoreError> {
        self.ensure_layout()?;
        let manager = open_lock(&self.root.join(MANAGER_LOCK_FILE))?;
        manager.lock().map_err(|source| OvenStoreError::Io {
            path: self.root.join(MANAGER_LOCK_FILE),
            source,
        })?;
        self.reclaim_stale_staging()?;
        let path = self.entry_root(identity);
        verify_entry(&path)?;
        let lease_path = path.join(ACTIVE_LOCK_FILE);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lease_path)
            .map_err(|source| OvenStoreError::Io {
                path: lease_path.clone(),
                source,
            })?;
        file.lock_shared().map_err(|source| OvenStoreError::Io {
            path: lease_path,
            source,
        })?;
        touch_entry(&path)?;
        Ok((measure_entry(&path)?, OvenStoreLease { file }))
    }

    /// Select one immutable payload with a held lease so an executor never races pruning after integrity verification.
    pub fn select_payload(&self, identity: &str) -> Result<(OvenStoreEntry, Vec<u8>, OvenStoreLease), OvenStoreError> {
        let (entry, lease) = self.select(identity)?;
        let payload_path = entry.path.join(PAYLOAD_FILE);
        let payload = fs::read(&payload_path).map_err(|source| OvenStoreError::Io {
            path: payload_path,
            source,
        })?;
        if u64::try_from(payload.len()).ok() != Some(entry.manifest.payload.logical_bytes)
            || digest_bytes(&payload) != entry.manifest.payload.digest
        {
            return Err(OvenStoreError::Integrity {
                identity: entry.manifest.identity,
                message: "payload changed after selection".to_string(),
            });
        }
        Ok((entry, payload, lease))
    }

    /// Select a payload and hold its lease for direct native execution.
    ///
    /// This verifies the entry manifest and payload identity but intentionally does not rehash every materialized
    /// compiler artifact. A caller-output receipt has already bound the executable to this manifest, source, and
    /// toolchain; rehashing the closure on that reuse path would turn a cache hit into an O(closure-size) operation.
    /// Cold native bakes and [`Self::inspect`] retain full materialized-closure verification.
    pub fn select_payload_for_execution(
        &self,
        identity: &str,
    ) -> Result<(OvenArtifactManifest, PathBuf, Vec<u8>, OvenStoreLease), OvenStoreError> {
        self.ensure_layout()?;
        let manager = open_lock(&self.root.join(MANAGER_LOCK_FILE))?;
        manager.lock().map_err(|source| OvenStoreError::Io {
            path: self.root.join(MANAGER_LOCK_FILE),
            source,
        })?;
        self.reclaim_stale_staging()?;
        let path = self.entry_root(identity);
        let manifest = verify_entry_manifest(&path)?;
        let payload_path = path.join(PAYLOAD_FILE);
        let payload = fs::read(&payload_path).map_err(|source| OvenStoreError::Io {
            path: payload_path,
            source,
        })?;
        if u64::try_from(payload.len()).ok() != Some(manifest.payload.logical_bytes)
            || digest_bytes(&payload) != manifest.payload.digest
        {
            return Err(OvenStoreError::Integrity {
                identity: manifest.identity,
                message: "manifest payload descriptor disagrees with stored bytes".to_string(),
            });
        }
        let lease_path = path.join(ACTIVE_LOCK_FILE);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lease_path)
            .map_err(|source| OvenStoreError::Io {
                path: lease_path.clone(),
                source,
            })?;
        file.lock_shared().map_err(|source| OvenStoreError::Io {
            path: lease_path,
            source,
        })?;
        touch_entry(&path)?;
        Ok((
            manifest,
            path.join(MATERIALIZED_DIRECTORY),
            payload,
            OvenStoreLease { file },
        ))
    }

    /// Select every immutable execution input for one batch and retain all their active leases together.
    ///
    /// Callers must provide a unique non-empty identity set. The manager lock covers complete manifest/payload
    /// verification and lease acquisition for the whole set, then the returned values keep those entries protected
    /// through execution. This deliberately avoids selecting one later shard after an earlier child has already run:
    /// a concurrent policy admission could otherwise legally prune the not-yet-selected shard.
    pub fn select_payloads_for_execution(
        &self,
        identities: &[String],
    ) -> Result<Vec<OvenStoreExecutionPayload>, OvenStoreError> {
        if identities.is_empty() {
            return Err(OvenStoreError::InvalidInput {
                field: "execution identities",
                message: "must contain at least one immutable entry identity".to_string(),
            });
        }
        let unique = identities.iter().collect::<BTreeSet<_>>();
        if unique.len() != identities.len() {
            return Err(OvenStoreError::InvalidInput {
                field: "execution identities",
                message: "must not repeat one immutable entry identity".to_string(),
            });
        }
        self.ensure_layout()?;
        let manager = open_lock(&self.root.join(MANAGER_LOCK_FILE))?;
        manager.lock().map_err(|source| OvenStoreError::Io {
            path: self.root.join(MANAGER_LOCK_FILE),
            source,
        })?;
        self.reclaim_stale_staging()?;

        let mut selected = Vec::with_capacity(identities.len());
        for identity in identities {
            let path = self.entry_root(identity);
            let manifest = verify_entry_manifest(&path)?;
            let payload_path = path.join(PAYLOAD_FILE);
            let payload = fs::read(&payload_path).map_err(|source| OvenStoreError::Io {
                path: payload_path,
                source,
            })?;
            if u64::try_from(payload.len()).ok() != Some(manifest.payload.logical_bytes)
                || digest_bytes(&payload) != manifest.payload.digest
            {
                return Err(OvenStoreError::Integrity {
                    identity: manifest.identity,
                    message: "manifest payload descriptor disagrees with stored bytes".to_string(),
                });
            }
            let lease_path = path.join(ACTIVE_LOCK_FILE);
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lease_path)
                .map_err(|source| OvenStoreError::Io {
                    path: lease_path.clone(),
                    source,
                })?;
            file.lock_shared().map_err(|source| OvenStoreError::Io {
                path: lease_path,
                source,
            })?;
            touch_entry(&path)?;
            selected.push(OvenStoreExecutionPayload {
                manifest,
                artifact_root: path.join(MATERIALIZED_DIRECTORY),
                payload,
                _lease: OvenStoreLease { file },
            });
        }
        Ok(selected)
    }

    /// Select every execution payload whose verified manifest satisfies `matches`, retaining each active lease.
    ///
    /// Matching, payload verification, and lease acquisition all occur under one manager lock. This is the safe
    /// selection primitive for receipt-based cache lookup: a separately returned manifest header has no lease and
    /// may legitimately be reclaimed by a concurrent bounded-policy publication before a later identity lookup.
    /// Non-matching entries are never opened or leased.
    pub fn select_payloads_matching_for_execution<F>(
        &self,
        matches: F,
    ) -> Result<Vec<OvenStoreExecutionPayload>, OvenStoreError>
    where
        F: Fn(&OvenArtifactManifest) -> bool,
    {
        self.ensure_layout()?;
        let manager = open_lock(&self.root.join(MANAGER_LOCK_FILE))?;
        manager.lock().map_err(|source| OvenStoreError::Io {
            path: self.root.join(MANAGER_LOCK_FILE),
            source,
        })?;
        self.reclaim_stale_staging()?;
        let root = self.entries_root();
        if !root.exists() {
            return Ok(Vec::new());
        }

        let mut selected = Vec::new();
        for candidate in fs::read_dir(&root).map_err(|source| OvenStoreError::Io {
            path: root.clone(),
            source,
        })? {
            let candidate = candidate.map_err(|source| OvenStoreError::Io {
                path: root.clone(),
                source,
            })?;
            let path = candidate.path();
            if !path.is_dir() {
                return Err(OvenStoreError::Integrity {
                    identity: path.display().to_string(),
                    message: "entries root contains a non-directory item".to_string(),
                });
            }
            let manifest = verify_entry_manifest(&path)?;
            if !matches(&manifest) {
                continue;
            }
            let payload_path = path.join(PAYLOAD_FILE);
            let payload = fs::read(&payload_path).map_err(|source| OvenStoreError::Io {
                path: payload_path,
                source,
            })?;
            if u64::try_from(payload.len()).ok() != Some(manifest.payload.logical_bytes)
                || digest_bytes(&payload) != manifest.payload.digest
            {
                return Err(OvenStoreError::Integrity {
                    identity: manifest.identity,
                    message: "manifest payload descriptor disagrees with stored bytes".to_string(),
                });
            }
            let lease_path = path.join(ACTIVE_LOCK_FILE);
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lease_path)
                .map_err(|source| OvenStoreError::Io {
                    path: lease_path.clone(),
                    source,
                })?;
            file.lock_shared().map_err(|source| OvenStoreError::Io {
                path: lease_path,
                source,
            })?;
            touch_entry(&path)?;
            selected.push(OvenStoreExecutionPayload {
                manifest,
                artifact_root: path.join(MATERIALIZED_DIRECTORY),
                payload,
                _lease: OvenStoreLease { file },
            });
        }
        Ok(selected)
    }

    /// Return immutable manifest headers for candidate selection without turning ordinary cache lookup into a full
    /// physical-accounting scan.
    ///
    /// The returned headers are not execution authority: callers must subsequently select the exact payload, which
    /// verifies both its descriptor and bytes while holding an active lease. Use [`Self::inspect`] when physical or
    /// logical accounting, or full materialized-closure verification, is required.
    pub fn manifests_for_selection(&self) -> Result<Vec<OvenArtifactManifest>, OvenStoreError> {
        self.ensure_layout()?;
        let manager = open_lock(&self.root.join(MANAGER_LOCK_FILE))?;
        manager.lock().map_err(|source| OvenStoreError::Io {
            path: self.root.join(MANAGER_LOCK_FILE),
            source,
        })?;
        self.reclaim_stale_staging()?;
        let root = self.entries_root();
        if !root.exists() {
            return Ok(Vec::new());
        }
        fs::read_dir(&root)
            .map_err(|source| OvenStoreError::Io {
                path: root.clone(),
                source,
            })?
            .map(|candidate| {
                let candidate = candidate.map_err(|source| OvenStoreError::Io {
                    path: root.clone(),
                    source,
                })?;
                let path = candidate.path();
                if !path.is_dir() {
                    return Err(OvenStoreError::Integrity {
                        identity: path.display().to_string(),
                        message: "entries root contains a non-directory item".to_string(),
                    });
                }
                verify_entry_manifest(&path)
            })
            .collect()
    }

    /// Return distinct logical artifact bytes and measured physical allocation for all published entries.
    pub fn inspect(&self) -> Result<OvenStoreInspection, OvenStoreError> {
        self.ensure_layout()?;
        let manager = open_lock(&self.root.join(MANAGER_LOCK_FILE))?;
        manager.lock().map_err(|source| OvenStoreError::Io {
            path: self.root.join(MANAGER_LOCK_FILE),
            source,
        })?;
        self.reclaim_stale_staging()?;
        self.inspect_while_locked()
    }

    /// Return store accounting while the manager lease excludes unreported staging writes and concurrent pruning.
    fn inspect_while_locked(&self) -> Result<OvenStoreInspection, OvenStoreError> {
        let entries = self.collect_entries()?;
        let logical_bytes = entries.iter().map(|entry| entry.logical_bytes).sum();
        let physical_bytes = entries.iter().map(|entry| entry.physical_bytes).sum();
        let (reclaimable_physical_bytes, active_lease_physical_bytes) = physical_bytes_by_lease(&entries)?;
        Ok(OvenStoreInspection {
            schema_version: OVEN_STORE_SCHEMA_VERSION,
            root: self.root.clone(),
            limits: self.limits,
            logical_bytes,
            physical_bytes,
            reclaimable_physical_bytes,
            active_lease_physical_bytes,
            entries,
        })
    }

    /// Prune least-recently-used inactive artifacts until aggregate physical policy is satisfied.
    pub fn prune(&self) -> Result<OvenStorePruneReport, OvenStoreError> {
        self.ensure_layout()?;
        let manager = open_lock(&self.root.join(MANAGER_LOCK_FILE))?;
        manager.lock().map_err(|source| OvenStoreError::Io {
            path: self.root.join(MANAGER_LOCK_FILE),
            source,
        })?;
        self.reclaim_stale_staging()?;
        self.prune_to_limits(None, 0, 0, true)
    }

    /// Preview the inactive entries policy would reclaim without removing any entry or staging data.
    ///
    /// The manager lease makes the candidate list coherent with a real prune, while deliberately leaving even
    /// stale staging untouched: a command advertised as a dry run must not delete user-visible store content.
    pub fn preview_prune(&self) -> Result<OvenStorePruneReport, OvenStoreError> {
        self.ensure_layout()?;
        let manager = open_lock(&self.root.join(MANAGER_LOCK_FILE))?;
        manager.lock().map_err(|source| OvenStoreError::Io {
            path: self.root.join(MANAGER_LOCK_FILE),
            source,
        })?;
        self.prune_to_limits(None, 0, 0, false)
    }

    /// Reserve the full aggregate allowance for the explicitly named `legacy_cargo` publisher before it starts
    /// creating private staging files.
    ///
    /// A compiler-suite publisher cannot know its final private target size before Cargo has produced it. Reserving
    /// the whole aggregate therefore prunes every inactive immutable entry now, or fails closed when an active
    /// lease prevents that. While the publisher lock remains held, ordinary publication is rejected; its staging
    /// monitor then enforces the same aggregate ceiling rather than merely inspecting it afterwards.
    pub(crate) fn reserve_legacy_cargo_publisher_capacity(&self) -> Result<OvenStorePruneReport, OvenStoreError> {
        self.ensure_layout()?;
        let manager = open_lock(&self.root.join(MANAGER_LOCK_FILE))?;
        manager.lock().map_err(|source| OvenStoreError::Io {
            path: self.root.join(MANAGER_LOCK_FILE),
            source,
        })?;
        self.reclaim_stale_staging()?;
        let reservation = self.limits.max_physical_bytes;
        let report = self.prune_to_limits(None, 0, reservation, true)?;
        let entries = self.collect_entries_for_admission()?;
        if policy_satisfied(&entries, self.limits, None, 0, reservation) {
            return Ok(report);
        }
        Err(OvenStoreError::CapacityBlocked {
            domain: "legacy-cargo-publisher".to_string(),
            message: format!(
                "full transient physical reservation {reservation} cannot be admitted; skipped active entries {:?}",
                report.skipped_active_entries
            ),
        })
    }

    /// Refuse the named publisher's final hand-off when its live private staging plus every new immutable file would
    /// exceed the aggregate physical policy.
    ///
    /// Materialized files beneath `legacy-cargo-staging` are hard-linked into the atomic entry staging, so they are
    /// counted once here. Sources outside that private tree are copied by [`write_staged_entry`] and are reserved
    /// once per digest/executable pair, exactly as the batch writer shares them. The prior full reservation leaves no
    /// retained entry unless an active lease correctly blocked publication; including entries nevertheless keeps this
    /// check safe if the method is ever called by another explicit transition owner.
    pub(crate) fn ensure_legacy_cargo_batch_physical_capacity(
        &self,
        staging: &Path,
        requests: &[OvenArtifactPublishRequest],
    ) -> Result<(), OvenStoreError> {
        if requests.is_empty() {
            return Err(OvenStoreError::InvalidInput {
                field: "legacy_cargo publication batch",
                message: "must contain at least one immutable artifact".to_string(),
            });
        }
        self.ensure_layout()?;
        let manager = open_lock(&self.root.join(MANAGER_LOCK_FILE))?;
        manager.lock().map_err(|source| OvenStoreError::Io {
            path: self.root.join(MANAGER_LOCK_FILE),
            source,
        })?;
        self.reclaim_stale_staging()?;
        let publisher_root = self.root.join(LEGACY_CARGO_STAGING_DIRECTORY);
        let publisher_root = fs::canonicalize(&publisher_root).map_err(|source| OvenStoreError::Io {
            path: publisher_root,
            source,
        })?;
        let staging = fs::canonicalize(staging).map_err(|source| OvenStoreError::Io {
            path: staging.to_path_buf(),
            source,
        })?;
        if !staging.starts_with(&publisher_root) {
            return Err(OvenStoreError::InvalidInput {
                field: "legacy_cargo staging",
                message: format!(
                    "{} must remain below the private publisher root {}",
                    staging.display(),
                    publisher_root.display()
                ),
            });
        }
        let mut observed_physical = unique_directory_physical_bytes(&staging)?;
        observed_physical = observed_physical.saturating_add(
            self.collect_entries_for_admission()?
                .iter()
                .map(|entry| entry.physical_bytes)
                .sum::<u64>(),
        );
        let mut copied_materializations = BTreeSet::new();
        for request in requests {
            let manifest = self.manifest_for_publication(request)?;
            observed_physical = observed_physical.saturating_add(round_physical(
                u64::try_from(request.payload.len()).map_err(|_| OvenStoreError::InvalidInput {
                    field: "legacy_cargo publication payload",
                    message: "length does not fit supported physical accounting".to_string(),
                })?,
            ));
            let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|error| OvenStoreError::Manifest {
                path: PathBuf::from(MANIFEST_FILE),
                message: error.to_string(),
            })?;
            let manifest_length = u64::try_from(manifest_bytes.len()).map_err(|_| OvenStoreError::Manifest {
                path: PathBuf::from(MANIFEST_FILE),
                message: "manifest length does not fit supported physical accounting".to_string(),
            })?;
            observed_physical = observed_physical
                .saturating_add(round_physical(manifest_length))
                // `write_staged_entry` adds the trailing manifest newline and the mutable access timestamp.
                .saturating_add(round_physical(1))
                .saturating_add(round_physical(20));
            let files = validated_materialized_files(&request.materialized_files)?;
            for file in files {
                let source = fs::canonicalize(&file.source_path).map_err(|source_error| OvenStoreError::Io {
                    path: file.source_path.clone(),
                    source: source_error,
                })?;
                if source.starts_with(&publisher_root)
                    || !copied_materializations.insert((file.manifest.digest, file.manifest.executable))
                {
                    continue;
                }
                observed_physical = observed_physical.saturating_add(round_physical(file.manifest.logical_bytes));
            }
        }
        if observed_physical <= self.limits.max_physical_bytes {
            return Ok(());
        }
        Err(OvenStoreError::CapacityBlocked {
            domain: "legacy-cargo-publisher".to_string(),
            message: format!(
                "private staging plus immutable batch reserve {observed_physical} physical bytes, exceeding aggregate allowance {}",
                self.limits.max_physical_bytes
            ),
        })
    }

    /// Ensure published entries leave enough capacity for the pending immutable artifact.
    fn prune_for_admission(
        &self,
        domain: &str,
        pending_logical_bytes: u64,
        pending_physical_bytes: u64,
    ) -> Result<(), OvenStoreError> {
        let report = self.prune_to_limits(Some(domain), pending_logical_bytes, pending_physical_bytes, true)?;
        let entries = self.collect_entries_for_admission()?;
        if policy_satisfied(
            &entries,
            self.limits,
            Some(domain),
            pending_logical_bytes,
            pending_physical_bytes,
        ) {
            return Ok(());
        }
        Err(OvenStoreError::CapacityBlocked {
            domain: domain.to_string(),
            message: format!(
                "policy cannot admit logical={pending_logical_bytes} physical={pending_physical_bytes}; skipped active entries {:?}",
                report.skipped_active_entries
            ),
        })
    }

    /// Ensure a related mixed-domain batch can be admitted without treating its foundations as separate unrelated
    /// publications. Aggregate physical policy applies to the complete set, while every named domain retains its
    /// own logical and physical allowance.
    fn prune_for_related_admission(
        &self,
        pending_by_domain: &BTreeMap<String, (u64, u64)>,
        pending_physical_bytes: u64,
    ) -> Result<(), OvenStoreError> {
        let report = self.prune_related_to_limits(pending_by_domain, pending_physical_bytes, true)?;
        let entries = self.collect_entries_for_admission()?;
        if related_policy_satisfied(&entries, self.limits, pending_by_domain, pending_physical_bytes) {
            return Ok(());
        }
        let domain = related_policy_offending_domains(&entries, self.limits, pending_by_domain, pending_physical_bytes)
            .into_iter()
            .next()
            .or_else(|| pending_by_domain.keys().next().cloned())
            .unwrap_or_else(|| "related-batch".to_string());
        Err(OvenStoreError::CapacityBlocked {
            domain,
            message: format!(
                "policy cannot admit related batch physical={pending_physical_bytes}; skipped active entries {:?}",
                report.skipped_active_entries
            ),
        })
    }

    /// Apply LRU pruning for a complete related batch while preserving every active lease.
    fn prune_related_to_limits(
        &self,
        pending_by_domain: &BTreeMap<String, (u64, u64)>,
        pending_physical_bytes: u64,
        apply: bool,
    ) -> Result<OvenStorePruneReport, OvenStoreError> {
        let mut entries = self.collect_entries_for_admission()?;
        let before_physical_bytes = entries.iter().map(|entry| entry.physical_bytes).sum();
        entries.sort_by_key(|entry| entry.last_used_unix_seconds);
        let mut removed_entries = Vec::new();
        let mut skipped_active_entries = Vec::new();
        let mut removed_logical_bytes = 0_u64;

        for entry in entries.clone() {
            if related_policy_satisfied(&entries, self.limits, pending_by_domain, pending_physical_bytes) {
                break;
            }
            let offending_domains =
                related_policy_offending_domains(&entries, self.limits, pending_by_domain, pending_physical_bytes);
            if !offending_domains.is_empty() && !offending_domains.contains(&entry.manifest.domain) {
                continue;
            }
            match try_lock_entry(&entry.path)? {
                Some(_lock) => {
                    if apply {
                        fs::remove_dir_all(&entry.path).map_err(|source| OvenStoreError::Io {
                            path: entry.path.clone(),
                            source,
                        })?;
                    }
                    removed_logical_bytes = removed_logical_bytes.saturating_add(entry.logical_bytes);
                    removed_entries.push(entry.manifest.identity);
                    entries.retain(|candidate| candidate.path != entry.path);
                    assign_unique_entry_physical_bytes(&mut entries)?;
                }
                None => skipped_active_entries.push(entry.manifest.identity),
            }
        }
        let after_physical_bytes = entries.iter().map(|entry| entry.physical_bytes).sum();
        Ok(OvenStorePruneReport {
            schema_version: OVEN_STORE_SCHEMA_VERSION,
            dry_run: !apply,
            before_physical_bytes,
            after_physical_bytes,
            removed_logical_bytes,
            removed_entries,
            skipped_active_entries,
        })
    }

    /// Calculate LRU pruning for pending aggregate/domain capacity, never selecting a held active lease.
    ///
    /// When `apply` is true, remove the selected inactive entries. A preview follows the same candidate and
    /// accounting path but retains every on-disk entry, so its after-bytes and removed identities are a policy
    /// prediction rather than an observed mutation.
    fn prune_to_limits(
        &self,
        pending_domain: Option<&str>,
        pending_logical_bytes: u64,
        pending_physical_bytes: u64,
        apply: bool,
    ) -> Result<OvenStorePruneReport, OvenStoreError> {
        let mut entries = self.collect_entries_for_admission()?;
        let before_physical_bytes = entries.iter().map(|entry| entry.physical_bytes).sum();
        entries.sort_by_key(|entry| entry.last_used_unix_seconds);
        let mut removed_entries = Vec::new();
        let mut skipped_active_entries = Vec::new();
        let mut removed_logical_bytes = 0_u64;

        for entry in entries.clone() {
            if policy_satisfied(
                &entries,
                self.limits,
                pending_domain,
                pending_logical_bytes,
                pending_physical_bytes,
            ) {
                break;
            }
            let domain_is_over = if let Some(domain) = pending_domain {
                let (logical, physical) = domain_totals(&entries, domain);
                logical.saturating_add(pending_logical_bytes) > self.limits.max_domain_logical_bytes
                    || physical.saturating_add(pending_physical_bytes) > self.limits.max_domain_physical_bytes
            } else {
                false
            };
            if domain_is_over && pending_domain != Some(entry.manifest.domain.as_str()) {
                continue;
            }
            match try_lock_entry(&entry.path)? {
                Some(_lock) => {
                    if apply {
                        fs::remove_dir_all(&entry.path).map_err(|source| OvenStoreError::Io {
                            path: entry.path.clone(),
                            source,
                        })?;
                    }
                    removed_logical_bytes = removed_logical_bytes.saturating_add(entry.logical_bytes);
                    removed_entries.push(entry.manifest.identity);
                    entries.retain(|candidate| candidate.path != entry.path);
                    assign_unique_entry_physical_bytes(&mut entries)?;
                }
                None => skipped_active_entries.push(entry.manifest.identity),
            }
        }
        let after_physical_bytes = entries.iter().map(|entry| entry.physical_bytes).sum();
        Ok(OvenStorePruneReport {
            schema_version: OVEN_STORE_SCHEMA_VERSION,
            dry_run: !apply,
            before_physical_bytes,
            after_physical_bytes,
            removed_logical_bytes,
            removed_entries,
            skipped_active_entries,
        })
    }

    /// Create store directories and the manager lock without constructing any artifact entry.
    fn ensure_layout(&self) -> Result<(), OvenStoreError> {
        fs::create_dir_all(self.entries_root()).map_err(|source| OvenStoreError::Io {
            path: self.entries_root(),
            source,
        })?;
        fs::create_dir_all(self.staging_root_base()).map_err(|source| OvenStoreError::Io {
            path: self.staging_root_base(),
            source,
        })?;
        let manager = self.root.join(MANAGER_LOCK_FILE);
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&manager)
            .map_err(|source| OvenStoreError::Io { path: manager, source })?;
        Ok(())
    }

    /// Remove only complete-or-partial staging children after the manager lock proves no publisher owns them.
    fn reclaim_stale_staging(&self) -> Result<(), OvenStoreError> {
        let staging = self.staging_root_base();
        for candidate in fs::read_dir(&staging).map_err(|source| OvenStoreError::Io {
            path: staging.clone(),
            source,
        })? {
            let candidate = candidate.map_err(|source| OvenStoreError::Io {
                path: staging.clone(),
                source,
            })?;
            let path = candidate.path();
            let metadata = fs::symlink_metadata(&path).map_err(|source| OvenStoreError::Io {
                path: path.clone(),
                source,
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(OvenStoreError::Integrity {
                    identity: path.display().to_string(),
                    message: "staging root may contain only compiler-owned directories".to_string(),
                });
            }
            fs::remove_dir_all(&path).map_err(|source| OvenStoreError::Io { path, source })?;
        }
        sync_directory(staging)
    }

    /// Reject a normal publication while the exclusive legacy publisher owns private staging, reclaiming only
    /// stale task-owned staging after the advisory publisher lock proves no process still owns it.
    fn reject_active_legacy_cargo_publisher(&self) -> Result<(), OvenStoreError> {
        let staging = self.root.join(LEGACY_CARGO_STAGING_DIRECTORY);
        if !staging.exists() {
            return Ok(());
        }
        let metadata = fs::symlink_metadata(&staging).map_err(|source| OvenStoreError::Io {
            path: staging.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(OvenStoreError::Integrity {
                identity: staging.display().to_string(),
                message: "legacy publisher staging root must be a regular directory".to_string(),
            });
        }
        let lock_path = staging.join(LEGACY_CARGO_PUBLISHER_LOCK_FILE);
        let lock = open_lock(&lock_path)?;
        match lock.try_lock() {
            Ok(()) => {
                for candidate in fs::read_dir(&staging).map_err(|source| OvenStoreError::Io {
                    path: staging.clone(),
                    source,
                })? {
                    let candidate = candidate.map_err(|source| OvenStoreError::Io {
                        path: staging.clone(),
                        source,
                    })?;
                    let name = candidate.file_name();
                    if !name.to_string_lossy().starts_with(LEGACY_CARGO_STAGING_PREFIX) {
                        continue;
                    }
                    let path = candidate.path();
                    let metadata = fs::symlink_metadata(&path).map_err(|source| OvenStoreError::Io {
                        path: path.clone(),
                        source,
                    })?;
                    if metadata.file_type().is_symlink() || !metadata.is_dir() {
                        return Err(OvenStoreError::Integrity {
                            identity: path.display().to_string(),
                            message: "legacy publisher staging may contain only owned directories".to_string(),
                        });
                    }
                    fs::remove_dir_all(&path).map_err(|source| OvenStoreError::Io { path, source })?;
                }
                lock.unlock().map_err(|source| OvenStoreError::Io {
                    path: lock_path,
                    source,
                })?;
                sync_directory(staging)
            }
            Err(TryLockError::WouldBlock) => Err(OvenStoreError::LegacyPublisherStagingActive { path: lock_path }),
            Err(TryLockError::Error(source)) => Err(OvenStoreError::Io {
                path: lock_path,
                source,
            }),
        }
    }

    /// Return all complete verified entries, skipping only no path and never malformed owned content.
    fn collect_entries(&self) -> Result<Vec<OvenStoreEntry>, OvenStoreError> {
        self.collect_entries_with(measure_entry)
    }

    /// Return entries with manifest-bound logical accounting and measured allocated blocks for admission/pruning.
    ///
    /// Admission must be proportional to the number of stored files, not their contents: it uses immutable manifest
    /// descriptors for logical bytes and a shape-checked allocation walk for physical bytes. Full payload and
    /// materialized-file digest verification is deliberately reserved for [`Self::inspect`] and the verifying
    /// selection APIs; direct native execution relies on its separately validated caller-output receipt.
    fn collect_entries_for_admission(&self) -> Result<Vec<OvenStoreEntry>, OvenStoreError> {
        self.collect_entries_with(measure_entry_for_admission)
    }

    /// Enumerate complete store entries through one caller-selected measurement policy.
    fn collect_entries_with(
        &self,
        measure: fn(&Path) -> Result<OvenStoreEntry, OvenStoreError>,
    ) -> Result<Vec<OvenStoreEntry>, OvenStoreError> {
        let root = self.entries_root();
        if !root.exists() {
            return Ok(Vec::new());
        }
        let directory = fs::read_dir(&root).map_err(|source| OvenStoreError::Io {
            path: root.clone(),
            source,
        })?;
        let mut entries = directory
            .map(|candidate| {
                let candidate = candidate.map_err(|source| OvenStoreError::Io {
                    path: root.clone(),
                    source,
                })?;
                let path = candidate.path();
                if !path.is_dir() {
                    return Err(OvenStoreError::Integrity {
                        identity: path.display().to_string(),
                        message: "entries root contains a non-directory item".to_string(),
                    });
                }
                measure(&path)
            })
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort_by(|left, right| left.manifest.identity.cmp(&right.manifest.identity));
        assign_unique_entry_physical_bytes(&mut entries)?;
        Ok(entries)
    }

    /// Return the owned immutable entry path for a validated identity.
    fn entry_root(&self, identity: &str) -> PathBuf {
        self.entries_root().join(identity)
    }

    /// Return the published-entry root.
    fn entries_root(&self) -> PathBuf {
        self.root.join(ENTRIES_DIRECTORY)
    }

    /// Return the staging root.
    fn staging_root_base(&self) -> PathBuf {
        self.root.join(STAGING_DIRECTORY)
    }

    /// Return a manager-serialized unique staging path.
    fn staging_root(&self, identity: &str) -> PathBuf {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        self.staging_root_base()
            .join(format!("{identity}-{}-{sequence}", std::process::id()))
    }
}

/// Remove only private, not-yet-visible batch staging roots after a failed all-or-nothing publication.
fn cleanup_batch_staging(staged: &[StagedOvenArtifactPublication]) {
    for publication in staged {
        let _ = fs::remove_dir_all(&publication.staging);
    }
}

impl Drop for OvenStoreLease {
    /// Release the advisory lease before a future capacity operation can prune the selected entry.
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Build one immutable artifact manifest from validated request input.
fn artifact_manifest(
    request: &OvenArtifactPublishRequest,
    domain: String,
    materialized_files: &[ValidatedMaterializedFile],
) -> Result<OvenArtifactManifest, OvenStoreError> {
    request
        .receipt
        .verify_identity()
        .map_err(|error| OvenStoreError::InvalidInput {
            field: "receipt",
            message: error.to_string(),
        })?;
    let payload = OvenArtifactPayload {
        digest: digest_bytes(&request.payload),
        logical_bytes: u64::try_from(request.payload.len()).map_err(|_| OvenStoreError::InvalidInput {
            field: "payload",
            message: "payload length does not fit the supported accounting range".to_string(),
        })?,
    };
    let materialized_files = materialized_files
        .iter()
        .map(|file| file.manifest.clone())
        .collect::<Vec<_>>();
    let input = ArtifactIdentityInput {
        schema_version: OVEN_STORE_SCHEMA_VERSION,
        receipt_identity: &request.receipt.identity,
        build_unit_identity: &request.receipt.build_unit_identity,
        intent: &request.receipt.intent,
        domain: &domain,
        kind: request.kind,
        payload: &payload,
        materialized_files: &materialized_files,
    };
    let serialized = serde_json::to_vec(&input).map_err(|error| OvenStoreError::Manifest {
        path: PathBuf::from(MANIFEST_FILE),
        message: error.to_string(),
    })?;
    Ok(OvenArtifactManifest {
        schema_version: OVEN_STORE_SCHEMA_VERSION,
        identity: digest_bytes(&serialized),
        receipt_identity: request.receipt.identity.clone(),
        build_unit_identity: request.receipt.build_unit_identity.clone(),
        domain,
        kind: request.kind,
        intent: request.receipt.intent.clone(),
        payload,
        materialized_files,
    })
}

/// Validate portable paths and content before a publisher-owned file becomes a store-owned artifact.
fn validated_materialized_files(
    files: &[OvenArtifactMaterializedFile],
) -> Result<Vec<ValidatedMaterializedFile>, OvenStoreError> {
    let mut by_path = BTreeMap::new();
    for file in files {
        let relative_path = normalized_materialized_relative_path(&file.relative_path)?;
        let metadata = fs::symlink_metadata(&file.source_path).map_err(|source| OvenStoreError::Io {
            path: file.source_path.clone(),
            source,
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(OvenStoreError::InvalidInput {
                field: "materialized file",
                message: format!("{} must be a non-symlink regular file", file.source_path.display()),
            });
        }
        let bytes = fs::read(&file.source_path).map_err(|source| OvenStoreError::Io {
            path: file.source_path.clone(),
            source,
        })?;
        let logical_bytes = u64::try_from(bytes.len()).map_err(|_| OvenStoreError::InvalidInput {
            field: "materialized file",
            message: format!("{} exceeds supported accounting range", file.source_path.display()),
        })?;
        let validated = ValidatedMaterializedFile {
            source_path: file.source_path.clone(),
            manifest: OvenArtifactMaterializedFileManifest {
                relative_path: relative_path.clone(),
                digest: digest_bytes(&bytes),
                logical_bytes,
                executable: source_is_executable(&metadata),
            },
        };
        if by_path.insert(relative_path.clone(), validated).is_some() {
            return Err(OvenStoreError::InvalidInput {
                field: "materialized file",
                message: format!("declares duplicate store path `{relative_path}`"),
            });
        }
    }
    Ok(by_path.into_values().collect())
}

/// Compute logical retention from the primary payload and every store-owned artifact file.
fn request_logical_bytes(payload: &[u8], files: &[ValidatedMaterializedFile]) -> Result<u64, OvenStoreError> {
    let payload_bytes = u64::try_from(payload.len()).map_err(|_| OvenStoreError::InvalidInput {
        field: "payload",
        message: "payload length does not fit the supported accounting range".to_string(),
    })?;
    Ok(files.iter().fold(payload_bytes, |total, file| {
        total.saturating_add(file.manifest.logical_bytes)
    }))
}

/// Reject a store-relative artifact path that can escape the immutable entry root.
fn normalized_materialized_relative_path(value: &str) -> Result<String, OvenStoreError> {
    let path = Path::new(value);
    if value.trim().is_empty()
        || path.components().any(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::ParentDir | Component::CurDir
            )
        })
    {
        return Err(OvenStoreError::InvalidInput {
            field: "materialized file path",
            message: "must be a non-empty normalized relative path".to_string(),
        });
    }
    Ok(path.to_string_lossy().replace('\\', "/"))
}

/// Validate one compatibility domain before it becomes a filesystem path component.
fn normalized_domain(domain: &str) -> Result<String, OvenStoreError> {
    let domain = domain.trim();
    if domain.is_empty() || domain.contains('/') || domain.contains('\\') || domain == "." || domain == ".." {
        return Err(OvenStoreError::InvalidInput {
            field: "domain",
            message: "must be a non-empty stable identifier without path separators".to_string(),
        });
    }
    Ok(domain.to_string())
}

/// Reserve enough physical capacity for one staged manifest, payload, materialized closure, and advisory files.
fn conservative_physical_reservation(manifest: &OvenArtifactManifest) -> Result<u64, OvenStoreError> {
    conservative_physical_reservation_with_shared_materialized_files(manifest, &mut BTreeSet::new())
}

/// Reserve a staged entry while counting one digest-matched immutable materialization only once in a publication
/// batch. Each entry still owns its complete logical manifest; this only models the physical hard link the batch
/// writer creates beneath its single managed store root.
fn conservative_physical_reservation_with_shared_materialized_files(
    manifest: &OvenArtifactManifest,
    shared_materialized_files: &mut BTreeSet<(String, bool)>,
) -> Result<u64, OvenStoreError> {
    let manifest_bytes = serde_json::to_vec_pretty(manifest).map_err(|error| OvenStoreError::Manifest {
        path: PathBuf::from(MANIFEST_FILE),
        message: error.to_string(),
    })?;
    let manifest_bytes = u64::try_from(manifest_bytes.len()).map_err(|_| OvenStoreError::InvalidInput {
        field: "manifest",
        message: "serialized manifest length does not fit the supported accounting range".to_string(),
    })?;
    let materialized_reservation = manifest.materialized_files.iter().fold(0_u64, |total, file| {
        let key = (file.digest.clone(), file.executable);
        if shared_materialized_files.insert(key) {
            total.saturating_add(round_physical(file.logical_bytes))
        } else {
            total
        }
    });
    Ok(round_physical(manifest.payload.logical_bytes)
        .saturating_add(materialized_reservation)
        .saturating_add(round_physical(manifest_bytes))
        .saturating_add(round_physical(20)))
}

/// Use a conservative 4 KiB reservation for pre-publication physical capacity admission.
fn round_physical(bytes: u64) -> u64 {
    const BLOCK: u64 = 4096;
    bytes.saturating_add(BLOCK - 1) / BLOCK * BLOCK
}

/// Write and synchronize a complete staged artifact directory before it becomes visible.
///
/// Sources inside this store's private `legacy-cargo-staging` root are closed publisher outputs. They may be linked
/// directly into the store's atomic staging entry: the publisher holds exclusive ownership until this method returns
/// and deletes the source link only after the immutable entry is visible. This avoids a multi-gigabyte physical copy
/// overlap during admission while external caller-owned sources still use an independently written immutable copy.
fn write_staged_entry(
    root: &Path,
    manifest: &OvenArtifactManifest,
    payload: &[u8],
    materialized_files: &[ValidatedMaterializedFile],
    shared_materialized_files: &mut BTreeMap<(String, bool), PathBuf>,
) -> Result<(), OvenStoreError> {
    write_synced_file(&root.join(PAYLOAD_FILE), payload, false)?;
    let materialized_root = root.join(MATERIALIZED_DIRECTORY);
    fs::create_dir(&materialized_root).map_err(|source| OvenStoreError::Io {
        path: materialized_root.clone(),
        source,
    })?;
    for file in materialized_files {
        let destination = materialized_root.join(&file.manifest.relative_path);
        let parent = destination.parent().ok_or_else(|| OvenStoreError::InvalidInput {
            field: "materialized file path",
            message: format!("{} has no parent", file.manifest.relative_path),
        })?;
        fs::create_dir_all(parent).map_err(|source| OvenStoreError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        let bytes = fs::read(&file.source_path).map_err(|source| OvenStoreError::Io {
            path: file.source_path.clone(),
            source,
        })?;
        if u64::try_from(bytes.len()).ok() != Some(file.manifest.logical_bytes)
            || digest_bytes(&bytes) != file.manifest.digest
        {
            return Err(OvenStoreError::Integrity {
                identity: manifest.identity.clone(),
                message: format!(
                    "publisher artifact changed before storage: {}",
                    file.source_path.display()
                ),
            });
        }
        let shared_key = (file.manifest.digest.clone(), file.manifest.executable);
        if let Some(shared) = shared_materialized_files.get(&shared_key) {
            fs::hard_link(shared, &destination).map_err(|source| OvenStoreError::Io {
                path: destination.clone(),
                source,
            })?;
        } else if is_private_publisher_materialized_source(root, &file.source_path)? {
            OpenOptions::new()
                .read(true)
                .open(&file.source_path)
                .and_then(|source| source.sync_all())
                .map_err(|source| OvenStoreError::Io {
                    path: file.source_path.clone(),
                    source,
                })?;
            fs::hard_link(&file.source_path, &destination).map_err(|source| OvenStoreError::Io {
                path: destination.clone(),
                source,
            })?;
            shared_materialized_files.insert(shared_key, destination.clone());
        } else {
            write_synced_file(&destination, &bytes, false)?;
            set_materialized_executable(&destination, file.manifest.executable)?;
            shared_materialized_files.insert(shared_key, destination.clone());
        }
    }
    sync_directory_tree(&materialized_root)?;
    let manifest_bytes = serde_json::to_vec_pretty(manifest).map_err(|error| OvenStoreError::Manifest {
        path: root.join(MANIFEST_FILE),
        message: error.to_string(),
    })?;
    write_synced_file(&root.join(MANIFEST_FILE), &manifest_bytes, true)?;
    OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(root.join(ACTIVE_LOCK_FILE))
        .map_err(|source| OvenStoreError::Io {
            path: root.join(ACTIVE_LOCK_FILE),
            source,
        })?;
    write_synced_file(
        &root.join(ACCESS_FILE),
        now_unix_seconds()?.to_string().as_bytes(),
        true,
    )?;
    sync_directory(root.to_path_buf())
}

/// Return whether a verified materialized source belongs to this store's private, publisher-owned staging tree.
///
/// `root` is always `<store>/staging/<identity>-...`; its grandparent is the only store root we trust for the
/// hard-link optimization. A canonical source path must remain below the separately named legacy publisher tree so
/// a caller cannot turn arbitrary mutable input into a store-owned inode by choosing a convenient path.
fn is_private_publisher_materialized_source(root: &Path, source: &Path) -> Result<bool, OvenStoreError> {
    let store_root = root
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| OvenStoreError::InvalidInput {
            field: "store staging root",
            message: format!("{} has no store-root ancestor", root.display()),
        })?;
    let publisher_root = store_root.join("legacy-cargo-staging");
    let publisher_root = match fs::canonicalize(&publisher_root) {
        Ok(path) => path,
        Err(source_error) if source_error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source_error) => {
            return Err(OvenStoreError::Io {
                path: publisher_root,
                source: source_error,
            });
        }
    };
    let source = fs::canonicalize(source).map_err(|source_error| OvenStoreError::Io {
        path: source.to_path_buf(),
        source: source_error,
    })?;
    Ok(source.starts_with(publisher_root))
}

/// Verify one published immutable entry and calculate both its logical and physical accounting.
fn verify_entry(root: &Path) -> Result<OvenStoreEntry, OvenStoreError> {
    let manifest = verify_entry_manifest(root)?;
    let payload_path = root.join(PAYLOAD_FILE);
    let payload = fs::read(&payload_path).map_err(|source| OvenStoreError::Io {
        path: payload_path.clone(),
        source,
    })?;
    let actual_bytes = u64::try_from(payload.len()).map_err(|_| OvenStoreError::Integrity {
        identity: manifest.identity.clone(),
        message: "payload length does not fit the supported accounting range".to_string(),
    })?;
    if actual_bytes != manifest.payload.logical_bytes || digest_bytes(&payload) != manifest.payload.digest {
        return Err(OvenStoreError::Integrity {
            identity: manifest.identity,
            message: "manifest payload descriptor disagrees with stored bytes".to_string(),
        });
    }
    let access_path = root.join(ACCESS_FILE);
    let last_used_unix_seconds = fs::read_to_string(&access_path)
        .map_err(|source| OvenStoreError::Io {
            path: access_path.clone(),
            source,
        })?
        .trim()
        .parse::<u64>()
        .map_err(|error| OvenStoreError::Manifest {
            path: access_path,
            message: error.to_string(),
        })?;
    let materialized_logical_bytes = verify_materialized_files(root, &manifest)?;
    Ok(OvenStoreEntry {
        logical_bytes: manifest
            .payload
            .logical_bytes
            .saturating_add(materialized_logical_bytes),
        physical_bytes: directory_physical_bytes(root)?,
        last_used_unix_seconds,
        manifest,
        path: root.to_path_buf(),
    })
}

/// Verify immutable manifest structure and identity without traversing the materialized compiler closure.
fn verify_entry_manifest(root: &Path) -> Result<OvenArtifactManifest, OvenStoreError> {
    let manifest_path = root.join(MANIFEST_FILE);
    let content = fs::read(&manifest_path).map_err(|source| OvenStoreError::Io {
        path: manifest_path.clone(),
        source,
    })?;
    let manifest =
        serde_json::from_slice::<OvenArtifactManifest>(&content).map_err(|error| OvenStoreError::Manifest {
            path: manifest_path,
            message: error.to_string(),
        })?;
    if manifest.schema_version != OVEN_STORE_SCHEMA_VERSION {
        return Err(OvenStoreError::Integrity {
            identity: manifest.identity,
            message: format!("unsupported store schema {}", manifest.schema_version),
        });
    }
    let identity = artifact_identity_from_manifest(&manifest)?;
    if identity != manifest.identity {
        return Err(OvenStoreError::Integrity {
            identity: manifest.identity,
            message: "manifest identity does not match its immutable content".to_string(),
        });
    }
    Ok(manifest)
}

/// Verify the exact recursive file closure materialized beneath one immutable entry.
fn verify_materialized_files(root: &Path, manifest: &OvenArtifactManifest) -> Result<u64, OvenStoreError> {
    let expected = manifest
        .materialized_files
        .iter()
        .map(|file| (file.relative_path.clone(), file))
        .collect::<BTreeMap<_, _>>();
    if expected.len() != manifest.materialized_files.len() {
        return Err(OvenStoreError::Integrity {
            identity: manifest.identity.clone(),
            message: "materialized artifact manifest declares a path more than once".to_string(),
        });
    }
    let materialized_root = root.join(MATERIALIZED_DIRECTORY);
    let mut actual = BTreeMap::new();
    collect_materialized_files(&materialized_root, &materialized_root, &mut actual)?;
    if actual.len() != expected.len() || actual.keys().ne(expected.keys()) {
        return Err(OvenStoreError::Integrity {
            identity: manifest.identity.clone(),
            message: "materialized artifact files differ from the immutable manifest".to_string(),
        });
    }
    for (relative_path, expected_file) in expected {
        let path = actual.get(&relative_path).ok_or_else(|| OvenStoreError::Integrity {
            identity: manifest.identity.clone(),
            message: format!("materialized artifact `{relative_path}` is missing"),
        })?;
        let (logical_bytes, digest) = digest_materialized_file(path)?;
        if logical_bytes != expected_file.logical_bytes || digest != expected_file.digest {
            return Err(OvenStoreError::Integrity {
                identity: manifest.identity.clone(),
                message: format!("materialized artifact `{relative_path}` failed digest verification"),
            });
        }
    }
    Ok(manifest
        .materialized_files
        .iter()
        .fold(0_u64, |total, file| total.saturating_add(file.logical_bytes)))
}

/// Hash one immutable materialized file in bounded memory while preserving exact byte-count verification.
fn digest_materialized_file(path: &Path) -> Result<(u64, String), OvenStoreError> {
    let mut file = File::open(path).map_err(|source| OvenStoreError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut logical_bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|source| OvenStoreError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        let read = u64::try_from(read).map_err(|_| OvenStoreError::Integrity {
            identity: path.display().to_string(),
            message: "materialized artifact read count does not fit the supported accounting range".to_string(),
        })?;
        logical_bytes = logical_bytes
            .checked_add(read)
            .ok_or_else(|| OvenStoreError::Integrity {
                identity: path.display().to_string(),
                message: "materialized artifact byte count exceeds the supported accounting range".to_string(),
            })?;
    }
    Ok((logical_bytes, format!("sha256:{}", hex::encode(hasher.finalize()))))
}

/// Collect regular materialized files while rejecting links and non-file entry types.
fn collect_materialized_files(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<String, PathBuf>,
) -> Result<(), OvenStoreError> {
    for child in fs::read_dir(directory).map_err(|source| OvenStoreError::Io {
        path: directory.to_path_buf(),
        source,
    })? {
        let child = child.map_err(|source| OvenStoreError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = child.path();
        let metadata = fs::symlink_metadata(&path).map_err(|source| OvenStoreError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(OvenStoreError::Integrity {
                identity: path.display().to_string(),
                message: "materialized artifact roots may not contain symlinks".to_string(),
            });
        }
        if metadata.is_dir() {
            collect_materialized_files(root, &path, files)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(OvenStoreError::Integrity {
                identity: path.display().to_string(),
                message: "materialized artifact roots may contain only regular files and directories".to_string(),
            });
        }
        let relative_path = path
            .strip_prefix(root)
            .map_err(|_| OvenStoreError::Integrity {
                identity: path.display().to_string(),
                message: "materialized artifact escaped its entry root".to_string(),
            })?
            .to_string_lossy()
            .replace('\\', "/");
        if files.insert(relative_path.clone(), path).is_some() {
            return Err(OvenStoreError::Integrity {
                identity: relative_path,
                message: "materialized artifact path occurred more than once".to_string(),
            });
        }
    }
    Ok(())
}

/// Measure one already verified entry after a selection or publication touch.
fn measure_entry(root: &Path) -> Result<OvenStoreEntry, OvenStoreError> {
    verify_entry(root)
}

/// Measure the capacity impact of one immutable entry without rereading its complete content closure.
///
/// The manifest identity and the entry filesystem shape are still checked. This is enough for conservative policy:
/// unlisted bytes are counted physically, while any byte-level corruption remains a selection/inspection integrity
/// error rather than making every later publisher hash unrelated multi-gigabyte artifacts.
fn measure_entry_for_admission(root: &Path) -> Result<OvenStoreEntry, OvenStoreError> {
    measure_entry_for_admission_with_directory_identity(root, true)
}

/// Measure a private, not-yet-visible staging entry for capacity admission.
///
/// Staging roots add a process-unique suffix so concurrent publishers cannot collide. They are never selection
/// candidates; once atomically renamed, the public-entry measurement above requires the manifest identity name.
fn measure_staged_entry_for_admission(root: &Path) -> Result<OvenStoreEntry, OvenStoreError> {
    measure_entry_for_admission_with_directory_identity(root, false)
}

/// Share shape and allocation accounting between public entries and private publisher staging.
fn measure_entry_for_admission_with_directory_identity(
    root: &Path,
    require_identity_directory_name: bool,
) -> Result<OvenStoreEntry, OvenStoreError> {
    let manifest = verify_entry_manifest(root)?;
    let expected_directory_name = manifest.identity.as_str();
    if require_identity_directory_name
        && root.file_name().and_then(|name| name.to_str()) != Some(expected_directory_name)
    {
        return Err(OvenStoreError::Integrity {
            identity: manifest.identity,
            message: "entry directory name does not match its immutable manifest identity".to_string(),
        });
    }
    let payload_path = root.join(PAYLOAD_FILE);
    let payload_metadata = fs::symlink_metadata(&payload_path).map_err(|source| OvenStoreError::Io {
        path: payload_path.clone(),
        source,
    })?;
    if !payload_metadata.is_file() || payload_metadata.file_type().is_symlink() {
        return Err(OvenStoreError::Integrity {
            identity: manifest.identity,
            message: "entry payload must be a regular non-symlink file".to_string(),
        });
    }
    let access_path = root.join(ACCESS_FILE);
    let last_used_unix_seconds = fs::read_to_string(&access_path)
        .map_err(|source| OvenStoreError::Io {
            path: access_path.clone(),
            source,
        })?
        .trim()
        .parse::<u64>()
        .map_err(|error| OvenStoreError::Manifest {
            path: access_path,
            message: error.to_string(),
        })?;
    let logical_bytes = manifest.payload.logical_bytes.saturating_add(
        manifest
            .materialized_files
            .iter()
            .fold(0_u64, |total, file| total.saturating_add(file.logical_bytes)),
    );
    Ok(OvenStoreEntry {
        logical_bytes,
        physical_bytes: directory_physical_bytes(root)?,
        last_used_unix_seconds,
        manifest,
        path: root.to_path_buf(),
    })
}

/// Recompute an immutable artifact identity from persisted manifest data.
fn artifact_identity_from_manifest(manifest: &OvenArtifactManifest) -> Result<String, OvenStoreError> {
    let input = ArtifactIdentityInput {
        schema_version: manifest.schema_version,
        receipt_identity: &manifest.receipt_identity,
        build_unit_identity: &manifest.build_unit_identity,
        intent: &manifest.intent,
        domain: &manifest.domain,
        kind: manifest.kind,
        payload: &manifest.payload,
        materialized_files: &manifest.materialized_files,
    };
    let serialized = serde_json::to_vec(&input).map_err(|error| OvenStoreError::Manifest {
        path: PathBuf::from(MANIFEST_FILE),
        message: error.to_string(),
    })?;
    Ok(digest_bytes(&serialized))
}

/// Return whether two immutable entries carry the same reusable execution content despite distinct publisher receipts.
fn reusable_manifest_equivalent(left: &OvenArtifactManifest, right: &OvenArtifactManifest) -> bool {
    left.kind == OvenArtifactKind::DirectRustcPlan
        && right.kind == OvenArtifactKind::DirectRustcPlan
        && left.schema_version == right.schema_version
        && left.build_unit_identity == right.build_unit_identity
        && left.domain == right.domain
        && left.kind == right.kind
        && left.intent == right.intent
        && left.payload == right.payload
        && left.materialized_files == right.materialized_files
}

/// Update the LRU selection time without modifying the immutable manifest or payload.
fn touch_entry(root: &Path) -> Result<(), OvenStoreError> {
    let access = root.join(ACCESS_FILE);
    let staged = root.join(format!(".{ACCESS_FILE}.tmp-{}", std::process::id()));
    write_synced_file(&staged, now_unix_seconds()?.to_string().as_bytes(), true)?;
    fs::rename(&staged, &access).map_err(|source| OvenStoreError::Io { path: access, source })?;
    sync_directory(root.to_path_buf())
}

/// Open one advisory lock file through the shared store error vocabulary.
fn open_lock(path: &Path) -> Result<File, OvenStoreError> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|source| OvenStoreError::Io {
            path: path.to_path_buf(),
            source,
        })
}

/// Return a held exclusive lock when an entry is inactive, or `None` when a live reader protects it.
fn try_lock_entry(path: &Path) -> Result<Option<File>, OvenStoreError> {
    let lock_path = path.join(ACTIVE_LOCK_FILE);
    let file = open_lock(&lock_path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(source)) => Err(OvenStoreError::Io {
            path: lock_path,
            source,
        }),
    }
}

/// Return true when all aggregate and pending-domain capacity constraints hold.
fn policy_satisfied(
    entries: &[OvenStoreEntry],
    limits: OvenStoreLimits,
    pending_domain: Option<&str>,
    pending_logical_bytes: u64,
    pending_physical_bytes: u64,
) -> bool {
    let physical = entries.iter().map(|entry| entry.physical_bytes).sum::<u64>();
    if physical.saturating_add(pending_physical_bytes) > limits.max_physical_bytes {
        return false;
    }
    pending_domain.is_none_or(|domain| {
        let (logical, physical) = domain_totals(entries, domain);
        logical.saturating_add(pending_logical_bytes) <= limits.max_domain_logical_bytes
            && physical.saturating_add(pending_physical_bytes) <= limits.max_domain_physical_bytes
    })
}

/// Return true when aggregate physical capacity and every pending compatibility domain can admit one related batch.
fn related_policy_satisfied(
    entries: &[OvenStoreEntry],
    limits: OvenStoreLimits,
    pending_by_domain: &BTreeMap<String, (u64, u64)>,
    pending_physical_bytes: u64,
) -> bool {
    let physical = entries.iter().map(|entry| entry.physical_bytes).sum::<u64>();
    if physical.saturating_add(pending_physical_bytes) > limits.max_physical_bytes {
        return false;
    }
    related_policy_offending_domains(entries, limits, pending_by_domain, pending_physical_bytes).is_empty()
}

/// Identify domains whose independently declared logical or physical allowance prevents related-batch admission.
///
/// `pending_physical_bytes` is deliberately not added per domain: each pending domain carries its own attributable
/// physical amount in `pending_by_domain`, while the separate aggregate check above accounts for the whole batch
/// exactly once across hard-linked files.
fn related_policy_offending_domains(
    entries: &[OvenStoreEntry],
    limits: OvenStoreLimits,
    pending_by_domain: &BTreeMap<String, (u64, u64)>,
    _pending_physical_bytes: u64,
) -> BTreeSet<String> {
    pending_by_domain
        .iter()
        .filter_map(|(domain, (pending_logical, pending_physical))| {
            let (logical, physical) = domain_totals(entries, domain);
            (logical.saturating_add(*pending_logical) > limits.max_domain_logical_bytes
                || physical.saturating_add(*pending_physical) > limits.max_domain_physical_bytes)
                .then(|| domain.clone())
        })
        .collect()
}

/// Sum logical and physical accounting for one compatibility domain.
fn domain_totals(entries: &[OvenStoreEntry], domain: &str) -> (u64, u64) {
    entries
        .iter()
        .filter(|entry| entry.manifest.domain == domain)
        .fold((0_u64, 0_u64), |(logical, physical), entry| {
            (
                logical.saturating_add(entry.logical_bytes),
                physical.saturating_add(entry.physical_bytes),
            )
        })
}

/// Recursively measure allocated file blocks, excluding directories from the physical file-byte definition.
fn directory_physical_bytes(path: &Path) -> Result<u64, OvenStoreError> {
    fs::read_dir(path)
        .map_err(|source| OvenStoreError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .try_fold(0_u64, |total, child| {
            let child = child.map_err(|source| OvenStoreError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            let child_path = child.path();
            let metadata = fs::symlink_metadata(&child_path).map_err(|source| OvenStoreError::Io {
                path: child_path.clone(),
                source,
            })?;
            if metadata.file_type().is_symlink() {
                return Err(OvenStoreError::Integrity {
                    identity: child_path.display().to_string(),
                    message: "store entries may not contain symlinks".to_string(),
                });
            }
            let bytes = if metadata.is_dir() {
                directory_physical_bytes(&child_path)?
            } else if metadata.is_file() {
                physical_file_bytes(&metadata)
            } else {
                return Err(OvenStoreError::Integrity {
                    identity: child_path.display().to_string(),
                    message: "store entries may contain only regular files and directories".to_string(),
                });
            };
            Ok(total.saturating_add(bytes))
        })
}

/// Measure a private publisher tree by inode so its own hard-linked preparation files are not charged twice.
#[cfg(unix)]
fn unique_directory_physical_bytes(path: &Path) -> Result<u64, OvenStoreError> {
    directory_unique_physical_bytes(path, &mut BTreeSet::new())
}

/// Hosts without inode identity retain conservative per-path staging accounting.
#[cfg(not(unix))]
fn unique_directory_physical_bytes(path: &Path) -> Result<u64, OvenStoreError> {
    directory_physical_bytes(path)
}

/// Attribute each hard-linked immutable file allocation to the first stable entry identity that references it.
///
/// Materialized closures may deliberately share byte-identical files across a related publication batch. Directory
/// accounting alone would count every hard link as newly allocated disk use, so admission/pruning/inspection use
/// this stable attribution pass and report physical bytes once while retaining each entry's independent logical
/// bytes. Re-running it after a simulated or real prune is essential: deleting one link cannot reclaim a block that
/// another selected entry still references.
#[cfg(unix)]
fn assign_unique_entry_physical_bytes(entries: &mut [OvenStoreEntry]) -> Result<(), OvenStoreError> {
    let mut seen_files = BTreeSet::new();
    for entry in entries {
        entry.physical_bytes = directory_unique_physical_bytes(&entry.path, &mut seen_files)?;
    }
    Ok(())
}

/// Hosts without inode identity cannot distinguish hard links portably, so retain conservative per-entry accounting.
#[cfg(not(unix))]
fn assign_unique_entry_physical_bytes(_entries: &mut [OvenStoreEntry]) -> Result<(), OvenStoreError> {
    Ok(())
}

/// Assign physical bytes for a complete staged batch before any member becomes visible.
#[cfg(unix)]
fn assign_unique_staged_physical_bytes(staged: &mut [StagedOvenArtifactPublication]) -> Result<u64, OvenStoreError> {
    let mut seen_files = BTreeSet::new();
    let mut total = 0_u64;
    for publication in staged {
        publication.physical_bytes = directory_unique_physical_bytes(&publication.staging, &mut seen_files)?;
        total = total.saturating_add(publication.physical_bytes);
    }
    Ok(total)
}

/// Hosts without inode identity retain conservative staged-batch accounting.
#[cfg(not(unix))]
fn assign_unique_staged_physical_bytes(staged: &mut [StagedOvenArtifactPublication]) -> Result<u64, OvenStoreError> {
    let mut total = 0_u64;
    for publication in staged {
        publication.physical_bytes = directory_physical_bytes(&publication.staging)?;
        total = total.saturating_add(publication.physical_bytes);
    }
    Ok(total)
}

/// Measure regular files below one root once by `(device, inode)`, preserving directory/link integrity checks.
#[cfg(unix)]
fn directory_unique_physical_bytes(path: &Path, seen_files: &mut BTreeSet<(u64, u64)>) -> Result<u64, OvenStoreError> {
    use std::os::unix::fs::MetadataExt;

    fs::read_dir(path)
        .map_err(|source| OvenStoreError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .try_fold(0_u64, |total, child| {
            let child = child.map_err(|source| OvenStoreError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            let child_path = child.path();
            let metadata = fs::symlink_metadata(&child_path).map_err(|source| OvenStoreError::Io {
                path: child_path.clone(),
                source,
            })?;
            if metadata.file_type().is_symlink() {
                return Err(OvenStoreError::Integrity {
                    identity: child_path.display().to_string(),
                    message: "store entries may not contain symlinks".to_string(),
                });
            }
            let bytes = if metadata.is_dir() {
                directory_unique_physical_bytes(&child_path, seen_files)?
            } else if metadata.is_file() {
                let identity = (metadata.dev(), metadata.ino());
                if seen_files.insert(identity) {
                    physical_file_bytes(&metadata)
                } else {
                    0
                }
            } else {
                return Err(OvenStoreError::Integrity {
                    identity: child_path.display().to_string(),
                    message: "store entries may contain only regular files and directories".to_string(),
                });
            };
            Ok(total.saturating_add(bytes))
        })
}

/// Split measured physical allocation into bytes that an inactive-only prune could reclaim and bytes retained by at
/// least one active lease. A shared immutable inode is lease-protected if any entry that links it is active.
#[cfg(unix)]
fn physical_bytes_by_lease(entries: &[OvenStoreEntry]) -> Result<(u64, u64), OvenStoreError> {
    let mut allocation_leases = BTreeMap::<(u64, u64), (u64, bool)>::new();
    for entry in entries {
        let active = try_lock_entry(&entry.path)?.is_none();
        record_physical_file_leases(&entry.path, active, &mut allocation_leases)?;
    }
    let (reclaimable, active) =
        allocation_leases
            .into_values()
            .fold((0_u64, 0_u64), |(reclaimable, active), (bytes, lease_protected)| {
                if lease_protected {
                    (reclaimable, active.saturating_add(bytes))
                } else {
                    (reclaimable.saturating_add(bytes), active)
                }
            });
    Ok((reclaimable, active))
}

/// Preserve conservative pre-hard-link accounting where inode identity is unavailable.
#[cfg(not(unix))]
fn physical_bytes_by_lease(entries: &[OvenStoreEntry]) -> Result<(u64, u64), OvenStoreError> {
    let mut reclaimable = 0_u64;
    let mut active = 0_u64;
    for entry in entries {
        if try_lock_entry(&entry.path)?.is_some() {
            reclaimable = reclaimable.saturating_add(entry.physical_bytes);
        } else {
            active = active.saturating_add(entry.physical_bytes);
        }
    }
    Ok((reclaimable, active))
}

/// Record every regular physical allocation beneath one entry, promoting an allocation to lease-protected whenever
/// any hard-linked entry is active.
#[cfg(unix)]
fn record_physical_file_leases(
    path: &Path,
    active: bool,
    allocations: &mut BTreeMap<(u64, u64), (u64, bool)>,
) -> Result<(), OvenStoreError> {
    use std::os::unix::fs::MetadataExt;

    for child in fs::read_dir(path).map_err(|source| OvenStoreError::Io {
        path: path.to_path_buf(),
        source,
    })? {
        let child = child.map_err(|source| OvenStoreError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let child_path = child.path();
        let metadata = fs::symlink_metadata(&child_path).map_err(|source| OvenStoreError::Io {
            path: child_path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(OvenStoreError::Integrity {
                identity: child_path.display().to_string(),
                message: "store entries may not contain symlinks".to_string(),
            });
        }
        if metadata.is_dir() {
            record_physical_file_leases(&child_path, active, allocations)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(OvenStoreError::Integrity {
                identity: child_path.display().to_string(),
                message: "store entries may contain only regular files and directories".to_string(),
            });
        }
        let identity = (metadata.dev(), metadata.ino());
        let allocation = allocations
            .entry(identity)
            .or_insert_with(|| (physical_file_bytes(&metadata), false));
        allocation.1 |= active;
    }
    Ok(())
}

/// Return measured allocated bytes for one regular file, preserving a portable fallback outside Unix.
#[cfg(unix)]
fn physical_file_bytes(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;

    metadata.blocks().saturating_mul(512)
}

/// Return logical bytes where the host cannot expose allocated Unix block counts.
#[cfg(not(unix))]
fn physical_file_bytes(metadata: &fs::Metadata) -> u64 {
    metadata.len()
}

/// Preserve the only permission bit that affects an Oven artifact's runtime semantics.
///
/// Store-owned files are otherwise written read-only; retaining writable source permissions would make the
/// immutable-artifact contract weaker. The executable bit is preserved so native test binaries and CLI artifacts
/// can run directly from a selected artifact (or a hard link to one) without a Cargo-side repair step.
#[cfg(unix)]
fn source_is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    metadata.mode() & 0o111 != 0
}

/// Non-Unix hosts have no portable executable permission bit to preserve.
#[cfg(not(unix))]
fn source_is_executable(_metadata: &fs::Metadata) -> bool {
    false
}

/// Apply immutable read permissions and, when requested, executable permissions to one store-owned artifact file.
#[cfg(unix)]
fn set_materialized_executable(path: &Path, executable: bool) -> Result<(), OvenStoreError> {
    use std::os::unix::fs::PermissionsExt;

    let mode = if executable { 0o555 } else { 0o444 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|source| OvenStoreError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Non-Unix hosts retain the platform default after the file contents have been synchronized.
#[cfg(not(unix))]
fn set_materialized_executable(_path: &Path, _executable: bool) -> Result<(), OvenStoreError> {
    Ok(())
}

/// Persist one file and its data before its containing directory is renamed into the published store.
fn write_synced_file(path: &Path, bytes: &[u8], trailing_newline: bool) -> Result<(), OvenStoreError> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|source| OvenStoreError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(bytes).map_err(|source| OvenStoreError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if trailing_newline {
        file.write_all(b"\n").map_err(|source| OvenStoreError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }
    file.sync_all().map_err(|source| OvenStoreError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Synchronize a directory entry after atomic store publication where the host supports directory handles.
pub(crate) fn sync_directory(path: PathBuf) -> Result<(), OvenStoreError> {
    File::open(&path)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| OvenStoreError::Io { path, source })
}

/// Synchronize nested materialized directories from leaves to root before their entry becomes visible.
pub(crate) fn sync_directory_tree(directory: &Path) -> Result<(), OvenStoreError> {
    for child in fs::read_dir(directory).map_err(|source| OvenStoreError::Io {
        path: directory.to_path_buf(),
        source,
    })? {
        let child = child.map_err(|source| OvenStoreError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = child.path();
        let metadata = fs::symlink_metadata(&path).map_err(|source| OvenStoreError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(OvenStoreError::Integrity {
                identity: path.display().to_string(),
                message: "staged materialized artifacts may not contain symlinks".to_string(),
            });
        }
        if metadata.is_dir() {
            sync_directory_tree(&path)?;
        }
    }
    sync_directory(directory.to_path_buf())
}

/// Return the current Unix timestamp for mutable access metadata, not immutable artifact identity.
fn now_unix_seconds() -> Result<u64, OvenStoreError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| OvenStoreError::Manifest {
            path: PathBuf::from(ACCESS_FILE),
            message: error.to_string(),
        })
}

/// Canonical artifact content used solely to derive a stable store identity.
#[derive(Serialize)]
struct ArtifactIdentityInput<'a> {
    schema_version: u32,
    receipt_identity: &'a str,
    build_unit_identity: &'a str,
    intent: &'a OvenBuildIntent,
    domain: &'a str,
    kind: OvenArtifactKind,
    payload: &'a OvenArtifactPayload,
    materialized_files: &'a [OvenArtifactMaterializedFileManifest],
}

/// Source and immutable descriptor retained only during one staged publication.
#[derive(Debug, Clone)]
struct ValidatedMaterializedFile {
    source_path: PathBuf,
    manifest: OvenArtifactMaterializedFileManifest,
}

#[cfg(test)]
mod tests {
    use super::{
        LEGACY_CARGO_PUBLISHER_LOCK_FILE, LEGACY_CARGO_STAGING_DIRECTORY, OvenArtifactKind,
        OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore, OvenStoreError, OvenStoreLimits,
    };
    use crate::oven::{
        OvenGeneratedProjectRequest, OvenImportRequest, import_frozen_project, receipt_generated_project,
    };
    use std::fs::{self, OpenOptions};
    use std::path::Path;

    #[test]
    fn store_reports_distinct_logical_and_physical_bytes() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let manifest = store.publish(&request(project.path(), "engine-arm64", b"engine payload")?)?;
        let inspection = store.inspect()?;

        assert_eq!(inspection.entries.len(), 1);
        assert_eq!(inspection.entries[0].manifest.identity, manifest.identity);
        assert_eq!(inspection.logical_bytes, 14);
        assert!(inspection.physical_bytes >= inspection.logical_bytes);
        Ok(())
    }

    #[test]
    fn store_rejects_one_domain_that_exceeds_its_logical_allowance() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 3));

        let result = store.publish(&request(project.path(), "engine-arm64", b"four")?);
        assert!(matches!(result, Err(OvenStoreError::CapacityBlocked { .. })));
        assert!(store.inspect()?.entries.is_empty());
        Ok(())
    }

    #[test]
    fn store_copies_materialized_files_and_accounts_for_their_logical_bytes() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        let publisher = tempfile::tempdir()?;
        write_project(project.path())?;
        let source = publisher.path().join("native/libpublisher-proof.a");
        fs::create_dir_all(source.parent().ok_or("materialized fixture must have a parent")?)?;
        fs::write(&source, b"publisher proof")?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let mut publication = request(project.path(), "engine-arm64", b"plan")?;
        publication.materialized_files = vec![OvenArtifactMaterializedFile {
            source_path: source.clone(),
            relative_path: "native/libpublisher-proof.a".to_string(),
        }];

        let manifest = store.publish(&publication)?;
        fs::remove_file(&source)?;
        let (entry, _payload, _lease) = store.select_payload(&manifest.identity)?;
        assert_eq!(
            fs::read(entry.materialized_root().join("native/libpublisher-proof.a"))?,
            b"publisher proof"
        );
        assert_eq!(entry.logical_bytes, 4 + u64::try_from(b"publisher proof".len())?);
        assert_eq!(entry.manifest.materialized_files.len(), 1);
        assert_eq!(store.inspect()?.logical_bytes, entry.logical_bytes);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn store_links_closed_private_publisher_artifacts_without_a_second_physical_copy()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::MetadataExt;

        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let source = temp
            .path()
            .join("legacy-cargo-staging/publisher/native/libpublisher-proof.rlib");
        fs::create_dir_all(source.parent().ok_or("private publisher fixture must have a parent")?)?;
        fs::write(&source, b"publisher-owned proof")?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let mut publication = request(project.path(), "engine-arm64", b"plan")?;
        publication.materialized_files = vec![OvenArtifactMaterializedFile {
            source_path: source.clone(),
            relative_path: "native/libpublisher-proof.rlib".to_string(),
        }];

        let manifest = store.publish(&publication)?;
        let destination = store
            .select(&manifest.identity)?
            .0
            .materialized_root()
            .join("native/libpublisher-proof.rlib");
        let source_metadata = fs::metadata(&source)?;
        let destination_metadata = fs::metadata(&destination)?;
        assert_eq!(source_metadata.dev(), destination_metadata.dev());
        assert_eq!(source_metadata.ino(), destination_metadata.ino());
        assert!(source_metadata.nlink() >= 2);

        fs::remove_file(&source)?;
        let (entry, _payload, _lease) = store.select_payload(&manifest.identity)?;
        assert_eq!(
            fs::read(entry.materialized_root().join("native/libpublisher-proof.rlib"))?,
            b"publisher-owned proof"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn store_preserves_executable_materialization_as_an_immutable_runtime_property()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        let publisher = tempfile::tempdir()?;
        write_project(project.path())?;
        let source = publisher.path().join("native/runner");
        fs::create_dir_all(source.parent().ok_or("materialized fixture must have a parent")?)?;
        fs::write(&source, b"native executable")?;
        fs::set_permissions(&source, fs::Permissions::from_mode(0o755))?;

        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let mut publication = request(project.path(), "engine-arm64", b"plan")?;
        publication.materialized_files = vec![OvenArtifactMaterializedFile {
            source_path: source,
            relative_path: "native/runner".to_string(),
        }];

        let manifest = store.publish(&publication)?;
        assert!(manifest.materialized_files[0].executable);
        let (entry, _payload, _lease) = store.select_payload(&manifest.identity)?;
        let stored = entry.materialized_root().join("native/runner");
        assert_ne!(fs::metadata(&stored)?.mode() & 0o111, 0);
        assert_eq!(fs::metadata(&stored)?.mode() & 0o222, 0);
        Ok(())
    }

    #[test]
    fn store_rejects_a_mutated_receipt_before_publication() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let mut forged = request(project.path(), "engine-arm64", b"payload")?;
        forged.receipt.intent.profile = "debug".to_string();

        let result = store.publish(&forged);
        assert!(matches!(
            result,
            Err(OvenStoreError::InvalidInput { field: "receipt", .. })
        ));
        assert!(store.inspect()?.entries.is_empty());
        Ok(())
    }

    #[test]
    fn active_lease_blocks_unsafe_pruning_then_inactive_entry_is_reclaimed() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let permissive = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let first = permissive.publish(&request(project.path(), "engine-one", b"first payload")?)?;
        let first_physical = permissive.inspect()?.physical_bytes;
        let bounded = OvenStore::new(
            temp.path(),
            OvenStoreLimits::new(first_physical.saturating_add(1), 1_000_000, 1_000_000),
        );
        let (_entry, lease) = bounded.select(&first.identity)?;

        let blocked = bounded.publish(&request(project.path(), "engine-two", b"second payload")?);
        assert!(matches!(blocked, Err(OvenStoreError::CapacityBlocked { .. })));
        assert_eq!(bounded.inspect()?.entries.len(), 1);

        drop(lease);
        let second = bounded.publish(&request(project.path(), "engine-two", b"second payload")?)?;
        let inspection = bounded.inspect()?;
        assert_eq!(inspection.entries.len(), 1);
        assert_eq!(inspection.entries[0].manifest.identity, second.identity);
        Ok(())
    }

    #[test]
    fn matching_execution_selection_holds_the_lease_before_policy_can_prune() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let permissive = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let first = permissive.publish(&request(project.path(), "engine-one", b"first payload")?)?;
        let first_physical = permissive.inspect()?.physical_bytes;
        let bounded = OvenStore::new(
            temp.path(),
            OvenStoreLimits::new(first_physical.saturating_add(1), 1_000_000, 1_000_000),
        );

        let selected =
            bounded.select_payloads_matching_for_execution(|manifest| manifest.identity == first.identity)?;
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].manifest.identity, first.identity);

        let blocked = bounded.publish(&request(project.path(), "engine-two", b"second payload")?);
        assert!(matches!(blocked, Err(OvenStoreError::CapacityBlocked { .. })));
        drop(selected);

        let second = bounded.publish(&request(project.path(), "engine-two", b"second payload")?)?;
        assert_eq!(bounded.inspect()?.entries[0].manifest.identity, second.identity);
        Ok(())
    }

    #[test]
    fn compatible_receipts_reuse_one_identical_immutable_entry() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let first_project = tempfile::tempdir()?;
        let second_project = tempfile::tempdir()?;
        let first_source = first_project.path().join("main.rs");
        let second_source = second_project.path().join("main.rs");
        fs::write(&first_source, "fn main() { println!(\"first\"); }\n")?;
        fs::write(&second_source, "fn main() { println!(\"second\"); }\n")?;
        let receipt_for = |project: &Path, source: &Path| {
            receipt_generated_project(
                &OvenGeneratedProjectRequest::new(
                    project,
                    "shared-Loaf",
                    "0.1.0",
                    "aarch64-apple-darwin",
                    "rustc 1.96.0",
                    "debug",
                    Vec::new(),
                )
                .with_generated_source("generated-root", source)
                .with_build_unit_input("runtime", "sha256:shared-runtime"),
            )
        };
        let first_receipt = receipt_for(first_project.path(), &first_source)?;
        let second_receipt = receipt_for(second_project.path(), &second_source)?;
        assert_ne!(first_receipt.identity, second_receipt.identity);
        assert_eq!(first_receipt.build_unit_identity, second_receipt.build_unit_identity);
        let first_request = OvenArtifactPublishRequest {
            receipt: first_receipt,
            domain: "shared-Loaf".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: b"shared payload".to_vec(),
            materialized_files: Vec::new(),
        };
        let second_request = OvenArtifactPublishRequest {
            receipt: second_receipt,
            ..first_request.clone()
        };

        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let first = store.publish(&first_request)?;
        let second = store.publish(&second_request)?;

        assert_eq!(second.identity, first.identity);
        assert_eq!(store.inspect()?.entries.len(), 1);
        Ok(())
    }

    #[test]
    fn legacy_publisher_reservation_prunes_inactive_entries_and_refuses_active_leases()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let first = store.publish(&request(project.path(), "engine-one", b"first publisher entry")?)?;
        let (_entry, lease) = store.select(&first.identity)?;

        let blocked = store.reserve_legacy_cargo_publisher_capacity();
        assert!(matches!(blocked, Err(OvenStoreError::CapacityBlocked { .. })));
        assert_eq!(store.inspect()?.entries.len(), 1);

        drop(lease);
        let report = store.reserve_legacy_cargo_publisher_capacity()?;
        assert_eq!(report.removed_entries, vec![first.identity]);
        assert!(store.inspect()?.entries.is_empty());
        Ok(())
    }

    #[test]
    fn normal_publication_refuses_active_legacy_publisher_staging() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let staging = temp.path().join(LEGACY_CARGO_STAGING_DIRECTORY);
        fs::create_dir_all(&staging)?;
        let publisher_lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(staging.join(LEGACY_CARGO_PUBLISHER_LOCK_FILE))?;
        publisher_lock.lock()?;

        let blocked = store.publish(&request(project.path(), "engine-one", b"blocked")?);
        assert!(matches!(
            blocked,
            Err(OvenStoreError::LegacyPublisherStagingActive { .. })
        ));

        publisher_lock.unlock()?;
        assert!(
            store
                .publish(&request(project.path(), "engine-one", b"unblocked")?)
                .is_ok()
        );
        Ok(())
    }

    #[test]
    fn legacy_publisher_batch_preflight_counts_private_staging_and_copied_sources()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(60 * 1024, 60 * 1024, 1_000_000));
        let staging = temp
            .path()
            .join(LEGACY_CARGO_STAGING_DIRECTORY)
            .join(".legacy-cargo-fixture");
        let staged_source = staging.join("native/staged.rlib");
        let copied_source = temp.path().join("outside/copied.rlib");
        fs::create_dir_all(staged_source.parent().ok_or("staged parent missing")?)?;
        fs::create_dir_all(copied_source.parent().ok_or("copied parent missing")?)?;
        fs::write(&staged_source, vec![b's'; 32 * 1024])?;
        fs::write(&copied_source, vec![b'c'; 32 * 1024])?;
        let mut publication = request(project.path(), "engine-one", b"plan")?;
        publication.materialized_files = vec![
            OvenArtifactMaterializedFile {
                source_path: staged_source,
                relative_path: "native/staged.rlib".to_string(),
            },
            OvenArtifactMaterializedFile {
                source_path: copied_source,
                relative_path: "native/copied.rlib".to_string(),
            },
        ];

        let result = store.ensure_legacy_cargo_batch_physical_capacity(&staging, &[publication]);
        assert!(matches!(result, Err(OvenStoreError::CapacityBlocked { .. })));
        Ok(())
    }

    #[test]
    fn batch_execution_leases_protect_every_selected_shard_from_policy_pruning()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let permissive = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let first = permissive.publish(&request(project.path(), "suite-shard-one", b"first shard")?)?;
        let second = permissive.publish(&request(project.path(), "suite-shard-two", b"second shard")?)?;
        let retained_physical = permissive.inspect()?.physical_bytes;
        let bounded = OvenStore::new(
            temp.path(),
            OvenStoreLimits::new(retained_physical.saturating_add(1), 1_000_000, 1_000_000),
        );

        let selected = bounded.select_payloads_for_execution(&[first.identity.clone(), second.identity.clone()])?;
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].manifest.identity, first.identity);
        assert_eq!(selected[0].payload, b"first shard");
        assert_eq!(selected[1].manifest.identity, second.identity);
        assert_eq!(selected[1].payload, b"second shard");
        let inspection = bounded.inspect()?;
        assert_eq!(inspection.active_lease_physical_bytes, inspection.physical_bytes);

        let blocked = bounded.publish(&request(project.path(), "suite-shard-three", b"third shard")?);
        assert!(matches!(blocked, Err(OvenStoreError::CapacityBlocked { .. })));
        assert_eq!(bounded.inspect()?.entries.len(), 2);

        drop(selected);
        let third = bounded.publish(&request(project.path(), "suite-shard-three", b"third shard")?)?;
        let identities = bounded
            .inspect()?
            .entries
            .into_iter()
            .map(|entry| entry.manifest.identity)
            .collect::<Vec<_>>();
        assert!(identities.contains(&third.identity));
        Ok(())
    }

    #[test]
    fn batch_execution_rejects_empty_or_duplicate_identity_sets() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let artifact = store.publish(&request(project.path(), "suite-shard", b"shard")?)?;

        assert!(matches!(
            store.select_payloads_for_execution(&[]),
            Err(OvenStoreError::InvalidInput {
                field: "execution identities",
                ..
            })
        ));
        assert!(matches!(
            store.select_payloads_for_execution(&[artifact.identity.clone(), artifact.identity]),
            Err(OvenStoreError::InvalidInput {
                field: "execution identities",
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn batch_publication_admits_all_related_entries_in_one_domain() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));

        let requests = [
            request(project.path(), "compiler-suite", b"suite index")?,
            request(project.path(), "compiler-suite", b"suite shard")?,
        ];
        let previews = requests
            .iter()
            .map(|request| store.manifest_for_publication(request))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(store.inspect()?.entries.len(), 0);

        let manifests = store.publish_batch(&requests)?;

        assert_eq!(manifests.len(), 2);
        assert_eq!(manifests, previews);
        let entries = store.inspect()?.entries;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].manifest.domain, "compiler-suite");
        assert_eq!(entries[1].manifest.domain, "compiler-suite");
        assert!(entries.iter().all(|entry| manifests.contains(&entry.manifest)));
        Ok(())
    }

    #[test]
    fn compiler_suite_batch_commits_its_authority_index_after_durable_members() -> Result<(), Box<dyn std::error::Error>>
    {
        use std::cell::Cell;

        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let mut index = request(project.path(), "compiler-suite", b"suite index")?;
        index.kind = OvenArtifactKind::CompilerTestSuite;
        let mut shard = request(project.path(), "compiler-suite", b"suite shard")?;
        shard.kind = OvenArtifactKind::CompilerTestSuiteShard;
        let index_identity = store.manifest_for_publication(&index)?.identity;
        let shard_identity = store.manifest_for_publication(&shard)?.identity;
        let reached_commit_point = Cell::new(false);

        let interrupted = store.publish_batch_with_legacy_cargo_publisher_permission_and_commit_hook(
            &[index.clone(), shard.clone()],
            false,
            || {
                reached_commit_point.set(true);
                assert!(store.entry_root(&shard_identity).is_dir());
                assert!(!store.entry_root(&index_identity).exists());
                Err(OvenStoreError::InvalidInput {
                    field: "test compiler-suite commit hook",
                    message: "simulated interruption before authority commit".to_string(),
                })
            },
        );

        assert!(matches!(interrupted, Err(OvenStoreError::InvalidInput { .. })));
        assert!(reached_commit_point.get());
        assert!(store.inspect()?.entries.is_empty());
        let published = store.publish_batch(&[index, shard])?;
        assert_eq!(published.len(), 2);
        assert!(store.select(&index_identity).is_ok());
        assert!(store.select(&shard_identity).is_ok());
        Ok(())
    }

    #[test]
    fn related_batch_refuses_foundation_partitions_that_overflow_one_compatibility_domain()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        // Each partition is below 12 logical bytes, while the complete suite closure is deliberately larger. A
        // suite selects both foundations, so they share a compatibility domain and fail as one overage.
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 12));
        let requests = [
            request(project.path(), "compiler-suite", b"foundation-a")?,
            request(project.path(), "compiler-suite", b"foundation-b")?,
        ];

        assert!(matches!(
            store.publish_batch(&requests),
            Err(OvenStoreError::CapacityBlocked { .. })
        ));
        assert!(store.inspect()?.entries.is_empty());
        Ok(())
    }

    #[test]
    fn related_batch_keeps_all_active_leases_safe_under_aggregate_pressure() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let permissive = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let first = permissive.publish(&request(project.path(), "foundation-a", b"retained foundation a")?)?;
        let second = permissive.publish(&request(project.path(), "foundation-b", b"retained foundation b")?)?;
        let retained_physical = permissive.inspect()?.physical_bytes;
        let bounded = OvenStore::new(
            temp.path(),
            OvenStoreLimits::new(retained_physical.saturating_add(1), 1_000_000, 1_000_000),
        );
        let leases = bounded.select_payloads_for_execution(&[first.identity, second.identity])?;
        let requests = [
            request(project.path(), "foundation-c", b"incoming foundation c")?,
            request(project.path(), "foundation-d", b"incoming foundation d")?,
        ];

        let result = bounded.publish_batch(&requests);
        assert!(matches!(result, Err(OvenStoreError::CapacityBlocked { .. })));
        let inspection = bounded.inspect()?;
        assert_eq!(inspection.entries.len(), 2);
        assert_eq!(inspection.active_lease_physical_bytes, inspection.physical_bytes);
        drop(leases);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn batch_publication_shares_identical_materialized_closure_files_once_physically()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::MetadataExt;

        let sizing_root = tempfile::tempdir()?;
        let bounded_root = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        let publisher = tempfile::tempdir()?;
        write_project(project.path())?;
        let first_source = publisher.path().join("first/libshared.rlib");
        let second_source = publisher.path().join("second/libshared.rlib");
        let shared_bytes = vec![7_u8; 256 * 1024];
        fs::create_dir_all(first_source.parent().ok_or("first source parent missing")?)?;
        fs::create_dir_all(second_source.parent().ok_or("second source parent missing")?)?;
        fs::write(&first_source, &shared_bytes)?;
        fs::write(&second_source, &shared_bytes)?;

        let requests = || -> Result<[OvenArtifactPublishRequest; 2], Box<dyn std::error::Error>> {
            let mut first = request(project.path(), "compiler-suite", b"suite index")?;
            first.materialized_files = vec![OvenArtifactMaterializedFile {
                source_path: first_source.clone(),
                relative_path: "closure/libshared.rlib".to_string(),
            }];
            let mut second = request(project.path(), "compiler-suite", b"suite shard")?;
            second.materialized_files = vec![OvenArtifactMaterializedFile {
                source_path: second_source.clone(),
                relative_path: "closure/libshared.rlib".to_string(),
            }];
            Ok([first, second])
        };

        let sizing = OvenStore::new(
            sizing_root.path(),
            OvenStoreLimits::new(2_000_000, 2_000_000, 2_000_000),
        );
        sizing.publish_batch(&requests()?)?;
        let sized = sizing.inspect()?;
        assert!(sized.physical_bytes < sized.logical_bytes);

        let bounded = OvenStore::new(
            bounded_root.path(),
            OvenStoreLimits::new(
                sized.physical_bytes.saturating_add(64 * 1024),
                sized.physical_bytes.saturating_add(64 * 1024),
                sized.logical_bytes.saturating_add(1),
            ),
        );
        let manifests = bounded.publish_batch(&requests()?)?;
        let inspection = bounded.inspect()?;
        assert_eq!(inspection.entries.len(), 2);
        assert_eq!(inspection.logical_bytes, sized.logical_bytes);
        assert_eq!(inspection.physical_bytes, sized.physical_bytes);
        assert!(inspection.physical_bytes < inspection.logical_bytes);

        let first_path = bounded
            .select(&manifests[0].identity)?
            .0
            .materialized_root()
            .join("closure/libshared.rlib");
        let second_path = bounded
            .select(&manifests[1].identity)?
            .0
            .materialized_root()
            .join("closure/libshared.rlib");
        assert_eq!(fs::metadata(first_path)?.nlink(), 2);
        assert_eq!(fs::metadata(second_path)?.nlink(), 2);

        let selected = bounded.select_payloads_for_execution(
            &manifests
                .iter()
                .map(|manifest| manifest.identity.clone())
                .collect::<Vec<_>>(),
        )?;
        let unique_source = publisher.path().join("third/libunique.rlib");
        fs::create_dir_all(unique_source.parent().ok_or("unique source parent missing")?)?;
        fs::write(&unique_source, vec![9_u8; 256 * 1024])?;
        let lease_bounded = OvenStore::new(
            bounded_root.path(),
            OvenStoreLimits::new(
                sized.physical_bytes.saturating_add(64 * 1024),
                sized.physical_bytes.saturating_add(64 * 1024),
                2_000_000,
            ),
        );
        let mut blocked_request = request(project.path(), "compiler-suite", b"new suite shard")?;
        blocked_request.materialized_files = vec![OvenArtifactMaterializedFile {
            source_path: unique_source,
            relative_path: "closure/libunique.rlib".to_string(),
        }];
        assert!(matches!(
            lease_bounded.publish(&blocked_request),
            Err(OvenStoreError::CapacityBlocked { .. })
        ));
        let protected = lease_bounded.inspect()?;
        assert_eq!(protected.entries.len(), 2);
        assert_eq!(protected.active_lease_physical_bytes, protected.physical_bytes);
        drop(selected);
        Ok(())
    }

    #[test]
    fn batch_publication_refuses_all_members_when_one_domain_cannot_fit_them() -> Result<(), Box<dyn std::error::Error>>
    {
        let sizing_root = tempfile::tempdir()?;
        let bounded_root = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let sizing = OvenStore::new(
            sizing_root.path(),
            OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000),
        );
        sizing.publish(&request(project.path(), "compiler-suite", b"suite shard one")?)?;
        let one_entry_physical = sizing.inspect()?.physical_bytes;
        let bounded = OvenStore::new(
            bounded_root.path(),
            OvenStoreLimits::new(
                one_entry_physical.saturating_add(1),
                one_entry_physical.saturating_add(1),
                1_000_000,
            ),
        );

        let result = bounded.publish_batch(&[
            request(project.path(), "compiler-suite", b"suite shard one")?,
            request(project.path(), "compiler-suite", b"suite shard two")?,
        ]);

        assert!(matches!(result, Err(OvenStoreError::CapacityBlocked { .. })));
        assert!(bounded.inspect()?.entries.is_empty());
        Ok(())
    }

    #[test]
    fn dry_run_prune_reports_policy_reclamation_without_removing_entries() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let permissive = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let first = permissive.publish(&request(project.path(), "engine-one", b"first payload")?)?;
        let first_physical = permissive.inspect()?.physical_bytes;
        let bounded = OvenStore::new(
            temp.path(),
            OvenStoreLimits::new(first_physical.saturating_sub(1), 1_000_000, 1_000_000),
        );

        let preview = bounded.preview_prune()?;
        assert!(preview.dry_run);
        assert_eq!(
            preview.removed_entries.as_slice(),
            std::slice::from_ref(&first.identity)
        );
        assert_eq!(preview.after_physical_bytes, 0);
        assert!(
            bounded.select(&first.identity).is_ok(),
            "a dry run must retain the selected entry"
        );

        let applied = bounded.prune()?;
        assert!(!applied.dry_run);
        assert_eq!(applied.removed_entries, [first.identity]);
        assert!(bounded.inspect()?.entries.is_empty());
        Ok(())
    }

    #[test]
    fn inspection_reclaims_stale_staging_before_reporting_physical_usage() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        store.publish(&request(project.path(), "engine-arm64", b"payload")?)?;
        let stale = temp.path().join("staging").join("interrupted-publication");
        fs::create_dir_all(&stale)?;
        fs::write(stale.join("payload"), vec![0_u8; 32 * 1024])?;

        let inspection = store.inspect()?;
        assert!(!stale.exists());
        assert_eq!(inspection.entries.len(), 1);
        assert!(inspection.physical_bytes >= inspection.logical_bytes);
        Ok(())
    }

    fn request(
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
        })
    }

    fn write_project(root: &Path) -> Result<(), std::io::Error> {
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"store_fixture\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(root.join("Cargo.lock"), "version = 4\n")?;
        Ok(())
    }
}

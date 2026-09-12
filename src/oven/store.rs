//! Bounded, lease-aware storage for immutable Oven Alpha artifacts.
//!
//! This store is intentionally separate from generated Cargo targets. It owns versioned Oven artifacts only, reports
//! logical artifact bytes and measured physical file allocation separately, and refuses publication when its active
//! leases leave no safe way to satisfy capacity policy.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{OvenBuildIntent, OvenReceipt, digest_bytes};

/// Version of the persistent Oven artifact-store layout.
pub const OVEN_STORE_SCHEMA_VERSION: u32 = 4;
const ENTRIES_DIRECTORY: &str = "entries";
const STAGING_DIRECTORY: &str = "staging";
const ARTIFACT_MANIFEST_FILE: &str = "artifact.json";
const LOAF_MANIFEST_FILE: &str = "loaf.json";
const PAYLOAD_FILE: &str = "payload";
const MATERIALIZED_DIRECTORY: &str = "artifacts";
const ACCESS_FILE: &str = "last-used";
/// Sidecar cache for one immutable entry's recursively measured physical allocation.
///
/// Entries under [`ENTRIES_DIRECTORY`] never change after publication, so a physical-byte measurement taken once
/// stays valid for the entry's lifetime. Missing or unreadable cache files fall back to a fresh recursive walk, so
/// this is purely an optimization: it never becomes a second source of truth for admission decisions.
const PHYSICAL_BYTES_CACHE_FILE: &str = ".physical-bytes-cache";
/// Sidecar cache for one immutable entry's `(device, inode, size)` regular-file identities.
///
/// Cross-entry hard-link deduplication (see [`assign_unique_entry_physical_bytes`]) needs each file's inode
/// identity, not just an aggregate byte count, so it cannot reuse [`PHYSICAL_BYTES_CACHE_FILE`] directly. Entries
/// are immutable once published, so this listing is as safe to cache as the plain byte total; a missing or
/// unparsable cache file falls back to a fresh recursive walk exactly like its sibling.
const NATIVE_RECEIPT_FILE: &str = "native-receipt.json";
const NATIVE_RECEIPT_SCHEMA_VERSION: u32 = 1;
const MAX_NATIVE_RECEIPT_BYTES: u64 = 16 * 1024 * 1024;
const UNIQUE_FILE_RECORDS_CACHE_FILE: &str = ".physical-bytes-unique-cache";
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

/// Resolve the bounded store directory that belongs to one Incan home.
///
/// The `oven/store/v2` layout is the store's own, so every caller that has a home directory and needs the store
/// derives the path here instead of restating the segments and drifting when the layout version moves.
#[must_use]
pub fn store_root_for_home(home: &Path) -> PathBuf {
    home.join("oven").join("store").join("v2")
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
    /// Completed project-native output selected before frontend work on an exact authored-source match.
    ProjectOutput,
    /// One immutable JEC result and its compiler-produced logical dependency observation.
    NativeCompilationOutput,
    /// One immutable compiler/sysroot closure selected under lease for JEC lookup and execution.
    NativeCompilerClosure,
    /// Compiler-bound Rust source, cfg, and target facts selected under lease for semantic inspection.
    RustInspectionToolchain,
    /// Project-level Rust inspection authority selected only through a source-current completed project output.
    ProjectInspectionAuthority,
    /// Verified direct-rustc artifact plan consumed by a later executor stage.
    DirectRustcPlan,
    /// Publisher-prepared compiler test executable and matching CLI, executed later without Cargo.
    CompilerTestSuite,
    /// One independently admitted direct-rustc compiler-suite shard referenced by a small suite index.
    CompilerTestSuiteShard,
    /// One immutable Cargo-free runtime closure rebuilt above a sealed SDK runtime foundation.
    NativeRuntimeClosure,
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
    /// Complete receipt of the compilation that produced these bytes, when the entry retains one.
    ///
    /// The manifest already names that receipt's identity. This is the record behind the name, which is what a
    /// reader of a *reused* artifact needs: reuse hands a consumer bytes some other invocation produced, and the
    /// identity alone does not say with which intent, toolchain, or inputs. `None` is an entry published before
    /// the witness existed, or one whose kind never carries it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_native_receipt: Option<OvenReceipt>,
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
    /// Canonical selected entry coordinate, retained independently of the public materialized root.
    admitted_entry_root: PathBuf,
    /// Original content identity; public record mutation cannot retarget the held lease.
    admitted_identity: String,
    /// Publisher's own receipt, retained beside the entry and decoded once at admission.
    ///
    /// A reused output's manifest names the receipt that authorizes *this* selection; this is the receipt of the
    /// compilation that actually produced the bytes. They differ precisely when reuse happened, which is the case a
    /// consumer needs to be able to report. `None` is an entry published before the witness existed, which is
    /// legacy evidence rather than an empty recipe.
    original_native_receipt: Option<AdmittedNativeReceipt>,
    _lease: OvenStoreLease,
}

impl OvenStoreExecutionPayload {
    /// Borrow the original publisher recipe; a legacy entry without a witness does not acquire new receipt facts.
    ///
    /// `OvenStoreEntry` exposes the same record for inspection, which is the surface a reader uses. This borrow is
    /// for an executor holding a lease on reused bytes, and its production consumer is the runtime input owner that
    /// arrives with the Oven runtime substrate (#1037). The retention itself is not speculative: the witness digest
    /// is re-checked in `verify_admitted_payload` on every held payload, whether or not the receipt is read.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn original_native_receipt(&self) -> Option<&OvenReceipt> {
        self.original_native_receipt.as_ref().map(|witness| &witness.receipt)
    }

    /// Revalidate the original admitted record, payload and complete materialized closure under the held lease.
    ///
    /// A lease protects the selected entry from store pruning; it does not authenticate mutable public fields. This
    /// checks those fields against the original selected coordinate and content identity before a new physical
    /// consumer borrows them. The existing materialized validator performs the sole full artifact walk.
    pub(crate) fn verify_admitted_payload(&self) -> Result<(), OvenStoreError> {
        let manifest = verify_published_entry_manifest(&self.admitted_entry_root)?;
        if manifest.identity != self.admitted_identity
            || manifest != self.manifest
            || self.artifact_root != self.admitted_entry_root.join(MATERIALIZED_DIRECTORY)
        {
            return Err(OvenStoreError::Integrity {
                identity: self.admitted_identity.clone(),
                message: "execution payload no longer matches its original admitted record and root".to_string(),
            });
        }
        if u64::try_from(self.payload.len()).ok() != Some(manifest.payload.logical_bytes)
            || digest_bytes(&self.payload) != manifest.payload.digest
        {
            return Err(OvenStoreError::Integrity {
                identity: self.admitted_identity.clone(),
                message: "execution payload bytes disagree with the original admitted descriptor".to_string(),
            });
        }
        // The lease stops the entry being pruned; it does not stop the witness file being rewritten underneath a
        // held payload. Comparing the digest admission recorded is what makes the retained receipt evidence rather
        // than a cached read.
        let witness_bytes = read_native_receipt_bytes(&self.admitted_entry_root, &manifest)?;
        let current_digest = witness_bytes.as_deref().map(digest_bytes);
        if current_digest.as_deref()
            != self
                .original_native_receipt
                .as_ref()
                .map(|witness| witness.bytes_digest.as_str())
        {
            return Err(OvenStoreError::Integrity {
                identity: self.admitted_identity.clone(),
                message: "original native receipt witness changed after admission".to_string(),
            });
        }
        verify_materialized_root(&self.admitted_entry_root, &manifest)?;
        verify_materialized_files(&self.admitted_entry_root, &manifest).map(|_| ())
    }

    /// Verify the complete materialized file closure while retaining this payload's active lease.
    ///
    /// Call before importing the source files. An already admitted destination can reuse its own leased content
    /// without rereading the source closure. The check is anchored to the originally admitted entry and also
    /// revalidates the mutable public manifest, artifact root, and payload bytes before trusting that closure. This
    /// verification does not populate physical-accounting caches.
    pub fn verify_materialized_files(&self) -> Result<(), OvenStoreError> {
        self.verify_admitted_payload()
    }

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

/// Capacity reserved for one serialized compatibility-baker staging run.
///
/// The publisher lock prevents another compatibility baker from consuming this allowance concurrently. Active
/// immutable entries remain leased and therefore reduce, rather than invalidate, the remaining staging budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OvenLegacyCargoPublisherReservation {
    /// Applied inactive-entry reclamation preceding this serialized publication.
    pub(crate) prune_report: OvenStorePruneReport,
    /// Maximum measured physical allocation allowed below the private publisher staging root.
    pub(crate) transient_limit_bytes: u64,
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
    #[error(
        "Oven store internal compatibility publisher staging is active at {path}; retry publication after it completes"
    )]
    LegacyPublisherStagingActive { path: PathBuf },
}

/// Root handle for a bounded Oven artifact store.
#[derive(Debug, Clone)]
pub struct OvenStore {
    root: PathBuf,
    limits: OvenStoreLimits,
}

/// Read-only access to an Oven store embedded in a published, content-addressed package.
///
/// Published packages are immutable inputs, not LRU caches. This handle requires their existing lock files and
/// verifies selected content while retaining the same active leases as a writable store. It never creates layout,
/// reclaims staging, records access times, or populates accounting caches.
#[derive(Debug, Clone)]
pub struct PublishedOvenStore {
    root: PathBuf,
}

impl PublishedOvenStore {
    /// Refer to an already published store; selection validates its existing layout and locks.
    #[must_use]
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    /// Read verified manifests and payloads without changing package files.
    ///
    /// The shared manager lock prevents a concurrent publisher from pruning candidates before their active leases
    /// are acquired. Missing locks are errors: a consumer cannot repair a published artifact in place. Before
    /// importing source files, verify their closure with [`OvenStoreExecutionPayload::verify_materialized_files`].
    pub fn select_payloads_matching_for_execution<F>(
        &self,
        matches: F,
    ) -> Result<Vec<OvenStoreExecutionPayload>, OvenStoreError>
    where
        F: Fn(&OvenArtifactManifest) -> bool,
    {
        let manager_path = self.root.join(MANAGER_LOCK_FILE);
        let manager = File::open(&manager_path).map_err(|source| OvenStoreError::Io {
            path: manager_path.clone(),
            source,
        })?;
        manager.lock_shared().map_err(|source| OvenStoreError::Io {
            path: manager_path,
            source,
        })?;
        Ok(
            select_matching_execution_payloads(&self.root.join(ENTRIES_DIRECTORY), matches)?
                .into_iter()
                .map(|(_, payload)| payload)
                .collect(),
        )
    }
}

/// Validated batch member retained while the store serializes one related publication.
struct PreparedOvenArtifactPublication<'a> {
    request: &'a OvenArtifactPublishRequest,
    manifest: OvenArtifactManifest,
    /// Encoded witness for this member, computed with the rest of the batch so capacity is reserved for it.
    native_receipt_bytes: Option<Vec<u8>>,
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
        self.publish_with_legacy_cargo_publisher_permission(request, false, true)
    }

    /// Publish a portable package constituent under its exact receipt-bound identity.
    ///
    /// A shared direct plan may be reusable under a compatible receipt in its originating store. A package handoff,
    /// however, records the precise receipt that owns the provider output, so its destination store must retain that
    /// receipt-specific manifest instead of returning another entry with equivalent bytes and older provenance.
    pub(crate) fn publish_receipt_bound(
        &self,
        request: &OvenArtifactPublishRequest,
    ) -> Result<OvenArtifactManifest, OvenStoreError> {
        self.publish_with_legacy_cargo_publisher_permission(request, false, false)
    }

    /// Publish one immutable result owned by the explicit compatibility baker.
    ///
    /// The caller must already have reserved the remaining aggregate/domain allowance through
    /// [`Self::reserve_legacy_cargo_publisher_capacity`]. This narrow entry point lets that owner finish its atomic
    /// hand-off while ordinary publishers refuse to overlap its private staging allocation.
    pub(crate) fn publish_from_legacy_cargo(
        &self,
        request: &OvenArtifactPublishRequest,
    ) -> Result<OvenArtifactManifest, OvenStoreError> {
        self.publish_with_legacy_cargo_publisher_permission(request, true, true)
    }

    /// Implement one publication, admitting the active legacy publisher only through its named transition boundary.
    fn publish_with_legacy_cargo_publisher_permission(
        &self,
        request: &OvenArtifactPublishRequest,
        allow_legacy_cargo_publisher: bool,
        reuse_equivalent_direct_plan: bool,
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

        let entry_path = self.entry_root_for_kind(&manifest.identity, manifest.kind);
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
        if reuse_equivalent_direct_plan
            && let Some((existing, existing_path)) = self.reusable_existing_manifest(&manifest)?
        {
            touch_entry(&existing_path)?;
            return Ok(existing);
        }

        let native_receipt_bytes = encode_native_receipt(request)?;
        let estimated_physical = conservative_physical_reservation(&manifest, native_receipt_bytes.as_deref())?;
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
            native_receipt_bytes.as_deref(),
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
                // Reuse hands a later consumer bytes this publisher did not produce. Admitting the retained
                // witness here is what refuses a corrupt or contradictory one at the moment of reuse rather than
                // at whichever later selection happens to read it.
                admit_native_receipt(&path, &existing)?;
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
                native_receipt_bytes: encode_native_receipt(request)?,
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
            let existing = self.entry_root_for_kind(&publication.manifest.identity, publication.manifest.kind);
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
                    publication.native_receipt_bytes.as_deref(),
                    &mut reserved_materialized_files,
                )?);
            let domain_reservation = conservative_physical_reservation_with_shared_materialized_files(
                &publication.manifest,
                publication.native_receipt_bytes.as_deref(),
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
                publication.native_receipt_bytes.as_deref(),
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
            let destination = self.entry_root_for_kind(&publication.manifest.identity, publication.manifest.kind);
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
        admit_native_receipt(&path, &manifest)?;
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
        for identity in identities {
            validate_entry_identity(identity)?;
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
            let path = canonical_published_entry_root(&self.entry_root(identity))?;
            let manifest = verify_published_entry_manifest(&path)?;
            verify_requested_entry_identity(identity, &manifest)?;
            let payload = verified_payload_bytes(&path, &manifest)?;
            let lease = acquire_execution_lease(&path, true)?;
            let original_native_receipt = admit_native_receipt(&path, &manifest)?;
            touch_entry(&path)?;
            selected.push(OvenStoreExecutionPayload {
                admitted_entry_root: path.clone(),
                admitted_identity: identity.clone(),
                original_native_receipt,
                manifest,
                artifact_root: path.join(MATERIALIZED_DIRECTORY),
                payload,
                _lease: lease,
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
        select_matching_execution_payloads(&self.entries_root(), matches)?
            .into_iter()
            .map(|(path, payload)| {
                touch_entry(&path)?;
                Ok(payload)
            })
            .collect()
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

    /// Return manifest-bound accounting for an exact warm reuse without rehashing every immutable artifact.
    ///
    /// The store manager lock excludes concurrent staging and pruning while allocation and lease state are measured.
    /// Full byte verification remains the contract of [`Self::inspect`] and selection; this path is only for a baker
    /// that has already matched its exact preparation identity and will verify each selected entry before execution.
    pub(crate) fn inspect_for_exact_reuse(&self) -> Result<OvenStoreInspection, OvenStoreError> {
        self.ensure_layout()?;
        let manager = open_lock(&self.root.join(MANAGER_LOCK_FILE))?;
        manager.lock().map_err(|source| OvenStoreError::Io {
            path: self.root.join(MANAGER_LOCK_FILE),
            source,
        })?;
        self.reclaim_stale_staging()?;
        let entries = self.collect_entries_for_admission()?;
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
        self.prune_with_superseded_release_reclamation(true)
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
        self.prune_with_superseded_release_reclamation(false)
    }

    /// Reserve the remaining aggregate and compatibility-domain allowance for the explicit compatibility baker.
    ///
    /// A live lease must never be pruned. The old all-or-nothing reservation treated even a tiny live Loaf as a
    /// reason to reject the next serialized bake, which made debug/release preparation impossible in one process.
    /// Instead, inactive entries are reclaimed as before, active entries stay intact, and Cargo's staging monitor is
    /// capped at the exact remaining aggregate/domain capacity. The publisher lock excludes another staging writer
    /// while that cap is in force, so this remains a hard physical bound rather than post-hoc accounting.
    pub(crate) fn reserve_legacy_cargo_publisher_capacity(
        &self,
        domain: &str,
    ) -> Result<OvenLegacyCargoPublisherReservation, OvenStoreError> {
        if domain.trim().is_empty() {
            return Err(OvenStoreError::InvalidInput {
                field: "compatibility baker domain",
                message: "must not be empty".to_string(),
            });
        }
        self.ensure_layout()?;
        let manager = open_lock(&self.root.join(MANAGER_LOCK_FILE))?;
        manager.lock().map_err(|source| OvenStoreError::Io {
            path: self.root.join(MANAGER_LOCK_FILE),
            source,
        })?;
        self.reclaim_stale_staging()?;
        // Entries sealed by a superseded compiler release can never be reused, so they are removed before the
        // remaining allowance is measured. Without this a store that merely fits its retention policy can still
        // starve a large build of transient staging space with bytes nothing will ever read again.
        if self.holds_superseded_release_entry(domain)? {
            self.reclaim_superseded_release_entries(domain, true)?;
        }
        // A reservation is not itself evidence that an existing immutable entry is obsolete. Keep every entry that
        // already fits policy so a debug/release sibling or a second project can reuse it; the measured hand-off
        // below is where the actual pending closure is admitted and any necessary inactive reclamation occurs.
        // The staging monitor still receives only the remaining aggregate/domain allowance, so this preserves the
        // hard transient bound without turning each explicit bake into a cache flush.
        let report = self.prune_to_limits(None, 0, 0, true)?;
        let entries = self.collect_entries_for_admission()?;
        let retained_physical_bytes = entries.iter().map(|entry| entry.physical_bytes).sum::<u64>();
        let (_, retained_domain_physical_bytes) = domain_totals(&entries, domain);
        let aggregate_remaining = self.limits.max_physical_bytes.saturating_sub(retained_physical_bytes);
        let domain_remaining = self
            .limits
            .max_domain_physical_bytes
            .saturating_sub(retained_domain_physical_bytes);
        let transient_limit_bytes = aggregate_remaining.min(domain_remaining);
        if transient_limit_bytes == 0 {
            return Err(OvenStoreError::CapacityBlocked {
                domain: domain.to_string(),
                message: format!(
                    "active retained physical bytes leave no compatibility-baker staging capacity; skipped active entries {:?}. Run `incan oven store inspect`, then `incan oven store prune --max-physical-bytes <bytes>` to reclaim inactive artifacts before retrying; active leases remain protected",
                    report.skipped_active_entries,
                ),
            });
        }
        Ok(OvenLegacyCargoPublisherReservation {
            prune_report: report,
            transient_limit_bytes,
        })
    }

    /// Refuse the named publisher's final hand-off when its live private staging plus every new immutable file would
    /// exceed the aggregate physical policy.
    ///
    /// Materialized files beneath `legacy-cargo-staging` are hard-linked into the atomic entry staging, so they are
    /// counted once here. Sources outside that private tree are copied by [`write_staged_entry`] and are reserved
    /// once per digest/executable pair, exactly as the batch writer shares them. The publisher reservation can retain
    /// leased entries while capping staging at the remaining capacity, so this hand-off includes those entries again
    /// and remains safe if another explicit transition owner reuses the primitive.
    pub(crate) fn ensure_legacy_cargo_batch_physical_capacity(
        &self,
        staging: &Path,
        requests: &[OvenArtifactPublishRequest],
    ) -> Result<(), OvenStoreError> {
        if requests.is_empty() {
            return Err(OvenStoreError::InvalidInput {
                field: "internal compatibility publication batch",
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
                field: "internal compatibility publisher staging",
                message: format!(
                    "{} must remain below the private publisher root {}",
                    staging.display(),
                    publisher_root.display()
                ),
            });
        }
        let mut observed_physical = unique_publisher_staging_physical_bytes(&staging)?;
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
                    field: "internal compatibility publication payload",
                    message: "length does not fit supported physical accounting".to_string(),
                })?,
            ));
            let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|error| OvenStoreError::Manifest {
                path: PathBuf::from(ARTIFACT_MANIFEST_FILE),
                message: error.to_string(),
            })?;
            let manifest_length = u64::try_from(manifest_bytes.len()).map_err(|_| OvenStoreError::Manifest {
                path: PathBuf::from(ARTIFACT_MANIFEST_FILE),
                message: "manifest length does not fit supported physical accounting".to_string(),
            })?;
            observed_physical = observed_physical
                .saturating_add(round_physical(manifest_length))
                // `write_staged_entry` adds the trailing manifest newline and the mutable access timestamp.
                .saturating_add(round_physical(1))
                .saturating_add(round_physical(20));
            // `manifest_for_publication` has just read and digest-validated this request's files. Capacity needs the
            // canonical source location plus the resulting manifest accounting, not a second full content read.
            // `publish_batch` validates them again at the actual atomic hand-off, so a source mutation between these
            // phases still fails closed rather than changing the admitted identity.
            let source_paths = request
                .materialized_files
                .iter()
                .map(|file| {
                    let relative = normalized_materialized_relative_path(&file.relative_path)?;
                    Ok((relative, file.source_path.clone()))
                })
                .collect::<Result<BTreeMap<_, _>, OvenStoreError>>()?;
            for file in &manifest.materialized_files {
                let source_path = source_paths
                    .get(&file.relative_path)
                    .ok_or_else(|| OvenStoreError::Integrity {
                        identity: manifest.identity.clone(),
                        message: format!(
                            "validated manifest path `{}` has no publication source",
                            file.relative_path
                        ),
                    })?;
                let source = fs::canonicalize(source_path).map_err(|source_error| OvenStoreError::Io {
                    path: source_path.clone(),
                    source: source_error,
                })?;
                if source.starts_with(&publisher_root)
                    || !copied_materializations.insert((file.digest.clone(), file.executable))
                {
                    continue;
                }
                observed_physical = observed_physical.saturating_add(round_physical(file.logical_bytes));
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

    /// Reclaim superseded releases, then prune what retention policy still requires.
    ///
    /// Both passes run under one manager lock and are reported as a single reclamation. The superseded pass is skipped
    /// entirely when nothing qualifies, so an ordinary store pays for one measurement pass rather than two.
    fn prune_with_superseded_release_reclamation(&self, apply: bool) -> Result<OvenStorePruneReport, OvenStoreError> {
        let active = active_release_domain();
        if !self.holds_superseded_release_entry(&active)? {
            return self.prune_to_limits(None, 0, 0, apply);
        }
        let superseded = self.reclaim_superseded_release_entries(&active, apply)?;
        let retained = self.prune_to_limits(None, 0, 0, apply)?;
        Ok(merge_prune_reports(superseded, retained))
    }

    /// Return whether any immutable entry belongs to a compiler release other than the active one.
    ///
    /// Reading manifests is far cheaper than admission measurement, which walks every file in every entry to compute
    /// physical allocation. A store holding only the active release — the common case on every bake — would otherwise
    /// pay for a second full measurement pass just to discover there is nothing to reclaim.
    fn holds_superseded_release_entry(&self, active_domain: &str) -> Result<bool, OvenStoreError> {
        let root = self.entries_root();
        if !root.exists() {
            return Ok(false);
        }
        for candidate in fs::read_dir(&root).map_err(|source| OvenStoreError::Io {
            path: root.clone(),
            source,
        })? {
            let path = candidate
                .map_err(|source| OvenStoreError::Io {
                    path: root.clone(),
                    source,
                })?
                .path();
            if !path.is_dir() {
                continue;
            }
            // A malformed or half-written entry is not this probe's problem to report: the reclamation and admission
            // paths below own that judgement, and failing here would turn an optimization into a new failure mode.
            let Ok(manifest) = verify_entry_manifest(&path) else {
                continue;
            };
            if is_superseded_release_domain(&manifest.domain, active_domain) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Remove immutable entries sealed for a compiler release other than the active one.
    ///
    /// Store domains carry the compiler release that produced them (`incan-release-<version>`), and a Loaf sealed by
    /// one release is never reusable by another. Retention alone cannot shed them: [`Self::prune_to_limits`] stops as
    /// soon as the store fits its policy, so a store below its limit keeps superseded releases forever while their
    /// bytes still count against the transient allowance a large build needs. Reclaiming them is therefore always
    /// safe and is the only reclamation that does not compete with reuse.
    ///
    /// Entries an active lease holds are left in place and reported as skipped, exactly as ordinary pruning does.
    pub(crate) fn reclaim_superseded_release_entries(
        &self,
        active_domain: &str,
        apply: bool,
    ) -> Result<OvenStorePruneReport, OvenStoreError> {
        let mut entries = self.collect_entries_for_admission()?;
        let before_physical_bytes = entries.iter().map(|entry| entry.physical_bytes).sum();
        let mut removed_entries = Vec::new();
        let mut skipped_active_entries = Vec::new();
        let mut removed_logical_bytes = 0_u64;

        for entry in entries.clone() {
            if !is_superseded_release_domain(&entry.manifest.domain, active_domain) {
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
                message: "internal compatibility publisher staging root must be a regular directory".to_string(),
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
                            message: "internal compatibility publisher staging may contain only owned directories"
                                .to_string(),
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
    ///
    /// The physical directory spelling is deliberately loader-safe: a content identity uses `sha256:`, but `:` is
    /// a path-list separator in ELF `RUNPATH`. Native direct-Rustc consumers may embed this directory in an rpath,
    /// so preserving the digest identity verbatim would split one immutable directory into two invalid locations on
    /// Linux. The v2 store root adopts the safe spelling below, so prior entries are never selected into a new
    /// direct-Rustc runtime closure.
    fn entry_root(&self, identity: &str) -> PathBuf {
        let directory_name = entry_directory_name(identity);
        let loaf = self.entries_root().join(format!("{directory_name}.loaf"));
        if loaf.exists() {
            loaf
        } else {
            self.entries_root().join(directory_name)
        }
    }

    /// Return the immutable destination whose layout is owned by the artifact kind.
    fn entry_root_for_kind(&self, identity: &str, kind: OvenArtifactKind) -> PathBuf {
        self.entries_root().join(entry_directory_name_for_kind(identity, kind))
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
        self.staging_root_base().join(format!(
            "{}-{}-{sequence}",
            entry_directory_name(identity),
            std::process::id()
        ))
    }
}

/// Reject any exact-selection identity that is not one canonical content digest before filesystem resolution.
fn validate_entry_identity(identity: &str) -> Result<(), OvenStoreError> {
    let valid = identity.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    });
    if !valid {
        return Err(OvenStoreError::InvalidInput {
            field: "artifact identity",
            message: "must be `sha256:` followed by 64 lowercase hexadecimal digits".to_string(),
        });
    }
    Ok(())
}

/// Encode an immutable identity for a directory that can later appear in a native runtime search path.
///
/// The manifest continues to retain the canonical `sha256:<hex>` identity. This is only a filesystem layout detail.
fn entry_directory_name(identity: &str) -> String {
    identity
        .strip_prefix("sha256:")
        .map_or_else(|| identity.to_string(), |digest| format!("sha256-{digest}"))
}

/// Resolve one selected entry to a stable absolute coordinate after rejecting a symlink at its public leaf.
fn canonical_published_entry_root(root: &Path) -> Result<PathBuf, OvenStoreError> {
    verify_store_entry_root(root)?;
    let canonical = fs::canonicalize(root).map_err(|source| OvenStoreError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    verify_store_entry_root(&canonical)?;
    Ok(canonical)
}

/// Require one entry coordinate to name a real directory rather than a link to another authority tree.
fn verify_store_entry_root(root: &Path) -> Result<(), OvenStoreError> {
    let metadata = fs::symlink_metadata(root).map_err(|source| OvenStoreError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(OvenStoreError::Integrity {
            identity: root.display().to_string(),
            message: "store entry root must be a real directory".to_string(),
        });
    }
    Ok(())
}

/// Return the on-disk directory name for one immutable artifact kind.
///
/// Direct-rustc plans and project extensions are Oven's project-level Loafs: each carries a receipt-bound direct
/// execution contract and immutable closure fragment. Other store objects remain generic execution or scheduling
/// artifacts and therefore keep the ordinary entry spelling.
fn entry_directory_name_for_kind(identity: &str, kind: OvenArtifactKind) -> String {
    let directory = entry_directory_name(identity);
    if matches!(
        kind,
        OvenArtifactKind::DirectRustcPlan
            | OvenArtifactKind::ProjectPayload
            | OvenArtifactKind::ProjectOutput
            | OvenArtifactKind::ProjectInspectionAuthority
    ) {
        format!("{directory}.loaf")
    } else {
        directory
    }
}

/// Verify that one published manifest still occupies its identity- and kind-derived immutable coordinate.
fn verify_published_entry_coordinate(root: &Path, manifest: &OvenArtifactManifest) -> Result<(), OvenStoreError> {
    let directory_name = root.file_name().and_then(|name| name.to_str());
    let manifest_path = manifest_path_for_entry(root);
    let manifest_name = manifest_path.file_name();
    let canonical_directory = entry_directory_name_for_kind(&manifest.identity, manifest.kind);
    let canonical_manifest = std::ffi::OsStr::new(manifest_file_name(manifest.kind));
    let canonical = directory_name == Some(canonical_directory.as_str()) && manifest_name == Some(canonical_manifest);
    if !canonical {
        return Err(OvenStoreError::Integrity {
            identity: manifest.identity.clone(),
            message: "entry directory and manifest names do not match the immutable artifact coordinate".to_string(),
        });
    }
    Ok(())
}

/// Read one immutable manifest from its authenticated published coordinate, without the identity recheck.
///
/// Everything here is cheap and every bit of it still runs for every entry: the entry root's shape, the manifest
/// being a regular file rather than a symlink, the schema version, and the coordinate check that binds the
/// directory name to the identity the manifest records. What is left out is only the proof that the content
/// hashes to that identity, which `verify_published_entry_manifest` adds.
fn read_published_entry_manifest(root: &Path) -> Result<OvenArtifactManifest, OvenStoreError> {
    verify_store_entry_root(root)?;
    let manifest_path = manifest_path_for_entry(root);
    let metadata = fs::symlink_metadata(&manifest_path).map_err(|source| OvenStoreError::Io {
        path: manifest_path,
        source,
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OvenStoreError::Integrity {
            identity: root.display().to_string(),
            message: "store entry manifest must be a regular non-symlink file".to_string(),
        });
    }
    let manifest = read_entry_manifest(root)?;
    verify_published_entry_coordinate(root, &manifest)?;
    Ok(manifest)
}

/// Read one immutable manifest only from its authenticated published coordinate.
fn verify_published_entry_manifest(root: &Path) -> Result<OvenArtifactManifest, OvenStoreError> {
    let manifest = read_published_entry_manifest(root)?;
    prove_entry_manifest(root, &manifest)?;
    Ok(manifest)
}

/// Bind an exact selector's caller-provided identity to the manifest reached through its filesystem spelling.
fn verify_requested_entry_identity(
    requested_identity: &str,
    manifest: &OvenArtifactManifest,
) -> Result<(), OvenStoreError> {
    if manifest.identity != requested_identity {
        return Err(OvenStoreError::Integrity {
            identity: requested_identity.to_string(),
            message: "selected entry manifest does not match the requested identity".to_string(),
        });
    }
    Ok(())
}

/// Return the immutable manifest spelling for one artifact class.
fn manifest_file_name(kind: OvenArtifactKind) -> &'static str {
    if matches!(
        kind,
        OvenArtifactKind::DirectRustcPlan
            | OvenArtifactKind::ProjectPayload
            | OvenArtifactKind::ProjectOutput
            | OvenArtifactKind::ProjectInspectionAuthority
    ) {
        LOAF_MANIFEST_FILE
    } else {
        ARTIFACT_MANIFEST_FILE
    }
}

/// Resolve the only accepted manifest file within one complete immutable entry.
///
/// Direct-rustc project plans are written as `<identity>.loaf/loaf.json`.
/// `artifact.json` remains readable solely for pre-existing generic entries;
/// a new direct-rustc project closure never receives that spelling.
fn manifest_path_for_entry(root: &Path) -> PathBuf {
    let loaf = root.join(LOAF_MANIFEST_FILE);
    if loaf.is_file() {
        loaf
    } else {
        root.join(ARTIFACT_MANIFEST_FILE)
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

/// Lock one existing Store-owned lease file without following a replacement link or creating new authority state.
fn acquire_execution_lease(root: &Path, writable: bool) -> Result<OvenStoreLease, OvenStoreError> {
    let lease_path = root.join(ACTIVE_LOCK_FILE);
    let metadata = fs::symlink_metadata(&lease_path).map_err(|source| OvenStoreError::Io {
        path: lease_path.clone(),
        source,
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OvenStoreError::Integrity {
            identity: root.display().to_string(),
            message: "store entry lease must be a regular non-symlink file".to_string(),
        });
    }
    let file = OpenOptions::new()
        .read(true)
        .write(writable)
        .open(&lease_path)
        .map_err(|source| OvenStoreError::Io {
            path: lease_path.clone(),
            source,
        })?;
    file.lock_shared().map_err(|source| OvenStoreError::Io {
        path: lease_path,
        source,
    })?;
    Ok(OvenStoreLease { file })
}

/// Acquire matching payloads while the caller holds this store's manager lock.
///
/// Both published readers and mutable caches share manifest, payload, and active-lease validation. The caller owns
/// any additional materialized-closure verification or writable-cache bookkeeping.
fn select_matching_execution_payloads<F>(
    root: &Path,
    matches: F,
) -> Result<Vec<(PathBuf, OvenStoreExecutionPayload)>, OvenStoreError>
where
    F: Fn(&OvenArtifactManifest) -> bool,
{
    let mut selected = Vec::new();
    for candidate in fs::read_dir(root).map_err(|source| OvenStoreError::Io {
        path: root.to_path_buf(),
        source,
    })? {
        let candidate = candidate.map_err(|source| OvenStoreError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        let path = candidate.path();
        if !path.is_dir() {
            return Err(OvenStoreError::Integrity {
                identity: path.display().to_string(),
                message: "entries root contains a non-directory item".to_string(),
            });
        }
        let path = canonical_published_entry_root(&path)?;
        // Read every candidate, but prove only the ones this selection actually takes. Enumerating a store means
        // touching every entry in it — 519 in one measured store, and the largest manifests carry close to four
        // thousand materialized-file records each — while a selection usually wants a handful. Recomputing every
        // enumerated entry's identity to answer a question that only reads its fields was most of the cost of a
        // bake that compiles nothing.
        //
        // Nothing that gets used goes unproven. An entry that does not match is never executed, and one that does
        // is verified here before its payload is read, so a tampered manifest cannot reach execution by matching.
        // The cheap structural checks — entry root shape, regular non-symlink manifest, schema version, and the
        // coordinate binding the directory name to the recorded identity — still run for every entry enumerated.
        let manifest = read_published_entry_manifest(&path)?;
        if !matches(&manifest) {
            continue;
        }
        prove_entry_manifest(&path, &manifest)?;
        let payload = verified_payload_bytes(&path, &manifest)?;
        let lease = acquire_execution_lease(&path, false)?;
        let original_native_receipt = admit_native_receipt(&path, &manifest)?;
        selected.push((
            path.clone(),
            OvenStoreExecutionPayload {
                admitted_entry_root: path.clone(),
                admitted_identity: manifest.identity.clone(),
                original_native_receipt,
                manifest,
                artifact_root: path.join(MATERIALIZED_DIRECTORY),
                payload,
                _lease: lease,
            },
        ));
    }
    Ok(selected)
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
        path: PathBuf::from(ARTIFACT_MANIFEST_FILE),
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
fn conservative_physical_reservation(
    manifest: &OvenArtifactManifest,
    native_receipt_bytes: Option<&[u8]>,
) -> Result<u64, OvenStoreError> {
    conservative_physical_reservation_with_shared_materialized_files(
        manifest,
        native_receipt_bytes,
        &mut BTreeSet::new(),
    )
}

/// Reserve a staged entry while counting one digest-matched immutable materialization only once in a publication
/// batch. Each entry still owns its complete logical manifest; this only models the physical hard link the batch
/// writer creates beneath its single managed store root.
fn conservative_physical_reservation_with_shared_materialized_files(
    manifest: &OvenArtifactManifest,
    native_receipt_bytes: Option<&[u8]>,
    shared_materialized_files: &mut BTreeSet<(String, bool)>,
) -> Result<u64, OvenStoreError> {
    let manifest_bytes = serde_json::to_vec_pretty(manifest).map_err(|error| OvenStoreError::Manifest {
        path: PathBuf::from(ARTIFACT_MANIFEST_FILE),
        message: error.to_string(),
    })?;
    let manifest_bytes = u64::try_from(manifest_bytes.len()).map_err(|_| OvenStoreError::InvalidInput {
        field: "manifest",
        message: "serialized manifest length does not fit the supported accounting range".to_string(),
    })?;
    let receipt_bytes =
        u64::try_from(native_receipt_bytes.map_or(0, <[u8]>::len)).map_err(|_| OvenStoreError::InvalidInput {
            field: "native receipt",
            message: "serialized receipt length does not fit the supported accounting range".to_string(),
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
        .saturating_add(round_physical(receipt_bytes))
        .saturating_add(round_physical(20)))
}

/// Use a conservative 4 KiB reservation for pre-publication physical capacity admission.
fn round_physical(bytes: u64) -> u64 {
    const BLOCK: u64 = 4096;
    bytes.saturating_add(BLOCK - 1) / BLOCK * BLOCK
}

/// Versioned preimage of the receipt identity already authenticated by a native entry header.
/// The writer borrows its validated request; admission retains the decoded receipt under the selected lease.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeReceiptWitness<T = OvenReceipt> {
    schema_version: u32,
    receipt: T,
}

/// Original decoded recipe and exact metadata bytes retained by one admitted execution owner.
struct AdmittedNativeReceipt {
    receipt: OvenReceipt,
    bytes_digest: String,
}

/// Encode a newly published native entry's original receipt after `artifact_manifest` validates it.
/// Borrowing the request avoids copying or rehashing its recipe; this metadata does not change the entry identity.
fn encode_native_receipt(request: &OvenArtifactPublishRequest) -> Result<Option<Vec<u8>>, OvenStoreError> {
    if request.kind != OvenArtifactKind::DirectRustcPlan {
        return Ok(None);
    }
    let bytes = serde_json::to_vec(&NativeReceiptWitness {
        schema_version: NATIVE_RECEIPT_SCHEMA_VERSION,
        receipt: &request.receipt,
    })
    .map_err(|error| OvenStoreError::InvalidInput {
        field: "native receipt",
        message: error.to_string(),
    })?;
    if u64::try_from(bytes.len())
        .ok()
        .is_none_or(|length| length > MAX_NATIVE_RECEIPT_BYTES)
    {
        return Err(OvenStoreError::InvalidInput {
            field: "native receipt",
            message: "original native receipt exceeds the 16 MiB metadata bound".to_string(),
        });
    }
    Ok(Some(bytes))
}

/// Read bounded regular metadata only at its original entry coordinate; absence is legacy evidence, not an empty
/// recipe.
fn read_native_receipt_bytes(root: &Path, manifest: &OvenArtifactManifest) -> Result<Option<Vec<u8>>, OvenStoreError> {
    let path = root.join(NATIVE_RECEIPT_FILE);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(OvenStoreError::Io { path, source }),
    };
    if manifest.kind != OvenArtifactKind::DirectRustcPlan
        || !metadata.file_type().is_file()
        || metadata.len() > MAX_NATIVE_RECEIPT_BYTES
    {
        return Err(OvenStoreError::Integrity {
            identity: manifest.identity.clone(),
            message: "native receipt must be bounded regular metadata on a DirectRustcPlan entry".to_string(),
        });
    }
    let file = File::open(&path).map_err(|source| OvenStoreError::Io {
        path: path.clone(),
        source,
    })?;
    let mut bytes = Vec::new();
    file.take(MAX_NATIVE_RECEIPT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| OvenStoreError::Io { path, source })?;
    if u64::try_from(bytes.len())
        .ok()
        .is_none_or(|length| length > MAX_NATIVE_RECEIPT_BYTES)
    {
        return Err(OvenStoreError::Integrity {
            identity: manifest.identity.clone(),
            message: "original native receipt exceeds the 16 MiB metadata bound".to_string(),
        });
    }
    Ok(Some(bytes))
}

/// Admit the publisher's complete receipt against the existing header, decoding selected metadata once.
fn admit_native_receipt(
    root: &Path,
    manifest: &OvenArtifactManifest,
) -> Result<Option<AdmittedNativeReceipt>, OvenStoreError> {
    let Some(bytes) = read_native_receipt_bytes(root, manifest)? else {
        return Ok(None);
    };
    let invalid = |message: String| OvenStoreError::Integrity {
        identity: manifest.identity.clone(),
        message,
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| invalid(error.to_string()))?;
    if value.get("schema_version").and_then(serde_json::Value::as_u64) != Some(u64::from(NATIVE_RECEIPT_SCHEMA_VERSION))
    {
        return Err(invalid(
            "unsupported original native receipt witness schema".to_string(),
        ));
    }
    let witness: NativeReceiptWitness = serde_json::from_value(value).map_err(|error| invalid(error.to_string()))?;
    witness
        .receipt
        .verify_identity()
        .map_err(|error| invalid(error.to_string()))?;
    if witness.receipt.identity != manifest.receipt_identity
        || witness.receipt.build_unit_identity != manifest.build_unit_identity
        || witness.receipt.intent != manifest.intent
    {
        return Err(invalid(
            "original native receipt witness disagrees with entry receipt, build unit, or intent".to_string(),
        ));
    }
    Ok(Some(AdmittedNativeReceipt {
        receipt: witness.receipt,
        bytes_digest: digest_bytes(&bytes),
    }))
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
    native_receipt_bytes: Option<&[u8]>,
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
    if let Some(bytes) = native_receipt_bytes {
        write_synced_file(&root.join(NATIVE_RECEIPT_FILE), bytes, false)?;
    }
    let manifest_bytes = serde_json::to_vec_pretty(manifest).map_err(|error| OvenStoreError::Manifest {
        path: root.join(manifest_file_name(manifest.kind)),
        message: error.to_string(),
    })?;
    write_synced_file(&root.join(manifest_file_name(manifest.kind)), &manifest_bytes, true)?;
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

/// Return whether a verified materialized source is already immutable and owned by this store.
///
/// `root` is always `<store>/staging/<identity>-...`; its grandparent is the only store root we trust for the
/// hard-link optimization. A canonical source path may be below the named legacy publisher staging tree or below
/// one immutable entry's `artifacts/` root. The latter supports receipt-held composition of a sealed runtime plan
/// with a native interop extension without doubling its physical disk allocation. A caller cannot turn arbitrary
/// mutable input into a store-owned inode by choosing a convenient path.
fn is_private_publisher_materialized_source(root: &Path, source: &Path) -> Result<bool, OvenStoreError> {
    let store_root = root
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| OvenStoreError::InvalidInput {
            field: "store staging root",
            message: format!("{} has no store-root ancestor", root.display()),
        })?;
    let source = fs::canonicalize(source).map_err(|source_error| OvenStoreError::Io {
        path: source.to_path_buf(),
        source: source_error,
    })?;
    let publisher_root = store_root.join(LEGACY_CARGO_STAGING_DIRECTORY);
    if let Ok(publisher_root) = fs::canonicalize(&publisher_root)
        && source.starts_with(publisher_root)
    {
        return Ok(true);
    }
    let entries_root = store_root.join(ENTRIES_DIRECTORY);
    let entries_root = match fs::canonicalize(&entries_root) {
        Ok(path) => path,
        Err(source_error) if source_error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source_error) => {
            return Err(OvenStoreError::Io {
                path: entries_root,
                source: source_error,
            });
        }
    };
    let Ok(relative) = source.strip_prefix(entries_root) else {
        return Ok(false);
    };
    let mut components = relative.components();
    let Some(Component::Normal(_entry_identity)) = components.next() else {
        return Ok(false);
    };
    let Some(Component::Normal(directory)) = components.next() else {
        return Ok(false);
    };
    Ok(directory == std::ffi::OsStr::new(MATERIALIZED_DIRECTORY) && components.next().is_some())
}

/// Verify one published immutable entry and calculate both its logical and physical accounting.
fn verify_entry(root: &Path) -> Result<OvenStoreEntry, OvenStoreError> {
    let manifest = verify_entry_manifest(root)?;
    // Every caller that verifies a whole entry gets the witness checked with it. Putting this at call sites
    // instead would leave whichever one was added last trusting a contradictory receipt.
    let admitted = admit_native_receipt(root, &manifest)?;
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
        // Every `verify_entry` caller resolves `root` through `entry_root`/`entry_root_for_kind`: always a real,
        // immutable entry, never staging, so the same physical-bytes cache used by the admission path applies here.
        physical_bytes: cached_directory_physical_bytes(root)?,
        last_used_unix_seconds,
        original_native_receipt: admitted.map(|witness| witness.receipt),
        manifest,
        path: root.to_path_buf(),
    })
}

/// Identity of one manifest file as seen on disk, used to key the read memo.
///
/// Length and modification time together are what a rewritten file changes, so a staging manifest replaced in
/// place misses the memo and is read again. An admitted entry never changes at all: its directory name is the
/// digest of its own content.
type ManifestFileStamp = (PathBuf, u64, Option<SystemTime>);

/// Process-local memo of manifests already read and structurally checked in this run.
///
/// A bake reaches the same admitted entries from many directions — resolving the plan, taking leases, and
/// answering each dependency query all land back on the same `loaf.json`. Measured on one no-op bake of a trivial
/// project, that was 1,454 reads of 61 distinct manifests.
///
/// Only the read is memoized. The identity proof deliberately is not: it must never be keyed by the identity a
/// manifest *claims*, or a tampered manifest that keeps its recorded identity rides on the proof the genuine one
/// earned earlier in the same process. Since the proof now runs only for entries a selection actually takes, it
/// is rare enough that memoizing it buys little and risks exactly that mistake.
fn read_manifest_memo() -> &'static Mutex<HashMap<ManifestFileStamp, OvenArtifactManifest>> {
    static MEMO: OnceLock<Mutex<HashMap<ManifestFileStamp, OvenArtifactManifest>>> = OnceLock::new();
    MEMO.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Describe one manifest file well enough to notice it being rewritten underneath us.
///
/// A manifest that cannot be stated — it has just been removed, or the filesystem reports no modification time —
/// is simply not memoized: the caller falls through to a full read, which produces the honest error.
fn manifest_file_stamp(manifest_path: &Path) -> Option<ManifestFileStamp> {
    let metadata = fs::metadata(manifest_path).ok()?;
    Some((manifest_path.to_path_buf(), metadata.len(), metadata.modified().ok()))
}

/// Read one entry manifest and check its structure, without recomputing the identity its content implies.
///
/// Reading and identity-checking are separated because they are needed at different moments. A caller deciding
/// *whether* it wants an entry needs the manifest's fields; only a caller about to use one needs proof that those
/// fields hash to the identity the entry is filed under. `verify_entry_manifest` still does both, so every path
/// that authenticated an entry before still authenticates it now.
fn read_entry_manifest(root: &Path) -> Result<OvenArtifactManifest, OvenStoreError> {
    let manifest_path = manifest_path_for_entry(root);
    let stamp = manifest_file_stamp(&manifest_path);
    if let Some(stamp) = stamp.as_ref()
        && let Ok(memo) = read_manifest_memo().lock()
        && let Some(manifest) = memo.get(stamp)
    {
        return Ok(manifest.clone());
    }
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
    if let Some(stamp) = stamp
        && let Ok(mut memo) = read_manifest_memo().lock()
    {
        memo.insert(stamp, manifest.clone());
    }
    Ok(manifest)
}

/// Process-local record of manifest files already proven in this run, keyed by the bytes that were proven.
///
/// The key must be the file's stamp and never the identity the manifest records. That identity is what the
/// manifest *claims*; keying by it would let a tampered manifest that keeps its recorded identity ride on the
/// proof the genuine one earned earlier in the same process, which
/// `a_tampered_manifest_is_refused_when_it_is_selected_and_ignored_when_it_is_not` exists to catch.
fn proven_manifest_memo() -> &'static Mutex<BTreeSet<ManifestFileStamp>> {
    static MEMO: OnceLock<Mutex<BTreeSet<ManifestFileStamp>>> = OnceLock::new();
    MEMO.get_or_init(|| Mutex::new(BTreeSet::new()))
}

/// Prove that a manifest's content hashes to the identity it records.
///
/// This is the expensive half of verification: it re-serializes the identity-bearing content, which for a Loaf
/// carrying several thousand materialized files is most of a megabyte. Call it for an entry that is about to be
/// used, not for every entry that happens to be enumerated on the way there.
fn verify_manifest_identity(manifest: &OvenArtifactManifest) -> Result<(), OvenStoreError> {
    let identity = artifact_identity_from_manifest(manifest)?;
    if identity != manifest.identity {
        return Err(OvenStoreError::Integrity {
            identity: manifest.identity.clone(),
            message: "manifest identity does not match its immutable content".to_string(),
        });
    }
    Ok(())
}

/// Prove one entry's manifest, skipping the work when this run already proved the very same bytes.
fn prove_entry_manifest(root: &Path, manifest: &OvenArtifactManifest) -> Result<(), OvenStoreError> {
    let stamp = manifest_file_stamp(&manifest_path_for_entry(root));
    if let Some(stamp) = stamp.as_ref()
        && let Ok(proven) = proven_manifest_memo().lock()
        && proven.contains(stamp)
    {
        return Ok(());
    }
    verify_manifest_identity(manifest)?;
    if let Some(stamp) = stamp
        && let Ok(mut proven) = proven_manifest_memo().lock()
    {
        proven.insert(stamp);
    }
    Ok(())
}

/// Verify immutable manifest structure and identity without traversing the materialized compiler closure.
fn verify_entry_manifest(root: &Path) -> Result<OvenArtifactManifest, OvenStoreError> {
    let manifest = read_entry_manifest(root)?;
    prove_entry_manifest(root, &manifest)?;
    Ok(manifest)
}

/// Read and authenticate one primary payload only from a regular file within its verified entry root.
fn verified_payload_bytes(root: &Path, manifest: &OvenArtifactManifest) -> Result<Vec<u8>, OvenStoreError> {
    let payload_path = root.join(PAYLOAD_FILE);
    let metadata = fs::symlink_metadata(&payload_path).map_err(|source| OvenStoreError::Io {
        path: payload_path.clone(),
        source,
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OvenStoreError::Integrity {
            identity: manifest.identity.clone(),
            message: "store entry payload must be a regular non-symlink file".to_string(),
        });
    }
    let payload = fs::read(&payload_path).map_err(|source| OvenStoreError::Io {
        path: payload_path,
        source,
    })?;
    if u64::try_from(payload.len()).ok() != Some(manifest.payload.logical_bytes)
        || digest_bytes(&payload) != manifest.payload.digest
    {
        return Err(OvenStoreError::Integrity {
            identity: manifest.identity.clone(),
            message: "manifest payload descriptor disagrees with stored bytes".to_string(),
        });
    }
    Ok(payload)
}

/// Require the selected owner's materialized closure to remain a real directory below its admitted entry.
fn verify_materialized_root(root: &Path, manifest: &OvenArtifactManifest) -> Result<(), OvenStoreError> {
    let materialized_root = root.join(MATERIALIZED_DIRECTORY);
    let metadata = fs::symlink_metadata(&materialized_root).map_err(|source| OvenStoreError::Io {
        path: materialized_root,
        source,
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(OvenStoreError::Integrity {
            identity: manifest.identity.clone(),
            message: "materialized artifact root must be a real store-owned directory".to_string(),
        });
    }
    Ok(())
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
    let directory_name = root.file_name().and_then(|name| name.to_str());
    let encoded_directory_name = entry_directory_name_for_kind(&manifest.identity, manifest.kind);
    if require_identity_directory_name && directory_name != Some(encoded_directory_name.as_str()) {
        return Err(OvenStoreError::Integrity {
            identity: manifest.identity,
            message: "entry directory name does not match its immutable manifest identity encoding".to_string(),
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
    // Staging directories are private, not-yet-finalized publisher work: their content can still change before an
    // atomic rename admits them as an immutable entry, so their physical bytes must never be cached.
    let physical_bytes = if require_identity_directory_name {
        cached_directory_physical_bytes(root)?
    } else {
        directory_physical_bytes(root)?
    };
    Ok(OvenStoreEntry {
        logical_bytes,
        physical_bytes,
        last_used_unix_seconds,
        // This measurement path walks staging directories too, where the witness is not yet admitted and the
        // manifest is not yet immutable. Reporting a record from one would name a compilation the store has not
        // accepted, so the accounting view leaves it absent and `verify_entry` remains the one place it is read.
        original_native_receipt: None,
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
        path: PathBuf::from(ARTIFACT_MANIFEST_FILE),
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
/// Return the store domain owned by the running compiler release.
fn active_release_domain() -> String {
    format!("{RELEASE_DOMAIN_PREFIX}{}", crate::version::INCAN_VERSION)
}

/// Fold a superseded-release reclamation report into the retention-prune report that followed it.
///
/// The two passes run back to back under one manager lock, so the user-visible result must read as a single
/// reclamation: the earliest `before`, the latest `after`, and the union of what each pass touched.
fn merge_prune_reports(first: OvenStorePruneReport, second: OvenStorePruneReport) -> OvenStorePruneReport {
    let mut removed_entries = first.removed_entries;
    removed_entries.extend(second.removed_entries);
    let mut skipped_active_entries = first.skipped_active_entries;
    for identity in second.skipped_active_entries {
        if !skipped_active_entries.contains(&identity) {
            skipped_active_entries.push(identity);
        }
    }
    OvenStorePruneReport {
        schema_version: second.schema_version,
        dry_run: second.dry_run,
        before_physical_bytes: first.before_physical_bytes,
        // A preview leaves both passes' entries on disk, so the retention pass re-measures the full store and its
        // `after` ignores what the superseded pass projected removing. Take the smaller reading so a dry run reports
        // the allocation the user would actually be left with, and an applied run keeps reporting the measured one.
        after_physical_bytes: first.after_physical_bytes.min(second.after_physical_bytes),
        removed_logical_bytes: first.removed_logical_bytes.saturating_add(second.removed_logical_bytes),
        removed_entries,
        skipped_active_entries,
    }
}

/// Compiler-release store domains are spelled `incan-release-<version>`.
const RELEASE_DOMAIN_PREFIX: &str = "incan-release-";

/// Return whether `candidate` names a compiler release superseded by `active`.
///
/// Only release domains are comparable this way. Any other domain (compiler-suite, fixtures, interop) is left alone,
/// because its reuse rules are not keyed to the compiler version.
fn is_superseded_release_domain(candidate: &str, active: &str) -> bool {
    candidate.starts_with(RELEASE_DOMAIN_PREFIX) && active.starts_with(RELEASE_DOMAIN_PREFIX) && candidate != active
}

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

/// Return one immutable store entry's physical byte allocation, backed by [`PHYSICAL_BYTES_CACHE_FILE`] when present.
///
/// Admission recomputed every retained entry's allocation via a fresh recursive [`directory_physical_bytes`] walk on
/// every publish, so admitting one new artifact cost time proportional to the store's entire accumulated content
/// rather than to that one artifact. Entries under [`ENTRIES_DIRECTORY`] are immutable once published, so caching
/// the one-time measurement is always safe: a missing or corrupt cache file simply falls back to the recursive
/// walk, so a cold or damaged cache degrades to the previous behavior rather than serving a stale value. Callers
/// must not use this for staging directories, whose content can still change before they are finalized.
fn cached_directory_physical_bytes(root: &Path) -> Result<u64, OvenStoreError> {
    let cache_path = root.join(PHYSICAL_BYTES_CACHE_FILE);
    if let Ok(cached) = fs::read_to_string(&cache_path)
        && let Ok(bytes) = cached.trim().parse::<u64>()
    {
        return Ok(bytes);
    }
    let bytes = directory_physical_bytes(root)?;
    // Best-effort: a read-only store or a lost race with a concurrent measurement must not fail the measurement
    // itself, since the recomputed value above is already correct without the cache.
    let _ = fs::write(&cache_path, bytes.to_string());
    Ok(bytes)
}

/// Return true when `name` is an OvenStore-internal sidecar bookkeeping file that must never count toward an
/// entry's measured content, since none of the raw recursive walkers below know how to exclude it structurally.
fn is_store_sidecar_file(name: &std::ffi::OsStr) -> bool {
    name == PHYSICAL_BYTES_CACHE_FILE || name == UNIQUE_FILE_RECORDS_CACHE_FILE
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
            if is_store_sidecar_file(&child.file_name()) {
                return Ok(total);
            }
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

#[cfg(unix)]
/// Measure private publisher staging without following Cargo-created symlinks.
///
/// Store entries remain link-free, but the disposable publisher target may contain executable aliases. Count the
/// link allocation without traversing it, so admission remains bounded to Oven-owned staging.
fn unique_publisher_staging_physical_bytes(path: &Path) -> Result<u64, OvenStoreError> {
    publisher_staging_physical_bytes(path, &mut BTreeSet::new())
}

#[cfg(not(unix))]
/// Measure private publisher staging on hosts without inode identity.
fn unique_publisher_staging_physical_bytes(path: &Path) -> Result<u64, OvenStoreError> {
    publisher_staging_physical_bytes(path)
}

/// Walk one Unix publisher staging root, counting regular allocations once and link allocations without traversal.
#[cfg(unix)]
fn publisher_staging_physical_bytes(path: &Path, seen_files: &mut BTreeSet<(u64, u64)>) -> Result<u64, OvenStoreError> {
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
            let bytes = if metadata.file_type().is_symlink() {
                round_physical(metadata.len())
            } else if metadata.is_dir() {
                publisher_staging_physical_bytes(&child_path, seen_files)?
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
                    message: "publisher staging may contain only regular files, directories, and transient symlinks"
                        .to_string(),
                });
            };
            Ok(total.saturating_add(bytes))
        })
}

/// Walk one non-Unix publisher staging root while keeping symlink targets outside the measured closure.
#[cfg(not(unix))]
fn publisher_staging_physical_bytes(path: &Path) -> Result<u64, OvenStoreError> {
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
            let bytes = if metadata.file_type().is_symlink() {
                round_physical(metadata.len())
            } else if metadata.is_dir() {
                publisher_staging_physical_bytes(&child_path)?
            } else if metadata.is_file() {
                physical_file_bytes(&metadata)
            } else {
                return Err(OvenStoreError::Integrity {
                    identity: child_path.display().to_string(),
                    message: "publisher staging may contain only regular files, directories, and transient symlinks"
                        .to_string(),
                });
            };
            Ok(total.saturating_add(bytes))
        })
}

/// Attribute each hard-linked immutable file allocation to the first stable entry identity that references it.
///
/// Materialized closures may deliberately share byte-identical files across a related publication batch. Directory
/// accounting alone would count every hard link as newly allocated disk use, so admission/pruning/inspection use
/// this stable attribution pass and report physical bytes once while retaining each entry's independent logical
/// bytes. Re-running it after a simulated or real prune is essential: deleting one link cannot reclaim a block that
/// another selected entry still references.
///
/// `entries` are always already-published, immutable store entries (never staging), so each entry's file identities
/// are read through [`cached_directory_file_records`] instead of a fresh walk. This runs on every admission scan
/// (see `collect_entries_with`), so an uncached walk here previously re-read every retained entry's entire directory
/// tree on every single publish, independent of whether anything was actually pruned.
#[cfg(unix)]
fn assign_unique_entry_physical_bytes(entries: &mut [OvenStoreEntry]) -> Result<(), OvenStoreError> {
    let mut seen_files = BTreeSet::new();
    for entry in entries {
        let records = cached_directory_file_records(&entry.path)?;
        entry.physical_bytes = records.into_iter().fold(0_u64, |total, (device, inode, bytes)| {
            if seen_files.insert((device, inode)) {
                total.saturating_add(bytes)
            } else {
                total
            }
        });
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

/// Return one immutable store entry's regular-file `(device, inode, physical_bytes)` identities, backed by
/// [`UNIQUE_FILE_RECORDS_CACHE_FILE`] when present.
///
/// See [`cached_directory_physical_bytes`] for the immutability argument that makes caching safe here: entries under
/// [`ENTRIES_DIRECTORY`] never change after publication, so a listing taken once stays valid for the entry's
/// lifetime, and a missing or unparsable cache file simply falls back to a fresh walk. Callers must not use this for
/// staging directories, whose content can still change before they are finalized.
#[cfg(unix)]
fn cached_directory_file_records(root: &Path) -> Result<Vec<(u64, u64, u64)>, OvenStoreError> {
    let cache_path = root.join(UNIQUE_FILE_RECORDS_CACHE_FILE);
    if let Ok(cached) = fs::read_to_string(&cache_path)
        && let Some(records) = parse_directory_file_records(&cached)
    {
        return Ok(records);
    }
    let records = directory_file_records(root)?;
    // Best-effort: a read-only store or a lost race with a concurrent measurement must not fail the measurement
    // itself, since the recomputed value above is already correct without the cache.
    let _ = fs::write(&cache_path, serialize_directory_file_records(&records));
    Ok(records)
}

/// Parse a [`UNIQUE_FILE_RECORDS_CACHE_FILE`] payload, returning `None` on any malformed line so the caller falls
/// back to a fresh walk instead of trusting a partially corrupt cache.
#[cfg(unix)]
fn parse_directory_file_records(cached: &str) -> Option<Vec<(u64, u64, u64)>> {
    cached
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut fields = line.split(':');
            let device = fields.next()?.parse::<u64>().ok()?;
            let inode = fields.next()?.parse::<u64>().ok()?;
            let bytes = fields.next()?.parse::<u64>().ok()?;
            if fields.next().is_some() {
                return None;
            }
            Some((device, inode, bytes))
        })
        .collect()
}

/// Encode `(device, inode, physical_bytes)` records for [`UNIQUE_FILE_RECORDS_CACHE_FILE`].
#[cfg(unix)]
fn serialize_directory_file_records(records: &[(u64, u64, u64)]) -> String {
    records
        .iter()
        .map(|(device, inode, bytes)| format!("{device}:{inode}:{bytes}\n"))
        .collect()
}

/// Recursively list regular files below one root as `(device, inode, physical_bytes)`, enforcing the same
/// symlink/type integrity rules as [`directory_unique_physical_bytes`] without performing cross-entry deduplication.
#[cfg(unix)]
fn directory_file_records(path: &Path) -> Result<Vec<(u64, u64, u64)>, OvenStoreError> {
    use std::os::unix::fs::MetadataExt;

    fs::read_dir(path)
        .map_err(|source| OvenStoreError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .try_fold(Vec::new(), |mut records, child| {
            let child = child.map_err(|source| OvenStoreError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            if is_store_sidecar_file(&child.file_name()) {
                return Ok(records);
            }
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
                records.extend(directory_file_records(&child_path)?);
            } else if metadata.is_file() {
                records.push((metadata.dev(), metadata.ino(), physical_file_bytes(&metadata)));
            } else {
                return Err(OvenStoreError::Integrity {
                    identity: child_path.display().to_string(),
                    message: "store entries may contain only regular files and directories".to_string(),
                });
            }
            Ok(records)
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
        if is_store_sidecar_file(&child.file_name()) {
            continue;
        }
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
    use std::collections::BTreeMap;
    use std::fs::{self, OpenOptions};
    use std::path::{Path, PathBuf};

    /// Relative directory entries and exact file bytes in a published store.
    type PublishedInventory = BTreeMap<PathBuf, Option<Vec<u8>>>;

    /// Capture every directory and file, including mutable-store bookkeeping, without following links.
    fn published_inventory(root: &Path) -> Result<PublishedInventory, Box<dyn std::error::Error>> {
        let mut inventory = PublishedInventory::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory)? {
                let entry = entry?;
                let path = entry.path();
                let relative = path.strip_prefix(root)?.to_path_buf();
                if entry.file_type()?.is_dir() {
                    inventory.insert(relative, None);
                    pending.push(path);
                } else {
                    assert!(entry.file_type()?.is_file(), "unexpected published entry: {path:?}");
                    inventory.insert(relative, Some(fs::read(path)?));
                }
            }
        }
        Ok(inventory)
    }

    #[test]
    fn published_reads_preserve_complete_inventory_and_active_leases() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let manifest = store.publish(&request(project.path(), "published", b"native plan")?)?;
        let entry = store.entry_root(&manifest.identity);
        fs::write(entry.join(super::ACCESS_FILE), b"1\n")?;
        let abandoned = temp.path().join(super::STAGING_DIRECTORY).join("retained-staging");
        fs::create_dir_all(&abandoned)?;
        fs::write(abandoned.join("evidence"), b"must remain untouched")?;
        let mut originals = Vec::new();
        for path in published_inventory(temp.path())?.keys() {
            let path = temp.path().join(path);
            if path.is_file() {
                let original = fs::metadata(&path)?.permissions();
                let mut readonly = original.clone();
                readonly.set_readonly(true);
                fs::set_permissions(&path, readonly)?;
                originals.push((path, original));
            }
        }
        let checked = (|| -> Result<(), Box<dyn std::error::Error>> {
            let before = published_inventory(temp.path())?;
            let published = super::PublishedOvenStore::new(temp.path());
            for _ in 0..2 {
                let selected =
                    published.select_payloads_matching_for_execution(|item| item.identity == manifest.identity)?;
                assert_eq!(selected.len(), 1);
                assert_eq!(selected[0].payload, b"native plan");
                selected[0].verify_materialized_files()?;
                let contender = fs::File::open(entry.join(super::ACTIVE_LOCK_FILE))?;
                let locked = contender.try_lock();
                assert!(
                    matches!(locked, Err(std::fs::TryLockError::WouldBlock)),
                    "published read must retain an active lease: {locked:?}"
                );
                assert_eq!(published_inventory(temp.path())?, before);
                drop(selected);
                contender.try_lock()?;
                contender.unlock()?;
            }
            assert_eq!(published_inventory(temp.path())?, before);
            Ok(())
        })();
        for (path, permissions) in originals {
            fs::set_permissions(path, permissions)?;
        }
        checked?;
        Ok(())
    }

    #[test]
    fn execution_payload_revalidation_binds_public_fields_to_the_admitted_owner()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let first = store.publish(&request(project.path(), "first-owner", b"shared payload")?)?;
        let second = store.publish(&request(project.path(), "second-owner", b"shared payload")?)?;
        let second_artifact_root = store.entry_root(&second.identity).join(super::MATERIALIZED_DIRECTORY);

        let mut selected = store.select_payloads_for_execution(std::slice::from_ref(&first.identity))?;
        assert_eq!(selected.len(), 1);
        selected[0].verify_materialized_files()?;
        let mut changed_root = selected.remove(0);
        changed_root.artifact_root = second_artifact_root.clone();
        assert!(matches!(
            changed_root.verify_materialized_files(),
            Err(OvenStoreError::Integrity { .. })
        ));

        let mut selected = store.select_payloads_for_execution(std::slice::from_ref(&first.identity))?;
        assert_eq!(selected.len(), 1);
        let mut changed_manifest = selected.remove(0);
        changed_manifest.manifest = second.clone();
        assert!(matches!(
            changed_manifest.verify_materialized_files(),
            Err(OvenStoreError::Integrity { .. })
        ));

        let mut selected = store.select_payloads_for_execution(std::slice::from_ref(&first.identity))?;
        assert_eq!(selected.len(), 1);
        let mut retargeted = selected.remove(0);
        retargeted.manifest = second.clone();
        retargeted.artifact_root = second_artifact_root;
        assert!(matches!(
            retargeted.verify_materialized_files(),
            Err(OvenStoreError::Integrity { .. })
        ));

        let mut selected = store.select_payloads_for_execution(std::slice::from_ref(&first.identity))?;
        assert_eq!(selected.len(), 1);
        let mut changed_payload = selected.remove(0);
        changed_payload.payload = b"forged payload".to_vec();
        assert!(matches!(
            changed_payload.verify_materialized_files(),
            Err(OvenStoreError::Integrity { .. })
        ));

        let published = super::PublishedOvenStore::new(temp.path());
        let matched =
            published.select_payloads_matching_for_execution(|manifest| manifest.identity == first.identity)?;
        assert_eq!(matched.len(), 1);
        matched[0].verify_materialized_files()?;
        Ok(())
    }

    #[test]
    fn exact_execution_selection_rejects_noncanonical_identities_before_path_resolution()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let manifest = store.publish(&request(project.path(), "canonical-owner", b"payload")?)?;
        let colliding_identity = manifest.identity.replacen("sha256:", "sha256-", 1);
        let absolute_identity = store.entry_root(&manifest.identity).to_string_lossy().into_owned();
        for invalid in [colliding_identity, "../outside-entry".to_string(), absolute_identity] {
            assert!(matches!(
                store.select_payloads_for_execution(std::slice::from_ref(&invalid)),
                Err(OvenStoreError::InvalidInput {
                    field: "artifact identity",
                    ..
                })
            ));
        }
        Ok(())
    }

    #[test]
    fn matching_execution_selection_rejects_a_misplaced_entry() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let manifest = store.publish(&request(project.path(), "misplaced-owner", b"payload")?)?;
        let original = store.entry_root(&manifest.identity);
        let misplaced = temp.path().join(super::ENTRIES_DIRECTORY).join("misplaced-entry");
        fs::rename(original, misplaced)?;

        let published = super::PublishedOvenStore::new(temp.path());
        assert!(matches!(
            published.select_payloads_matching_for_execution(|_| true),
            Err(OvenStoreError::Integrity { .. })
        ));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn execution_selection_rejects_a_symlinked_entry_root() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let manifest = store.publish(&request(project.path(), "symlinked-owner", b"payload")?)?;
        let canonical = store.entry_root(&manifest.identity);
        let moved = temp.path().join("moved-entry");
        fs::rename(&canonical, &moved)?;
        let moved_manifest = moved.join(super::ARTIFACT_MANIFEST_FILE);
        fs::remove_file(&moved_manifest)?;
        fs::write(moved_manifest, b"not a manifest")?;
        symlink(&moved, &canonical)?;

        assert!(matches!(
            store.select_payloads_for_execution(std::slice::from_ref(&manifest.identity)),
            Err(OvenStoreError::Integrity { .. })
        ));
        let published = super::PublishedOvenStore::new(temp.path());
        assert!(matches!(
            published.select_payloads_matching_for_execution(|_| true),
            Err(OvenStoreError::Integrity { .. })
        ));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn execution_selection_rejects_symlinked_authority_files() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        for file_name in [
            super::ARTIFACT_MANIFEST_FILE,
            super::PAYLOAD_FILE,
            super::ACTIVE_LOCK_FILE,
        ] {
            let temp = tempfile::tempdir()?;
            let project = tempfile::tempdir()?;
            write_project(project.path())?;
            let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
            let manifest = store.publish(&request(project.path(), "symlinked-file", b"payload")?)?;
            let authority_file = store.entry_root(&manifest.identity).join(file_name);
            let moved = temp.path().join(format!("moved-{file_name}"));
            fs::rename(&authority_file, &moved)?;
            symlink(moved, authority_file)?;

            assert!(matches!(
                store.select_payloads_for_execution(std::slice::from_ref(&manifest.identity)),
                Err(OvenStoreError::Integrity { .. })
            ));
            let published = super::PublishedOvenStore::new(temp.path());
            assert!(matches!(
                published.select_payloads_matching_for_execution(|_| true),
                Err(OvenStoreError::Integrity { .. })
            ));
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn execution_payload_owner_survives_store_ancestor_symlink_retargeting() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let actual_store = temp.path().join("actual-store");
        let replacement_store = temp.path().join("replacement-store");
        fs::create_dir(&actual_store)?;
        fs::create_dir(&replacement_store)?;
        let store_alias = temp.path().join("store-alias");
        symlink(&actual_store, &store_alias)?;
        let store = OvenStore::new(&store_alias, OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let manifest = store.publish(&request(project.path(), "stable-owner", b"payload")?)?;
        let selected = store.select_payloads_for_execution(std::slice::from_ref(&manifest.identity))?;
        assert_eq!(selected.len(), 1);
        let published = super::PublishedOvenStore::new(&store_alias);
        let matched = published.select_payloads_matching_for_execution(|item| item.identity == manifest.identity)?;
        assert_eq!(matched.len(), 1);
        let canonical_store = fs::canonicalize(&actual_store)?;
        assert!(selected[0].artifact_root.starts_with(&canonical_store));
        assert!(matched[0].artifact_root.starts_with(&canonical_store));

        fs::remove_file(&store_alias)?;
        symlink(replacement_store, store_alias)?;
        selected[0].verify_materialized_files()?;
        matched[0].verify_materialized_files()?;
        Ok(())
    }

    #[test]
    fn execution_payload_revalidation_detects_materialized_tampering_after_selection()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let source = project.path().join("native.rlib");
        fs::write(&source, b"native artifact")?;
        let mut publication = request(project.path(), "selected-owner", b"native plan")?;
        publication.materialized_files.push(OvenArtifactMaterializedFile {
            source_path: source,
            relative_path: "lib/native.rlib".to_string(),
        });
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let manifest = store.publish(&publication)?;
        let selected = store.select_payloads_for_execution(std::slice::from_ref(&manifest.identity))?;
        assert_eq!(selected.len(), 1);
        selected[0].verify_materialized_files()?;

        let artifact = store
            .entry_root(&manifest.identity)
            .join(super::MATERIALIZED_DIRECTORY)
            .join("lib/native.rlib");
        fs::remove_file(&artifact)?;
        fs::write(artifact, b"changed content")?;
        assert!(matches!(
            selected[0].verify_materialized_files(),
            Err(OvenStoreError::Integrity { .. })
        ));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn execution_payload_revalidation_rejects_a_symlinked_materialized_root() -> Result<(), Box<dyn std::error::Error>>
    {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let source = project.path().join("native.rlib");
        fs::write(&source, b"native artifact")?;
        let mut publication = request(project.path(), "symlinked-artifacts", b"native plan")?;
        publication.materialized_files.push(OvenArtifactMaterializedFile {
            source_path: source,
            relative_path: "lib/native.rlib".to_string(),
        });
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let manifest = store.publish(&publication)?;
        let selected = store.select_payloads_for_execution(std::slice::from_ref(&manifest.identity))?;
        assert_eq!(selected.len(), 1);
        selected[0].verify_materialized_files()?;

        let artifact_root = store.entry_root(&manifest.identity).join(super::MATERIALIZED_DIRECTORY);
        let moved = temp.path().join("moved-artifacts");
        fs::rename(&artifact_root, &moved)?;
        symlink(moved, artifact_root)?;
        assert!(matches!(
            selected[0].verify_materialized_files(),
            Err(OvenStoreError::Integrity { .. })
        ));
        Ok(())
    }

    #[test]
    fn published_reads_refuse_missing_locks_without_creating_them() -> Result<(), Box<dyn std::error::Error>> {
        for manager_missing in [true, false] {
            let temp = tempfile::tempdir()?;
            let project = tempfile::tempdir()?;
            write_project(project.path())?;
            let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
            let manifest = store.publish(&request(project.path(), "published", b"native plan")?)?;
            let lock = if manager_missing {
                temp.path().join(super::MANAGER_LOCK_FILE)
            } else {
                store.entry_root(&manifest.identity).join(super::ACTIVE_LOCK_FILE)
            };
            fs::remove_file(&lock)?;
            let expected_error_path = if manager_missing {
                lock.clone()
            } else {
                let entry_root = lock.parent().ok_or("active lease has no entry root")?;
                fs::canonicalize(entry_root)?.join(super::ACTIVE_LOCK_FILE)
            };
            let before = published_inventory(temp.path())?;
            let result = super::PublishedOvenStore::new(temp.path()).select_payloads_matching_for_execution(|_| true);
            assert!(matches!(result, Err(OvenStoreError::Io { path, .. }) if path == expected_error_path));
            assert_eq!(published_inventory(temp.path())?, before);
            assert!(!lock.exists());
        }
        Ok(())
    }

    #[test]
    fn published_reads_reject_payload_and_materialized_tampering() -> Result<(), Box<dyn std::error::Error>> {
        for tamper_payload in [true, false] {
            let temp = tempfile::tempdir()?;
            let project = tempfile::tempdir()?;
            write_project(project.path())?;
            let source = project.path().join("native.rlib");
            fs::write(&source, b"native artifact")?;
            let mut publication = request(project.path(), "published", b"native plan")?;
            publication.materialized_files.push(OvenArtifactMaterializedFile {
                source_path: source,
                relative_path: "lib/native.rlib".to_string(),
            });
            let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
            let manifest = store.publish(&publication)?;
            let entry = store.entry_root(&manifest.identity);
            let changed = if tamper_payload {
                entry.join(super::PAYLOAD_FILE)
            } else {
                entry.join(super::MATERIALIZED_DIRECTORY).join("lib/native.rlib")
            };
            // Publication seals files read-only; replacing this test-owned entry models external corruption.
            fs::remove_file(&changed)?;
            fs::write(changed, b"changed content")?;
            let before = published_inventory(temp.path())?;
            let result = super::PublishedOvenStore::new(temp.path())
                .select_payloads_matching_for_execution(|_| true)
                .and_then(|selected| {
                    for payload in selected {
                        payload.verify_materialized_files()?;
                    }
                    Ok(())
                });
            assert!(matches!(result, Err(OvenStoreError::Integrity { .. })));
            assert_eq!(published_inventory(temp.path())?, before);
        }
        Ok(())
    }

    #[test]
    fn a_tampered_manifest_is_refused_when_it_is_selected_and_ignored_when_it_is_not()
    -> Result<(), Box<dyn std::error::Error>> {
        // Identity is proven for the entries a selection takes, not for every entry it walks past. The property
        // that has to survive is that nothing reaches execution unproven: a tampered manifest that matches the
        // selector must still be refused, and it must be refused before its payload is read.
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let wanted = store.publish(&request(project.path(), "wanted", b"wanted payload")?)?;
        let other = store.publish(&request(project.path(), "other-domain", b"other payload")?)?;

        // Publication seals the manifest read-only; replacing this test-owned entry models external corruption.
        // The recorded identity stays put so the entry still matches its directory coordinate, and only the
        // content that identity is derived from moves.
        let manifest_path = super::manifest_path_for_entry(&store.entry_root(&other.identity));
        let tampered = fs::read_to_string(&manifest_path)?.replace("\"other-domain\"", "\"switched-domain\"");
        fs::remove_file(&manifest_path)?;
        fs::write(&manifest_path, tampered)?;

        // A selection that does not want the tampered entry still succeeds, and returns only what it asked for.
        let untouched = super::PublishedOvenStore::new(temp.path())
            .select_payloads_matching_for_execution(|manifest| manifest.identity == wanted.identity)?;
        assert_eq!(untouched.len(), 1);
        assert_eq!(untouched[0].admitted_identity, wanted.identity);

        // A selection that does want it is refused.
        let refused = super::PublishedOvenStore::new(temp.path()).select_payloads_matching_for_execution(|_| true);
        assert!(
            matches!(refused, Err(OvenStoreError::Integrity { .. })),
            "a tampered manifest must be refused once the selection takes it"
        );
        Ok(())
    }

    #[test]
    fn a_read_manifest_is_read_again_once_its_file_changes() -> Result<(), Box<dyn std::error::Error>> {
        // Repeated verification of one admitted entry must answer identically, and must stop answering from the
        // memo the moment the file behind it is no longer the file that was read.
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let published = store.publish(&request(project.path(), "memo-entry", b"memo payload")?)?;
        let entry = store.entry_root(&published.identity);

        let first = super::verify_entry_manifest(&entry)?;
        let second = super::verify_entry_manifest(&entry)?;
        assert_eq!(first, second);
        assert_eq!(first.identity, published.identity);

        // Publication seals the manifest read-only; replacing this test-owned entry models external corruption.
        let manifest_path = super::manifest_path_for_entry(&entry);
        let tampered = format!(
            "{}   ",
            fs::read_to_string(&manifest_path)?.replace("memo-entry", "other-name")
        );
        fs::remove_file(&manifest_path)?;
        fs::write(&manifest_path, tampered)?;
        assert!(
            super::verify_entry_manifest(&entry).is_err(),
            "a rewritten manifest must be read again rather than served from the memo"
        );
        Ok(())
    }

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

    #[cfg(unix)]
    #[test]
    fn admission_reuses_the_cached_physical_byte_measurement_for_retained_entries()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir()?;
        let first_project = tempfile::tempdir()?;
        let second_project = tempfile::tempdir()?;
        let third_project = tempfile::tempdir()?;
        write_project(first_project.path())?;
        write_project(second_project.path())?;
        write_project(third_project.path())?;
        // Generous limits keep every publish well under capacity, so admission only ever measures retained entries
        // to confirm there is room; it never has to prune one.
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(10_000_000, 10_000_000, 10_000_000));

        // Publishing the first entry does not yet measure it: admission only re-measures *retained* entries while
        // making room for a new one. Publishing a second, unrelated entry re-measures the first as part of that
        // aggregate capacity scan, which is what leaves both sidecar cache files behind (the plain physical-byte
        // total and the per-file identity listing that cross-entry hard-link deduplication consumes).
        let first_manifest = store.publish(&request(first_project.path(), "engine-arm64", b"engine payload")?)?;
        let first_entry_root = store.entry_root(&first_manifest.identity);
        store.publish(&request(second_project.path(), "engine-x64", b"second engine payload")?)?;
        assert!(first_entry_root.join(super::PHYSICAL_BYTES_CACHE_FILE).is_file());
        assert!(first_entry_root.join(super::UNIQUE_FILE_RECORDS_CACHE_FILE).is_file());

        // A fresh recursive walk of the first entry would now fail outright: `directory_physical_bytes` rejects any
        // symlink it encounters. Admitting a third, unrelated entry re-measures every retained entry (including
        // this one) as part of aggregate capacity accounting via the same `collect_entries_for_admission` path
        // exercised by every ordinary build; if that publish still succeeds, it can only have done so by trusting
        // the cache files instead of re-walking the first entry.
        symlink(
            first_entry_root.join(super::PAYLOAD_FILE),
            first_entry_root.join("untracked-symlink"),
        )?;
        store.publish(&request(third_project.path(), "engine-riscv", b"third engine payload")?)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn sidecar_cache_files_are_never_counted_toward_measured_physical_bytes() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempfile::tempdir()?;
        let first_project = tempfile::tempdir()?;
        let second_project = tempfile::tempdir()?;
        write_project(first_project.path())?;
        write_project(second_project.path())?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(10_000_000, 10_000_000, 10_000_000));

        let first_manifest = store.publish(&request(first_project.path(), "engine-arm64", b"first payload")?)?;
        // Publishing a second, unrelated entry forces admission to re-measure the first as a retained entry, which
        // is what writes both sidecar cache files into its directory.
        store.publish(&request(second_project.path(), "engine-x64", b"second payload")?)?;
        let first_entry_root = store.entry_root(&first_manifest.identity);
        assert!(first_entry_root.join(super::PHYSICAL_BYTES_CACHE_FILE).is_file());
        assert!(first_entry_root.join(super::UNIQUE_FILE_RECORDS_CACHE_FILE).is_file());

        // The cached admission measurement must equal a fresh sidecar-excluding walk of the same directory: if
        // admission had counted the sidecar files it was writing, the cached value would exceed the fresh one.
        // (Deliberately not compared against active-lease inspection totals: that path deduplicates by inode and
        // measures at a different moment, so its equality with per-entry cached sums is filesystem-dependent
        // rather than an invariant -- an exact-equality form of this test failed on ext4 while passing on APFS.)
        let cached = super::cached_directory_physical_bytes(&first_entry_root)?;
        let fresh_with_sidecars = super::directory_physical_bytes(&first_entry_root)?;
        assert_eq!(cached, fresh_with_sidecars);

        // And the walker's exclusion itself: physically deleting the sidecar files must not change the measured
        // total, proving they were never part of it.
        fs::remove_file(first_entry_root.join(super::PHYSICAL_BYTES_CACHE_FILE))?;
        fs::remove_file(first_entry_root.join(super::UNIQUE_FILE_RECORDS_CACHE_FILE))?;
        let fresh_without_sidecars = super::directory_physical_bytes(&first_entry_root)?;
        assert_eq!(fresh_with_sidecars, fresh_without_sidecars);
        Ok(())
    }

    #[test]
    fn exact_reuse_inspection_defers_content_hashing_to_verified_selection() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let manifest = store.publish(&request(project.path(), "engine-arm64", b"payload")?)?;
        fs::write(
            store.entry_root(&manifest.identity).join(super::PAYLOAD_FILE),
            b"corrupt",
        )?;

        let warm_inspection = store.inspect_for_exact_reuse()?;
        assert_eq!(warm_inspection.entries.len(), 1);
        assert!(matches!(store.inspect(), Err(OvenStoreError::Integrity { .. })));
        assert!(matches!(
            store.select_payload(&manifest.identity),
            Err(OvenStoreError::Integrity { .. })
        ));
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

    #[test]
    fn store_uses_a_loader_safe_entry_directory() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let manifest = store.publish(&request(project.path(), "engine-arm64", b"runtime plan")?)?;
        let encoded = store.entry_root(&manifest.identity);
        let encoded_name = encoded
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("encoded entry has no UTF-8 name")?;
        assert!(encoded_name.starts_with("sha256-"));
        assert!(!encoded_name.contains(':'));

        let (selected, _lease) = store.select(&manifest.identity)?;
        assert_eq!(selected.path, encoded);
        assert_eq!(store.inspect()?.entries.len(), 1);
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
        let loaf_root = store.entry_root(&first.identity);
        assert_eq!(
            loaf_root.extension().and_then(|extension| extension.to_str()),
            Some("loaf")
        );
        assert!(loaf_root.join(super::LOAF_MANIFEST_FILE).is_file());
        assert!(!loaf_root.join(super::ARTIFACT_MANIFEST_FILE).exists());
        assert!(store.select(&first.identity).is_ok());
        assert_eq!(store.inspect()?.entries.len(), 1);

        let extension = store.publish(&OvenArtifactPublishRequest {
            receipt: first_request.receipt.clone(),
            domain: "shared-Loaf".to_string(),
            kind: OvenArtifactKind::ProjectPayload,
            payload: b"project extension payload".to_vec(),
            materialized_files: Vec::new(),
        })?;
        let extension_root = store.entry_root(&extension.identity);
        assert_eq!(
            extension_root.extension().and_then(|extension| extension.to_str()),
            Some("loaf")
        );
        assert!(extension_root.join(super::LOAF_MANIFEST_FILE).is_file());
        assert!(store.select(&extension.identity).is_ok());
        Ok(())
    }

    #[test]
    fn legacy_publisher_reservation_preserves_active_leases_and_caps_staging() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let first = store.publish(&request(project.path(), "engine-one", b"first publisher entry")?)?;
        let (_entry, lease) = store.select(&first.identity)?;

        let active_reservation = store.reserve_legacy_cargo_publisher_capacity("engine")?;
        assert_eq!(store.inspect()?.entries.len(), 1);
        assert!(active_reservation.prune_report.removed_entries.is_empty());
        assert!(
            active_reservation.transient_limit_bytes < store.limits().max_physical_bytes,
            "a held lease must remain while reducing the baker's staging allowance"
        );

        drop(lease);
        let inactive_reservation = store.reserve_legacy_cargo_publisher_capacity("engine")?;
        assert!(inactive_reservation.prune_report.removed_entries.is_empty());
        assert!(
            inactive_reservation.transient_limit_bytes < store.limits().max_physical_bytes,
            "an inactive reusable entry must reduce the staging allowance instead of being discarded speculatively"
        );
        assert_eq!(store.inspect()?.entries.len(), 1);
        Ok(())
    }

    #[test]
    fn legacy_publisher_capacity_failure_names_the_safe_prune_recovery_path() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let unbounded = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let first = unbounded.publish(&request(project.path(), "engine", b"retained publisher entry")?)?;
        let physical_bytes = unbounded.inspect()?.physical_bytes;
        let bounded = OvenStore::new(
            temp.path(),
            OvenStoreLimits::new(physical_bytes, physical_bytes, 1_000_000),
        );
        let (_entry, lease) = bounded.select(&first.identity)?;

        let error = bounded
            .reserve_legacy_cargo_publisher_capacity("engine")
            .err()
            .ok_or("a fully retained active entry must block publisher staging")?;

        assert!(error.to_string().contains("incan oven store inspect"));
        assert!(
            error
                .to_string()
                .contains("incan oven store prune --max-physical-bytes")
        );
        drop(lease);
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

    #[cfg(unix)]
    #[test]
    fn legacy_publisher_preflight_counts_a_transient_symlink_without_following_its_target()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let staging = temp
            .path()
            .join(LEGACY_CARGO_STAGING_DIRECTORY)
            .join(".legacy-cargo-fixture");
        let staged_source = staging.join("native/staged.rlib");
        let retained_target = temp.path().join("outside/protoc-helper");
        let transient_link = staging.join("target/build/protoc-helper");
        fs::create_dir_all(staged_source.parent().ok_or("staged parent missing")?)?;
        fs::create_dir_all(transient_link.parent().ok_or("transient link parent missing")?)?;
        fs::create_dir_all(retained_target.parent().ok_or("retained target parent missing")?)?;
        fs::write(&staged_source, b"retained Rust artifact")?;
        fs::write(&retained_target, vec![b'x'; 2 * 1024 * 1024])?;
        symlink(&retained_target, &transient_link)?;
        let mut publication = request(project.path(), "engine-one", b"plan")?;
        publication.materialized_files = vec![OvenArtifactMaterializedFile {
            source_path: staged_source,
            relative_path: "native/staged.rlib".to_string(),
        }];

        store.ensure_legacy_cargo_batch_physical_capacity(&staging, &[publication])?;
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
    fn prune_reclaims_superseded_release_entries_while_the_store_fits_its_policy()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        // Limits far above what these fixtures occupy: retention alone has no reason to evict anything, which is the
        // condition under which superseded releases used to accumulate forever.
        let store = OvenStore::new(temp.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let active = super::active_release_domain();
        let superseded = format!("{}0.0.1-superseded", super::RELEASE_DOMAIN_PREFIX);

        store.publish(&request(project.path(), &superseded, b"stale release artifact")?)?;
        store.publish(&request(project.path(), &active, b"current release artifact")?)?;
        store.publish(&request(project.path(), "compiler-suite", b"unrelated domain")?)?;
        assert_eq!(store.inspect()?.entries.len(), 3);

        let preview = store.preview_prune()?;
        assert_eq!(
            preview.removed_entries.len(),
            1,
            "a dry run must report exactly the superseded release entry",
        );
        assert_eq!(store.inspect()?.entries.len(), 3, "a dry run must not remove anything",);

        let report = store.prune()?;
        assert_eq!(report.removed_entries.len(), 1);
        let remaining: Vec<String> = store
            .inspect()?
            .entries
            .into_iter()
            .map(|entry| entry.manifest.domain)
            .collect();
        assert!(
            remaining.contains(&active) && remaining.iter().any(|domain| domain == "compiler-suite"),
            "the active release and non-release domains must survive, got {remaining:?}",
        );
        assert!(
            !remaining.contains(&superseded),
            "the superseded release must be reclaimed, got {remaining:?}",
        );
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

    /// Generic equivalent-unit reuse preserves the first publisher's complete source receipt.
    #[test]
    fn native_receipt_witness_survives_source_changed_equivalent_publication() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let mut original = request(project.path(), "recipe", b"same native bytes")?;
        original.kind = OvenArtifactKind::DirectRustcPlan;
        let published = store.publish(&original)?;
        let original_bytes = fs::read(store.entry_root(&published.identity).join(super::NATIVE_RECEIPT_FILE))?;
        // The dev-line fixture project is a frozen Cargo package, so its receipt identity moves with the locked
        // sources rather than with an arbitrary file in the directory. The lock is a source input and not a
        // build-unit fact, which is exactly the shape these tests need: a different receipt for the same unit.
        fs::write(project.path().join("Cargo.lock"), "version = 4\n# changed\n")?;
        let mut current = request(project.path(), "recipe", b"same native bytes")?;
        current.kind = OvenArtifactKind::DirectRustcPlan;
        assert_ne!(original.receipt.identity, current.receipt.identity);
        assert_eq!(
            original.receipt.build_unit_identity,
            current.receipt.build_unit_identity
        );
        let reused = store.publish(&current)?;
        assert_eq!(reused, published);
        assert_eq!(fs::read_dir(root.path().join(super::ENTRIES_DIRECTORY))?.count(), 1);
        let selected = store.select_payloads_matching_for_execution(|header| {
            header.build_unit_identity == current.receipt.build_unit_identity
        })?;
        let owner = selected.first().ok_or("original native owner missing")?;
        assert_eq!(owner.original_native_receipt(), Some(&original.receipt));
        assert_ne!(owner.original_native_receipt(), Some(&current.receipt));
        owner.verify_admitted_payload()?;
        assert_eq!(
            fs::read(store.entry_root(&reused.identity).join(super::NATIVE_RECEIPT_FILE))?,
            original_bytes
        );
        Ok(())
    }

    /// Same-ID and equivalent publications cannot retrofit a recipe into a legacy immutable entry.
    #[test]
    fn native_receipt_witness_absence_stays_absent_on_legacy_deduplication() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let mut original = request(project.path(), "legacy", b"same native bytes")?;
        original.kind = OvenArtifactKind::DirectRustcPlan;
        let published = store.publish(&original)?;
        let entry = store.entry_root(&published.identity);
        fs::remove_file(entry.join(super::NATIVE_RECEIPT_FILE))?;
        let immutable = || -> Result<PublishedInventory, Box<dyn std::error::Error>> {
            let mut inventory = published_inventory(&entry)?;
            inventory.remove(Path::new(super::ACCESS_FILE));
            inventory.remove(Path::new(super::PHYSICAL_BYTES_CACHE_FILE));
            inventory.remove(Path::new(super::UNIQUE_FILE_RECORDS_CACHE_FILE));
            Ok(inventory)
        };
        let before = immutable()?;
        assert_eq!(store.publish(&original)?, published);
        assert_eq!(store.publish_batch(&[original.clone()])?, vec![published.clone()]);
        // The dev-line fixture project is a frozen Cargo package, so its receipt identity moves with the locked
        // sources rather than with an arbitrary file in the directory. The lock is a source input and not a
        // build-unit fact, which is exactly the shape these tests need: a different receipt for the same unit.
        fs::write(project.path().join("Cargo.lock"), "version = 4\n# changed\n")?;
        let mut current = request(project.path(), "legacy", b"same native bytes")?;
        current.kind = OvenArtifactKind::DirectRustcPlan;
        assert_ne!(original.receipt.identity, current.receipt.identity);
        assert_eq!(
            original.receipt.build_unit_identity,
            current.receipt.build_unit_identity
        );
        assert_eq!(store.publish(&current)?, published);
        let selected = store.select_payloads_for_execution(&[published.identity])?;
        assert!(
            selected
                .first()
                .ok_or("legacy owner missing")?
                .original_native_receipt()
                .is_none()
        );
        assert_eq!(immutable()?, before);
        assert!(!entry.join(super::NATIVE_RECEIPT_FILE).exists());
        Ok(())
    }

    /// A valid receipt from another source invocation, or malformed metadata, cannot replace the header's preimage.
    #[test]
    fn native_receipt_witness_rejects_contradiction_and_future_schema() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let mut original = request(project.path(), "recipe", b"native")?;
        original.kind = OvenArtifactKind::DirectRustcPlan;
        let published = store.publish(&original)?;
        let path = store.entry_root(&published.identity).join(super::NATIVE_RECEIPT_FILE);
        let original_bytes = fs::read(&path)?;
        store.select_payloads_for_execution(std::slice::from_ref(&published.identity))?;
        // The dev-line fixture project is a frozen Cargo package, so its receipt identity moves with the locked
        // sources rather than with an arbitrary file in the directory. The lock is a source input and not a
        // build-unit fact, which is exactly the shape these tests need: a different receipt for the same unit.
        fs::write(project.path().join("Cargo.lock"), "version = 4\n# changed\n")?;
        let foreign = request(project.path(), "recipe", b"native")?.receipt;
        foreign.verify_identity()?;
        assert_eq!(foreign.build_unit_identity, original.receipt.build_unit_identity);
        let contradictory = serde_json::to_vec(&super::NativeReceiptWitness {
            schema_version: 1,
            receipt: foreign,
        })?;
        replace_native_witness_fixture(&path, &contradictory)?;
        let error = store
            .select_payloads_for_execution(std::slice::from_ref(&published.identity))
            .err()
            .ok_or("contradictory witness accepted")?;
        assert!(error.to_string().contains("disagrees with entry receipt"), "{error}");
        assert!(
            store.publish(&original).is_err(),
            "dedup must validate an existing contradictory witness"
        );
        replace_native_witness_fixture(&path, br#"{"schema_version":2,"receipt":"not a version-1 receipt"}"#)?;
        let error = store
            .select_payloads_for_execution(std::slice::from_ref(&published.identity))
            .err()
            .ok_or("future schema accepted")?;
        assert!(
            error
                .to_string()
                .contains("unsupported original native receipt witness schema"),
            "{error}"
        );
        let mut invalid = serde_json::to_value(&original.receipt)?;
        invalid["identity"] = serde_json::json!("sha256:invalid");
        replace_native_witness_fixture(
            &path,
            &serde_json::to_vec(&serde_json::json!({"schema_version":1,"receipt":invalid}))?,
        )?;
        assert!(
            store
                .select_payloads_for_execution(std::slice::from_ref(&published.identity))
                .is_err()
        );
        replace_native_witness_fixture(&path, &original_bytes)?;
        store.select_payloads_for_execution(&[published.identity])?;
        Ok(())
    }

    /// The retained lease protects reachability while subsequent admission still detects removed or changed metadata.
    #[test]
    fn native_receipt_witness_revalidates_original_bytes_under_lease() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let mut original = request(project.path(), "recipe", b"native")?;
        original.kind = OvenArtifactKind::DirectRustcPlan;
        let published = store.publish(&original)?;
        let selected = store.select_payloads_for_execution(std::slice::from_ref(&published.identity))?;
        let owner = selected.first().ok_or("native owner missing")?;
        owner.verify_admitted_payload()?;
        let path = store.entry_root(&published.identity).join(super::NATIVE_RECEIPT_FILE);
        let original_bytes = fs::read(&path)?;
        let mut changed = original_bytes.clone();
        changed.push(b' ');
        replace_native_witness_fixture(&path, &changed)?;
        assert!(
            owner.verify_admitted_payload().is_err(),
            "even equivalent metadata must retain its admitted bytes"
        );
        replace_native_witness_fixture(&path, &original_bytes)?;
        owner.verify_admitted_payload()?;
        fs::remove_file(&path)?;
        assert!(
            owner.verify_admitted_payload().is_err(),
            "witness removal must not downgrade an admitted owner"
        );
        fs::write(&path, &original_bytes)?;
        owner.verify_admitted_payload()?;
        Ok(())
    }

    /// Batch publication retains each actual recipe and reserves its metadata without changing logical payload
    /// accounting.
    #[test]
    fn native_receipt_witness_batch_publication_accounts_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let mut first = request(project.path(), "recipe", b"native one")?;
        first.kind = OvenArtifactKind::DirectRustcPlan;
        let mut second = first.clone();
        second.payload = b"native two".to_vec();
        let manifests = store.publish_batch(&[first.clone(), second])?;
        assert_eq!(manifests.len(), 2);
        for manifest in manifests {
            let entry = store.entry_root(&manifest.identity);
            let bytes = fs::read(entry.join(super::NATIVE_RECEIPT_FILE))?;
            assert!(!bytes.is_empty());
            assert!(
                super::conservative_physical_reservation(&manifest, Some(&bytes))?
                    > super::conservative_physical_reservation(&manifest, None)?
            );
            let measured = super::verify_entry(&entry)?;
            assert_eq!(measured.logical_bytes, manifest.payload.logical_bytes);
            assert!(
                measured.physical_bytes
                    >= super::physical_file_bytes(&fs::metadata(entry.join(super::NATIVE_RECEIPT_FILE))?)
            );
            let selected = store.select_payloads_for_execution(&[manifest.identity])?;
            assert_eq!(
                selected.first().ok_or("batch owner missing")?.original_native_receipt(),
                Some(&first.receipt)
            );
        }
        Ok(())
    }

    /// A sparse oversized receipt refuses before allocation, even if its first bytes form valid JSON.
    #[test]
    fn native_receipt_witness_rejects_oversized_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let mut original = request(project.path(), "recipe", b"native")?;
        original.kind = OvenArtifactKind::DirectRustcPlan;
        let published = store.publish(&original)?;
        let path = store.entry_root(&published.identity).join(super::NATIVE_RECEIPT_FILE);
        OpenOptions::new()
            .write(true)
            .open(path)?
            .set_len(super::MAX_NATIVE_RECEIPT_BYTES + 1)?;
        let error = store
            .select_payloads_for_execution(&[published.identity])
            .err()
            .ok_or("oversized witness accepted")?;
        assert!(error.to_string().contains("bounded regular metadata"), "{error}");
        Ok(())
    }

    /// A link to matching receipt bytes does not confer authority on a different physical owner.
    #[cfg(unix)]
    #[test]
    fn native_receipt_witness_rejects_symlink_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let mut original = request(project.path(), "recipe", b"native")?;
        original.kind = OvenArtifactKind::DirectRustcPlan;
        let published = store.publish(&original)?;
        let path = store.entry_root(&published.identity).join(super::NATIVE_RECEIPT_FILE);
        let foreign = project.path().join("receipt.json");
        fs::rename(&path, &foreign)?;
        std::os::unix::fs::symlink(foreign, path)?;
        assert!(store.select_payloads_for_execution(&[published.identity]).is_err());
        Ok(())
    }

    /// Corrupt only this temporary fixture's metadata, restoring its permissions before admission.
    fn replace_native_witness_fixture(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        let original = fs::metadata(path)?.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(original.mode() | 0o200))?;
        }
        #[cfg(not(unix))]
        {
            let mut writable = original.clone();
            writable.set_readonly(false);
            fs::set_permissions(path, writable)?;
        }
        let result = fs::write(path, bytes);
        fs::set_permissions(path, original)?;
        result?;
        Ok(())
    }
}

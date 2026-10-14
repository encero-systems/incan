//! Ordinary per-unit native Loaf records and physical closure admission (#1337, #1698).
//!
//! Each record seals producer-selected physical edges to the exact native recipe. It grants no checked language,
//! Rust semantic, vocabulary, or macro authority. A physical closure must never prune those independent authorities.
//! Engine entries have no original native-receipt witness: the reproduced recipe is explicitly sealed in this record.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use oven_store::store::{
    OvenArtifactKind, OvenArtifactPublishRequest, OvenStore, OvenStoreError, OvenStoreExecutionPayload,
    PublishedOvenStore,
};
use oven_store::{OvenBuildIntent, OvenReceipt, digest_bytes, receipt_with_build_unit_input};
use serde::{Deserialize, Serialize};

use crate::plan::shared::{OvenSharedNativeOwners, OvenSharedNativeRoot};

const SCHEMA: &str = "incan.oven.native-loaf/1";
const DOMAIN: &str = "native-loaf-record";
const INPUT: &str = "native-loaf-record";
/// Native recipe input binding its full independently checked source selection.
pub(crate) const SOURCE_INPUT: &str = "native-source-binding";
/// Native recipe input binding the complete physical selected-destination set.
pub(crate) const EDGES_INPUT: &str = "native-physical-edges";
/// Native recipe input retaining explicit archive-versus-local preparation provenance.
pub(crate) const ORIGIN_INPUT: &str = "native-source-origin";

mod preparation;
mod prepared;
mod producer;
mod request_observation;
mod selection;
pub use preparation::{
    NativeLoafFacet, NativeLoafPreparation, NativeLoafPreparationReport, NativeLoafPreparationRequest,
    prepare_native_loafs, prepare_resolved_native_loafs, prepare_resolved_native_loafs_in_store,
    resolved_native_graph_coordinates,
};
pub use prepared::{
    NativeLoafConsumerPreparation, NativeLoafConsumerReport, NativeLoafConsumerRequest, prepare_declared_native_loafs,
    prepare_declared_native_loafs_in_store,
};
pub(crate) use selection::verify_native_source_declaration;

pub use request_observation::NativeLoafRequestObservation;

pub use producer::{NativeLoafInspectionInputs, NativeLoafInspectionUnit, NativeLoafInspectionWork};

/// Independently established producer boundary for a native source generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeLoafOrigin {
    /// Source admitted from a digest-verified archive and pinned adopted index.
    Registry,
    /// Source mapped from the explicitly selected current authored local project.
    Local,
}

impl NativeLoafOrigin {
    /// Stable recipe spelling, assigned by the producer rather than inferred from a graph name.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Registry => "registry",
            Self::Local => "local",
        }
    }
}

/// Exact source selection supplied by the native producer, without catalog-only graph edges.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeLoafSource {
    /// Registry-qualified or local package name.
    pub loaf: String,
    /// Exact selected version.
    pub version: String,
    /// Producer's complete source-generation digest.
    pub archive_digest: String,
    /// Exact host or target compilation domain.
    pub domain: String,
    /// Complete enabled features, in producer order.
    pub features: Vec<String>,
    /// Evaluated dependency predicates retained from the resolver.
    pub target_predicates: Vec<NativeLoafPredicate>,
}

/// One already evaluated source dependency condition; admission does not resolve or reevaluate it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeLoafPredicate {
    /// Original declaration position.
    pub declaration: usize,
    /// Authored condition text.
    pub target: String,
    /// Resolver's result for this source selection.
    pub matches: bool,
}

/// Portable native coordinates resolved only in the explicitly supplied original Store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeLoafReference {
    /// Immutable native entry identity, distinct from native-byte equality.
    pub identity: String,
    /// Exact receipt under which the native entry was published.
    pub receipt_identity: String,
    /// Exact native publication domain, independent of this record's publication domain.
    pub domain: String,
    /// Native member relative to that entry's artifact root.
    pub relative_path: String,
    /// Native bytes bound by the admitted member inventory.
    pub digest: String,
}

/// One producer-selected physical extern and the child's durable ordinary record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeLoafDependency {
    /// Exact normalized Rust extern alias; renames remain distinct.
    pub alias: String,
    /// Child record identity that seals its complete physical generation.
    pub record_identity: String,
    /// Exact child native owner and output selected by the parent producer.
    pub native: NativeLoafReference,
    /// Child's exact producer-bound source selection; checked independently of the graph record seal.
    pub source: NativeLoafSource,
}

/// Full portable physical destination bound before native compilation, without the later child record address.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct NativeLoafPhysicalBinding {
    pub(crate) alias: String,
    pub(crate) source: NativeLoafSource,
    pub(crate) native: NativeLoafReference,
}

/// Bind complete source selection to the actual native recipe, independently of the later graph record.
pub(crate) fn source_binding_input(source: &NativeLoafSource) -> Result<String> {
    serde_json::to_vec(source)
        .map(|bytes| digest_bytes(&bytes))
        .map_err(failed)
}

/// Canonicalize complete alias/destination source/owner coordinates before native recipe publication.
pub(crate) fn physical_edges_input(edges: &[NativeLoafPhysicalBinding]) -> Result<String> {
    let mut canonical = BTreeMap::new();
    for edge in edges {
        if edge.alias.is_empty() || canonical.insert(&edge.alias, (&edge.source, &edge.native)).is_some() {
            return Err(refused("physical native extern alias is empty or duplicated"));
        }
    }
    serde_json::to_vec(&canonical)
        .map(|bytes| digest_bytes(&bytes))
        .map_err(failed)
}

/// Durable per-unit physical authority, sealed by an ordinary Engine receipt and immutable payload.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeLoafRecord {
    schema: String,
    /// Complete source identity selected by the producer.
    pub source: NativeLoafSource,
    /// Native owner authorized by this record.
    pub native: NativeLoafReference,
    /// Original native owner payload digest; source descriptors cannot be exchanged for equal output bytes.
    pub native_payload_digest: String,
    /// Exact reproduced native recipe, not a claimed on-disk Engine witness.
    pub recipe: OvenReceipt,
    /// Complete producer-selected extern set, including proven empty leaves.
    pub dependencies: Vec<NativeLoafDependency>,
}

/// An exact declared root; callers supply current source and intent authority instead of matching by package name.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeLoafRoot {
    /// Authored consumer alias, retained independently of the native producer's internal extern aliases.
    pub alias: String,
    /// Exact durable record selected by the caller's declaration/lock authority.
    pub record_identity: String,
    /// Current source identity the caller expects.
    pub source: NativeLoafSource,
    /// Exact current target, toolchain, profile and enabled features.
    pub intent: OvenBuildIntent,
}

/// Definitive admission failures; neither variant authorizes a compatibility baker or resolver fallback.
#[derive(Debug, thiserror::Error)]
pub enum NativeLoafError {
    /// An exact requested record or physical owner is unavailable in the supplied Store.
    #[error("ordinary native Loaf {identity} is unavailable: {source}")]
    Unavailable {
        /// Missing exact coordinate.
        identity: String,
        /// Underlying Store failure, when one exists.
        source: Box<dyn std::error::Error>,
    },
    /// A selected record, recipe, root or retained owner contradicts its authority.
    #[error("ordinary native Loaf refused: {0}")]
    Refused(String),
    /// An integrity or I/O failure that must remain distinct from an ordinary preparation miss.
    #[error("ordinary native Loaf admission failed: {0}")]
    Failed(#[source] Box<dyn std::error::Error>),
}

type Result<T> = std::result::Result<T, NativeLoafError>;

/// One sealed immutable record and its original native/record execution leases.
///
/// Construction validates the complete recipe and owner bindings. Private fields prevent substitution afterward;
/// every physical handoff still verifies the original owners and consumed on-disk members.
pub struct SelectedNativeLoaf {
    record: NativeLoafRecord,
    record_owner: Arc<OvenStoreExecutionPayload>,
    native_owner: Arc<OvenStoreExecutionPayload>,
    read_only: bool,
    /// Parsed declaration belongs to this exact leased owner; every reuse still reconciles its source bytes.
    source_declaration: OnceLock<toml::Value>,
}

impl SelectedNativeLoaf {
    /// Seal immutable record/recipe/owner bindings before exposing any selected metadata.
    ///
    /// This is the only constructor for producer and Store admission. It grants no freshness exemption: accessors
    /// and graph selection verify original on-disk payloads, native members and declarations independently.
    fn new(
        record: NativeLoafRecord,
        record_owner: Arc<OvenStoreExecutionPayload>,
        native_owner: Arc<OvenStoreExecutionPayload>,
        read_only: bool,
    ) -> Result<Self> {
        verify_record_bindings(&record, &record_owner, &native_owner)?;
        Ok(Self {
            record,
            record_owner,
            native_owner,
            read_only,
            source_declaration: OnceLock::new(),
        })
    }

    /// Borrow the explicit preparation origin authenticated by this native recipe.
    pub fn source_origin(&self) -> Result<NativeLoafOrigin> {
        self.verify()?;
        source_origin(&self.record.recipe)
    }
    /// Project an exact declared-root request from this current producer/admission authority, preserving its alias.
    pub fn declared_root(&self, alias: &str) -> Result<NativeLoafRoot> {
        self.verify()?;
        if alias.is_empty() {
            return Err(refused("declared native root alias is empty"));
        }
        Ok(NativeLoafRoot {
            alias: alias.to_string(),
            record_identity: self.identity().to_string(),
            source: self.record.source.clone(),
            intent: self.record.recipe.intent.clone(),
        })
    }
    /// Borrow the authenticated per-unit record without implying semantic or macro completeness.
    pub fn record(&self) -> &NativeLoafRecord {
        &self.record
    }

    /// Borrow the exact durable record coordinate under its held lease.
    pub fn identity(&self) -> &str {
        &self.record_owner.manifest().identity
    }

    /// Revalidate both original payloads and consumed native files on each physical handoff.
    /// Immutable recipe bindings were sealed by construction and cannot change through this capability.
    #[track_caller]
    pub fn verify(&self) -> Result<()> {
        let caller = std::panic::Location::caller();
        let _invocation = tracing::trace_span!(target: "incan::invocation", "oven_rustc::verify_selected_native_loaf", input = %self.identity(), caller_file = caller.file(), caller_line = u64::from(caller.line())).entered();
        self.record_owner.verify_admitted_payload().map_err(failed)?;
        verify_native_inventory(&self.native_owner, NativeInventory::Execution)
    }

    /// Return the native output only after revalidating its original held owner.
    pub fn output(&self) -> Result<PathBuf> {
        self.verify()?;
        Ok(self
            .native_owner
            .artifact_root()
            .join(&self.record.native.relative_path))
    }

    /// Borrow the verified complete native member inventory while retaining the original owner, excluding sources.
    ///
    /// Callers project these exact paths/digests into direct plans, including any admitted static archives or support
    /// files. This accessor neither copies files nor adds source snapshots to native search paths.
    pub fn materialized_files(
        &self,
    ) -> Result<impl Iterator<Item = &oven_store::store::OvenArtifactMaterializedFileManifest>> {
        self.verify()?;
        Ok(self.native_files())
    }

    /// Project a shared root and its complete native inventory after one original-owner verification.
    ///
    /// Consumer plan construction needs both coordinates and members in the same handoff. Returning them together
    /// avoids walking the same retained source and native files again for each accessor. Every new call verifies
    /// current bytes; the returned inventory borrows this selected unit and excludes retained source snapshots.
    pub fn plan_members(
        &self,
        prefix: &str,
    ) -> Result<(
        OvenSharedNativeRoot,
        impl Iterator<Item = &oven_store::store::OvenArtifactMaterializedFileManifest>,
    )> {
        if prefix.is_empty()
            || !Path::new(prefix)
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
        {
            return Err(refused("native shared-root prefix is unsafe"));
        }
        self.verify()?;
        Ok((
            OvenSharedNativeRoot {
                store: original_store(&self.native_owner)?,
                identity: self.record.native.identity.clone(),
                receipt_identity: self.record.native.receipt_identity.clone(),
                prefix: prefix.to_string(),
            },
            self.native_files(),
        ))
    }

    /// Borrow member metadata only within an accessor that has already verified this handoff.
    fn native_files(&self) -> impl Iterator<Item = &oven_store::store::OvenArtifactMaterializedFileManifest> {
        self.native_owner
            .admitted_materialized_files()
            .iter()
            .filter(|file| !file.relative_path.starts_with("source/"))
    }
}

/// Command-local graph of already admitted ordinary records, retaining original leases without reacquisition.
///
/// This is not a persisted aggregate index. Installed admission discovers only dependencies sealed in selected
/// per-unit records; catalog or root-facing SDK edges never enter this graph.
#[derive(Default)]
pub struct NativeLoafGraph {
    units: BTreeMap<String, Arc<SelectedNativeLoaf>>,
}

/// Declared-root forward physical closure, with authored aliases and independent native owners retained.
pub struct NativeLoafClosure {
    roots: BTreeMap<String, String>,
    graph: NativeLoafGraph,
}

/// Actual graph verification attempts within a caller-owned admission or selection handoff.
///
/// Counts accumulate across calls, including work before refusal. They exclude canonical Store selection checks,
/// internal file I/O and later independent handoffs; a retained lease does not grant permission to skip those checks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct NativeLoafVerificationWork {
    /// Original record/native-owner integrity checks, including repeated visits to shared records.
    /// Immutable recipe bindings are sealed by construction rather than reconstructed on each handoff.
    pub record_checks: usize,
    /// Exact child source/native-coordinate comparisons attempted for sealed physical edges.
    pub edge_checks: usize,
}

impl NativeLoafGraph {
    /// Retain and validate inspection sources for exactly this admitted set, without claiming semantic completeness.
    pub fn inspection_inputs(&self) -> Result<NativeLoafInspectionInputs> {
        self.inspection_inputs_with_work(&mut NativeLoafInspectionWork::default())
    }

    /// Retain the same exact source set with command-owned counts of actual projection work and refused attempts.
    pub fn inspection_inputs_with_work(
        &self,
        work: &mut NativeLoafInspectionWork,
    ) -> Result<NativeLoafInspectionInputs> {
        producer::from_graph(self, work)
    }

    /// Project already active declaration requirements onto exact prepared source/version/feature/domain roots.
    ///
    /// The caller owns optional/cfg/test activation through its existing resolver. This helper does not activate or
    /// silently skip declarations, resolve new versions, or compile missing units. Local paths must reproduce the
    /// producer's current authored source mapping; registry roots use verified archive-origin source metadata.
    /// The owner is a directory. Relative dependency paths are owner-relative; load parsed declarations from a
    /// canonical manifest path so parser-resolved paths are absolute and cannot be joined to their owner twice.
    pub fn select_dependency_roots(
        &self,
        dependencies: &[oven_model::manifest::DependencySpec],
        declaration_owner: &Path,
        domain: &str,
    ) -> Result<Vec<NativeLoafRoot>> {
        selection::select_roots(self, dependencies, declaration_owner, domain)
    }
    /// Borrow exact admitted records, for declaration-authorized root selection and inspection of physical facts.
    pub fn units(&self) -> &BTreeMap<String, Arc<SelectedNativeLoaf>> {
        &self.units
    }

    /// Select only declared roots and their authenticated forward physical dependencies, sharing original leases.
    pub fn select(&self, roots: &[NativeLoafRoot]) -> Result<NativeLoafClosure> {
        self.select_with_work(roots, &mut NativeLoafVerificationWork::default())
    }

    /// Select the same authenticated closure while recording actual verification work for this handoff.
    pub fn select_with_work(
        &self,
        roots: &[NativeLoafRoot],
        work: &mut NativeLoafVerificationWork,
    ) -> Result<NativeLoafClosure> {
        let root_map = validate_roots(roots, &self.units)?;
        let mut selected = BTreeMap::new();
        let mut visiting = BTreeSet::new();
        for identity in root_map.values() {
            retain_forward(identity, &self.units, &mut selected, &mut visiting, work)?;
        }
        Ok(NativeLoafClosure {
            roots: root_map,
            graph: Self { units: selected },
        })
    }

    /// Find affected reverse dependents within this explicitly admitted graph for invalidation, never linking.
    pub fn affected_dependents(&self, changed: &BTreeSet<String>) -> Result<BTreeSet<String>> {
        for identity in changed {
            if !self.units.contains_key(identity) {
                return Err(refused("changed native record is not admitted"));
            }
        }
        let mut affected = changed.clone();
        loop {
            let before = affected.len();
            for (identity, unit) in &self.units {
                unit.verify()?;
                if unit
                    .record
                    .dependencies
                    .iter()
                    .any(|edge| affected.contains(&edge.record_identity))
                {
                    affected.insert(identity.clone());
                }
            }
            if affected.len() == before {
                return Ok(affected);
            }
        }
    }

    /// Publish a producer-authenticated per-unit record in the same Store as its original native owner.
    ///
    /// The caller is the native producer boundary, which has checked exact selected destinations against its recipe.
    /// Native bytes are never copied or reacquired. A graph spanning multiple Stores needs an explicit portable Store
    /// authority contract and is refused by this initial implementation rather than guessed from ambient paths.
    pub(crate) fn publish(
        &mut self,
        store: &OvenStore,
        source: NativeLoafSource,
        native: NativeLoafReference,
        recipe: OvenReceipt,
        owner: Arc<OvenStoreExecutionPayload>,
        dependencies: Vec<NativeLoafDependency>,
    ) -> Result<String> {
        if original_store(&owner)? != std::fs::canonicalize(store.root()).map_err(failed)? {
            return Err(refused(
                "native record and original owner must belong to the same Store",
            ));
        }
        let record = NativeLoafRecord {
            schema: SCHEMA.to_string(),
            source,
            native,
            native_payload_digest: owner.manifest().payload.digest.clone(),
            recipe,
            dependencies,
        };
        verify_native(&record, &owner, NativeInventory::Execution)?;
        verify_children(&record, &self.units)?;
        let payload = serde_json::to_vec(&record).map_err(failed)?;
        let receipt = record_receipt(&record, &payload)?;
        let manifest = store
            .publish(&OvenArtifactPublishRequest {
                receipt,
                domain: DOMAIN.to_string(),
                kind: OvenArtifactKind::Engine,
                payload,
                materialized_files: Vec::new(),
                materialized_directories: Vec::new(),
            })
            .map_err(failed)?;
        let record_owner = select_owner(Store::Writable(store), &manifest.identity)?;
        let unit = Arc::new(SelectedNativeLoaf::new(record, record_owner, owner, false)?);
        unit.verify()?;
        self.units.insert(manifest.identity.clone(), unit);
        Ok(manifest.identity)
    }
}

impl NativeLoafClosure {
    /// Retain the selected physical set as source inputs; callers still own complete semantic-world selection.
    pub fn inspection_inputs(&self) -> Result<NativeLoafInspectionInputs> {
        self.graph.inspection_inputs()
    }

    /// Admit installed or prepared ordinary records through the canonical writable Store selector.
    ///
    /// Missing exact authority is an explicit error; this path never compiles, resolves, or invokes a fallback baker.
    pub fn admit(store: &OvenStore, roots: &[NativeLoafRoot]) -> Result<Self> {
        Self::admit_with_work(store, roots, &mut NativeLoafVerificationWork::default())
    }

    /// Admit the same writable Store closure with actual graph verification counts, without changing authority.
    pub fn admit_with_work(
        store: &OvenStore,
        roots: &[NativeLoafRoot],
        work: &mut NativeLoafVerificationWork,
    ) -> Result<Self> {
        admit(Store::Writable(store), roots, work)
    }

    /// Admit an immutable installed Store through its existing read-only selector, without writing package files.
    pub fn admit_published(store: &PublishedOvenStore, roots: &[NativeLoafRoot]) -> Result<Self> {
        Self::admit_published_with_work(store, roots, &mut NativeLoafVerificationWork::default())
    }

    /// Admit the same installed closure with actual graph verification counts and no installed-file writes.
    pub fn admit_published_with_work(
        store: &PublishedOvenStore,
        roots: &[NativeLoafRoot],
        work: &mut NativeLoafVerificationWork,
    ) -> Result<Self> {
        admit(Store::Published(store), roots, work)
    }

    /// Borrow the complete authenticated physical closure retained for this command.
    pub fn graph(&self) -> &NativeLoafGraph {
        &self.graph
    }

    /// Borrow authored root aliases without exposing transitive dependencies as direct imports.
    pub fn roots(&self) -> &BTreeMap<String, String> {
        &self.roots
    }

    /// Build the existing shared-owner handoff index using the original Arc-held native leases.
    pub fn shared_owners(&self) -> Result<OvenSharedNativeOwners> {
        for unit in self.graph.units.values() {
            unit.verify()?;
        }
        let owners = self
            .graph
            .units
            .values()
            .map(|unit| Arc::clone(&unit.native_owner))
            .collect::<Vec<_>>();
        let read_only = self.graph.units.values().any(|unit| unit.read_only);
        OvenSharedNativeOwners::from_record_bound(&owners, read_only).map_err(failed)
    }

    /// Export exact physical shared-root references for a caller-assigned logical prefix per admitted record.
    pub fn shared_root(&self, identity: &str, prefix: &str) -> Result<OvenSharedNativeRoot> {
        let unit = self
            .graph
            .units
            .get(identity)
            .ok_or_else(|| refused("shared root is not admitted"))?;
        let (root, _) = unit.plan_members(prefix)?;
        Ok(root)
    }
}

/// Borrow an existing canonical Store selector; neither mode invents another admission authority.
#[derive(Clone, Copy)]
enum Store<'a> {
    Writable(&'a OvenStore),
    Published(&'a PublishedOvenStore),
}

/// Acquire record frontiers and native owners in batches, then run the existing complete graph verifier once.
///
/// Discovery supplies coordinates only. Construction seals each record's recipe and owner bindings; `select_with_work`
/// then verifies original on-disk members and every sealed edge before granting execution authority. Both Store modes
/// retain their canonical selectors and original leases; batching avoids a manager-lock acquisition and access update
/// transaction per individual owner.
fn admit(
    store: Store<'_>,
    roots: &[NativeLoafRoot],
    work: &mut NativeLoafVerificationWork,
) -> Result<NativeLoafClosure> {
    // ---- Discovery: retain exact record owners without granting their contents authority ----
    let mut records = BTreeMap::new();
    let mut pending = roots
        .iter()
        .map(|root| root.record_identity.clone())
        .collect::<BTreeSet<_>>();
    while !pending.is_empty() {
        let identities = pending
            .into_iter()
            .filter(|identity| !records.contains_key(identity))
            .collect::<Vec<_>>();
        pending = BTreeSet::new();
        for owner in select_owners(store, &identities)? {
            let record: NativeLoafRecord = serde_json::from_slice(owner.payload()).map_err(failed)?;
            pending.extend(record.dependencies.iter().map(|edge| edge.record_identity.clone()));
            records.insert(owner.manifest().identity.clone(), (record, owner));
        }
    }

    // ---- Original native owners: one canonical batch, shared by every referencing record ----
    let identities = records
        .values()
        .map(|(record, _)| record.native.identity.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let native = select_owners(store, &identities)?
        .into_iter()
        .map(|owner| (owner.manifest().identity.clone(), owner))
        .collect::<BTreeMap<_, _>>();
    let mut graph = NativeLoafGraph::default();
    let read_only = matches!(store, Store::Published(_));
    for (identity, (record, record_owner)) in records {
        let owner = native
            .get(&record.native.identity)
            .ok_or_else(|| refused("native owner batch is incomplete"))?;
        graph.units.insert(
            identity,
            Arc::new(SelectedNativeLoaf::new(
                record,
                record_owner,
                Arc::clone(owner),
                read_only,
            )?),
        );
    }
    // ---- Admission: the existing verifier refuses cycles, altered owners and substituted edges ----
    graph.select_with_work(roots, work)
}

/// Acquire one exact descriptor, payload and lease through the canonical selector, preserving preparation misses.
/// The record verifier owns complete inventory validation for this admission; selection must not walk it first.
fn select_owner(store: Store<'_>, identity: &str) -> Result<Arc<OvenStoreExecutionPayload>> {
    select_owners(store, &[identity.to_string()])?
        .pop()
        .ok_or_else(|| refused("exact native Loaf selection became empty"))
}

/// Acquire a deduplicated exact-owner batch through the same canonical selector, preserving preparation misses.
fn select_owners(store: Store<'_>, identities: &[String]) -> Result<Vec<Arc<OvenStoreExecutionPayload>>> {
    let Some(first) = identities.first() else {
        return Ok(Vec::new());
    };
    let selected = match store {
        Store::Writable(store) => store.select_payloads_for_execution(identities),
        Store::Published(store) => store.select_payloads_for_execution(identities),
    };
    let selected = selected.map_err(|error| match &error {
        OvenStoreError::Io { path, source } if source.kind() == std::io::ErrorKind::NotFound => {
            let missing = identities
                .iter()
                .find(|identity| {
                    let coordinate = identity.replace(':', "-");
                    path.components().any(|part| part.as_os_str() == coordinate.as_str())
                })
                .unwrap_or(first);
            NativeLoafError::Unavailable {
                identity: missing.clone(),
                source: Box::new(error),
            }
        }
        _ => failed(error),
    })?;
    if selected.len() != identities.len() {
        return Err(NativeLoafError::Unavailable {
            identity: first.clone(),
            source: Box::new(std::io::Error::other("exact native Loaf owner not found")),
        });
    }
    Ok(selected.into_iter().map(Arc::new).collect())
}

/// Check declared source/intent and aliases; the containing traversal independently verifies every root owner.
fn validate_roots(
    roots: &[NativeLoafRoot],
    units: &BTreeMap<String, Arc<SelectedNativeLoaf>>,
) -> Result<BTreeMap<String, String>> {
    let mut aliases = BTreeMap::new();
    for root in roots {
        let unit = units
            .get(&root.record_identity)
            .ok_or_else(|| refused("declared native root is not admitted"))?;
        if root.alias.is_empty()
            || aliases
                .insert(root.alias.clone(), root.record_identity.clone())
                .is_some()
        {
            return Err(refused("declared native root alias is empty or duplicated"));
        }
        if root.source != unit.record.source || root.intent != unit.record.recipe.intent {
            return Err(refused(
                "declared native root source or intent differs from its sealed record",
            ));
        }
    }
    Ok(aliases)
}

/// Verify each selected vertex once in this handoff while comparing every exact sealed child reference.
fn retain_forward(
    identity: &str,
    units: &BTreeMap<String, Arc<SelectedNativeLoaf>>,
    selected: &mut BTreeMap<String, Arc<SelectedNativeLoaf>>,
    visiting: &mut BTreeSet<String>,
    work: &mut NativeLoafVerificationWork,
) -> Result<()> {
    if selected.contains_key(identity) {
        return Ok(());
    }
    if !visiting.insert(identity.to_string()) {
        return Err(refused("native physical dependency cycle"));
    }
    let unit = units
        .get(identity)
        .ok_or_else(|| refused("physical native dependency record is missing"))?;
    work.record_checks += 1;
    unit.verify()?;
    verify_child_bindings_with_work(&unit.record, units, work)?;
    for edge in &unit.record.dependencies {
        retain_forward(&edge.record_identity, units, selected, visiting, work)?;
    }
    selected.insert(identity.to_string(), Arc::clone(unit));
    visiting.remove(identity);
    Ok(())
}

/// Check sealed child coordinates against the exact admitted child records, never just equal native digests.
fn verify_children(record: &NativeLoafRecord, units: &BTreeMap<String, Arc<SelectedNativeLoaf>>) -> Result<()> {
    for edge in &record.dependencies {
        units
            .get(&edge.record_identity)
            .ok_or_else(|| refused("physical native dependency record is missing"))?
            .verify()?;
    }
    verify_child_bindings(record, units)
}

/// Compare exact physical destinations after the caller verifies each selected owner for its handoff.
fn verify_child_bindings(record: &NativeLoafRecord, units: &BTreeMap<String, Arc<SelectedNativeLoaf>>) -> Result<()> {
    verify_child_bindings_with_work(record, units, &mut NativeLoafVerificationWork::default())
}

/// Count attempted edge comparisons separately from complete original-owner verification.
fn verify_child_bindings_with_work(
    record: &NativeLoafRecord,
    units: &BTreeMap<String, Arc<SelectedNativeLoaf>>,
    work: &mut NativeLoafVerificationWork,
) -> Result<()> {
    for edge in &record.dependencies {
        work.edge_checks += 1;
        let child = units
            .get(&edge.record_identity)
            .ok_or_else(|| refused("physical native dependency record is missing"))?;
        if child.record.native != edge.native || child.record.source != edge.source {
            return Err(refused("physical native dependency owner was substituted"));
        }
    }
    Ok(())
}

/// Derive the ordinary record receipt from exact native source/intent evidence and its canonical payload digest.
fn record_receipt(record: &NativeLoafRecord, payload: &[u8]) -> Result<OvenReceipt> {
    receipt_with_build_unit_input(&record.recipe, INPUT, digest_bytes(payload)).map_err(failed)
}

/// Choose the materialized inventory consumed by this handoff; both scopes authenticate the same original owners.
enum NativeInventory {
    /// Physical execution consumes native members and separately checks the exact source declaration.
    Execution,
    /// Metadata inspection consumes the complete native owner, including its archived source members.
    Complete,
}

/// Check the selected record's complete derived receipt and original native owner once for the consumed inventory.
fn verify_record(
    record: &NativeLoafRecord,
    owner: &OvenStoreExecutionPayload,
    native: &OvenStoreExecutionPayload,
    inventory: NativeInventory,
) -> Result<()> {
    verify_record_bindings(record, owner, native)?;
    owner.verify_admitted_payload().map_err(failed)?;
    verify_native_inventory(native, inventory)
}

/// Validate immutable record, recipe and owner coordinates once before sealing a selected capability.
/// Physical freshness remains the responsibility of each consuming handoff, never this metadata check.
fn verify_record_bindings(
    record: &NativeLoafRecord,
    owner: &OvenStoreExecutionPayload,
    native: &OvenStoreExecutionPayload,
) -> Result<()> {
    let _invocation =
        tracing::trace_span!(target: "incan::invocation", "oven_rustc::validate_native_loaf_bindings").entered();
    let payload = serde_json::to_vec(record).map_err(failed)?;
    let receipt = record_receipt(record, &payload)?;
    if record.schema != SCHEMA
        || owner.manifest().kind != OvenArtifactKind::Engine
        || owner.manifest().domain != DOMAIN
        || owner.payload() != payload
        || owner.manifest().receipt_identity != receipt.identity
        || owner.manifest().build_unit_identity != receipt.build_unit_identity
        || owner.manifest().intent != receipt.intent
        || original_store(owner)? != original_store(native)?
    {
        return Err(refused(
            "native record payload, recipe, intent or Store identity differs",
        ));
    }
    verify_native_bindings(record, native)
}

/// Revalidate the original native generation and prove the complete extern alias-to-bytes recipe.
/// Metadata inspection chooses the full inventory here, avoiding a second walk over native members afterward.
fn verify_native(
    record: &NativeLoafRecord,
    owner: &OvenStoreExecutionPayload,
    inventory: NativeInventory,
) -> Result<()> {
    verify_native_bindings(record, owner)?;
    verify_native_inventory(owner, inventory)
}

/// Check original on-disk payloads and consumed members without reconstructing sealed recipe metadata.
/// Execution excludes archived source members while independently validating the source declaration.
fn verify_native_inventory(owner: &OvenStoreExecutionPayload, inventory: NativeInventory) -> Result<()> {
    let _invocation = tracing::trace_span!(target: "incan::invocation", "oven_rustc::verify_native_inventory", input = %owner.manifest().identity).entered();
    match inventory {
        NativeInventory::Execution => owner.verify_admitted_payload_except_directory("source"),
        NativeInventory::Complete => owner.verify_admitted_payload(),
    }
    .map_err(failed)?;
    selection::verify_native_source_declaration(owner)
}

/// Bind the immutable native owner and complete physical extern set to the producer's exact recipe.
fn verify_native_bindings(record: &NativeLoafRecord, owner: &OvenStoreExecutionPayload) -> Result<()> {
    record.recipe.verify_identity().map_err(failed)?;
    source_origin(&record.recipe)?;
    let source = &record.source;
    let native = &record.native;
    let admitted_source: NativeLoafSource = serde_json::from_slice(owner.payload()).map_err(failed)?;
    let source_input = source_binding_input(source)?;
    if owner.manifest().kind != OvenArtifactKind::Engine
        || owner.manifest().identity != native.identity
        || owner.manifest().receipt_identity != native.receipt_identity
        || record.recipe.identity != native.receipt_identity
        || owner.manifest().domain != native.domain
        || owner.manifest().build_unit_identity != record.recipe.build_unit_identity
        || owner.manifest().intent != record.recipe.intent
        || owner.manifest().payload.digest != record.native_payload_digest
        || source.loaf != record.recipe.project.name
        || source.version != record.recipe.project.version
        || admitted_source != *source
        || record.recipe.sources.build_unit_inputs.get(SOURCE_INPUT) != Some(&source_input)
        || source.features.iter().collect::<BTreeSet<_>>()
            != record.recipe.intent.features.iter().collect::<BTreeSet<_>>()
    {
        return Err(refused(
            "native owner differs from its sealed source, receipt or intent",
        ));
    }
    if native.relative_path.is_empty()
        || native.relative_path.contains(['/', '\\'])
        || !owner
            .admitted_materialized_files()
            .iter()
            .any(|file| file.relative_path == native.relative_path && file.digest == native.digest)
    {
        return Err(refused("native output differs from its exact admitted member"));
    }
    let mut edges = BTreeMap::new();
    for edge in &record.dependencies {
        if edge.alias.is_empty()
            || edges
                .insert(format!("extern:{}", edge.alias), edge.native.digest.clone())
                .is_some()
        {
            return Err(refused("physical native extern alias is empty or duplicated"));
        }
    }
    let witnessed = record
        .recipe
        .sources
        .build_unit_inputs
        .iter()
        .filter(|(key, _)| key.starts_with("extern:"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<BTreeMap<_, _>>();
    if edges != witnessed {
        return Err(refused("physical native edges disagree with the exact extern recipe"));
    }
    let bindings = record
        .dependencies
        .iter()
        .map(|edge| NativeLoafPhysicalBinding {
            alias: edge.alias.clone(),
            source: edge.source.clone(),
            native: edge.native.clone(),
        })
        .collect::<Vec<_>>();
    let physical_input = physical_edges_input(&bindings)?;
    if record.recipe.sources.build_unit_inputs.get(EDGES_INPUT) != Some(&physical_input) {
        return Err(refused(
            "physical native owner coordinates disagree with the actual producer recipe",
        ));
    }
    Ok(())
}

/// Read only explicit independently assigned producer provenance; unknown or absent authority is refused.
fn source_origin(recipe: &OvenReceipt) -> Result<NativeLoafOrigin> {
    match recipe.sources.build_unit_inputs.get(ORIGIN_INPUT).map(String::as_str) {
        Some("registry") => Ok(NativeLoafOrigin::Registry),
        Some("local") => Ok(NativeLoafOrigin::Local),
        _ => Err(refused("native source preparation origin is unavailable")),
    }
}

/// Derive the actual canonical Store coordinate from an originally admitted owner, never a caller-provided path.
fn original_store(owner: &OvenStoreExecutionPayload) -> Result<PathBuf> {
    owner
        .artifact_root()
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| refused("native owner has no original Store coordinate"))
}

/// Preserve a concrete lower-layer failure as a source without flattening its integrity evidence.
fn failed(error: impl std::error::Error + 'static) -> NativeLoafError {
    NativeLoafError::Failed(Box::new(error))
}

/// Preserve the failing native input coordinate and original error chain without changing admission classification.
#[derive(Debug, thiserror::Error)]
#[error("{operation} {}: {source}", path.display())]
struct NativeInputFailure {
    operation: &'static str,
    path: PathBuf,
    #[source]
    source: Box<dyn std::error::Error>,
}

/// Add an operation and path at the input boundary; the error remains a definitive admission failure.
fn input_failure(
    operation: &'static str,
    path: &Path,
    source: impl Into<Box<dyn std::error::Error>>,
) -> NativeLoafError {
    failed(NativeInputFailure {
        operation,
        path: path.to_path_buf(),
        source: source.into(),
    })
}

/// Construct a definitive semantic or physical authority refusal.
fn refused(message: &str) -> NativeLoafError {
    NativeLoafError::Refused(message.to_string())
}

#[cfg(test)]
mod tests;

//! The portable Rust facet graph selected before physical rust-analyzer projection: its wire schema, the
//! validation that admits it, and the relocation-independent identity a Store payload or inspection sidecar
//! carries for it.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

mod validation;

pub use validation::*;

/// Wire schema for the portable Rust facet graph selected before physical rust-analyzer projection.
pub const OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION: u32 = 4;
const OVEN_SELECTED_RUST_FACET_GRAPH_DIGEST_DOMAIN: &str = "incan.oven.selected-rust-facet-graph/1";
pub(crate) const OVEN_SELECTED_RUST_FACET_UNIT_DIGEST_DOMAIN: &str = "incan.oven.selected-rust-facet-unit/1";

/// Command purpose whose dependency roles and feature activation produced a selected Rust graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OvenSelectedRustFacetPurpose {
    /// Ordinary build, run, inspection, or publication inputs.
    Normal,
    /// Test inputs, including development dependencies selected for the owning test unit.
    Test,
}

/// Portable provenance class for one immutable physical owner retained outside the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OvenSelectedRustFacetOwnerKind {
    /// The singular project inspection authority owns the referenced bytes directly.
    ProjectAuthority,
    /// One exact Store or Loaf constituent owns the referenced bytes.
    Constituent,
    /// A selected toolchain inspection closure owns compiler source or target facts.
    Toolchain,
    /// An admitted provider execution owns generated output bytes.
    GeneratedOutput,
    /// One sealed linked-library or SDK provider owns a declared capability fact.
    LinkedLibraryProvider,
}

/// One immutable owner identity; its physical root is supplied separately under a retained lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenSelectedRustFacetOwner {
    /// Content-addressed Loaf, authority, or provider-output identity.
    ///
    /// A `Toolchain` owner uses `OvenRustInspectionToolchainPayload::closure_digest`; it is deliberately distinct
    /// from both the Store manifest identity and the human-readable Rust release in `selection.intent.toolchain`.
    pub identity: String,
    /// Provenance class used by physical admission to choose the correct owner table.
    pub kind: OvenSelectedRustFacetOwnerKind,
}

/// Selected source provenance without a machine-local checkout or Store path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OvenSelectedRustFacetSourceKind {
    /// Immutable registry package source.
    Registry,
    /// Immutable Git revision source.
    Git,
    /// Author-owned path source admitted by content.
    Path,
    /// Rust emitted by a checked Incan or provider compilation.
    Generated,
    /// Compiler-owned support source selected from the Toolchain owner's fixed `sysroot_units` and `rust_src`.
    Compiler,
}

/// Complete portable source-tree identity for one selected unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenSelectedRustFacetSource {
    /// Source category retained from the selected producer.
    pub kind: OvenSelectedRustFacetSourceKind,
    /// Producer-established source coordinate, such as a locked registry package or compiler component.
    pub identity: String,
    /// Stable owner reference resolved only by the later physical adapter.
    pub owner: String,
    /// Portable directory below the owner's immutable root; `.` denotes the owner root itself.
    pub root: String,
    /// Complete source-tree digest computed from `source_members`.
    pub digest: String,
}

/// One exact regular file below a unit's portable source root.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenSelectedRustFacetSourceMember {
    /// Portable path relative to the selected source root.
    pub path: String,
    /// Exact file byte digest.
    pub digest: String,
}

/// Portable reference to a path below one retained owner.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenSelectedRustFacetPath {
    /// Content identity from the graph's complete owner table.
    pub owner: String,
    /// Portable owner-relative path; `.` denotes the owner root itself.
    pub path: String,
}

/// Environment value retained without embedding a machine-local delivery path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OvenSelectedRustFacetEnvironmentValue {
    /// Explicit portable non-path value, including an explicitly selected empty string.
    Text { value: String },
    /// Path rebound through one retained owner by the physical projection adapter.
    Path { value: OvenSelectedRustFacetPath },
    /// Selection-only identity for a sensitive value that must never enter the projection payload.
    SensitiveDigest { hmac_sha256: String },
}

/// One selected generated input and the exact owner whose receipt produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenSelectedRustFacetGeneratedInput {
    /// Stable name used by cfg, environment, or source projection policy.
    pub name: String,
    /// Portable path and exact owner of the generated file or tree.
    pub source: OvenSelectedRustFacetPath,
    /// Digest of the selected generated bytes or complete generated tree.
    pub digest: String,
    /// Complete selected generated-file inventory; it proves the tree contains no unadmitted extra file.
    pub members: Vec<OvenSelectedRustFacetSourceMember>,
}

/// Linker spelling class selected for one admitted linked-library input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OvenSelectedRustFacetLinkedLibraryKind {
    /// A named static archive supplied by an admitted owner.
    Static,
    /// A named shared library supplied by an admitted owner.
    Dynamic,
    /// A framework resolved through one selected framework provider.
    Framework,
    /// A system library resolved through one selected SDK or platform provider.
    System,
}

/// One exact linked-library input, either byte-owned or capability-owned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "input", rename_all = "snake_case", deny_unknown_fields)]
pub enum OvenSelectedRustFacetLinkedLibrary {
    /// A static or shared archive whose selected bytes are retained beneath an immutable owner.
    Archive {
        /// Linker-visible library name without `lib` prefix or platform suffix.
        name: String,
        /// Static or dynamic linkage selected for the archive.
        kind: OvenSelectedRustFacetLinkedLibraryKind,
        /// Exact archive path under the immutable owner.
        artifact: OvenSelectedRustFacetPath,
        /// Digest of the archive bytes.
        digest: String,
    },
    /// A framework or system capability supplied by an explicitly selected provider.
    Provider {
        /// Linker-visible library or framework name.
        name: String,
        /// Framework or system linkage selected from the provider contract.
        kind: OvenSelectedRustFacetLinkedLibraryKind,
        /// Exact retained provider identity.
        provider: String,
    },
}

/// Exact dependency alias and selected child unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenSelectedRustFacetDependency {
    /// Rust-facing alias used by the parent source, including an admitted package rename.
    pub alias: String,
    /// Stable ID of the exact selected unit.
    pub unit: String,
}

/// Rust crate output kind selected for one unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OvenSelectedRustFacetCrateKind {
    /// Reusable Rust library.
    Rlib,
    /// Rust executable.
    Binary,
    /// Host procedural-macro library.
    ProcMacro,
}

/// Inspection role controlling why a Rust crate unit participates in this graph.
///
/// RFC 119 build scripts are provider candidates whose checked cfg, environment and generated outputs enter the
/// consuming unit; they are not rust-analyzer crate units in this v1 projection. Generated sources are represented
/// by `source_members` and `generated_inputs`, rather than a synthetic unit. Carrier-only `dylib`, `cdylib` and
/// `staticlib` outputs likewise belong to bake/link planning; inspection consumes their underlying library or
/// caller-projection unit. Those roles must refuse at the producer until a later schema explicitly represents them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OvenSelectedRustFacetUnitRole {
    /// Ordinary library root or dependency.
    Library,
    /// Ordinary executable root.
    Binary,
    /// Unit-test harness compiled with its owning crate root.
    UnitTest,
    /// Separate integration-test crate.
    IntegrationTest,
    /// Named executable example.
    Example,
    /// Generated documentation-test crate.
    Doctest,
    /// Named benchmark executable.
    Benchmark,
    /// Procedural-macro host unit.
    ProcMacro,
    /// Compiler support projected from the selected Toolchain owner's fixed sysroot-unit catalog.
    ///
    /// The producer copies the catalog's root, edition and dependency facts; it must not rediscover sysroot units.
    CompilerSupport,
    /// Target library projected for a Rust-hosted Incan caller.
    CallerProjection,
}

/// Whether a selected unit executes for the build host or compiles for the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OvenSelectedRustFacetDomain {
    /// Build script, procedural macro, or other host unit.
    Host,
    /// Ordinary target-linked unit.
    Target,
}

/// One stable crate unit selected by the Oven-native Rust facet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenSelectedRustFacetUnit {
    /// Content-derived unit identity used by dependency and exposed-root references.
    pub identity: String,
    /// Selected package identity.
    pub package: String,
    /// Exact selected package version.
    pub package_version: String,
    /// Rust crate name passed to compiler and inspection consumers.
    pub crate_name: String,
    /// Selected crate output kind.
    pub crate_kind: OvenSelectedRustFacetCrateKind,
    /// Reason this unit participates in the graph.
    pub role: OvenSelectedRustFacetUnitRole,
    /// Host or target compilation domain.
    pub domain: OvenSelectedRustFacetDomain,
    /// Selected Rust edition.
    pub edition: String,
    /// Complete source identity and owner reference.
    pub source: OvenSelectedRustFacetSource,
    /// Portable source-root-relative crate root, which must occur in `source_members`.
    pub root_module: String,
    /// Complete sorted source-file catalog. Absence cannot be represented as an empty leaf.
    pub source_members: Vec<OvenSelectedRustFacetSourceMember>,
    /// Sorted effective feature set observed for this exact physical unit.
    ///
    /// This is compiler-facing evidence, not a record of how Cargo's feature requests were authored. In particular,
    /// it cannot distinguish a default-enabled dependency edge from an explicit request for the `default` feature.
    /// Root request intent is carried separately by the policy exchange and must be bound to its inspection authority.
    pub features: Vec<String>,
    /// Sorted complete cfg facts supplied to inspection; an explicitly checked empty set remains empty.
    pub cfg: Vec<String>,
    /// Complete selected environment. Empty means the producer checked and selected no values.
    pub environment: BTreeMap<String, OvenSelectedRustFacetEnvironmentValue>,
    /// Sorted, nonempty source directories visible to rust-analyzer.
    pub include_dirs: Vec<OvenSelectedRustFacetPath>,
    /// Sorted source directories explicitly excluded from rust-analyzer.
    pub exclude_dirs: Vec<OvenSelectedRustFacetPath>,
    /// Exact sorted dependency aliases. An empty vector is a checked leaf, not missing evidence.
    pub dependencies: Vec<OvenSelectedRustFacetDependency>,
    /// Sorted generated inputs with their exact output owners.
    pub generated_inputs: Vec<OvenSelectedRustFacetGeneratedInput>,
    /// Sorted complete linked-library inputs retained by the selected publisher closure.
    pub linked_libraries: Vec<OvenSelectedRustFacetLinkedLibrary>,
}

/// One exposed physical root and the authority-owned request that selected it.
///
/// `features` on a unit is only observed effective activation. This record retains the separate authored request and
/// the owner that the physical adapter must resolve to an admitted inspection receipt before policy evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenSelectedRustFacetRoot {
    /// Selected physical unit identity for this Rust-facing alias.
    pub unit: String,
    /// Sorted explicitly requested root feature names.
    pub requested_features: Vec<String>,
    /// Whether the owning declaration requested default features.
    pub default_features: bool,
    /// Exact admitted owner identity that supplied this authored request.
    pub intent_owner: String,
}

/// Strict wire form of the target, toolchain, and profile selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenSelectedRustFacetIntent {
    /// Target triple selected for the requested graph.
    pub target: String,
    /// Exact selected Rust toolchain release identity.
    pub toolchain: String,
    /// Named Oven profile.
    pub profile: String,
}

/// Exact target-spec bytes selected from one Store-owned toolchain closure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenSelectedRustFacetTargetSpec {
    /// Toolchain-owner-relative target-spec JSON file, matching the inspection-toolchain payload's recorded path.
    pub source: OvenSelectedRustFacetPath,
    /// Digest of the exact target-spec JSON bytes recorded by that same toolchain payload.
    pub digest: String,
}

/// Complete canonical cfg facts reported by the sealed Rust compiler for one compilation target.
///
/// The snapshot is evidence from `rustc --print cfg`, not a target database projection. It deliberately keeps
/// bare cfg flags and quoted key/value facts separate, matching Rust's own output and the Incan policy exchange.
/// Both collections must already be canonical when a graph is admitted so a producer cannot make equivalent cfg
/// evidence hash differently by changing iteration order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenSelectedRustFacetCfgSnapshot {
    /// Sorted, duplicate-free bare cfg flags reported by Rustc.
    pub flags: Vec<String>,
    /// Sorted cfg values grouped under their sorted keys.
    pub values: BTreeMap<String, Vec<String>>,
}

/// Complete selection context shared by every unit in one graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenSelectedRustFacetSelection {
    /// Strict target/toolchain/profile wire intent.
    pub intent: OvenSelectedRustFacetIntent,
    /// Exact build host triple; host and target remain distinct.
    pub host: String,
    /// Complete cfg facts reported by the retained compiler for the build host.
    pub host_cfg: OvenSelectedRustFacetCfgSnapshot,
    /// Complete cfg facts reported by the retained compiler for the selected target.
    pub target_cfg: OvenSelectedRustFacetCfgSnapshot,
    /// Dependency-role activation purpose.
    pub purpose: OvenSelectedRustFacetPurpose,
    /// Semver-only compiler version required by rust-analyzer.
    pub toolchain_version: String,
    /// Exact target-spec file bound to the selected Store-owned toolchain closure.
    pub target_spec: OvenSelectedRustFacetTargetSpec,
}

/// Portable selected Rust graph; all physical roots and leases remain outside this payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenSelectedRustFacetGraph {
    /// Exact wire schema checked before any other field is consumed.
    pub schema_version: u32,
    /// Complete host/target/profile/purpose/feature/toolchain selection.
    pub selection: OvenSelectedRustFacetSelection,
    /// Sorted complete owner table referenced by sources and generated inputs, with exactly one Toolchain closure.
    pub owners: Vec<OvenSelectedRustFacetOwner>,
    /// Sorted complete selected unit table.
    pub units: Vec<OvenSelectedRustFacetUnit>,
    /// Exact Rust-facing root aliases, selected physical units and authority-owned request intent.
    pub exposed_roots: BTreeMap<String, OvenSelectedRustFacetRoot>,
}

/// Failure to admit a portable selected Rust graph.
#[derive(Debug, thiserror::Error)]
pub enum OvenSelectedRustFacetGraphError {
    /// A required producer fact was not supplied.
    #[error("missing selected Rust facet graph evidence: {field}")]
    Missing { field: String },
    /// Supplied facts are malformed, inconsistent, noncanonical, or reference absent records.
    #[error("invalid selected Rust facet graph evidence for {field}: {message}")]
    Invalid { field: String, message: String },
    /// The graph uses a wire schema this reader does not implement.
    #[error("unsupported selected Rust facet graph schema {found}; expected {expected}")]
    UnsupportedSchema { found: u32, expected: u32 },
}

/// Validated portable graph paired with its relocation-independent content identity.
#[derive(Debug, Clone)]
pub struct ValidatedOvenSelectedRustFacetGraph {
    graph: OvenSelectedRustFacetGraph,
    #[allow(
        dead_code,
        reason = "read by the Store payload consumer of Gate 6 of RFC 119, which lands after this producer"
    )]
    digest: String,
}

/// Minimal header decoded before any version-specific graph fields.
#[derive(Deserialize)]
#[allow(
    dead_code,
    reason = "read by the Store payload consumer of Gate 6 of RFC 119, which lands after this producer"
)]
struct OvenSelectedRustFacetGraphHeader {
    schema_version: u32,
}

/// Return the canonical ordering key for one exact linked-library input.
fn linked_library_sort_key(link: &OvenSelectedRustFacetLinkedLibrary) -> (u8, &str, &str) {
    match link {
        OvenSelectedRustFacetLinkedLibrary::Archive { name, artifact, .. } => (0, name, artifact.path.as_str()),
        OvenSelectedRustFacetLinkedLibrary::Provider { name, kind, provider } => {
            let family = match kind {
                OvenSelectedRustFacetLinkedLibraryKind::Framework => 1,
                OvenSelectedRustFacetLinkedLibraryKind::System => 2,
                OvenSelectedRustFacetLinkedLibraryKind::Static | OvenSelectedRustFacetLinkedLibraryKind::Dynamic => 3,
            };
            (family, name, provider)
        }
    }
}

/// Validate one declared linked-library input and all identities it references.
fn validate_selected_graph_linked_library(
    link: &OvenSelectedRustFacetLinkedLibrary,
    owners: &BTreeMap<&str, OvenSelectedRustFacetOwnerKind>,
    referenced_owners: &mut BTreeSet<String>,
    field: &str,
) -> Result<(), OvenSelectedRustFacetGraphError> {
    match link {
        OvenSelectedRustFacetLinkedLibrary::Archive {
            name,
            kind,
            artifact,
            digest,
        } => {
            if !matches!(
                kind,
                OvenSelectedRustFacetLinkedLibraryKind::Static | OvenSelectedRustFacetLinkedLibraryKind::Dynamic
            ) {
                return Err(selected_graph_invalid(
                    field,
                    "archive input must use static or dynamic linkage",
                ));
            }
            validate_selected_graph_alias(name, &format!("{field}.name"))?;
            validate_selected_graph_path_reference(artifact, owners, &format!("{field}.artifact"), false)?;
            referenced_owners.insert(artifact.owner.clone());
            validate_selected_graph_digest(digest, &format!("{field}.digest"))?;
        }
        OvenSelectedRustFacetLinkedLibrary::Provider { name, kind, provider } => {
            if !matches!(
                kind,
                OvenSelectedRustFacetLinkedLibraryKind::Framework | OvenSelectedRustFacetLinkedLibraryKind::System
            ) {
                return Err(selected_graph_invalid(
                    field,
                    "provider input must use framework or system linkage",
                ));
            }
            validate_selected_graph_alias(name, &format!("{field}.name"))?;
            let kind = validate_selected_graph_owner_reference(provider, owners, &format!("{field}.provider"))?;
            if kind != OvenSelectedRustFacetOwnerKind::LinkedLibraryProvider {
                return Err(selected_graph_invalid(
                    format!("{field}.provider"),
                    "must name a linked-library provider owner",
                ));
            }
            referenced_owners.insert(provider.clone());
        }
    }
    Ok(())
}

impl OvenSelectedRustFacetGraph {
    /// Decode a graph only after its minimal schema header selects this reader.
    #[allow(
        dead_code,
        reason = "read by the Store payload consumer of Gate 6 of RFC 119, which lands after this producer"
    )]
    pub fn decode_validated(
        bytes: &[u8],
    ) -> Result<ValidatedOvenSelectedRustFacetGraph, OvenSelectedRustFacetGraphError> {
        let header = serde_json::from_slice::<OvenSelectedRustFacetGraphHeader>(bytes).map_err(|error| {
            selected_graph_invalid("graph header", format!("cannot decode schema version: {error}"))
        })?;
        if header.schema_version != OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION {
            return Err(OvenSelectedRustFacetGraphError::UnsupportedSchema {
                found: header.schema_version,
                expected: OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION,
            });
        }
        let graph = serde_json::from_slice::<Self>(bytes)
            .map_err(|error| selected_graph_invalid("graph", format!("cannot decode payload: {error}")))?;
        graph.validated()
    }

    /// Validate complete evidence after canonical insertion-order normalization.
    fn validate_shape(&self) -> Result<(), OvenSelectedRustFacetGraphError> {
        if self.schema_version != OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION {
            return Err(OvenSelectedRustFacetGraphError::UnsupportedSchema {
                found: self.schema_version,
                expected: OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION,
            });
        }
        for (field, value) in [
            ("selection.host", self.selection.host.as_str()),
            ("selection.intent.target", self.selection.intent.target.as_str()),
            ("selection.intent.toolchain", self.selection.intent.toolchain.as_str()),
            ("selection.intent.profile", self.selection.intent.profile.as_str()),
            ("selection.toolchain_version", self.selection.toolchain_version.as_str()),
        ] {
            validate_selected_graph_text(value, field)?;
        }
        semver::Version::parse(&self.selection.toolchain_version).map_err(|error| {
            selected_graph_invalid(
                "selection.toolchain_version",
                format!("is not semantic version evidence: {error}"),
            )
        })?;
        validate_selected_graph_digest(&self.selection.target_spec.digest, "selection.target_spec.digest")?;
        validate_selected_graph_cfg_snapshot(&self.selection.host_cfg, "selection.host_cfg")?;
        validate_selected_graph_cfg_snapshot(&self.selection.target_cfg, "selection.target_cfg")?;

        if self.owners.is_empty() {
            return Err(selected_graph_missing("owners"));
        }
        let mut owners = BTreeMap::new();
        let mut toolchain_owner_count = 0usize;
        for (index, owner) in self.owners.iter().enumerate() {
            let field = format!("owners[{index}].identity");
            validate_selected_graph_digest(&owner.identity, &field)?;
            if owners.insert(owner.identity.as_str(), owner.kind).is_some() {
                return Err(selected_graph_invalid(field, "repeats an owner identity"));
            }
            if owner.kind == OvenSelectedRustFacetOwnerKind::Toolchain {
                toolchain_owner_count += 1;
            }
        }
        if toolchain_owner_count != 1 {
            return Err(selected_graph_invalid(
                "owners",
                format!("must contain exactly one Toolchain owner; found {toolchain_owner_count}"),
            ));
        }
        let mut referenced_owners = BTreeSet::new();
        let target_spec_kind = validate_selected_graph_path_reference(
            &self.selection.target_spec.source,
            &owners,
            "selection.target_spec.source",
            false,
        )?;
        if target_spec_kind != OvenSelectedRustFacetOwnerKind::Toolchain {
            return Err(selected_graph_invalid(
                "selection.target_spec.source.owner",
                "must reference a Toolchain owner",
            ));
        }
        referenced_owners.insert(self.selection.target_spec.source.owner.clone());

        if self.units.is_empty() {
            return Err(selected_graph_missing("units"));
        }
        let mut unit_indexes = BTreeMap::new();
        for (index, unit) in self.units.iter().enumerate() {
            let field = format!("units[{index}].identity");
            validate_selected_graph_digest(&unit.identity, &field)?;
            if unit_indexes.insert(unit.identity.as_str(), index).is_some() {
                return Err(selected_graph_invalid(field, "repeats a unit identity"));
            }
        }

        for (index, unit) in self.units.iter().enumerate() {
            let field = format!("units[{index}]");
            for (name, value) in [
                ("package", unit.package.as_str()),
                ("package_version", unit.package_version.as_str()),
                ("crate_name", unit.crate_name.as_str()),
                ("edition", unit.edition.as_str()),
            ] {
                validate_selected_graph_text(value, &format!("{field}.{name}"))?;
            }
            semver::Version::parse(&unit.package_version).map_err(|error| {
                selected_graph_invalid(
                    format!("{field}.package_version"),
                    format!("is not an exact semantic version: {error}"),
                )
            })?;
            if !matches!(unit.edition.as_str(), "2015" | "2018" | "2021" | "2024") {
                return Err(selected_graph_invalid(format!("{field}.edition"), "is unsupported"));
            }
            if !selected_graph_unit_role_is_valid(unit.role, unit.domain, unit.crate_kind) {
                return Err(selected_graph_invalid(
                    format!("{field}.role"),
                    format!(
                        "role {:?} cannot use {:?} domain with {:?} crate kind",
                        unit.role, unit.domain, unit.crate_kind
                    ),
                ));
            }

            let source_owner_kind =
                validate_selected_graph_owner_reference(&unit.source.owner, &owners, &format!("{field}.source.owner"))?;
            if matches!(unit.source.kind, OvenSelectedRustFacetSourceKind::Compiler)
                != matches!(source_owner_kind, OvenSelectedRustFacetOwnerKind::Toolchain)
            {
                return Err(selected_graph_invalid(
                    format!("{field}.source.owner"),
                    "Compiler source must use the Toolchain owner and other source kinds must not",
                ));
            }
            // Owner coherence settles what kind of source this really is before its identity spelling is judged.
            validate_selected_graph_source_identity(&unit.source, &format!("{field}.source.identity"))?;
            referenced_owners.insert(unit.source.owner.clone());
            validate_selected_graph_path(&unit.source.root, &format!("{field}.source.root"), true)?;
            validate_selected_graph_digest(&unit.source.digest, &format!("{field}.source.digest"))?;
            validate_selected_graph_path(&unit.root_module, &format!("{field}.root_module"), false)?;
            if unit.source_members.is_empty() {
                return Err(selected_graph_missing(format!("{field}.source_members")));
            }
            for (member_index, member) in unit.source_members.iter().enumerate() {
                validate_selected_graph_path(
                    &member.path,
                    &format!("{field}.source_members[{member_index}].path"),
                    false,
                )?;
                validate_selected_graph_digest(
                    &member.digest,
                    &format!("{field}.source_members[{member_index}].digest"),
                )?;
            }
            if unit.source_members.windows(2).any(|pair| pair[0].path >= pair[1].path) {
                return Err(selected_graph_invalid(
                    format!("{field}.source_members"),
                    "repeat a portable source path",
                ));
            }
            if !unit.source_members.iter().any(|member| member.path == unit.root_module) {
                return Err(selected_graph_missing(format!("{field}.root_module source member")));
            }
            if selected_graph_source_digest(&unit.source_members)? != unit.source.digest {
                return Err(selected_graph_invalid(
                    format!("{field}.source.digest"),
                    "does not bind the complete source member catalog",
                ));
            }

            validate_selected_graph_sorted_strings(&unit.features, &format!("{field}.features"))?;
            validate_selected_graph_sorted_strings(&unit.cfg, &format!("{field}.cfg"))?;
            for (name, value) in &unit.environment {
                validate_selected_graph_environment(
                    name,
                    value,
                    &owners,
                    &mut referenced_owners,
                    &format!("{field}.environment.{name}"),
                )?;
            }
            if unit.include_dirs.is_empty() {
                return Err(selected_graph_missing(format!("{field}.include_dirs")));
            }
            for (name, paths) in [
                ("include_dirs", unit.include_dirs.as_slice()),
                ("exclude_dirs", unit.exclude_dirs.as_slice()),
            ] {
                if paths.windows(2).any(|pair| pair[0] >= pair[1]) {
                    return Err(selected_graph_invalid(
                        format!("{field}.{name}"),
                        "repeat an owner-relative path",
                    ));
                }
                for path in paths {
                    validate_selected_graph_path_reference(path, &owners, &format!("{field}.{name}"), true)?;
                    referenced_owners.insert(path.owner.clone());
                }
            }
            for (dependency_index, dependency) in unit.dependencies.iter().enumerate() {
                validate_selected_graph_alias(
                    &dependency.alias,
                    &format!("{field}.dependencies[{dependency_index}].alias"),
                )?;
                validate_selected_graph_digest(
                    &dependency.unit,
                    &format!("{field}.dependencies[{dependency_index}].unit"),
                )?;
                if !unit_indexes.contains_key(dependency.unit.as_str()) {
                    return Err(selected_graph_missing(format!(
                        "{field}.dependencies[{dependency_index}] unit `{}`",
                        dependency.unit
                    )));
                }
            }
            if unit.dependencies.windows(2).any(|pair| pair[0].alias >= pair[1].alias) {
                return Err(selected_graph_invalid(
                    format!("{field}.dependencies"),
                    "repeat a Rust-facing alias",
                ));
            }
            for (generated_index, generated) in unit.generated_inputs.iter().enumerate() {
                validate_selected_graph_text(
                    &generated.name,
                    &format!("{field}.generated_inputs[{generated_index}].name"),
                )?;
                validate_selected_graph_path_reference(
                    &generated.source,
                    &owners,
                    &format!("{field}.generated_inputs[{generated_index}].source"),
                    true,
                )?;
                referenced_owners.insert(generated.source.owner.clone());
                validate_selected_graph_digest(
                    &generated.digest,
                    &format!("{field}.generated_inputs[{generated_index}].digest"),
                )?;
                if generated.members.is_empty() {
                    return Err(selected_graph_missing(format!(
                        "{field}.generated_inputs[{generated_index}].members"
                    )));
                }
                for (member_index, member) in generated.members.iter().enumerate() {
                    validate_selected_graph_path(
                        &member.path,
                        &format!("{field}.generated_inputs[{generated_index}].members[{member_index}].path"),
                        false,
                    )?;
                    validate_selected_graph_digest(
                        &member.digest,
                        &format!("{field}.generated_inputs[{generated_index}].members[{member_index}].digest"),
                    )?;
                }
                if generated.members.windows(2).any(|pair| pair[0].path >= pair[1].path)
                    || selected_graph_source_digest(&generated.members)? != generated.digest
                {
                    return Err(selected_graph_invalid(
                        format!("{field}.generated_inputs[{generated_index}].members"),
                        "do not form the selected generated-output digest",
                    ));
                }
            }
            if unit
                .generated_inputs
                .windows(2)
                .any(|pair| pair[0].name >= pair[1].name)
            {
                return Err(selected_graph_invalid(
                    format!("{field}.generated_inputs"),
                    "repeat a generated-input name",
                ));
            }
            for (link_index, link) in unit.linked_libraries.iter().enumerate() {
                validate_selected_graph_linked_library(
                    link,
                    &owners,
                    &mut referenced_owners,
                    &format!("{field}.linked_libraries[{link_index}]"),
                )?;
            }
            if unit
                .linked_libraries
                .windows(2)
                .any(|pair| linked_library_sort_key(&pair[0]) >= linked_library_sort_key(&pair[1]))
            {
                return Err(selected_graph_invalid(
                    format!("{field}.linked_libraries"),
                    "must be sorted and unique",
                ));
            }
        }

        if self.exposed_roots.is_empty() {
            return Err(selected_graph_missing("exposed_roots"));
        }
        for (alias, root) in &self.exposed_roots {
            validate_selected_graph_alias(alias, "exposed_roots alias")?;
            validate_selected_graph_digest(&root.unit, &format!("exposed_roots.{alias}.unit"))?;
            validate_selected_graph_sorted_strings(
                &root.requested_features,
                &format!("exposed_roots.{alias}.requested_features"),
            )?;
            let _intent_owner_kind = validate_selected_graph_owner_reference(
                &root.intent_owner,
                &owners,
                &format!("exposed_roots.{alias}.intent_owner"),
            )?;
            // The retained owner table is the graph's admitted authority set. The later policy adapter chooses the
            // payload-specific ProjectAuthority or Toolchain record; this physical schema must not guess that
            // semantic classification from a source root.
            referenced_owners.insert(root.intent_owner.clone());
            if !unit_indexes.contains_key(root.unit.as_str()) {
                return Err(selected_graph_missing(format!(
                    "exposed_roots.{alias} unit `{}`",
                    root.unit
                )));
            }
        }

        let mut incoming = self
            .units
            .iter()
            .map(|unit| unit.dependencies.len())
            .collect::<Vec<_>>();
        let mut dependents = vec![Vec::new(); self.units.len()];
        for (index, unit) in self.units.iter().enumerate() {
            for dependency in &unit.dependencies {
                let selected = unit_indexes.get(dependency.unit.as_str()).copied().ok_or_else(|| {
                    selected_graph_missing(format!("units[{index}] dependency unit `{}`", dependency.unit))
                })?;
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
        if visited != self.units.len() {
            return Err(selected_graph_invalid("units.dependencies", "contain a cycle"));
        }

        let mut reachable = BTreeSet::new();
        let mut pending = self
            .exposed_roots
            .values()
            .filter_map(|root| unit_indexes.get(root.unit.as_str()).copied())
            .collect::<Vec<_>>();
        while let Some(index) = pending.pop() {
            if !reachable.insert(index) {
                continue;
            }
            pending.extend(
                self.units[index]
                    .dependencies
                    .iter()
                    .filter_map(|dependency| unit_indexes.get(dependency.unit.as_str()).copied()),
            );
        }
        if reachable.len() != self.units.len() {
            let detached = self
                .units
                .iter()
                .enumerate()
                .find(|(index, _)| !reachable.contains(index))
                .map(|(_, unit)| unit.identity.as_str())
                .ok_or_else(|| selected_graph_invalid("units", "reachability accounting failed"))?;
            return Err(selected_graph_invalid(
                "units",
                format!("unit `{detached}` is not reachable from an exposed root"),
            ));
        }

        if let Some(detached) = self
            .owners
            .iter()
            .find(|owner| !referenced_owners.contains(owner.identity.as_str()))
        {
            return Err(selected_graph_invalid(
                "owners",
                format!("owner `{}` is not referenced by the selection", detached.identity),
            ));
        }

        for (index, unit) in self.units.iter().enumerate() {
            let expected = selected_graph_unit_identity(&self.selection, unit)?;
            if unit.identity != expected {
                return Err(selected_graph_invalid(
                    format!("units[{index}].identity"),
                    format!("does not match canonical unit identity {expected}"),
                ));
            }
        }
        Ok(())
    }

    /// Canonicalize, validate and hash the portable graph without ambient discovery.
    pub fn validated(mut self) -> Result<ValidatedOvenSelectedRustFacetGraph, OvenSelectedRustFacetGraphError> {
        canonicalize_selected_graph(&mut self);
        self.validate_shape()?;
        let bytes = serde_json::to_vec(&(OVEN_SELECTED_RUST_FACET_GRAPH_DIGEST_DOMAIN, &self)).map_err(|error| {
            selected_graph_invalid("graph", format!("cannot encode deterministic digest input: {error}"))
        })?;
        Ok(ValidatedOvenSelectedRustFacetGraph {
            graph: self,
            digest: selected_graph_sha256(&bytes),
        })
    }
}

impl ValidatedOvenSelectedRustFacetGraph {
    /// Borrow the validated portable graph.
    pub fn graph(&self) -> &OvenSelectedRustFacetGraph {
        &self.graph
    }

    /// Return the graph's location-independent content identity.
    #[allow(
        dead_code,
        reason = "read by the Store payload consumer of Gate 6 of RFC 119, which lands after this producer"
    )]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Encode the admitted graph for a Store payload or inspection sidecar.
    #[allow(
        dead_code,
        reason = "read by the Store payload consumer of Gate 6 of RFC 119, which lands after this producer"
    )]
    pub fn to_json_bytes(&self) -> Result<Vec<u8>, OvenSelectedRustFacetGraphError> {
        serde_json::to_vec(&self.graph)
            .map_err(|error| selected_graph_invalid("graph", format!("cannot encode wire payload: {error}")))
    }

    /// Recover the validated graph for ownership transfer into a publisher.
    #[allow(
        dead_code,
        reason = "read by the Store payload consumer of Gate 6 of RFC 119, which lands after this producer"
    )]
    pub fn into_graph(self) -> OvenSelectedRustFacetGraph {
        self.graph
    }
}

//! Receipt-selected direct-Rustc execution plans and their composition from stored Loaf entries.
//!
//! This is the Oven-side half of what `incan build` used to hold inline: the shapes a normal command executes against
//! (`OvenDirectRustcPlanSelection` and the stored, extension, and packaged-provider plans behind it), the selectors
//! that lease exactly one receipt-compatible closure from the store (`selection`), and the compositor that assembles
//! one closure from compatible public package Loafs (`composition`). Nothing here reads an Incan manifest, provider
//! record, or library manifest: callers pass receipts, payloads, and store handles in, and get leased plans or an
//! `OvenPlanError` back. The CLI renders that error; it never shapes it.

pub(crate) mod composition;
pub(crate) mod selection;

use std::path::{Path, PathBuf};

use super::OvenReceipt;
use super::legacy_cargo::OvenProjectExtensionPayload;
use super::loaf::OvenToolchainLoaf;
use super::rustc::{
    OvenRegistryLeafAuthority, OvenRustcArtifactManifest, OvenRustcArtifactPlan, OvenRustcError,
    OvenRustcSupportingArtifact, trusted_artifact_plan_for_source_evidence,
};
use super::store::{OvenArtifactKind, OvenStoreLease};
use composition::retain_packaged_provider_fragment_dependency_search_paths;
use serde::{Deserialize, Serialize};

/// Why one exact executable closure could not be selected or composed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum OvenPlanError {
    /// A direct-Rustc manifest, artifact, or bake refused underneath selection; the CLI renders compilation
    /// transcripts from this variant specially, so it stays distinguishable rather than flattened to a message.
    #[error(transparent)]
    Rustc(#[from] OvenRustcError),
    /// The stored candidates cannot form one receipt-bound closure: a missing base Loaf, a payload that does not
    /// decode, a conflicting public crate identity, or a plan the caller's authority never admitted.
    #[error("{0}")]
    Selection(String),
}

impl OvenPlanError {
    /// Refuse selection or composition with one rendered reason.
    pub(crate) fn selection(message: impl Into<String>) -> Self {
        Self::Selection(message.into())
    }
}

/// Result of selecting or composing a direct-Rustc execution plan.
pub(crate) type OvenPlanResult<T> = Result<T, OvenPlanError>;

/// One imported public-provider package the consumer composes against.
///
/// The driver checks a provider's source authority and sealed outputs once per command; what the plan layer needs
/// from that check is only the dependency key it reports under, the producer receipt whose intent the consumer must
/// match, and the immutable store entries the package's closure links against.
#[derive(Clone, Copy)]
pub(crate) struct PackagedProviderCandidate<'a> {
    /// Dependency key the consumer manifest declares the provider under (`pub::<key>`).
    pub(crate) dependency_key: &'a str,
    /// Producer receipt that authorized the package closure; its intent must equal the consumer's.
    pub(crate) receipt: &'a OvenReceipt,
    /// Store entries the package closure links against; empty when the provider uses only the compiler base.
    pub(crate) entries: &'a [OvenPackagedLibraryLoafEntry],
}

/// One immutable store entry transported by a public library package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenPackagedLibraryLoafEntry {
    /// Receipt that originally authorized this immutable store entry.
    pub(crate) receipt: OvenReceipt,
    /// Content address of the entry payload.
    pub(crate) identity: String,
    /// Semantic payload role of `identity`.
    pub(crate) kind: OvenArtifactKind,
    /// Exact compiler-shipped base Loaf required by a project-extension entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) base_loaf_identity: Option<String>,
}

/// Receipt-selected direct-Rustc closure for a normal Oven command.
///
/// A selection is either a receipt-bound project closure in the bounded local store or a complete compiler-shipped
/// standard-library Loaf held stable by its immutable generation lock. The latter remains direct so one versioned
/// stdlib closure cannot become many per-project cache copies.
pub(crate) enum OvenDirectRustcPlanSelection {
    Stored(Box<OvenStoredDirectRustcExecutionPlan>),
    ToolchainLoaf(Box<OvenToolchainLoaf>),
    ProjectExtension(Box<OvenProjectExtensionExecutionPlan>),
    /// The ABI-compatible package Loaf closure selected for public providers in this consumer.
    PackagedProvider(Box<OvenPackagedProviderExecutionPlan>),
}

impl OvenDirectRustcPlanSelection {
    /// Return the receipt-bound identity included in a normal-command build report.
    pub(crate) fn report_identity(&self) -> String {
        match self {
            Self::Stored(selected) => selected.identity.clone(),
            Self::ToolchainLoaf(native) => {
                format!("loaf:{}", native.loaf_build_unit_identity)
            }
            Self::ProjectExtension(extension) => format!(
                "loaf:{}+extension:{}",
                extension.base.loaf_identity, extension.extension.identity
            ),
            Self::PackagedProvider(packages) => packages.report_identity(),
        }
    }

    /// Return every immutable project entry that must travel with a public package.
    ///
    /// Compiler-shipped Loafs remain in the installed release envelope. Project deltas, by contrast, contain the
    /// provider's third-party Rust closure and must be carried by the package that owns that provider.
    pub(crate) fn package_entries(&self, receipt: &OvenReceipt) -> Vec<OvenPackagedLibraryLoafEntry> {
        match self {
            Self::Stored(selected) => vec![OvenPackagedLibraryLoafEntry {
                receipt: receipt.clone(),
                identity: selected.identity.clone(),
                kind: OvenArtifactKind::DirectRustcPlan,
                base_loaf_identity: None,
            }],
            Self::ProjectExtension(extension) => vec![OvenPackagedLibraryLoafEntry {
                receipt: receipt.clone(),
                identity: extension.extension.identity.clone(),
                kind: OvenArtifactKind::ProjectPayload,
                base_loaf_identity: Some(extension.base.loaf_identity.clone()),
            }],
            Self::PackagedProvider(packages) => packages.package_entries(),
            Self::ToolchainLoaf(_) => Vec::new(),
        }
    }

    /// Return the exact already-selected direct-Rustc closure.
    ///
    /// Callers must derive both omission and selected-path authority from this same plan. Reconstructing only its
    /// crate-name set loses the path/receipt relationship that distinguishes a compiler runtime from a lookalike
    /// caller dependency.
    pub(crate) fn artifact_plan(&self) -> &OvenRustcArtifactPlan {
        match self {
            Self::Stored(selected) => &selected.artifact_plan,
            Self::ToolchainLoaf(native) => &native.artifact_plan,
            Self::ProjectExtension(extension) => &extension.artifact_plan,
            Self::PackagedProvider(packages) => packages.artifact_plan(),
        }
    }

    /// Project the verified selected plan to the externs that its receipt admits to one generated source root.
    ///
    /// The complete plan also carries compiler-private support crates. They remain available to compiler-owned
    /// roots, but must not cause a normal project declaration with the same crate name to be skipped.
    pub(crate) fn source_artifact_plan(
        &self,
        source_evidence_key: &str,
    ) -> Result<OvenRustcArtifactPlan, OvenRustcError> {
        let mut plan =
            trusted_artifact_plan_for_source_evidence(self.artifact_plan(), self.artifacts(), source_evidence_key)?;
        if let Self::PackagedProvider(packages) = self {
            packages.retain_fragment_dependency_search_paths(&mut plan);
        }
        Ok(plan)
    }

    /// Return the complete execution manifest retained by this selected closure.
    pub(crate) fn artifacts(&self) -> &OvenRustcArtifactManifest {
        match self {
            Self::Stored(selected) => &selected.artifacts,
            Self::ToolchainLoaf(native) => &native.artifacts,
            Self::ProjectExtension(extension) => &extension.artifacts,
            Self::PackagedProvider(packages) => packages.artifacts(),
        }
    }

    /// Return one immutable root used only for caller-output containment checks.
    ///
    /// A composed extension passes its extension root here while its already verified `artifact_plan` supplies paths
    /// from both leased roots.  The output itself remains caller-owned and must be outside either root.
    pub(crate) fn output_guard_root(&self) -> &Path {
        match self {
            Self::Stored(selected) => &selected.artifact_root,
            Self::ToolchainLoaf(native) => &native.artifact_root,
            Self::ProjectExtension(extension) => &extension.extension.artifact_root,
            Self::PackagedProvider(packages) => packages.output_guard_root(),
        }
    }

    /// Return whether this selection already contains the public provider's complete compiled Rust closure.
    pub(crate) fn uses_packaged_provider_closure(&self) -> bool {
        matches!(self, Self::PackagedProvider(_))
    }

    /// Return whether this exact receipt-selected plan already seals the current generated project's path crates.
    ///
    /// A stored direct plan and a project extension are both produced for the current receipt, so their projected
    /// externs can satisfy matching declared path dependencies without a second materialization. A toolchain Loaf
    /// and an imported public-package closure are not current-project authority: a same-named caller path crate
    /// there must remain explicit rather than being mistaken for compiler or provider-private code.
    pub(crate) fn seals_current_project_path_dependencies(&self) -> bool {
        matches!(self, Self::Stored(_) | Self::ProjectExtension(_))
    }

    /// Return the one root that contains every vocabulary auxiliary closure, when it is not split across fragments.
    pub(crate) fn vocab_artifact_root(&self) -> Option<&Path> {
        match self {
            Self::Stored(selected) => Some(&selected.artifact_root),
            Self::ToolchainLoaf(native) => Some(&native.artifact_root),
            Self::ProjectExtension(extension) => extension.vocab_artifact_root.as_deref(),
            Self::PackagedProvider(packages) => packages.vocab_artifact_root(),
        }
    }

    /// Build the registry authority from exactly the roots that form this selected closure.
    pub(crate) fn registry_leaf_authority(&self) -> Option<OvenRegistryLeafAuthority> {
        match self {
            Self::Stored(selected) => selected
                .artifacts
                .registry_leaf_authority(&selected.artifact_root, &selected.artifact_plan),
            Self::ToolchainLoaf(native) => Some(native.registry_leaf_authority()),
            Self::ProjectExtension(extension) => extension.registry_leaf_authority.clone(),
            Self::PackagedProvider(packages) => packages.registry_leaf_authority(),
        }
    }
}

/// Receipt-validated stored direct-Rustc inputs held under a caller-owned lease.
///
/// The lease stays alive while a caller-owned package is re-materialized, so policy pruning cannot remove the
/// selected cohort between that compilation and the consuming normal Oven bake.
pub(crate) struct OvenStoredDirectRustcExecutionPlan {
    pub identity: String,
    pub artifacts: OvenRustcArtifactManifest,
    pub artifact_root: PathBuf,
    pub artifact_plan: OvenRustcArtifactPlan,
    _lease: OvenStoreLease,
}

/// Receipt-bound store entry that contributes only project-specific files to one exact compiler Loaf.
pub(crate) struct OvenStoredProjectExtensionExecutionPlan {
    pub(crate) identity: String,
    pub(crate) artifact_root: PathBuf,
    pub(crate) receipt: OvenReceipt,
    pub(crate) _lease: OvenStoreLease,
}

/// One complete direct-Rustc execution contract composed from a compiler Loaf and a store-owned project extension.
///
/// Both fields hold their independent leases/locks for the entire normal command.  The composed plan is resolved
/// once before any compiler process starts, so publication/pruning cannot replace or reclaim one side mid-command.
pub(crate) struct OvenProjectExtensionExecutionPlan {
    pub(crate) base: OvenToolchainLoaf,
    pub(crate) extension: OvenStoredProjectExtensionExecutionPlan,
    pub(crate) artifacts: OvenRustcArtifactManifest,
    pub(crate) artifact_plan: OvenRustcArtifactPlan,
    pub(crate) registry_leaf_authority: Option<OvenRegistryLeafAuthority>,
    pub(crate) vocab_artifact_root: Option<PathBuf>,
    pub(crate) source_payload: OvenProjectExtensionPayload,
}

/// One package-owned project-extension fragment retained while a consumer uses the composed closure.
///
/// A public package may contribute a closure that overlaps another package's compiler base or registry leaves.
/// The compositor assigns one canonical copy of every byte-identical path and retains every selected lease.
/// Duplicate-only contributors can still supply clean directories for exact source-role search bindings.
pub(crate) struct OvenPackagedProviderFragment {
    dependency_key: String,
    receipt: OvenReceipt,
    identity: String,
    extension: OvenStoredProjectExtensionExecutionPlan,
    dependency_search_paths: Vec<String>,
    native_search_paths: Vec<String>,
    supporting_artifacts: Vec<OvenRustcSupportingArtifact>,
    /// Complete inventory of the same leased root, retained before fragment deduplication.
    root_inventory: Vec<OvenRustcSupportingArtifact>,
    /// Original declared dependency directories in this leased extension, before canonical assignment.
    root_dependency_search_paths: Vec<String>,
}

/// Complete direct-Rustc closure assembled from compatible public package Loafs.
///
/// Public providers are independently baked and may share compiler/runtime artifacts.  The consumer never asks
/// Cargo to resolve those packages again: it admits only exact extension entries that agree on the compiler base,
/// build intent, artifact bytes, registry source identity, and public crate identities.  Distinct compatible
/// package deltas are then materialized from their separately leased immutable roots.
pub(crate) struct OvenExtensionPackagedProviderExecutionPlan {
    base: OvenToolchainLoaf,
    fragments: Vec<OvenPackagedProviderFragment>,
    artifacts: OvenRustcArtifactManifest,
    artifact_plan: OvenRustcArtifactPlan,
    registry_leaf_authority: Option<OvenRegistryLeafAuthority>,
    vocab_artifact_root: Option<PathBuf>,
    output_guard_root: PathBuf,
}

/// One self-contained direct-plan package fragment retained while a consumer composes compatible providers.
///
/// This form is used only when a provider was baked outside an installed compiler Loaf layout. Its closure is still
/// receipt-bound and immutable; it simply cannot be partitioned against a compiler-owned base that was unavailable
/// to the explicit publisher.
pub(crate) struct OvenPackagedDirectProviderFragment {
    dependency_key: String,
    receipt: OvenReceipt,
    identity: String,
    plan: OvenStoredDirectRustcExecutionPlan,
    dependency_search_paths: Vec<String>,
    native_search_paths: Vec<String>,
    supporting_artifacts: Vec<OvenRustcSupportingArtifact>,
    /// Complete inventory of the same leased root, retained before fragment deduplication.
    root_inventory: Vec<OvenRustcSupportingArtifact>,
}

/// Complete direct-Rustc closure assembled from compatible self-contained public package Loafs.
pub(crate) struct OvenDirectPackagedProviderExecutionPlan {
    fragments: Vec<OvenPackagedDirectProviderFragment>,
    artifacts: OvenRustcArtifactManifest,
    artifact_plan: OvenRustcArtifactPlan,
    registry_leaf_authority: Option<OvenRegistryLeafAuthority>,
    vocab_artifact_root: Option<PathBuf>,
    output_guard_root: PathBuf,
}

/// One complete public-provider closure. Providers baked with the same authority compose either through one shared
/// compiler base or through compatible self-contained direct plans; a mixed authority set is refused before Rustc
/// observes an ambiguous runtime closure.
pub(crate) enum OvenPackagedProviderExecutionPlan {
    Extensions(Box<OvenExtensionPackagedProviderExecutionPlan>),
    Direct(Box<OvenDirectPackagedProviderExecutionPlan>),
}

impl OvenPackagedProviderExecutionPlan {
    /// Retain the verified private dependency paths needed to load each public provider library.
    ///
    /// Source projection deliberately keeps provider-private crates out of the consumer's direct extern set. A
    /// caller-owned provider rlib may still refer to those crates in its metadata, so Rustc needs the package
    /// fragment's exact dependency directories on its search path. Composition has already restricted these paths to
    /// directories containing digest-verified fragment artifacts; restoring them here does not expose a private crate
    /// as a direct consumer dependency.
    fn retain_fragment_dependency_search_paths(&self, plan: &mut OvenRustcArtifactPlan) {
        match self {
            Self::Extensions(packages) => retain_packaged_provider_fragment_dependency_search_paths(
                plan,
                packages.fragments.iter().map(|fragment| {
                    (
                        fragment.extension.artifact_root.as_path(),
                        fragment.dependency_search_paths.as_slice(),
                    )
                }),
            ),
            Self::Direct(packages) => retain_packaged_provider_fragment_dependency_search_paths(
                plan,
                packages.fragments.iter().map(|fragment| {
                    (
                        fragment.plan.artifact_root.as_path(),
                        fragment.dependency_search_paths.as_slice(),
                    )
                }),
            ),
        }
    }

    /// Report every selected package entry, not merely the first compatible provider.
    pub(crate) fn report_identity(&self) -> String {
        match self {
            Self::Extensions(packages) => {
                let extensions = packages
                    .fragments
                    .iter()
                    .map(|fragment| format!("{}:{}", fragment.dependency_key, fragment.identity))
                    .collect::<Vec<_>>()
                    .join(",");
                format!("package-loafs:{}+extensions:{extensions}", packages.base.loaf_identity)
            }
            Self::Direct(packages) => {
                let plans = packages
                    .fragments
                    .iter()
                    .map(|fragment| format!("{}:{}", fragment.dependency_key, fragment.identity))
                    .collect::<Vec<_>>()
                    .join(",");
                format!("package-loafs:direct:{plans}")
            }
        }
    }

    /// Return the complete imported closure so a library depending on several public packages exports all of it.
    pub(crate) fn package_entries(&self) -> Vec<OvenPackagedLibraryLoafEntry> {
        match self {
            Self::Extensions(packages) => packages
                .fragments
                .iter()
                .map(|fragment| OvenPackagedLibraryLoafEntry {
                    receipt: fragment.receipt.clone(),
                    identity: fragment.identity.clone(),
                    kind: OvenArtifactKind::ProjectPayload,
                    base_loaf_identity: Some(packages.base.loaf_identity.clone()),
                })
                .collect(),
            Self::Direct(packages) => packages
                .fragments
                .iter()
                .map(|fragment| OvenPackagedLibraryLoafEntry {
                    receipt: fragment.receipt.clone(),
                    identity: fragment.identity.clone(),
                    kind: OvenArtifactKind::DirectRustcPlan,
                    base_loaf_identity: None,
                })
                .collect(),
        }
    }

    /// Return the composed direct-Rustc plan for this packaged-provider closure.
    pub(crate) fn artifact_plan(&self) -> &OvenRustcArtifactPlan {
        match self {
            Self::Extensions(packages) => &packages.artifact_plan,
            Self::Direct(packages) => &packages.artifact_plan,
        }
    }

    /// Return the merged artifact manifest for this packaged-provider closure.
    fn artifacts(&self) -> &OvenRustcArtifactManifest {
        match self {
            Self::Extensions(packages) => &packages.artifacts,
            Self::Direct(packages) => &packages.artifacts,
        }
    }

    /// Return the immutable root used for caller-output containment checks.
    fn output_guard_root(&self) -> &Path {
        match self {
            Self::Extensions(packages) => &packages.output_guard_root,
            Self::Direct(packages) => &packages.output_guard_root,
        }
    }

    /// Return the root containing the complete vocabulary auxiliary closure, when one exists.
    fn vocab_artifact_root(&self) -> Option<&Path> {
        match self {
            Self::Extensions(packages) => packages.vocab_artifact_root.as_deref(),
            Self::Direct(packages) => packages.vocab_artifact_root.as_deref(),
        }
    }

    /// Return the registry-leaf authority assembled for the selected package fragments.
    fn registry_leaf_authority(&self) -> Option<OvenRegistryLeafAuthority> {
        match self {
            Self::Extensions(packages) => packages.registry_leaf_authority.clone(),
            Self::Direct(packages) => packages.registry_leaf_authority.clone(),
        }
    }
}

/// Manifest fixtures shared by the plan tests and the driver tests that compose through this module.
#[cfg(test)]
pub(crate) mod test_support {
    use std::collections::BTreeMap;

    use super::super::rustc::{OvenRustcArtifactExtern, OvenRustcArtifactManifest};

    /// A two-extern package manifest whose closure is recaptured so `validate_shape` accepts it.
    pub(crate) fn package_loaf_manifest(
        intent: crate::oven::OvenBuildIntent,
        provider_crate: &str,
        provider_digest: &str,
    ) -> OvenRustcArtifactManifest {
        let mut manifest = OvenRustcArtifactManifest {
            schema_version: crate::oven::rustc::OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent,
            dependency_search_paths: vec!["artifacts/deps".to_string()],
            native_search_paths: Vec::new(),
            externs: vec![
                OvenRustcArtifactExtern {
                    crate_name: "incan_stdlib".to_string(),
                    relative_path: "artifacts/deps/libincan_stdlib-shared.rlib".to_string(),
                    digest: "sha256:shared".to_string(),
                },
                OvenRustcArtifactExtern {
                    crate_name: provider_crate.to_string(),
                    relative_path: format!("artifacts/deps/lib{provider_crate}-{provider_digest}.rlib"),
                    digest: provider_digest.to_string(),
                },
            ],
            entrypoint_dependency_search_paths: BTreeMap::new(),
            entrypoint_externs: BTreeMap::from([(
                "generated-root".to_string(),
                vec!["incan_stdlib".to_string(), provider_crate.to_string()],
            )]),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        };
        recapture_package_loaf_closure(&mut manifest);
        manifest
    }

    /// Rebuild every source role's publisher-selected closure from the manifest's current artifact set.
    ///
    /// A closure records each search directory's members by digest, so any fixture that rewrites an extern's digest
    /// after construction leaves the closure claiming bytes the manifest no longer declares -- which is exactly what
    /// `validate_shape` refuses. Deriving the closure here keeps the two from being written out twice by hand.
    pub(crate) fn recapture_package_loaf_closure(manifest: &mut OvenRustcArtifactManifest) {
        let closure = crate::oven::rustc::OvenRustcSourceSearchClosure::publisher_selected(
            manifest.dependency_search_paths.clone(),
            &manifest
                .externs
                .iter()
                .map(|artifact| (artifact.relative_path.clone(), artifact.digest.clone()))
                .collect(),
        );
        manifest.entrypoint_dependency_search_paths = manifest
            .entrypoint_externs
            .keys()
            .map(|key| (key.clone(), closure.clone()))
            .collect();
    }
}

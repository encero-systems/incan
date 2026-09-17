//! Sealed runtime-foundation authority for Cargo-free Oven publication.
//!
//! A runtime foundation is an SDK-provided, immutable description of the third-party Rust artifacts and the small
//! compiler-owned source layer that Oven may rebuild above them. It does not resolve packages, inspect a Cargo
//! manifest, or scan a target directory. Instead it validates a producer-selected Rust facet graph and projects its
//! declared prebuilt dependency edges for the direct-Rustc publisher.

#![allow(
    dead_code,
    reason = "the runtime source publisher loads this sealed authority in the next wiring slice"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{
    OvenMaterializedRustFacetGraph, OvenRustcArtifactExtern, OvenRustcArtifactManifest, OvenRustcArtifactPlan,
    OvenRustcError, OvenSelectedRustFacetDomain, OvenSelectedRustFacetGraph, OvenSelectedRustFacetOwnerKind,
    OvenSelectedRustFacetOwnerRoot, OvenSelectedRustFacetPath, OvenSelectedRustFacetSourceMember,
    OvenSelectedRustFacetSupplementalSourceMembers, ValidatedOvenSelectedRustFacetGraph,
    materialize_selected_rust_facet_graph_with_supplemental_source_members,
};

mod asset;
mod validation;

pub use asset::*;
use validation::*;

/// Wire schema for the first sealed compiler-runtime foundation.
pub const OVEN_RUNTIME_FOUNDATION_SCHEMA_VERSION: u32 = 1;

/// Wire schema for a release-owned runtime-foundation asset.
pub const OVEN_RUNTIME_FOUNDATION_ASSET_SCHEMA_VERSION: u32 = 4;

/// Canonical descriptor filename retained at the root of one installed runtime-foundation asset.
pub const OVEN_RUNTIME_FOUNDATION_ASSET_FILENAME: &str = "foundation.json";

/// A versioned SDK-owned authority for one direct-Rustc compiler-runtime closure.
///
/// selected_graph is the one source/unit/dependency projection consumed by both direct Rustc and inspection. The
/// foundation contributes only immutable source and artifact facts plus the policy declaring which units are already
/// prebuilt and which compiler-owned units may be rebuilt. It never rediscovers those relationships from Cargo data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenRuntimeFoundation {
    /// Runtime-foundation wire schema.
    pub schema_version: u32,
    /// Exact direct-Rustc compiler closure identity that owns every compiled-unit identity below this foundation.
    pub compiler_closure_digest: String,
    /// Immutable selected-graph owner whose root contains this foundation's sealed source and artifact catalogue.
    pub artifact_owner: String,
    /// Complete sealed artifact/source catalogue retained by the SDK foundation.
    pub artifacts: OvenRustcArtifactManifest,
    /// One selected Rust source graph produced by the foundation publisher.
    pub selected_graph: OvenSelectedRustFacetGraph,
    /// One explicit execution policy for every selected graph unit.
    pub units: Vec<OvenRuntimeFoundationUnit>,
}

/// One versioned release asset carrying a sealed runtime foundation and exact selected-package source inventories.
///
/// `foundation_identity` names this authority record, including selected source coordinates. It deliberately differs
/// from individual compiled-unit identities: changing an observed source authority can replace this asset while
/// unchanged compiler-visible units still reuse their content-addressed outputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenRuntimeFoundationAsset {
    /// Runtime-foundation asset wire schema.
    pub schema_version: u32,
    /// Canonical digest of this asset's foundation and source-inventory facts.
    pub foundation_identity: String,
    /// Complete selected source/artifact policy for the installed foundation.
    pub foundation: OvenRuntimeFoundation,
    /// One exhaustive source inventory for every selected source unit.
    pub source_inventories: Vec<OvenRuntimeFoundationSourceInventory>,
}

/// Source inventory retained for one selected runtime unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenRuntimeFoundationSourceInventory {
    /// Selected-source identity of the unit whose package was inspected.
    pub selected_identity: String,
    /// Exact selected package source inventory retained independently of compiled-unit identity.
    pub package: OvenRuntimeFoundationPackageSource,
    /// Whether checked source inventory found a build unit. This produces a warning and grants no execution authority.
    pub build_unit_present: bool,
}

/// Exact retained package source inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenRuntimeFoundationPackageSource {
    /// Retained owner-relative package root, which may sit above the compiler-visible unit root.
    pub root: OvenSelectedRustFacetPath,
    /// Exact package manifest retained as source evidence.
    pub manifest: OvenSelectedRustFacetSourceMember,
    /// Complete package-root inventory apart from the manifest.
    pub members: Vec<OvenSelectedRustFacetSourceMember>,
}

/// Execution policy for one selected Rust unit within a sealed runtime foundation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenRuntimeFoundationUnit {
    /// Selected-source identity naming exactly one unit in the foundation selected graph.
    pub selected_identity: String,
    /// Explicit host or target compilation domain; callers must never infer this from an artifact filename.
    pub domain: OvenSelectedRustFacetDomain,
    /// Whether the foundation supplies an immutable artifact or Oven may rebuild compiler-owned source.
    pub execution: OvenRuntimeFoundationUnitExecution,
}

/// The source-to-artifact policy for one selected runtime unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum OvenRuntimeFoundationUnitExecution {
    /// A sealed third-party artifact is already present in the SDK foundation.
    Prebuilt {
        /// Exact artifact that direct Rustc may expose through a selected dependency edge.
        artifact: OvenRustcArtifactExtern,
    },
    /// A compiler-owned source unit must be compiled by Oven above the sealed third-party foundation.
    Rebuild,
}

/// One direct prebuilt dependency projected from a selected graph edge.
///
/// The dependency alias comes from the shared selected graph rather than an artifact filename. This preserves package
/// renames and makes the projection a physical compiler-input adapter instead of a second dependency resolver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OvenRuntimeFoundationPrebuiltDependency {
    /// Rust-facing dependency alias passed to rustc --extern.
    pub alias: String,
    /// Selected identity of the prebuilt child unit.
    pub selected_identity: String,
    /// Explicit host or target domain of the child artifact.
    pub domain: OvenSelectedRustFacetDomain,
    /// Digest-verified SDK artifact for the child unit.
    pub artifact: OvenRustcArtifactExtern,
}

/// One direct prebuilt dependency rebound to its verified foundation-owned physical artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OvenMaterializedRuntimeFoundationPrebuiltDependency {
    /// Rust-facing dependency alias passed to direct Rustc.
    pub alias: String,
    /// Selected identity of the prebuilt child unit.
    pub selected_identity: String,
    /// Explicit host or target domain of the prebuilt child unit.
    pub domain: OvenSelectedRustFacetDomain,
    /// Foundation-owned immutable artifact path verified during publication materialization.
    pub artifact: PathBuf,
    /// Digest recorded for the exact artifact path.
    pub digest: String,
}

/// Publisher-only physical projection of a sealed runtime foundation.
///
/// The caller supplies roots held by existing Store or Loaf leases. This adapter verifies the complete selected source
/// trees and sealed artifact catalogue once, then hands later direct-Rustc code only admitted physical files and
/// topological rebuild order. It neither locates a cache nor reads Cargo metadata.
#[derive(Debug, Clone)]
pub struct OvenMaterializedRuntimeFoundation {
    sources: OvenMaterializedRustFacetGraph,
    artifact_plan: OvenRustcArtifactPlan,
    rebuild_order: Vec<String>,
    prebuilt_dependencies: BTreeMap<String, Vec<OvenMaterializedRuntimeFoundationPrebuiltDependency>>,
}

/// Publisher-only physical projection paired with the admitted release asset that authorized it.
///
/// A bare [`OvenMaterializedRuntimeFoundation`] is useful for lower-level materialization tests, but it does not
/// prove that exhaustive source inventory was admitted. Downstream runtime-closure publication must consume this
/// wrapper so its provenance begins at the validated release asset while its reuse identity can remain limited to
/// effective compiler inputs.
#[derive(Debug, Clone)]
pub struct OvenMaterializedRuntimeFoundationAsset {
    foundation_identity: String,
    foundation: ValidatedOvenRuntimeFoundation,
    source_inventories: BTreeMap<String, OvenRuntimeFoundationSourceInventory>,
    materialized: OvenMaterializedRuntimeFoundation,
}

/// An admitted runtime foundation that retains canonical selected source facts and execution policy.
#[derive(Debug, Clone)]
pub struct ValidatedOvenRuntimeFoundation {
    compiler_closure_digest: String,
    artifact_owner: String,
    artifacts: OvenRustcArtifactManifest,
    selected_graph: ValidatedOvenSelectedRustFacetGraph,
    units: BTreeMap<String, OvenRuntimeFoundationUnit>,
}

/// An admitted release foundation asset with complete selected-package source inventory.
#[derive(Debug, Clone)]
pub struct ValidatedOvenRuntimeFoundationAsset {
    foundation_identity: String,
    foundation: ValidatedOvenRuntimeFoundation,
    source_inventories: BTreeMap<String, OvenRuntimeFoundationSourceInventory>,
}

/// A release asset whose descriptor and complete foundation-owned file set have passed publisher-time admission.
///
/// The toolchain root remains separate because it is a named Toolchain owner, not content hidden underneath an SDK
/// foundation. This type does not lease or select anything for normal execution; it exists only for explicit bake or
/// release publication work.
#[derive(Debug, Clone)]
pub struct OvenAdmittedRuntimeFoundationAsset {
    asset: ValidatedOvenRuntimeFoundationAsset,
    owner_roots: Vec<OvenSelectedRustFacetOwnerRoot>,
}

impl OvenRuntimeFoundation {
    /// Canonicalize and validate an SDK runtime foundation without consulting Cargo, the ambient filesystem, or a
    /// neighbouring build cache.
    ///
    /// The result proves that every selected unit has exactly one declared execution policy, all prebuilt units are
    /// registry-backed immutable artifacts present in the sealed manifest, and every rebuildable unit is an admitted
    /// compiler-owned library or procedural macro.
    pub fn validated(self) -> Result<ValidatedOvenRuntimeFoundation, OvenRustcError> {
        let Self {
            schema_version,
            compiler_closure_digest,
            artifact_owner,
            artifacts,
            selected_graph,
            units,
        } = self;
        if schema_version != OVEN_RUNTIME_FOUNDATION_SCHEMA_VERSION {
            return Err(runtime_foundation_invalid(
                "runtime foundation schema",
                format!("expected schema {OVEN_RUNTIME_FOUNDATION_SCHEMA_VERSION}, found {schema_version}"),
            ));
        }
        validate_sha256_identity(&compiler_closure_digest, "runtime foundation compiler closure")?;
        validate_sha256_identity(&artifact_owner, "runtime foundation artifact owner")?;
        let selected_graph = selected_graph
            .validated()
            .map_err(|error| runtime_foundation_invalid("runtime foundation selected graph", error.to_string()))?;
        let artifact_owner_kind = selected_graph
            .graph()
            .owners
            .iter()
            .find(|owner| owner.identity == artifact_owner)
            .map(|owner| owner.kind)
            .ok_or_else(|| {
                runtime_foundation_invalid(
                    "runtime foundation artifact owner",
                    "is absent from the selected graph owner table",
                )
            })?;
        if artifact_owner_kind != OvenSelectedRustFacetOwnerKind::Constituent {
            return Err(runtime_foundation_invalid(
                "runtime foundation artifact owner",
                "must name the sealed constituent that owns the foundation root",
            ));
        }
        let expected_intent = &selected_graph.graph().selection.intent;
        if artifacts.intent.target != expected_intent.target
            || artifacts.intent.toolchain != expected_intent.toolchain
            || artifacts.intent.profile != expected_intent.profile
            || artifacts.intent.features != expected_intent.features
        {
            return Err(runtime_foundation_invalid(
                "runtime foundation artifact intent",
                "does not match the selected graph intent",
            ));
        }
        artifacts.validate_shape(&artifacts.intent)?;
        let declared_artifacts = artifacts.declared_artifact_digests()?;
        let graph_units = selected_graph
            .graph()
            .units
            .iter()
            .map(|unit| (unit.identity.as_str(), unit))
            .collect::<BTreeMap<_, _>>();
        let mut policies = BTreeMap::new();
        for policy in units {
            validate_sha256_identity(&policy.selected_identity, "runtime foundation unit identity")?;
            let Some(unit) = graph_units.get(policy.selected_identity.as_str()) else {
                return Err(runtime_foundation_invalid(
                    "runtime foundation unit identity",
                    format!("does not name a selected graph unit {}", policy.selected_identity),
                ));
            };
            let unit = *unit;
            if policy.domain != unit.domain {
                return Err(runtime_foundation_invalid(
                    "runtime foundation unit domain",
                    format!(
                        "unit {} declares {:?} but its selected graph unit is {:?}",
                        unit.crate_name, policy.domain, unit.domain
                    ),
                ));
            }
            validate_runtime_unit_policy(unit, &policy, &artifact_owner, &artifacts, &declared_artifacts)?;
            if policies.insert(policy.selected_identity.clone(), policy).is_some() {
                return Err(runtime_foundation_invalid(
                    "runtime foundation units",
                    "declares one selected unit more than once",
                ));
            }
        }
        let declared = policies.keys().map(String::as_str).collect::<BTreeSet<_>>();
        let selected = graph_units.keys().copied().collect::<BTreeSet<_>>();
        if declared != selected {
            let missing = selected.difference(&declared).copied().collect::<Vec<_>>();
            let extra = declared.difference(&selected).copied().collect::<Vec<_>>();
            let mut message = Vec::new();
            if !missing.is_empty() {
                message.push(format!("omits selected unit(s): {}", missing.join(", ")));
            }
            if !extra.is_empty() {
                message.push(format!("declares unknown unit(s): {}", extra.join(", ")));
            }
            return Err(runtime_foundation_invalid(
                "runtime foundation units",
                message.join("; "),
            ));
        }
        Ok(ValidatedOvenRuntimeFoundation {
            compiler_closure_digest,
            artifact_owner,
            artifacts,
            selected_graph,
            units: policies,
        })
    }
}

impl OvenRuntimeFoundationAsset {
    /// Construct a canonical runtime-foundation asset from checked selected-package source inventories.
    pub fn sealed(
        mut foundation: OvenRuntimeFoundation,
        mut source_inventories: Vec<OvenRuntimeFoundationSourceInventory>,
    ) -> Result<Self, OvenRustcError> {
        canonicalize_runtime_foundation_asset_facts(&mut foundation, &mut source_inventories)?;
        let foundation_identity = runtime_foundation_asset_identity(&foundation, &source_inventories)?;
        let asset = Self {
            schema_version: OVEN_RUNTIME_FOUNDATION_ASSET_SCHEMA_VERSION,
            foundation_identity,
            foundation,
            source_inventories,
        };
        let _ = asset.clone().validated()?;
        Ok(asset)
    }

    /// Canonicalize and validate this complete release asset without locating any physical root.
    pub fn validated(self) -> Result<ValidatedOvenRuntimeFoundationAsset, OvenRustcError> {
        let Self {
            schema_version,
            foundation_identity,
            mut foundation,
            mut source_inventories,
        } = self;
        if schema_version != OVEN_RUNTIME_FOUNDATION_ASSET_SCHEMA_VERSION {
            return Err(runtime_foundation_invalid(
                "runtime foundation asset schema",
                format!("expected schema {OVEN_RUNTIME_FOUNDATION_ASSET_SCHEMA_VERSION}, found {schema_version}"),
            ));
        }
        canonicalize_runtime_foundation_asset_facts(&mut foundation, &mut source_inventories)?;
        validate_sha256_identity(&foundation_identity, "runtime foundation asset identity")?;
        let expected_identity = runtime_foundation_asset_identity(&foundation, &source_inventories)?;
        if foundation_identity != expected_identity {
            return Err(runtime_foundation_invalid(
                "runtime foundation asset identity",
                "does not match its canonical foundation and source-inventory facts",
            ));
        }
        let foundation = foundation.validated()?;
        let source_inventories = validate_runtime_foundation_source_inventories(&foundation, source_inventories)?;
        Ok(ValidatedOvenRuntimeFoundationAsset {
            foundation_identity,
            foundation,
            source_inventories,
        })
    }
}

impl ValidatedOvenRuntimeFoundation {
    /// Return the direct compiler closure identity that participates in every compiled-unit identity.
    pub fn compiler_closure_digest(&self) -> &str {
        &self.compiler_closure_digest
    }

    /// Return the sole selected owner whose root carries this foundation's source and artifact payload.
    pub fn artifact_owner(&self) -> &str {
        &self.artifact_owner
    }

    /// Borrow the sealed artifact catalogue while its validated foundation remains owned by the caller.
    pub fn artifacts(&self) -> &OvenRustcArtifactManifest {
        &self.artifacts
    }

    /// Borrow the sole selected source/unit graph used by direct Rustc and inspection.
    pub fn selected_graph(&self) -> &ValidatedOvenSelectedRustFacetGraph {
        &self.selected_graph
    }

    /// Return the declared source-to-artifact policy for one selected unit.
    pub fn unit_policy(&self, selected_identity: &str) -> Option<&OvenRuntimeFoundationUnit> {
        self.units.get(selected_identity)
    }

    /// Iterate the compiler-owned units that the publisher must compile above the immutable third-party foundation.
    pub fn rebuild_units(&self) -> impl Iterator<Item = &OvenRuntimeFoundationUnit> {
        self.units
            .values()
            .filter(|unit| matches!(unit.execution, OvenRuntimeFoundationUnitExecution::Rebuild))
    }

    /// Project only prebuilt direct dependencies of one selected parent unit.
    ///
    /// Rebuildable children are deliberately absent: the direct-Rustc publisher supplies their caller-owned outputs
    /// after it compiles their own selected units. This derives aliases and edges from the shared selected graph,
    /// preventing an artifact catalogue from becoming a second dependency graph.
    pub fn prebuilt_dependencies(
        &self,
        selected_identity: &str,
    ) -> Result<Vec<OvenRuntimeFoundationPrebuiltDependency>, OvenRustcError> {
        let unit = self
            .selected_graph
            .graph()
            .units
            .iter()
            .find(|unit| unit.identity == selected_identity)
            .ok_or_else(|| {
                runtime_foundation_invalid(
                    "runtime foundation dependency parent",
                    format!("does not name a selected unit {selected_identity}"),
                )
            })?;
        let mut dependencies = Vec::new();
        for dependency in &unit.dependencies {
            let policy = self.units.get(&dependency.unit).ok_or_else(|| {
                runtime_foundation_invalid(
                    "runtime foundation dependency",
                    format!("selected child {} has no execution policy", dependency.unit),
                )
            })?;
            if let OvenRuntimeFoundationUnitExecution::Prebuilt { artifact } = &policy.execution {
                dependencies.push(OvenRuntimeFoundationPrebuiltDependency {
                    alias: dependency.alias.clone(),
                    selected_identity: dependency.unit.clone(),
                    domain: policy.domain,
                    artifact: artifact.clone(),
                });
            }
        }
        Ok(dependencies)
    }

    /// Verify and bind this foundation's selected source trees and artifact catalogue for one publisher invocation.
    ///
    /// This is deliberately publisher-only work. A normal command receives an already selected and leased plan; it
    /// must not invoke this method as a way to probe an SDK root or construct a new direct-Rustc closure.
    pub fn materialize_for_publication(
        &self,
        owner_roots: &[OvenSelectedRustFacetOwnerRoot],
    ) -> Result<OvenMaterializedRuntimeFoundation, OvenRustcError> {
        self.materialize_for_publication_with_supplemental_source_members(owner_roots, &[])
    }

    /// Verify and bind this foundation while retaining exact selected-package source inventories.
    ///
    /// The generic selected graph still supplies every compiler-visible input and compiled-unit identity. The
    /// supplemental closure makes the package-local physical tree exact without granting execution authority.
    fn materialize_for_publication_with_supplemental_source_members(
        &self,
        owner_roots: &[OvenSelectedRustFacetOwnerRoot],
        supplemental_source_members: &[OvenSelectedRustFacetSupplementalSourceMembers],
    ) -> Result<OvenMaterializedRuntimeFoundation, OvenRustcError> {
        let artifact_root = owner_roots
            .iter()
            .find(|owner| owner.identity == self.artifact_owner)
            .map(|owner| owner.root.clone())
            .ok_or_else(|| {
                runtime_foundation_invalid(
                    "runtime foundation artifact owner",
                    "has no physical root for publication",
                )
            })?;
        let sources = materialize_selected_rust_facet_graph_with_supplemental_source_members(
            &self.selected_graph,
            &self.compiler_closure_digest,
            owner_roots,
            supplemental_source_members,
        )?;
        let _ = self
            .artifacts
            .materialized_artifacts(&artifact_root, &self.artifacts.intent)?;
        let artifact_plan = self
            .artifacts
            .materialize_trusted_store(&artifact_root, &self.artifacts.intent)?;
        let paths = artifact_plan
            .externs
            .iter()
            .map(|(crate_name, path)| (crate_name.as_str(), path.clone()))
            .collect::<BTreeMap<_, _>>();
        let rebuild_order = runtime_foundation_rebuild_order(self.selected_graph.graph(), &self.units)?;
        let mut prebuilt_dependencies = BTreeMap::new();
        for selected_identity in &rebuild_order {
            let dependencies = self
                .prebuilt_dependencies(selected_identity)?
                .into_iter()
                .map(|dependency| {
                    let artifact = paths
                        .get(dependency.artifact.crate_name.as_str())
                        .cloned()
                        .ok_or_else(|| {
                            runtime_foundation_invalid(
                                "runtime foundation prebuilt artifact",
                                format!(
                                    "unit {} lost materialized direct extern {}",
                                    dependency.selected_identity, dependency.artifact.crate_name
                                ),
                            )
                        })?;
                    Ok(OvenMaterializedRuntimeFoundationPrebuiltDependency {
                        alias: dependency.alias,
                        selected_identity: dependency.selected_identity,
                        domain: dependency.domain,
                        artifact,
                        digest: dependency.artifact.digest,
                    })
                })
                .collect::<Result<Vec<_>, OvenRustcError>>()?;
            prebuilt_dependencies.insert(selected_identity.clone(), dependencies);
        }
        Ok(OvenMaterializedRuntimeFoundation {
            sources,
            artifact_plan,
            rebuild_order,
            prebuilt_dependencies,
        })
    }
}

impl OvenMaterializedRuntimeFoundation {
    /// Borrow the full-source-verified selected unit projection used by the publisher.
    pub fn sources(&self) -> &OvenMaterializedRustFacetGraph {
        &self.sources
    }

    /// Borrow the complete sealed artifact plan after its publisher-time byte verification.
    pub fn artifact_plan(&self) -> &OvenRustcArtifactPlan {
        &self.artifact_plan
    }

    /// Iterate selected identities in the dependency-safe order direct Rustc must rebuild them.
    pub fn rebuild_order(&self) -> impl Iterator<Item = &str> {
        self.rebuild_order.iter().map(String::as_str)
    }

    /// Borrow direct prebuilt dependencies for one compiler-owned unit.
    pub fn prebuilt_dependencies(
        &self,
        selected_identity: &str,
    ) -> Option<&[OvenMaterializedRuntimeFoundationPrebuiltDependency]> {
        self.prebuilt_dependencies.get(selected_identity).map(Vec::as_slice)
    }
}

impl OvenMaterializedRuntimeFoundationAsset {
    /// Return the canonical release-asset identity that admitted this physical projection.
    pub fn foundation_identity(&self) -> &str {
        &self.foundation_identity
    }

    /// Borrow the selected source/artifact policy admitted from the same release asset.
    pub fn foundation(&self) -> &ValidatedOvenRuntimeFoundation {
        &self.foundation
    }

    /// Return retained source inventory for one selected unit.
    pub fn source_inventory(&self, selected_identity: &str) -> Option<&OvenRuntimeFoundationSourceInventory> {
        self.source_inventories.get(selected_identity)
    }

    /// Borrow the physical source/artifact paths verified through this release asset.
    pub fn materialized(&self) -> &OvenMaterializedRuntimeFoundation {
        &self.materialized
    }
}

impl ValidatedOvenRuntimeFoundationAsset {
    /// Return the canonical content identity of this release-owned foundation authority.
    pub fn foundation_identity(&self) -> &str {
        &self.foundation_identity
    }

    /// Borrow the selected source/artifact foundation admitted by this release asset.
    pub fn foundation(&self) -> &ValidatedOvenRuntimeFoundation {
        &self.foundation
    }

    /// Return whether checked source inventory found a build unit for one selected unit.
    pub fn build_unit_present(&self, selected_identity: &str) -> Option<bool> {
        self.source_inventories
            .get(selected_identity)
            .map(|record| record.build_unit_present)
    }

    /// Return retained source inventory for one selected unit.
    pub fn source_inventory(&self, selected_identity: &str) -> Option<&OvenRuntimeFoundationSourceInventory> {
        self.source_inventories.get(selected_identity)
    }

    /// Bind this asset to physical publisher roots, including exact package inventories.
    pub fn materialize_for_publication(
        &self,
        owner_roots: &[OvenSelectedRustFacetOwnerRoot],
    ) -> Result<OvenMaterializedRuntimeFoundation, OvenRustcError> {
        let mut supplemental_source_members = Vec::new();
        for record in self.source_inventories.values() {
            let package = &record.package;
            let mut members = Vec::with_capacity(package.members.len() + 1);
            members.push(package.manifest.clone());
            members.extend(package.members.clone());
            members.sort();
            supplemental_source_members.push(OvenSelectedRustFacetSupplementalSourceMembers {
                selected_identity: record.selected_identity.clone(),
                source_root: package.root.clone(),
                members,
            });
        }
        self.foundation
            .materialize_for_publication_with_supplemental_source_members(owner_roots, &supplemental_source_members)
    }
}

impl OvenAdmittedRuntimeFoundationAsset {
    /// Return the canonical release-asset identity admitted from the foundation descriptor.
    pub fn foundation_identity(&self) -> &str {
        self.asset.foundation_identity()
    }

    /// Bind the admitted physical roots to direct-Rustc publication after all descriptor and member checks passed.
    pub fn materialize_for_publication(&self) -> Result<OvenMaterializedRuntimeFoundation, OvenRustcError> {
        self.asset.materialize_for_publication(&self.owner_roots)
    }

    /// Materialize the release asset for downstream runtime publication with its inventory-bound identity.
    ///
    /// This is the hand-off for direct-Rustc execution and runtime-closure publication. It cannot be constructed from
    /// a bare selected graph or a loose foundation descriptor, so a later closure records the asset whose exhaustive
    /// selected package inventory was actually admitted.
    pub fn materialize_asset_for_publication(&self) -> Result<OvenMaterializedRuntimeFoundationAsset, OvenRustcError> {
        let materialized = self.asset.materialize_for_publication(&self.owner_roots)?;
        Ok(OvenMaterializedRuntimeFoundationAsset {
            foundation_identity: self.asset.foundation_identity().to_string(),
            foundation: self.asset.foundation().clone(),
            source_inventories: self.asset.source_inventories.clone(),
            materialized,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::rustc::{
        OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION,
        OvenRustcRegistryLeaf, OvenRustcRegistrySource, OvenRustcRegistrySourcePackage, OvenRustcSupportingArtifact,
        OvenSelectedRustFacetCfgSnapshot, OvenSelectedRustFacetDependency, OvenSelectedRustFacetGeneratedInput,
        OvenSelectedRustFacetIntent, OvenSelectedRustFacetOwner, OvenSelectedRustFacetOwnerKind,
        OvenSelectedRustFacetPath, OvenSelectedRustFacetPurpose, OvenSelectedRustFacetSelection,
        OvenSelectedRustFacetSource, OvenSelectedRustFacetSourceMember, OvenSelectedRustFacetTargetSpec,
        selected_graph_sha256, selected_graph_source_digest, selected_graph_unit_identity,
    };
    use crate::rustc::{
        OvenSelectedRustFacetCrateKind, OvenSelectedRustFacetSourceKind, OvenSelectedRustFacetUnit,
        OvenSelectedRustFacetUnitRole,
    };
    use fs2::FileExt;
    use oven_store::OvenBuildIntent;

    use crate::loaf::{
        OVEN_LOAF_ENVELOPE_LOCK_FILE, OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION, OVEN_LOAF_SCHEMA_VERSION,
        OVEN_RELEASE_RUNTIME_FOUNDATION_MEMBER_LABEL, OVEN_RELEASE_RUNTIME_FOUNDATION_MEMBER_SCHEMA_VERSION, OvenLoaf,
        OvenLoafAccounting, OvenLoafCompatibility, OvenLoafEnvelopeManifest, OvenLoafEnvelopeMember,
        OvenLoafMemberRole, OvenLoafProvenance, OvenReleaseRuntimeFoundationMember,
        acquire_committed_release_runtime_foundation, acquire_exclusive_loaf_generation_lock,
        bind_release_runtime_foundation_evidence,
    };
    use crate::loaf_mirror::{LoafEnvelopeExpectation, LoafMemberExpectation, import_loaf_envelope_from_mirrors};

    const DIRECT_COMPILER: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    /// Return a stable owner identity for the foundation sealed third-party source/artifact root.
    fn foundation_owner() -> String {
        selected_graph_sha256(b"runtime foundation owner")
    }

    /// Return a stable owner identity for compiler-owned source and target facts.
    fn toolchain_owner() -> String {
        selected_graph_sha256(b"runtime foundation toolchain owner")
    }

    /// Create one complete source-tree member for a small fixture crate.
    fn source_member(path: &str, bytes: &[u8]) -> OvenSelectedRustFacetSourceMember {
        OvenSelectedRustFacetSourceMember {
            path: path.to_string(),
            digest: selected_graph_sha256(bytes),
        }
    }

    /// Return the fixture package manifest retained with a registry source tree.
    fn fixture_cargo_toml(package: &str) -> String {
        format!("[package]\nname = \"{package}\"\nversion = \"1.0.0\"\n")
    }

    /// Return the one source member emitted for a small fixture crate.
    fn fixture_source_bytes(crate_name: &str) -> String {
        format!("pub fn {crate_name}_marker() -> u8 {{ 7 }}\n")
    }

    /// Return a source member used as the declared fixture build-script entrypoint.
    fn fixture_build_script_bytes(crate_name: &str) -> String {
        format!("fn main() {{ let _ = \"{crate_name} inert build unit\"; }}\n")
    }

    /// Return the package member selected as the fixture build-script entrypoint.
    fn fixture_package_members(crate_name: &str) -> Vec<OvenSelectedRustFacetSourceMember> {
        vec![source_member(
            "build.rs",
            fixture_build_script_bytes(crate_name).as_bytes(),
        )]
    }

    /// Build complete package-root evidence without adding it to the compiler-visible source catalogue.
    fn fixture_package_source(
        unit: &OvenSelectedRustFacetUnit,
        additional_members: Vec<OvenSelectedRustFacetSourceMember>,
    ) -> Result<OvenRuntimeFoundationPackageSource, Box<dyn std::error::Error>> {
        let mut members = unit
            .source_members
            .iter()
            .filter(|member| member.path != "Cargo.toml")
            .cloned()
            .collect::<Vec<_>>();
        members.extend(additional_members);
        members.sort();
        Ok(OvenRuntimeFoundationPackageSource {
            root: OvenSelectedRustFacetPath {
                owner: unit.source.owner.clone(),
                path: unit.source.root.clone(),
            },
            manifest: source_member("Cargo.toml", fixture_cargo_toml(&unit.package).as_bytes()),
            members,
        })
    }

    /// Construct the selected build context shared by every fixture unit.
    fn cfg_snapshot(architecture: &str, operating_system: &str) -> OvenSelectedRustFacetCfgSnapshot {
        OvenSelectedRustFacetCfgSnapshot {
            flags: vec!["unix".to_string()],
            values: BTreeMap::from([
                ("target_arch".to_string(), vec![architecture.to_string()]),
                ("target_os".to_string(), vec![operating_system.to_string()]),
            ]),
        }
    }

    fn selection() -> OvenSelectedRustFacetSelection {
        OvenSelectedRustFacetSelection {
            intent: OvenSelectedRustFacetIntent {
                target: "x86_64-unknown-linux-gnu".to_string(),
                toolchain: "rustc 1.85.0 (fixture)".to_string(),
                profile: "debug".to_string(),
                features: vec!["async".to_string(), "json".to_string(), "ordinal".to_string()],
            },
            host: "aarch64-apple-darwin".to_string(),
            host_cfg: cfg_snapshot("aarch64", "macos"),
            target_cfg: cfg_snapshot("x86_64", "linux"),
            purpose: OvenSelectedRustFacetPurpose::Normal,
            default_features: true,
            toolchain_version: "1.85.0".to_string(),
            target_spec: OvenSelectedRustFacetTargetSpec {
                source: OvenSelectedRustFacetPath {
                    owner: toolchain_owner(),
                    path: "target-spec.json".to_string(),
                },
                digest: selected_graph_sha256(b"target spec"),
            },
        }
    }

    /// Build one source unit with a selected owner and direct graph dependencies.
    #[allow(clippy::too_many_arguments)]
    fn unit(
        selection: &OvenSelectedRustFacetSelection,
        package: &str,
        crate_name: &str,
        source_kind: OvenSelectedRustFacetSourceKind,
        owner: &str,
        role: OvenSelectedRustFacetUnitRole,
        domain: OvenSelectedRustFacetDomain,
        crate_kind: OvenSelectedRustFacetCrateKind,
        dependencies: Vec<OvenSelectedRustFacetDependency>,
    ) -> Result<OvenSelectedRustFacetUnit, Box<dyn std::error::Error>> {
        let member_bytes = fixture_source_bytes(crate_name);
        let mut members = Vec::new();
        if source_kind == OvenSelectedRustFacetSourceKind::Registry {
            let cargo_toml = fixture_cargo_toml(package);
            members.push(source_member("Cargo.toml", cargo_toml.as_bytes()));
        }
        members.push(source_member("src/lib.rs", member_bytes.as_bytes()));
        let source_identity = match source_kind {
            OvenSelectedRustFacetSourceKind::Registry => format!("registry:{package}@1.0.0"),
            OvenSelectedRustFacetSourceKind::Compiler => selected_graph_sha256(package.as_bytes()),
            OvenSelectedRustFacetSourceKind::Git
            | OvenSelectedRustFacetSourceKind::Path
            | OvenSelectedRustFacetSourceKind::Generated => {
                return Err("fixture uses only registry and compiler source kinds".into());
            }
        };
        let mut unit = OvenSelectedRustFacetUnit {
            identity: String::new(),
            package: package.to_string(),
            package_version: "1.0.0".to_string(),
            crate_name: crate_name.to_string(),
            crate_kind,
            role,
            domain,
            edition: "2024".to_string(),
            source: OvenSelectedRustFacetSource {
                kind: source_kind,
                identity: source_identity,
                owner: owner.to_string(),
                root: ".".to_string(),
                digest: selected_graph_source_digest(&members)?,
            },
            root_module: "src/lib.rs".to_string(),
            source_members: members,
            features: Vec::new(),
            default_features: false,
            cfg: Vec::new(),
            environment: BTreeMap::new(),
            include_dirs: vec![OvenSelectedRustFacetPath {
                owner: owner.to_string(),
                path: ".".to_string(),
            }],
            exclude_dirs: Vec::new(),
            dependencies,
            generated_inputs: Vec::new(),
        };
        unit.identity = selected_graph_unit_identity(selection, &unit)?;
        Ok(unit)
    }

    /// Rebind one fixture unit to its exact owner-relative source root and generated-output catalogue.
    fn bind_source_root(
        selection: &OvenSelectedRustFacetSelection,
        unit: &mut OvenSelectedRustFacetUnit,
        source_root: &str,
        generated_inputs: Vec<OvenSelectedRustFacetGeneratedInput>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        unit.source.root = source_root.to_string();
        unit.include_dirs = vec![OvenSelectedRustFacetPath {
            owner: unit.source.owner.clone(),
            path: source_root.to_string(),
        }];
        unit.generated_inputs = generated_inputs;
        unit.source.digest = selected_graph_source_digest(&unit.source_members)?;
        unit.identity = selected_graph_unit_identity(selection, unit)?;
        Ok(())
    }

    /// Construct the sealed artifact manifest used by the runtime-foundation fixture.
    fn artifacts(
        selection: &OvenSelectedRustFacetSelection,
        serde_source_digest: &str,
        serde_derive_source_digest: &str,
    ) -> OvenRustcArtifactManifest {
        let serde_source = OvenRustcRegistrySource {
            registry: "registry+https://example.invalid/index".to_string(),
            checksum: "serde-checksum".to_string(),
            relative_root: "registry-sources/serde-1.0.0".to_string(),
            digest: serde_source_digest.to_string(),
        };
        let serde_derive_source = OvenRustcRegistrySource {
            registry: "registry+https://example.invalid/index".to_string(),
            checksum: "serde-derive-checksum".to_string(),
            relative_root: "registry-sources/serde_derive-1.0.0".to_string(),
            digest: serde_derive_source_digest.to_string(),
        };
        let serde_artifact = OvenRustcArtifactExtern {
            crate_name: "serde".to_string(),
            relative_path: "deps/libserde.rlib".to_string(),
            digest: selected_graph_sha256(b"serde rlib"),
        };
        OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: OvenBuildIntent {
                target: selection.intent.target.clone(),
                toolchain: selection.intent.toolchain.clone(),
                profile: selection.intent.profile.clone(),
                features: selection.intent.features.clone(),
            },
            dependency_search_paths: vec!["deps".to_string()],
            native_search_paths: Vec::new(),
            externs: vec![
                serde_artifact.clone(),
                OvenRustcArtifactExtern {
                    crate_name: "serde_derive".to_string(),
                    relative_path: "deps/libserde_derive.dylib".to_string(),
                    digest: selected_graph_sha256(b"serde derive dylib"),
                },
            ],
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: vec![OvenRustcRegistryLeaf {
                package: "serde".to_string(),
                version: "1.0.0".to_string(),
                crate_name: "serde".to_string(),
                features: Vec::new(),
                source: serde_source.clone(),
                artifact: serde_artifact,
            }],
            registry_sources: vec![
                OvenRustcRegistrySourcePackage {
                    package: "serde".to_string(),
                    version: "1.0.0".to_string(),
                    features: Vec::new(),
                    source: serde_source,
                },
                OvenRustcRegistrySourcePackage {
                    package: "serde_derive".to_string(),
                    version: "1.0.0".to_string(),
                    features: Vec::new(),
                    source: serde_derive_source,
                },
            ],
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: vec![
                OvenRustcSupportingArtifact {
                    relative_path: "registry-sources/serde-1.0.0/Cargo.toml".to_string(),
                    digest: selected_graph_sha256(fixture_cargo_toml("serde").as_bytes()),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "registry-sources/serde_derive-1.0.0/Cargo.toml".to_string(),
                    digest: selected_graph_sha256(fixture_cargo_toml("serde_derive").as_bytes()),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "generated/serde/private.rs".to_string(),
                    digest: selected_graph_sha256(b"serde private.rs"),
                },
            ],
            entrypoint_dependency_search_paths: Default::default(),
        }
    }

    /// Build a complete mixed-domain runtime foundation with two prebuilt leaves and two compiler-owned rebuilds.
    fn foundation() -> Result<OvenRuntimeFoundation, Box<dyn std::error::Error>> {
        let selection = selection();
        let mut serde = unit(
            &selection,
            "serde",
            "serde",
            OvenSelectedRustFacetSourceKind::Registry,
            &foundation_owner(),
            OvenSelectedRustFacetUnitRole::Library,
            OvenSelectedRustFacetDomain::Target,
            OvenSelectedRustFacetCrateKind::Rlib,
            Vec::new(),
        )?;
        bind_source_root(
            &selection,
            &mut serde,
            "registry-sources/serde-1.0.0",
            vec![OvenSelectedRustFacetGeneratedInput {
                name: "serde-private".to_string(),
                source: OvenSelectedRustFacetPath {
                    owner: foundation_owner(),
                    path: "generated/serde/private.rs".to_string(),
                },
                digest: selected_graph_sha256(b"serde private.rs"),
            }],
        )?;
        let mut serde_derive = unit(
            &selection,
            "serde_derive",
            "serde_derive",
            OvenSelectedRustFacetSourceKind::Registry,
            &foundation_owner(),
            OvenSelectedRustFacetUnitRole::ProcMacro,
            OvenSelectedRustFacetDomain::Host,
            OvenSelectedRustFacetCrateKind::ProcMacro,
            Vec::new(),
        )?;
        serde_derive.cfg = vec!["fixture_cfg".to_string()];
        bind_source_root(
            &selection,
            &mut serde_derive,
            "registry-sources/serde_derive-1.0.0",
            Vec::new(),
        )?;
        let mut core = unit(
            &selection,
            "incan_lang",
            "incan_lang",
            OvenSelectedRustFacetSourceKind::Compiler,
            &toolchain_owner(),
            OvenSelectedRustFacetUnitRole::Library,
            OvenSelectedRustFacetDomain::Target,
            OvenSelectedRustFacetCrateKind::Rlib,
            vec![
                OvenSelectedRustFacetDependency {
                    alias: "serde".to_string(),
                    unit: serde.identity.clone(),
                },
                OvenSelectedRustFacetDependency {
                    alias: "serde_derive".to_string(),
                    unit: serde_derive.identity.clone(),
                },
            ],
        )?;
        bind_source_root(&selection, &mut core, "compiler/incan_lang", Vec::new())?;
        let mut stdlib = unit(
            &selection,
            "incan_std_core",
            "incan_std_core",
            OvenSelectedRustFacetSourceKind::Compiler,
            &toolchain_owner(),
            OvenSelectedRustFacetUnitRole::Library,
            OvenSelectedRustFacetDomain::Target,
            OvenSelectedRustFacetCrateKind::Rlib,
            vec![OvenSelectedRustFacetDependency {
                alias: "incan_lang".to_string(),
                unit: core.identity.clone(),
            }],
        )?;
        bind_source_root(&selection, &mut stdlib, "compiler/incan_std_core", Vec::new())?;
        let artifacts = artifacts(&selection, &serde.source.digest, &serde_derive.source.digest);
        let units = vec![
            OvenRuntimeFoundationUnit {
                selected_identity: serde.identity.clone(),
                domain: serde.domain,
                execution: OvenRuntimeFoundationUnitExecution::Prebuilt {
                    artifact: artifacts.externs[0].clone(),
                },
            },
            OvenRuntimeFoundationUnit {
                selected_identity: serde_derive.identity.clone(),
                domain: serde_derive.domain,
                execution: OvenRuntimeFoundationUnitExecution::Prebuilt {
                    artifact: artifacts.externs[1].clone(),
                },
            },
            OvenRuntimeFoundationUnit {
                selected_identity: core.identity.clone(),
                domain: core.domain,
                execution: OvenRuntimeFoundationUnitExecution::Rebuild,
            },
            OvenRuntimeFoundationUnit {
                selected_identity: stdlib.identity.clone(),
                domain: stdlib.domain,
                execution: OvenRuntimeFoundationUnitExecution::Rebuild,
            },
        ];
        Ok(OvenRuntimeFoundation {
            schema_version: OVEN_RUNTIME_FOUNDATION_SCHEMA_VERSION,
            compiler_closure_digest: DIRECT_COMPILER.to_string(),
            artifact_owner: foundation_owner(),
            artifacts,
            selected_graph: OvenSelectedRustFacetGraph {
                schema_version: OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION,
                selection,
                owners: vec![
                    OvenSelectedRustFacetOwner {
                        identity: foundation_owner(),
                        kind: OvenSelectedRustFacetOwnerKind::Constituent,
                    },
                    OvenSelectedRustFacetOwner {
                        identity: toolchain_owner(),
                        kind: OvenSelectedRustFacetOwnerKind::Toolchain,
                    },
                ],
                units: vec![serde, serde_derive, core, stdlib.clone()],
                exposed_roots: BTreeMap::from([("incan_std_core".to_string(), stdlib.identity)]),
            },
            units,
        })
    }

    /// Build exhaustive package source inventories; build.rs remains warning-only source evidence.
    fn source_inventories(
        foundation: &OvenRuntimeFoundation,
    ) -> Result<Vec<OvenRuntimeFoundationSourceInventory>, Box<dyn std::error::Error>> {
        foundation
            .selected_graph
            .units
            .iter()
            .map(|unit| {
                let build_unit_present = matches!(unit.crate_name.as_str(), "serde" | "serde_derive");
                let additional = if build_unit_present {
                    fixture_package_members(&unit.crate_name)
                } else {
                    Vec::new()
                };
                Ok(OvenRuntimeFoundationSourceInventory {
                    selected_identity: unit.identity.clone(),
                    package: fixture_package_source(unit, additional)?,
                    build_unit_present,
                })
            })
            .collect()
    }

    /// Construct one canonical release asset around the fixture foundation.
    fn foundation_asset() -> Result<OvenRuntimeFoundationAsset, Box<dyn std::error::Error>> {
        let foundation = foundation()?;
        OvenRuntimeFoundationAsset::sealed(foundation.clone(), source_inventories(&foundation)?).map_err(Into::into)
    }

    /// Write one exact foundation fixture member below a temporary retained owner root.
    fn write_fixture_file(root: &Path, relative: &str, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        let path = root.join(relative);
        let parent = path.parent().ok_or("fixture path has no parent")?;
        fs::create_dir_all(parent)?;
        fs::write(path, bytes)?;
        Ok(())
    }

    /// Populate the exact two-root physical payload named by the runtime-foundation fixture.
    fn write_materialization_fixture(
        foundation_root: &Path,
        toolchain_root: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        write_fixture_file(
            foundation_root,
            "registry-sources/serde-1.0.0/Cargo.toml",
            fixture_cargo_toml("serde").as_bytes(),
        )?;
        write_fixture_file(
            foundation_root,
            "registry-sources/serde-1.0.0/src/lib.rs",
            fixture_source_bytes("serde").as_bytes(),
        )?;
        write_fixture_file(
            foundation_root,
            "registry-sources/serde-1.0.0/build.rs",
            fixture_build_script_bytes("serde").as_bytes(),
        )?;
        write_fixture_file(
            foundation_root,
            "registry-sources/serde_derive-1.0.0/Cargo.toml",
            fixture_cargo_toml("serde_derive").as_bytes(),
        )?;
        write_fixture_file(
            foundation_root,
            "registry-sources/serde_derive-1.0.0/src/lib.rs",
            fixture_source_bytes("serde_derive").as_bytes(),
        )?;
        write_fixture_file(
            foundation_root,
            "registry-sources/serde_derive-1.0.0/build.rs",
            fixture_build_script_bytes("serde_derive").as_bytes(),
        )?;
        write_fixture_file(foundation_root, "generated/serde/private.rs", b"serde private.rs")?;
        write_fixture_file(foundation_root, "deps/libserde.rlib", b"serde rlib")?;
        write_fixture_file(foundation_root, "deps/libserde_derive.dylib", b"serde derive dylib")?;
        write_fixture_file(toolchain_root, "target-spec.json", b"target spec")?;
        write_fixture_file(
            toolchain_root,
            "compiler/incan_lang/Cargo.toml",
            fixture_cargo_toml("incan_lang").as_bytes(),
        )?;
        write_fixture_file(
            toolchain_root,
            "compiler/incan_lang/src/lib.rs",
            fixture_source_bytes("incan_lang").as_bytes(),
        )?;
        write_fixture_file(
            toolchain_root,
            "compiler/incan_std_core/Cargo.toml",
            fixture_cargo_toml("incan_std_core").as_bytes(),
        )?;
        write_fixture_file(
            toolchain_root,
            "compiler/incan_std_core/src/lib.rs",
            fixture_source_bytes("incan_std_core").as_bytes(),
        )?;
        Ok(())
    }

    /// Populate only the compiler-visible portion of the fixture for lower-level foundation materialization tests.
    ///
    /// A bare foundation deliberately has no package inventories, so it must not admit inventory-only files itself.
    fn write_foundation_materialization_fixture(
        foundation_root: &Path,
        toolchain_root: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        write_materialization_fixture(foundation_root, toolchain_root)?;
        fs::remove_file(foundation_root.join("registry-sources/serde-1.0.0/build.rs"))?;
        fs::remove_file(foundation_root.join("registry-sources/serde_derive-1.0.0/build.rs"))?;
        fs::remove_file(toolchain_root.join("compiler/incan_lang/Cargo.toml"))?;
        fs::remove_file(toolchain_root.join("compiler/incan_std_core/Cargo.toml"))?;
        Ok(())
    }

    /// Write the one canonical foundation descriptor at the root of a complete fixture asset.
    fn write_foundation_descriptor(
        foundation_root: &Path,
        asset: &OvenRuntimeFoundationAsset,
    ) -> Result<(), Box<dyn std::error::Error>> {
        fs::write(
            foundation_root.join(OVEN_RUNTIME_FOUNDATION_ASSET_FILENAME),
            serde_json::to_vec(asset)?,
        )?;
        Ok(())
    }

    /// Bind the fixture's two immutable roots to its selected owner table.
    fn materialization_owner_roots(
        foundation_root: &Path,
        toolchain_root: &Path,
    ) -> Vec<OvenSelectedRustFacetOwnerRoot> {
        vec![
            OvenSelectedRustFacetOwnerRoot {
                identity: foundation_owner(),
                root: foundation_root.to_path_buf(),
            },
            OvenSelectedRustFacetOwnerRoot {
                identity: toolchain_owner(),
                root: toolchain_root.to_path_buf(),
            },
        ]
    }

    /// A foundation retains the explicit host proc macro alongside target rlibs and projects only direct prebuilt
    /// graph edges for the compiler-owned unit that consumes them.
    #[test]
    fn runtime_foundation_preserves_host_target_and_rebuild_boundary() -> Result<(), Box<dyn std::error::Error>> {
        let foundation = foundation()?.validated()?;
        let core = foundation
            .selected_graph()
            .graph()
            .units
            .iter()
            .find(|unit| unit.crate_name == "incan_lang")
            .ok_or("fixture lost incan_lang")?;
        let dependencies = foundation.prebuilt_dependencies(&core.identity)?;
        assert_eq!(foundation.compiler_closure_digest(), DIRECT_COMPILER);
        assert_eq!(foundation.rebuild_units().count(), 2);
        assert_eq!(dependencies.len(), 2);
        assert!(dependencies.iter().any(|dependency| {
            dependency.alias == "serde" && dependency.domain == OvenSelectedRustFacetDomain::Target
        }));
        assert!(dependencies.iter().any(|dependency| {
            dependency.alias == "serde_derive" && dependency.domain == OvenSelectedRustFacetDomain::Host
        }));
        Ok(())
    }

    /// A prebuilt unit cannot quietly change from target to host simply because its artifact filename is reusable.
    #[test]
    fn runtime_foundation_refuses_domain_mismatch() -> Result<(), Box<dyn std::error::Error>> {
        let mut foundation = foundation()?;
        let policy = foundation
            .units
            .iter_mut()
            .find(|policy| matches!(policy.execution, OvenRuntimeFoundationUnitExecution::Prebuilt { .. }))
            .ok_or("fixture lost prebuilt policy")?;
        policy.domain = OvenSelectedRustFacetDomain::Host;
        assert!(matches!(
            foundation.validated(),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation unit domain",
                ..
            })
        ));
        Ok(())
    }

    /// Registry source cannot become a compiler-owned rebuild just because source bytes happen to be present.
    #[test]
    fn runtime_foundation_refuses_registry_rebuild() -> Result<(), Box<dyn std::error::Error>> {
        let mut foundation = foundation()?;
        let policy = foundation
            .units
            .iter_mut()
            .find(|policy| matches!(policy.execution, OvenRuntimeFoundationUnitExecution::Prebuilt { .. }))
            .ok_or("fixture lost prebuilt policy")?;
        policy.execution = OvenRuntimeFoundationUnitExecution::Rebuild;
        assert!(matches!(
            foundation.validated(),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation rebuild unit",
                ..
            })
        ));
        Ok(())
    }

    /// A declared prebuilt artifact must be present byte-for-byte in the sealed artifact manifest.
    #[test]
    fn runtime_foundation_refuses_undeclared_prebuilt_artifact() -> Result<(), Box<dyn std::error::Error>> {
        let mut foundation = foundation()?;
        let policy = foundation
            .units
            .iter_mut()
            .find(|policy| matches!(policy.execution, OvenRuntimeFoundationUnitExecution::Prebuilt { .. }))
            .ok_or("fixture lost prebuilt policy")?;
        let OvenRuntimeFoundationUnitExecution::Prebuilt { artifact } = &mut policy.execution else {
            return Err("fixture prebuilt policy changed modes".into());
        };
        artifact.digest = selected_graph_sha256(b"substituted artifact");
        assert!(matches!(
            foundation.validated(),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation prebuilt artifact",
                ..
            })
        ));
        Ok(())
    }

    /// Build-script output used by a prebuilt crate must remain a digest-verified member of the sealed foundation.
    #[test]
    fn runtime_foundation_refuses_unsealed_generated_input() -> Result<(), Box<dyn std::error::Error>> {
        let mut foundation = foundation()?;
        foundation
            .artifacts
            .supporting_artifacts
            .retain(|artifact| artifact.relative_path != "generated/serde/private.rs");
        assert!(matches!(
            foundation.validated(),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation generated input",
                ..
            })
        ));
        Ok(())
    }

    /// Every selected unit requires an explicit rebuild or prebuilt policy, so omission cannot become a fallback.
    #[test]
    fn runtime_foundation_refuses_missing_unit_policy() -> Result<(), Box<dyn std::error::Error>> {
        let mut foundation = foundation()?;
        let _ = foundation.units.pop().ok_or("fixture has no policies")?;
        assert!(matches!(
            foundation.validated(),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation units",
                ..
            })
        ));
        Ok(())
    }

    /// Inventory retains build-unit presence only, while proc-macro identity stays in the selected unit.
    #[test]
    fn runtime_foundation_asset_retains_inert_inventory_and_proc_macro_role() -> Result<(), Box<dyn std::error::Error>>
    {
        let asset = foundation_asset()?.validated()?;
        let selected = asset.foundation().selected_graph().graph();
        let serde = selected
            .units
            .iter()
            .find(|unit| unit.crate_name == "serde")
            .ok_or("fixture lost serde")?;
        let derive = selected
            .units
            .iter()
            .find(|unit| unit.crate_name == "serde_derive")
            .ok_or("fixture lost serde_derive")?;
        assert_eq!(asset.build_unit_present(&serde.identity), Some(true));
        assert_eq!(derive.role, OvenSelectedRustFacetUnitRole::ProcMacro);
        assert_eq!(derive.crate_kind, OvenSelectedRustFacetCrateKind::ProcMacro);
        Ok(())
    }

    /// Package-root evidence changes the release-asset payload, so an earlier asset schema cannot be admitted under
    /// the new interpretation by merely carrying otherwise well-formed fields.
    #[test]
    fn runtime_foundation_asset_refuses_previous_schema() -> Result<(), Box<dyn std::error::Error>> {
        let mut asset = foundation_asset()?;
        asset.schema_version = OVEN_RUNTIME_FOUNDATION_ASSET_SCHEMA_VERSION - 1;
        assert!(matches!(
            asset.clone().validated(),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation asset schema",
                ..
            })
        ));
        let foundation_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;
        write_foundation_descriptor(foundation_root.path(), &asset)?;
        assert!(matches!(
            admit_runtime_foundation_asset_for_publication(foundation_root.path(), toolchain_root.path()),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation asset schema",
                ..
            })
        ));
        Ok(())
    }

    /// Source inventory must still cover every compiler-visible source byte; inert does not mean incomplete.
    #[test]
    fn runtime_foundation_asset_refuses_incomplete_source_inventory() -> Result<(), Box<dyn std::error::Error>> {
        let mut asset = foundation_asset()?;
        let inventory = asset
            .source_inventories
            .iter_mut()
            .find(|inventory| inventory.build_unit_present)
            .ok_or("fixture lost build-unit inventory")?;
        inventory.package.members.retain(|member| member.path != "src/lib.rs");
        asset.foundation_identity = runtime_foundation_asset_identity(&asset.foundation, &asset.source_inventories)?;
        assert!(matches!(
            asset.validated(),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation package source members",
                ..
            })
        ));
        Ok(())
    }

    /// Inventory and selected-unit presentation order cannot manufacture a different foundation identity.
    #[test]
    fn runtime_foundation_asset_identity_canonicalizes_unordered_facts() -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?;
        let expected = asset.foundation_identity.clone();
        let mut reordered = asset.clone();
        reordered.foundation.selected_graph.units.reverse();
        reordered.foundation.units.reverse();
        reordered.source_inventories.reverse();
        reordered.foundation_identity =
            runtime_foundation_asset_identity(&reordered.foundation, &reordered.source_inventories)?;
        assert_eq!(reordered.foundation_identity, expected);
        let validated = reordered.validated()?;
        assert_eq!(validated.foundation_identity(), expected);
        Ok(())
    }

    /// The publisher copies only descriptor-derived members, re-admits the staged payload, and exposes the
    /// completed foundation atomically at its requested destination.
    #[test]
    fn runtime_foundation_asset_publisher_writes_only_declared_members() -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?;
        let source_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;
        let install_root = tempfile::tempdir()?;
        write_materialization_fixture(source_root.path(), toolchain_root.path())?;
        write_fixture_file(source_root.path(), "ambient/target-cache.bin", b"must not publish")?;
        let destination = install_root.path().join("runtime-foundation");

        let admitted =
            publish_runtime_foundation_asset(asset.clone(), source_root.path(), toolchain_root.path(), &destination)?;
        assert_eq!(admitted.foundation_identity(), asset.foundation_identity);
        assert_eq!(
            admitted
                .materialize_asset_for_publication()?
                .materialized()
                .rebuild_order()
                .count(),
            2
        );
        assert!(!destination.join("ambient/target-cache.bin").exists());
        assert!(destination.join(OVEN_RUNTIME_FOUNDATION_ASSET_FILENAME).is_file());
        assert_eq!(
            fs::read_dir(install_root.path())?.collect::<Result<Vec<_>, _>>()?.len(),
            1,
            "a completed asset leaves no visible staging sibling"
        );
        Ok(())
    }

    /// The release carrier survives mirror admission and keeps the selected generation locked for its held lifetime.
    #[test]
    fn release_carrier_mirrors_and_acquires_a_real_runtime_foundation() -> Result<(), Box<dyn std::error::Error>> {
        let mirror = tempfile::tempdir()?;
        let generation_identity = digest_bytes(b"runtime foundation generation");
        let generation_relative = Path::new("generations").join(
            generation_identity
                .strip_prefix("sha256:")
                .ok_or("fixture generation identity is not canonical")?,
        );
        let generation = mirror.path().join(&generation_relative);
        let compiled_root = generation.join("compiled.loaf");
        let foundation_source = tempfile::tempdir()?;
        let compiled_toolchain = tempfile::tempdir()?;
        let toolchain_root = generation.join("toolchain");
        fs::create_dir_all(&compiled_root)?;
        fs::create_dir_all(&toolchain_root)?;
        write_materialization_fixture(foundation_source.path(), &toolchain_root)?;
        write_foundation_materialization_fixture(&compiled_root, compiled_toolchain.path())?;

        let asset = foundation_asset()?;
        let plan = asset.foundation.artifacts.clone();
        let (payload_logical_bytes, payload_physical_bytes) = crate::loaf::loaf_directory_byte_counts(&compiled_root)?;
        let loaf = OvenLoaf {
            schema_version: OVEN_LOAF_SCHEMA_VERSION,
            build_unit_identity: digest_bytes(b"runtime foundation build unit"),
            provenance: OvenLoafProvenance {
                compiler_version: "fixture".to_string(),
                rust_toolchain: "fixture".to_string(),
                sdk_provider_codegen_revision: "fixture".to_string(),
                baker: "fixture".to_string(),
            },
            accounting: OvenLoafAccounting {
                payload_logical_bytes,
                payload_physical_bytes,
            },
            compatibility: OvenLoafCompatibility::default(),
            registry_leaves: plan.registry_leaves.clone(),
            plan: plan.clone(),
        };
        let loaf_path = compiled_root.join("loaf.json");
        let loaf_bytes = serde_json::to_vec(&loaf)?;
        fs::write(&loaf_path, &loaf_bytes)?;
        let loaf_identity = digest_bytes(&loaf_bytes);
        let plan_identity = digest_bytes(&serde_json::to_vec(&plan)?);
        let foundation_path = generation.join("runtime-foundation");
        let admitted = publish_runtime_foundation_asset(
            asset.clone(),
            foundation_source.path(),
            &toolchain_root,
            &foundation_path,
        )?;
        assert_eq!(admitted.foundation_identity(), asset.foundation_identity);

        let envelope_member = OvenLoafEnvelopeMember {
            label: "runtime-foundation-fixture".to_string(),
            profile: "release".to_string(),
            action: "build".to_string(),
            role: OvenLoafMemberRole::CompiledClosure,
            build_unit_identity: loaf.build_unit_identity.clone(),
            loaf_identity: loaf_identity.clone(),
            plan_identity: plan_identity.clone(),
            logical_bytes: 0,
            physical_bytes: 0,
            path: generation_relative.join("compiled.loaf/loaf.json"),
        };
        let runtime_member = OvenReleaseRuntimeFoundationMember {
            schema_version: OVEN_RELEASE_RUNTIME_FOUNDATION_MEMBER_SCHEMA_VERSION,
            label: OVEN_RELEASE_RUNTIME_FOUNDATION_MEMBER_LABEL.to_string(),
            foundation_relative_path: "runtime-foundation".into(),
            foundation_identity: asset.foundation_identity.clone(),
            compiled_loaf_identity: loaf_identity,
            compiled_plan_identity: plan_identity,
            toolchain_owner_identity: toolchain_owner(),
            toolchain_root_relative_path: "toolchain".into(),
        };
        let mut evidence = BTreeMap::new();
        bind_release_runtime_foundation_evidence(&mut evidence, &runtime_member)?;
        let manifest = OvenLoafEnvelopeManifest {
            schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
            envelope: "release".to_string(),
            generation_identity: generation_identity.clone(),
            evidence: evidence.clone(),
            loafs: vec![envelope_member],
            release_store_member: None,
            runtime_foundation: Some(runtime_member.clone()),
        };
        fs::write(mirror.path().join("envelope.json"), serde_json::to_vec(&manifest)?)?;
        drop(acquire_exclusive_loaf_generation_lock(mirror.path())?);

        let output = tempfile::tempdir()?;
        let scratch = tempfile::tempdir_in(output.path())?;
        let expected_members = [LoafMemberExpectation {
            label: "runtime-foundation-fixture".to_string(),
            profile: "release".to_string(),
            action: "build".to_string(),
            role: OvenLoafMemberRole::CompiledClosure,
        }];
        import_loaf_envelope_from_mirrors(
            output.path(),
            scratch.path(),
            &LoafEnvelopeExpectation {
                schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
                envelope: "release",
                generation_identity: &generation_identity,
                evidence: &evidence,
                members: &expected_members,
                release_store_member: None,
                runtime_foundation: Some(&runtime_member),
            },
            &[mirror.path().to_path_buf()],
        )
        .map_err(|error| format!("mirror import failed: {error}"))?;
        let held =
            acquire_committed_release_runtime_foundation(output.path(), OVEN_RELEASE_RUNTIME_FOUNDATION_MEMBER_LABEL)?
                .ok_or("runtime foundation was not acquired")?;
        assert_eq!(held.asset.foundation_identity(), asset.foundation_identity);
        let exclusive = fs::File::open(output.path().join(OVEN_LOAF_ENVELOPE_LOCK_FILE))?;
        assert!(matches!(
            exclusive.try_lock_exclusive(),
            Err(fs::TryLockError::WouldBlock)
        ));
        drop(held);
        exclusive.try_lock_exclusive()?;
        Ok(())
    }

    /// A changed source byte is rejected before the publisher creates a visible asset or leaves a private staging
    /// root behind.
    #[test]
    fn runtime_foundation_asset_publisher_refuses_changed_source_before_exposure()
    -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?;
        let source_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;
        let install_root = tempfile::tempdir()?;
        write_materialization_fixture(source_root.path(), toolchain_root.path())?;
        fs::write(source_root.path().join("deps/libserde.rlib"), b"changed rlib")?;
        let destination = install_root.path().join("runtime-foundation");

        assert!(matches!(
            publish_runtime_foundation_asset(asset, source_root.path(), toolchain_root.path(), &destination),
            Err(OvenRustcError::ArtifactDigestMismatch { .. })
        ));
        assert!(matches!(
            fs::symlink_metadata(&destination),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        ));
        assert!(fs::read_dir(install_root.path())?.next().is_none());
        Ok(())
    }

    /// Inventory-only source bytes are verified before publishing and cannot be replaced while the compiled Rust unit
    /// itself remains reusable.
    #[test]
    fn runtime_foundation_asset_publisher_refuses_changed_inventory_source_before_exposure()
    -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?;
        let source_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;
        let install_root = tempfile::tempdir()?;
        write_materialization_fixture(source_root.path(), toolchain_root.path())?;
        fs::write(
            source_root.path().join("registry-sources/serde-1.0.0/build.rs"),
            b"fn main() { println!(\"changed inventory source\"); }\n",
        )?;
        let destination = install_root.path().join("runtime-foundation");

        assert!(matches!(
            publish_runtime_foundation_asset(asset, source_root.path(), toolchain_root.path(), &destination),
            Err(OvenRustcError::ArtifactDigestMismatch { .. })
        ));
        assert!(matches!(
            fs::symlink_metadata(&destination),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        ));
        assert!(fs::read_dir(install_root.path())?.next().is_none());
        Ok(())
    }

    /// Existing installation content is immutable to a publisher: a retry must fail rather than replacing a
    /// possibly leased asset.
    #[test]
    fn runtime_foundation_asset_publisher_refuses_existing_destination() -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?;
        let source_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;
        let install_root = tempfile::tempdir()?;
        write_materialization_fixture(source_root.path(), toolchain_root.path())?;
        let destination = install_root.path().join("runtime-foundation");
        fs::create_dir(&destination)?;
        fs::write(destination.join("sentinel"), b"existing asset remains untouched")?;

        assert!(matches!(
            publish_runtime_foundation_asset(asset, source_root.path(), toolchain_root.path(), &destination),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation asset destination",
                ..
            })
        ));
        assert_eq!(
            fs::read(destination.join("sentinel"))?,
            b"existing asset remains untouched"
        );
        Ok(())
    }

    /// A release payload cannot smuggle a file beside otherwise valid source and artifact members.
    #[test]
    fn runtime_foundation_asset_refuses_undeclared_physical_member() -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?;
        let foundation_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;
        write_materialization_fixture(foundation_root.path(), toolchain_root.path())?;
        write_foundation_descriptor(foundation_root.path(), &asset)?;
        fs::write(foundation_root.path().join("unrecorded.bin"), b"not in the descriptor")?;

        assert!(matches!(
            admit_runtime_foundation_asset_for_publication(foundation_root.path(), toolchain_root.path()),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation asset members",
                ..
            })
        ));
        Ok(())
    }

    /// A package source inventory remains closed even when the selected unit source tree is otherwise valid.
    #[test]
    fn runtime_foundation_asset_refuses_undeclared_inventory_source_member() -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?;
        let foundation_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;
        write_materialization_fixture(foundation_root.path(), toolchain_root.path())?;
        write_foundation_descriptor(foundation_root.path(), &asset)?;
        write_fixture_file(
            foundation_root.path(),
            "registry-sources/serde-1.0.0/inventory-unrecorded.rs",
            b"fn hidden_inventory_input() {}\n",
        )?;

        assert!(matches!(
            admit_runtime_foundation_asset_for_publication(foundation_root.path(), toolchain_root.path()),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation asset members",
                ..
            })
        ));
        Ok(())
    }

    /// An empty directory is still release content and cannot hide beside a descriptor-derived asset tree.
    #[test]
    fn runtime_foundation_asset_refuses_undeclared_empty_directory() -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?;
        let foundation_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;
        write_materialization_fixture(foundation_root.path(), toolchain_root.path())?;
        write_foundation_descriptor(foundation_root.path(), &asset)?;
        fs::create_dir(foundation_root.path().join("unrecorded-empty"))?;

        assert!(matches!(
            admit_runtime_foundation_asset_for_publication(foundation_root.path(), toolchain_root.path()),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation asset members",
                ..
            })
        ));
        Ok(())
    }

    /// The recursive audit refuses links instead of letting a descriptor-derived name escape the asset root.
    #[cfg(unix)]
    #[test]
    fn runtime_foundation_asset_refuses_symlinked_member() -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?;
        let foundation_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;
        write_materialization_fixture(foundation_root.path(), toolchain_root.path())?;
        write_foundation_descriptor(foundation_root.path(), &asset)?;
        std::os::unix::fs::symlink(
            foundation_root.path().join("deps/libserde.rlib"),
            foundation_root.path().join("linked-artifact"),
        )?;

        assert!(matches!(
            admit_runtime_foundation_asset_for_publication(foundation_root.path(), toolchain_root.path()),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation asset members",
                ..
            })
        ));
        Ok(())
    }

    /// Publisher materialization admits only the sealed physical foundation and returns direct-Rustc inputs in
    /// compiler-owned dependency order without consulting a Cargo manifest, cache, or registry.
    #[test]
    fn runtime_foundation_materializes_sealed_direct_rustc_inputs() -> Result<(), Box<dyn std::error::Error>> {
        let foundation = foundation()?.validated()?;
        let graph = foundation.selected_graph().graph();
        let core = graph
            .units
            .iter()
            .find(|unit| unit.crate_name == "incan_lang")
            .ok_or("fixture lost incan_lang")?;
        let stdlib = graph
            .units
            .iter()
            .find(|unit| unit.crate_name == "incan_std_core")
            .ok_or("fixture lost incan_std_core")?;
        let foundation_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;

        write_foundation_materialization_fixture(foundation_root.path(), toolchain_root.path())?;

        let roots = materialization_owner_roots(foundation_root.path(), toolchain_root.path());
        let materialized = foundation.materialize_for_publication(&roots)?;
        let order = materialized.rebuild_order().collect::<Vec<_>>();
        assert_eq!(order, vec![core.identity.as_str(), stdlib.identity.as_str()]);
        assert!(materialized.sources().unit(&core.identity).is_some());
        assert!(materialized.sources().unit(&stdlib.identity).is_some());
        assert_eq!(materialized.artifact_plan().externs.len(), 2);

        let dependencies = materialized
            .prebuilt_dependencies(&core.identity)
            .ok_or("fixture did not project core prebuilt dependencies")?;
        assert_eq!(dependencies.len(), 2);
        let serde = dependencies
            .iter()
            .find(|dependency| dependency.alias == "serde")
            .ok_or("fixture did not project serde")?;
        assert_eq!(serde.domain, OvenSelectedRustFacetDomain::Target);
        assert_eq!(
            serde.artifact,
            fs::canonicalize(foundation_root.path().join("deps/libserde.rlib"))?
        );
        let serde_derive = dependencies
            .iter()
            .find(|dependency| dependency.alias == "serde_derive")
            .ok_or("fixture did not project serde_derive")?;
        assert_eq!(serde_derive.domain, OvenSelectedRustFacetDomain::Host);
        assert_eq!(
            serde_derive.artifact,
            fs::canonicalize(foundation_root.path().join("deps/libserde_derive.dylib"))?
        );
        Ok(())
    }

    /// Publication-time materialization must rehash an SDK artifact before the later trusted plan can expose it.
    #[test]
    fn runtime_foundation_refuses_mutated_artifact_during_publication_materialization()
    -> Result<(), Box<dyn std::error::Error>> {
        let foundation = foundation()?.validated()?;
        let foundation_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;
        write_foundation_materialization_fixture(foundation_root.path(), toolchain_root.path())?;
        fs::write(
            foundation_root.path().join("deps/libserde.rlib"),
            b"substituted artifact",
        )?;
        let roots = materialization_owner_roots(foundation_root.path(), toolchain_root.path());
        assert!(matches!(
            foundation.materialize_for_publication(&roots),
            Err(OvenRustcError::ArtifactDigestMismatch { .. })
        ));
        Ok(())
    }
}
